//! HTTP CONNECT tunnel shared by WebSocket rendezvous and the proxy settings validator.

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use url::Url;

/// Only accept the proxy protocol supported by both rendezvous and iroh relays.
pub fn parse_proxy(value: &str) -> Result<Url> {
    let url = Url::parse(value.trim()).context("代理地址格式不对，请填写 http://主机:端口")?;
    if url.scheme() != "http"
        || url.host_str().is_none()
        || !matches!(url.port_or_known_default(), Some(1..=65535))
    {
        bail!("代理地址必须是 http://主机:端口，暂不支持 SOCKS5 或 HTTPS 代理");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("暂不支持需要账号密码的代理");
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        bail!("代理地址不能包含路径、查询参数或片段");
    }
    Ok(url)
}

/// Caller bounds the entire TCP / CONNECT / TLS / WebSocket handshake with a timeout.
pub async fn tunnel(proxy: &Url, authority: &str) -> Result<TcpStream> {
    let host = proxy.host_str().context("代理地址缺少主机")?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port = proxy.port_or_known_default().context("代理地址缺少端口")?;
    let mut stream = TcpStream::connect((host, port))
        .await
        .context("连接 HTTP 代理失败")?;
    stream.set_nodelay(true)?;
    stream
        .write_all(format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes())
        .await
        .context("发送代理连接请求失败")?;

    // Read through the header only: never consume bytes belonging to the TLS/WebSocket stream.
    let mut header = Vec::new();
    loop {
        if header.len() >= 8192 {
            bail!("HTTP 代理响应头过长");
        }
        header.push(
            stream
                .read_u8()
                .await
                .context("HTTP 代理在握手时断开连接")?,
        );
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let text = std::str::from_utf8(&header).context("HTTP 代理返回了无效响应")?;
    let mut status = text.lines().next().unwrap_or_default().split_whitespace();
    let version = status.next().unwrap_or_default();
    let code = status.next().unwrap_or_default();
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        bail!("代理未返回 HTTP 响应，请确认使用的是 HTTP 或混合代理端口");
    }
    let code: u16 = code.parse().context("HTTP 代理返回了无效状态码")?;
    if code == 407 {
        bail!("HTTP 代理要求身份认证，暂不支持账号密码认证");
    }
    if !(200..300).contains(&code) {
        bail!("HTTP 代理拒绝连接（状态码 {code}）");
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn validate_proxy_addresses() {
        assert!(parse_proxy("http://127.0.0.1:7890").is_ok());
        assert!(parse_proxy("http://[::1]:7890").is_ok());
        for invalid in [
            "socks5://localhost:1080",
            "https://localhost",
            "http://user:pass@localhost",
            "http://localhost/path",
            "http://localhost?x=1",
            "http://localhost:0",
        ] {
            assert!(parse_proxy(invalid).is_err(), "{invalid}");
        }
    }

    async fn mock_proxy(response: Vec<u8>) -> (Url, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = parse_proxy(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(stream.read_u8().await.unwrap());
            }
            assert_eq!(
                request,
                b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n"
            );
            stream.write_all(&response).await.unwrap();
        });
        (proxy, task)
    }

    #[tokio::test]
    async fn connect_preserves_bytes_after_headers() {
        let (proxy, task) =
            mock_proxy(b"HTTP/1.1 200 Connection established\r\n\r\nhello".to_vec()).await;
        let mut stream = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            tunnel(&proxy, "example.com:443"),
        )
        .await
        .unwrap()
        .unwrap();
        let mut bytes = [0; 5];
        stream.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"hello");
        task.await.unwrap();
    }

    #[tokio::test]
    async fn reject_authentication_and_oversized_responses() {
        for response in [
            b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n".to_vec(),
            vec![b'x'; 8192],
        ] {
            let (proxy, task) = mock_proxy(response).await;
            assert!(
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    tunnel(&proxy, "example.com:443")
                )
                .await
                .unwrap()
                .is_err()
            );
            task.await.unwrap();
        }
    }
}

//! 远程编辑的代码补全、跳转、报错：在被连方找到 rust-analyzer、gopls 这类语言服务并启动它，
//! 把它的标准输入输出原样转给连接方的编辑器。
//!
//! 被连方只当一根管子：不解析语言服务的消息，按 Content-Length 分消息的事由连接方做。
//! 语言服务按用户自己的 PATH 找（和终端里一样），另外看几个常见的安装位置，
//! 包括 VS Code、Cursor 自带的 rust-analyzer，用户不用另外装或配置。
//! 流关了（编辑器关掉、断线）语言服务和它启动的子进程（比如 cargo check）一起结束。

use anyhow::{Context, Result, anyhow, bail};

/// 找不到语言服务时错误信息的开头，后面跟语言；连接方据此显示怎么安装
pub const MISSING: &str = "lsp_missing:";
/// 一条消息最大多少字节，防内存被撑爆
pub const MAX_MESSAGE: usize = 64 * 1024 * 1024;
/// 给一条消息加上 Content-Length 头
pub fn encode(body: &[u8]) -> Vec<u8> {
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body);
    out
}

/// 从攒下的字节里取出一条完整的消息；还不够一条时返回 None
pub fn take_message(buf: &mut Vec<u8>) -> Result<Option<String>> {
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        if buf.len() > 8192 { bail!("语言服务的输出格式不对"); }
        return Ok(None);
    };
    let head = std::str::from_utf8(&buf[..end]).context("语言服务的输出格式不对")?;
    let len: usize = head.split("\r\n")
        .find_map(|line| line.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-length")).map(|(_, v)| v.trim()))
        .ok_or_else(|| anyhow!("语言服务的输出缺少 Content-Length"))?
        .parse().context("语言服务的输出格式不对")?;
    if len > MAX_MESSAGE { bail!("语言服务的一条消息太大"); }
    let start = end + 4;
    if buf.len() < start + len { return Ok(None); }
    let body = String::from_utf8_lossy(&buf[start..start + len]).into_owned();
    buf.drain(..start + len);
    Ok(Some(body))
}

// 找语言服务、启动它、转发它输入输出的那部分只有被连的一方用，手机端（关掉 host）不编
#[cfg(feature = "host")]
mod server;
#[cfg(feature = "host")]
pub use server::serve;
#[cfg(feature = "host")]
pub(crate) use server::search_path;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_round_trip() {
        let mut buf = Vec::new();
        buf.extend(encode(br#"{"a":1}"#));
        buf.extend(b"content-length: 11\r\nContent-Type: x\r\n\r\n{\"b\":\"\xe4\xb8\xad\"}");
        assert_eq!(take_message(&mut buf).unwrap().as_deref(), Some(r#"{"a":1}"#));
        // 第二条还差几个字节
        let cut = buf.split_off(buf.len() - 3);
        assert_eq!(take_message(&mut buf).unwrap(), None);
        buf.extend(cut);
        assert_eq!(take_message(&mut buf).unwrap().as_deref(), Some("{\"b\":\"中\"}"));
        assert!(buf.is_empty());
        let mut bad = b"Content-Type: x\r\n\r\n{}".to_vec();
        assert!(take_message(&mut bad).is_err());
    }
}

//! 端口转发：把对方电脑上 localhost 的端口接到本机，用本机浏览器打开对方跑着的开发服务器。
//!
//! 本机（连接方）在 127.0.0.1（和 ::1）上监听，每来一个连接就开一条 P2P 流，对方（被连方）
//! 连它自己的 localhost 上的那个端口，两边原样对拷字节。被连方只连本机回环地址，
//! 不会变成通往它所在局域网的代理。另外能列出对方电脑上正在监听的端口和程序名，方便挑。

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::timeout;

use crate::client::RemoteHost;

/// 对方电脑上一个正在监听的端口
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListeningPort {
    pub port: u16,
    /// 监听它的程序名，看不到时为空
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// 被连方：连本机回环地址上的端口。localhost 可能只在 IPv4 或只在 IPv6 上（新版 Node 常常只听 ::1），两个都试
pub async fn connect_local(port: u16) -> Result<TcpStream> {
    if port == 0 { bail!("端口不对"); }
    let mut last = None;
    for addr in [SocketAddr::from((Ipv4Addr::LOCALHOST, port)), SocketAddr::from((Ipv6Addr::LOCALHOST, port))] {
        match timeout(Duration::from_secs(5), TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => {
                let _ = stream.set_nodelay(true);
                return Ok(stream);
            }
            Ok(Err(e)) => last = Some(e.to_string()),
            Err(_) => last = Some("连接超时".into()),
        }
    }
    tracing::debug!(port, "连不上本机端口：{}", last.unwrap_or_default());
    // 界面上知道是哪个端口，这里用固定的一句，方便翻译
    bail!("对方电脑上这个端口没有服务在监听")
}

/// 被连方：两头原样对拷，直到一边关掉
pub async fn pipe<A, B>(mut tcp: A, mut stream: B) -> Result<()>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    tokio::io::copy_bidirectional(&mut tcp, &mut stream).await?;
    Ok(())
}

/// 本机正在监听的 TCP 端口（被连方回答“对方电脑上有哪些服务”用）。只要回环地址或所有地址上的，
/// 只连得到这些；1024 以下的系统端口和 P2PCLI 自己的不列
pub async fn listening() -> Result<Vec<ListeningPort>> {
    let mut ports = tokio::task::spawn_blocking(scan).await??;
    let me = std::process::id();
    ports.retain(|p| p.port >= 1024 && p.pid != Some(me) && !p.process.as_deref().is_some_and(is_system_process));
    ports.sort_by_key(|p| p.port);
    ports.dedup_by_key(|p| p.port);
    Ok(ports)
}

/// Mac 上系统自带、和开发无关的服务（隔空播放、接力这些）
fn is_system_process(name: &str) -> bool {
    matches!(name, "ControlCenter" | "rapportd" | "sharingd" | "identityservicesd" | "remoted" | "launchd" | "AirPlayXPCHelper")
}

/// 地址是回环或“所有地址”：从 localhost 连得到
fn reachable(addr: &str) -> bool {
    let host = addr.trim_start_matches('[').trim_end_matches(']');
    matches!(host, "*" | "0.0.0.0" | "::" | "127.0.0.1" | "::1" | "localhost") || host.starts_with("127.")
}

/// "127.0.0.1:5173"、"[::1]:5173"、"*:5173" → (地址, 端口)
fn split_addr(text: &str) -> Option<(&str, u16)> {
    let (host, port) = text.rsplit_once(':')?;
    Some((host, port.parse().ok()?))
}

#[cfg(target_os = "macos")]
fn scan() -> Result<Vec<ListeningPort>> {
    // -F 是给程序读的格式：p 开头是进程号，c 开头是程序名，n 开头是地址
    let out = std::process::Command::new("/usr/sbin/lsof").args(["+c", "0", "-nP", "-iTCP", "-sTCP:LISTEN", "-Fpcn"]).output().context("运行 lsof 失败")?;
    Ok(parse_lsof(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_lsof(text: &str) -> Vec<ListeningPort> {
    let (mut pid, mut process) = (None, None);
    let mut ports = Vec::new();
    for line in text.lines() {
        let (tag, value) = line.split_at(line.len().min(1));
        match tag {
            "p" => { pid = value.parse().ok(); process = None; }
            "c" => process = Some(value.to_string()),
            "n" => if let Some((host, port)) = split_addr(value) && reachable(host) {
                ports.push(ListeningPort { port, process: process.clone(), pid });
            },
            _ => {}
        }
    }
    ports
}

#[cfg(target_os = "linux")]
fn scan() -> Result<Vec<ListeningPort>> {
    // ss 能给出程序名（自己的进程）；没有 ss 时读 /proc/net/tcp，只有端口
    if let Ok(out) = std::process::Command::new("ss").args(["-ltnpH"]).output() && out.status.success() {
        return Ok(parse_ss(&String::from_utf8_lossy(&out.stdout)));
    }
    let mut ports = Vec::new();
    for file in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        for line in text.lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            // 第 4 列是状态，0A 是 LISTEN；第 2 列是 十六进制地址:十六进制端口
            if cols.len() < 4 || cols[3] != "0A" { continue; }
            let Some((addr, port)) = cols[1].split_once(':') else { continue };
            let loopback = addr.chars().all(|c| c == '0') || addr == "0100007F" || addr == "00000000000000000000000001000000";
            if let (true, Ok(port)) = (loopback, u16::from_str_radix(port, 16)) {
                ports.push(ListeningPort { port, process: None, pid: None });
            }
        }
    }
    Ok(ports)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_ss(text: &str) -> Vec<ListeningPort> {
    text.lines().filter_map(|line| {
        let cols: Vec<&str> = line.split_whitespace().collect();
        let (host, port) = split_addr(cols.get(3)?)?;
        if !reachable(host.split('%').next().unwrap_or(host)) { return None; }
        // users:(("node",pid=123,fd=20))
        let users = cols.iter().find(|c| c.starts_with("users:"));
        let process = users.and_then(|u| u.split('"').nth(1)).map(str::to_string);
        let pid = users.and_then(|u| u.split("pid=").nth(1)).and_then(|p| p.split(|c: char| !c.is_ascii_digit()).next()).and_then(|p| p.parse().ok());
        Some(ListeningPort { port, process, pid })
    }).collect()
}

#[cfg(windows)]
fn scan() -> Result<Vec<ListeningPort>> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("netstat").args(["-ano", "-p", "TCP"]).creation_flags(crate::exec::CREATE_NO_WINDOW).output().context("运行 netstat 失败")?;
    let mut tcp6 = std::process::Command::new("netstat").args(["-ano", "-p", "TCPv6"]).creation_flags(crate::exec::CREATE_NO_WINDOW).output().map(|o| o.stdout).unwrap_or_default();
    let mut all = out.stdout;
    all.append(&mut tcp6);
    let mut ports = parse_netstat(&String::from_utf8_lossy(&all));
    // netstat 只给进程号，程序名用 sysinfo 查
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    for p in &mut ports {
        p.process = p.pid.and_then(|pid| system.process(sysinfo::Pid::from_u32(pid))).map(|proc| proc.name().to_string_lossy().trim_end_matches(".exe").to_string());
    }
    Ok(ports)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn parse_netstat(text: &str) -> Vec<ListeningPort> {
    text.lines().filter_map(|line| {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 5 || !cols[0].eq_ignore_ascii_case("TCP") || cols[3] != "LISTENING" { return None; }
        let (host, port) = split_addr(cols[1])?;
        if !reachable(host) { return None; }
        Some(ListeningPort { port, process: None, pid: cols[4].parse().ok() })
    }).collect()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn scan() -> Result<Vec<ListeningPort>> {
    Ok(Vec::new())
}

/// 连接方：一个正在进行的转发。丢掉它就停止监听，正在转的连接也一起断掉
pub struct Forward {
    pub remote_port: u16,
    pub local_port: u16,
    connections: Arc<AtomicUsize>,
    tasks: Vec<JoinHandle<()>>,
}

impl Forward {
    /// 开始转发对方电脑上的 remote_port。本机端口尽量和对方一样（网页里写死的地址也能用），
    /// 被占了就换一个空闲的；指定了 local_port 就只用它
    pub async fn start(remote: Arc<RemoteHost>, remote_port: u16, local_port: Option<u16>) -> Result<Forward> {
        if remote_port == 0 { bail!("端口不对"); }
        // 本机自己：直接用 localhost，转发反而会占掉同号端口、连回自己
        if remote.is_local() { bail!("这是本机，直接在浏览器里打开 localhost 就行，不用转发"); }
        // 先试连一次：对方那个端口上没有服务就直接说，不要等浏览器打开才发现
        drop(remote.open_port(remote_port).await?);
        let wanted = local_port.unwrap_or(remote_port);
        let v4 = match TcpListener::bind((Ipv4Addr::LOCALHOST, wanted)).await {
            Ok(listener) => listener,
            Err(e) if local_port.is_some() => bail!("本机的 {wanted} 端口用不了：{e}"),
            Err(_) => TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.context("本机找不到空闲端口")?,
        };
        let port = v4.local_addr()?.port();
        // localhost 在有的浏览器里先解析成 ::1，IPv6 上也听一下；听不了就算了
        let v6 = TcpListener::bind((Ipv6Addr::LOCALHOST, port)).await.ok();
        let connections = Arc::new(AtomicUsize::new(0));
        let mut tasks = vec![tokio::spawn(serve(v4, remote.clone(), remote_port, connections.clone()))];
        if let Some(v6) = v6 { tasks.push(tokio::spawn(serve(v6, remote, remote_port, connections.clone()))); }
        tracing::info!(remote_port, local_port = port, "开始端口转发");
        Ok(Forward { remote_port, local_port: port, connections, tasks })
    }

    /// 正在转的连接数
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::Relaxed)
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        for task in &self.tasks { task.abort(); }
        tracing::info!(remote_port = self.remote_port, local_port = self.local_port, "停止端口转发");
    }
}

/// 每接一个连接开一条 P2P 流；监听任务被停掉时，JoinSet 一起丢掉，正在转的连接都断开
async fn serve(listener: TcpListener, remote: Arc<RemoteHost>, port: u16, connections: Arc<AtomicUsize>) {
    let mut running = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((tcp, _)) = accepted else { continue };
                let _ = tcp.set_nodelay(true);
                let (remote, connections) = (remote.clone(), connections.clone());
                running.spawn(async move {
                    connections.fetch_add(1, Ordering::Relaxed);
                    match remote.open_port(port).await {
                        Ok(tunnel) => if let Err(e) = tunnel.pipe(tcp).await { tracing::debug!(port, "转发的连接断开：{e:#}"); },
                        Err(e) => tracing::debug!(port, "转发时连不上对方：{e:#}"),
                    }
                    connections.fetch_sub(1, Ordering::Relaxed);
                });
            }
            // 顺手收掉已经结束的
            Some(_) = running.join_next(), if !running.is_empty() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只听 ::1 的服务（新版 Node 的 localhost）也连得上
    #[tokio::test]
    async fn connects_to_ipv6_only_service() {
        let Ok(server) = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).await else { return };
        let port = server.local_addr().unwrap().port();
        // 同号的 IPv4 端口没人用时才测得出退路
        if std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_err() { return; }
        tokio::spawn(async move { let _ = server.accept().await; });
        let stream = connect_local(port).await.unwrap();
        assert!(stream.peer_addr().unwrap().is_ipv6());
    }

    #[test]
    fn parses_lsof() {
        let text = "p123\ncnode\nn127.0.0.1:5173\nn[::1]:5173\np456\ncControlCenter\nn*:7000\np789\ncpostgres\nn192.168.1.5:5432\np42\ncpython3\nn*:8000\n";
        let ports = parse_lsof(text);
        assert_eq!(ports.iter().map(|p| (p.port, p.process.as_deref().unwrap(), p.pid.unwrap())).collect::<Vec<_>>(),
            [(5173, "node", 123), (5173, "node", 123), (7000, "ControlCenter", 456), (8000, "python3", 42)]);
    }

    #[test]
    fn parses_ss_and_netstat() {
        let ss = "LISTEN 0 511 127.0.0.1:5173 0.0.0.0:* users:((\"node\",pid=123,fd=20))\nLISTEN 0 128 [::]:8080 [::]:*\nLISTEN 0 128 10.0.0.2:22 0.0.0.0:*\n";
        assert_eq!(parse_ss(ss), [
            ListeningPort { port: 5173, process: Some("node".into()), pid: Some(123) },
            ListeningPort { port: 8080, process: None, pid: None },
        ]);
        let netstat = "  TCP    0.0.0.0:3000           0.0.0.0:0              LISTENING       4242\n  TCP    192.168.1.2:139        0.0.0.0:0              LISTENING       4\n  TCP    [::1]:5173             [::]:0                 LISTENING       77\n  TCP    127.0.0.1:5000         127.0.0.1:6000         ESTABLISHED     1\n";
        assert_eq!(parse_netstat(netstat).iter().map(|p| (p.port, p.pid)).collect::<Vec<_>>(), [(3000, Some(4242)), (5173, Some(77))]);
    }
}

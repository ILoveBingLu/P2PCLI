//! 被连方：起一个真终端（macOS 登录 shell，Windows PowerShell），接到一条流上。

use std::io::{Read, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until};

use crate::proto::{Frame, Msg, read_frame, write_data, write_msg};

/// 子进程退出后最多再等多久把残余输出读完
const DRAIN_AFTER_EXIT: Duration = Duration::from_millis(1500);

/// 本机要起的 shell，返回命令和给对面看的名字
pub fn shell_command() -> (CommandBuilder, String) {
    #[cfg(windows)]
    let (mut cmd, label) = {
        let exe = if which("pwsh.exe") { "pwsh.exe" } else { "powershell.exe" };
        let mut cmd = CommandBuilder::new(exe);
        cmd.arg("-NoLogo");
        (cmd, exe.trim_end_matches(".exe").to_string())
    };
    #[cfg(not(windows))]
    let (mut cmd, label) = {
        // 默认程序就是用户的登录 shell，PATH 等环境和他平时开终端一样
        let cmd = CommandBuilder::new_default_prog();
        let label = cmd
            .get_shell()
            .rsplit('/')
            .next()
            .unwrap_or("shell")
            .to_string();
        (cmd, label)
    };
    if let Some(home) = std::env::home_dir() {
        cmd.cwd(home);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "P2PCLI");
    // 从访达启动的程序常常没有 LANG，中文文件名会变成转义
    if std::env::var_os("LANG").is_none() {
        cmd.env("LANG", "en_US.UTF-8");
    }
    (cmd, label)
}

#[cfg(windows)]
fn which(exe: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
        .unwrap_or(false)
}

/// 断线时结束当前附着客户端；tmux/psmux 后台会话由独立进程持有。
struct KillGuard {
    killer: Box<dyn ChildKiller + Send + Sync>,
    armed: bool,
    session_id: String,
    pid: Option<u32>,
    stopping: Arc<AtomicBool>,
}

impl KillGuard {
    fn detach(&mut self, reason: &str) {
        if !self.stopping.swap(true, Ordering::SeqCst) {
            if self.session_id.is_empty() {
                tracing::info!(pid = ?self.pid, %reason, "关闭一次性临时终端");
            } else {
                tracing::info!(terminal = %self.session_id, pid = ?self.pid, %reason, "关闭终端连接客户端（不结束后台会话）");
            }
        }
        if let Err(error) = self.killer.kill() {
            tracing::warn!(terminal = %self.session_id, pid = ?self.pid, %error, "关闭终端连接客户端失败");
        }
    }
}

impl Drop for KillGuard {
    fn drop(&mut self) {
        if self.armed {
            self.detach("流处理结束或任务被取消");
        }
    }
}

type SharedMaster = Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>;

struct InputGuard(tokio::task::AbortHandle, SharedMaster);

impl Drop for InputGuard {
    fn drop(&mut self) {
        self.0.abort();
        self.1.lock().unwrap().take();
    }
}

/// 起终端并在 recv/send 上跑到结束。调用前对面已经发过 Open。
pub async fn serve_terminal<R, W>(recv: R, send: W, cols: u16, rows: u16) -> Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send,
{
    let (cmd, shell) = shell_command();
    serve_command(recv, send, cols, rows, cmd, shell, String::new()).await
}

pub async fn serve_session<R, W>(recv: R, send: W, cols: u16, rows: u16, cmd: CommandBuilder, session_id: String) -> Result<()>
where R: AsyncRead + Unpin + Send + 'static, W: AsyncWrite + Unpin + Send {
    serve_command(recv, send, cols, rows, cmd, if cfg!(windows) { "psmux" } else { "tmux" }.into(), session_id).await
}

async fn serve_command<R, W>(mut recv: R, mut send: W, cols: u16, rows: u16, cmd: CommandBuilder, shell: String, session_id: String) -> Result<()>
where R: AsyncRead + Unpin + Send + 'static, W: AsyncWrite + Unpin + Send {
    let pair = native_pty_system()
        .openpty(size(cols, rows))
        .context("打开终端失败")?;
    let mut child = pair.slave.spawn_command(cmd).context("启动 shell 失败")?;
    let pid = child.process_id();
    let stopping = Arc::new(AtomicBool::new(false));
    tracing::info!(?pid, %shell, terminal = %session_id, "终端连接客户端已启动");
    drop(pair.slave);
    let mut guard = KillGuard {
        killer: child.clone_killer(),
        armed: true,
        session_id: session_id.clone(),
        pid,
        stopping: stopping.clone(),
    };
    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    let master: SharedMaster = Arc::new(Mutex::new(Some(pair.master)));

    write_msg(&mut send, &Msg::Opened { shell, session_id: session_id.clone() }).await?;

    // 终端输出：阻塞读放在独立线程
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    tracing::trace!("终端输出 {n} 字节");
                    if out_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let (exit_tx, mut exit_rx) = oneshot::channel::<Option<u32>>();
    // 子进程真的退出了：写入线程等这个信号才放掉写入端，见下面
    let gone = Arc::new((Mutex::new(false), Condvar::new()));
    let wait_gone = gone.clone();
    let wait_session = session_id.clone();
    std::thread::spawn(move || {
        let code = match child.wait() {
            Ok(status) => {
                let detach_requested = stopping.load(Ordering::SeqCst);
                if !status.success() && !detach_requested {
                    tracing::warn!(terminal = %wait_session, ?pid, code = status.exit_code(), signal = ?status.signal(), "终端连接客户端异常退出（不是后台服务退出码）");
                } else {
                    tracing::info!(terminal = %wait_session, ?pid, code = status.exit_code(), signal = ?status.signal(), detach_requested, "终端连接客户端已退出");
                }
                Some(status.exit_code())
            }
            Err(error) => {
                tracing::warn!(terminal = %wait_session, ?pid, %error, "读取终端连接客户端退出状态失败");
                None
            }
        };
        let (lock, cvar) = &*wait_gone;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
        let _ = exit_tx.send(code);
    });

    // 键盘输入：阻塞写也放独立线程
    let (in_tx, in_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        while let Ok(buf) = in_rx.recv() {
            if writer.write_all(&buf).is_err() {
                break;
            }
            let _ = writer.flush();
        }
        // portable-pty 放掉写入端时会往终端里写“换行 + Ctrl+D”，好让里面的程序读到输入结束。
        // 可这里跑的是 tmux 的附着客户端，它会把这两个字节原样转给会话里的 shell，shell 一退出后台会话就没了；
        // 断开时这边同时在杀附着客户端，谁先谁后看运气。所以等客户端真的退出了再放；等不到就宁可不放，漏一个文件描述符
        let (lock, cvar) = &*gone;
        let exited = cvar.wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(5), |gone| !*gone).unwrap().0;
        if *exited {
            drop(writer);
        } else {
            std::mem::forget(writer);
        }
    });

    let master_in = master.clone();
    let input_session = session_id.clone();
    let mut input = tokio::spawn(async move {
        loop {
            match read_frame(&mut recv).await {
                Ok(Some(Frame::Data(b))) => {
                    tracing::trace!("终端输入 {} 字节", b.len());
                    if in_tx.send(b.to_vec()).is_err() {
                        break;
                    }
                }
                Ok(Some(Frame::Msg(Msg::Resize { cols, rows }))) => {
                    if let Some(m) = master_in.lock().unwrap().as_ref() {
                        let _ = m.resize(size(cols, rows));
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => {
                    tracing::info!(terminal = %input_session, "终端输入流关闭：远端脱离或断线");
                    break;
                }
                Err(error) => {
                    tracing::warn!(terminal = %input_session, %error, "终端输入流异常断开");
                    break;
                }
            }
        }
    });

    // Also release the input task and PTY when an output write fails on disconnect.
    let _input_guard = InputGuard(input.abort_handle(), master.clone());
    let mut exit_code = None;
    let mut exited = false;
    let mut input_done = false;
    let mut deadline: Option<Instant> = None;
    loop {
        tokio::select! {
            data = out_rx.recv() => match data {
                Some(d) => write_data(&mut send, &d).await.inspect_err(|error| {
                    tracing::warn!(terminal = %session_id, %error, "终端输出流写入失败");
                })?,
                None => break,
            },
            code = &mut exit_rx, if !exited => {
                exited = true;
                guard.armed = false;
                exit_code = code.ok().flatten();
                // Windows 上不关掉终端，读线程收不到结束
                let m = master.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    m.lock().unwrap().take();
                });
                deadline = Some(Instant::now() + DRAIN_AFTER_EXIT);
            }
            _ = &mut input, if !input_done => {
                // 对面脱离终端：只结束 attach 客户端，后台会话继续运行。
                input_done = true;
                guard.detach("远端输入流结束");
            }
            _ = async { sleep_until(deadline.unwrap()).await }, if deadline.is_some() => break,
        }
    }
    if !exited {
        // 输出先结束了，再等一下退出码
        if let Ok(Ok(code)) = tokio::time::timeout(Duration::from_secs(2), exit_rx).await {
            exit_code = code;
            guard.armed = false;
        }
    }
    master.lock().unwrap().take();
    input.abort();
    let _ = write_msg(&mut send, &Msg::Exit { code: exit_code }).await;
    let _ = send.shutdown().await;
    Ok(())
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols: cols.max(2),
        rows: rows.max(2),
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::read_msg;

    /// 真起一个 shell，跑一条命令，看输出和退出码
    #[tokio::test]
    async fn runs_real_shell() {
        let (client, server) = tokio::io::duplex(1 << 20);
        let (srv_r, srv_w) = tokio::io::split(server);
        let task = tokio::spawn(serve_terminal(srv_r, srv_w, 100, 30));
        let (mut cr, mut cw) = tokio::io::split(client);

        let Msg::Opened { shell, .. } = read_msg(&mut cr).await.unwrap() else {
            panic!("没收到 Opened");
        };
        assert!(!shell.is_empty());

        #[cfg(windows)]
        let line = "echo p2pcli-$(1+1); exit 7\r";
        #[cfg(not(windows))]
        let line = "echo p2pcli-$((1+1)); exit 7\r";
        write_data(&mut cw, line.as_bytes()).await.unwrap();

        let mut out = Vec::new();
        let code = loop {
            let frame = match tokio::time::timeout(Duration::from_secs(20), read_frame(&mut cr)).await {
                Ok(f) => f.unwrap(),
                Err(_) => panic!("终端 20 秒没反应，已收到：{:?}", String::from_utf8_lossy(&out)),
            };
            match frame {
                Some(Frame::Data(d)) => {
                    if crate::client::asks_cursor_position(&d) {
                        write_data(&mut cw, b"\x1b[1;1R").await.unwrap();
                    }
                    out.extend_from_slice(&d)
                }
                Some(Frame::Msg(Msg::Exit { code })) => break code,
                Some(other) => panic!("意外的帧 {other:?}"),
                None => panic!("没收到 Exit 流就结束了"),
            }
        };
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("p2pcli-2"), "输出里没有命令结果：{text}");
        assert_eq!(code, Some(7));
        task.await.unwrap().unwrap();
    }
}

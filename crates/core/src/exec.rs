//! 被连方：在本机跑一条命令，不开终端，输出分标准输出和标准错误两路回传，最后给退出码。
//!
//! 和终端的区别：没有伪终端，命令的输入是空的，程序不会因为“连着终端”而换一副样子（颜色、分页），
//! 输出能原样拿回去。对面中途取消或走了，整组进程一起结束；命令自己放到后台的进程不管，
//! 和 ssh 一样，要长期跑的东西自己用 nohup 并重定向输出。

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until, timeout};

use crate::proto::{Msg, read_frame, write_data, write_msg};

/// 输出帧第一个字节：标准输出
pub const STDOUT: u8 = 1;
/// 输出帧第一个字节：标准错误
pub const STDERR: u8 = 2;
/// 每个输出帧最多带多少字节
const CHUNK: usize = 64 * 1024;
/// 命令最长多少字节，防对面塞超大的东西
pub const MAX_COMMAND_LEN: usize = 256 * 1024;
/// 超时最长设一周
const MAX_TIMEOUT_SECS: u64 = 7 * 24 * 3600;
/// 超时先请它自己退出，这么久还不退就强杀
const TERM_GRACE: Duration = Duration::from_secs(2);
/// 命令退出后，输出管道空闲多久就不等了（后台进程可能还攥着管道不放）
const DRAIN_IDLE: Duration = Duration::from_millis(500);

/// 跑一条命令，输出写进 send。`recv` 上对面发任何东西或者关流，都当成取消。
/// 返回 Ok 时 ExecExit 已经写出去了。
pub async fn serve<R, W>(recv: &mut R, send: &mut W, command: String, cwd: Option<String>, timeout_secs: Option<u64>) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    if command.trim().is_empty() {
        bail!("没写要跑的命令");
    }
    if command.len() > MAX_COMMAND_LEN {
        bail!("命令太长，最多 {} 字节", MAX_COMMAND_LEN);
    }
    let mut cmd = build(&command, cwd.as_deref())?;
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let mut child = cmd.spawn().context("启动命令失败")?;
    let mut group = Group { pid: child.id(), armed: true };
    tracing::debug!(pid = ?group.pid, "远程执行命令");

    let (tx, mut rx) = mpsc::channel::<(u8, Bytes)>(16);
    let mut pumps = Vec::new();
    if let Some(out) = child.stdout.take() {
        pumps.push(pump(out, STDOUT, tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        pumps.push(pump(err, STDERR, tx.clone()));
    }
    drop(tx);
    let _abort = AbortOnDrop(pumps);

    let deadline = timeout_secs.map(|s| Instant::now() + Duration::from_secs(s.clamp(1, MAX_TIMEOUT_SECS)));
    let mut timed_out = false;
    let mut kill_at: Option<Instant> = None;
    let mut cancel = std::pin::pin!(read_frame(recv));

    // 第一段：等命令退出，期间输出随到随转
    let status = loop {
        tokio::select! {
            chunk = rx.recv() => {
                if let Some((tag, data)) = chunk {
                    forward(send, tag, &data).await?;
                }
            }
            status = child.wait() => break status.context("等命令结束失败")?,
            _ = async { sleep_until(deadline.unwrap()).await }, if deadline.is_some() && !timed_out => {
                timed_out = true;
                group.term();
                kill_at = Some(Instant::now() + TERM_GRACE);
            }
            _ = async { sleep_until(kill_at.unwrap()).await }, if kill_at.is_some() => {
                group.kill();
                kill_at = None;
            }
            _ = &mut cancel => bail!("对面取消了命令"),
        }
    };
    // 正常结束就不再杀它的进程组了，它放到后台的进程由它自己负责
    group.armed = false;

    // 第二段：退出前最后的输出可能还在管道里，读到空闲为止
    while let Ok(Some((tag, data))) = timeout(DRAIN_IDLE, rx.recv()).await {
        forward(send, tag, &data).await?;
    }

    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    };
    #[cfg(not(unix))]
    let signal = None;
    write_msg(send, &Msg::ExecExit { code: status.code(), signal, timed_out }).await?;
    Ok(())
}

async fn forward<W: AsyncWrite + Unpin>(send: &mut W, tag: u8, data: &[u8]) -> Result<()> {
    let mut body = Vec::with_capacity(data.len() + 1);
    body.push(tag);
    body.extend_from_slice(data);
    write_data(send, &body).await
}

/// 把一路输出管道读出来，一块块送进 tx；读完或者对面不收了就结束
fn pump<R: AsyncRead + Unpin + Send + 'static>(mut pipe: R, tag: u8, tx: mpsc::Sender<(u8, Bytes)>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf = vec![0u8; CHUNK];
        loop {
            match pipe.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send((tag, Bytes::copy_from_slice(&buf[..n]))).await.is_err() {
                        break;
                    }
                }
            }
        }
    })
}

struct AbortOnDrop(Vec<JoinHandle<()>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        for h in &self.0 {
            h.abort();
        }
    }
}

/// 命令的进程组：没正常跑完就被丢掉（对面取消、断线、出错）时，整组结束
struct Group {
    pid: Option<u32>,
    armed: bool,
}

impl Group {
    /// 请它自己退出
    fn term(&self) {
        #[cfg(unix)]
        self.signal(libc::SIGTERM);
        #[cfg(windows)]
        self.kill();
    }

    fn kill(&self) {
        #[cfg(unix)]
        self.signal(libc::SIGKILL);
        #[cfg(windows)]
        if let Some(pid) = self.pid {
            use std::os::windows::process::CommandExt;
            // 连子进程一起结束
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .creation_flags(CREATE_NO_WINDOW)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    #[cfg(unix)]
    fn signal(&self, sig: i32) {
        if let Some(pid) = self.pid {
            // 子进程自己是一个进程组的组长（process_group(0)），负号是发给整组
            unsafe {
                libc::kill(-(pid as i32), sig);
            }
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        if self.armed {
            self.kill();
        }
    }
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// 命令用的工作目录：不写是用户目录，`~` 开头展开，相对路径从用户目录算起
fn work_dir(cwd: Option<&str>) -> Result<PathBuf> {
    let dir = match cwd.map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => crate::files::resolve_path(c),
        None => std::env::home_dir().context("找不到用户目录")?,
    };
    if !dir.is_dir() {
        bail!("工作目录不存在：{}", dir.display());
    }
    Ok(dir)
}

#[cfg(windows)]
fn build(command: &str, cwd: Option<&str>) -> Result<Command> {
    let (_, shell) = crate::pty::shell_command();
    let mut cmd = Command::new(format!("{shell}.exe"));
    // 和 AI 检测一样加载用户 profile，npm、pnpm 等装在 profile 里扩展的 PATH 下的命令才找得到
    cmd.args(["-NoLogo", "-NonInteractive", "-Command", command]);
    cmd.current_dir(work_dir(cwd)?);
    Ok(cmd)
}

#[cfg(not(windows))]
fn build(command: &str, cwd: Option<&str>) -> Result<Command> {
    let shell = portable_pty::CommandBuilder::new_default_prog().get_shell().to_string();
    let name = shell.rsplit('/').next().unwrap_or_default();
    // zsh、fish 的 PATH 常在交互配置里（nvm、pnpm），要带 -i；bash 带 -i 会往标准错误吐“没有作业控制”
    let flag = if matches!(name, "zsh" | "fish") { "-lic" } else { "-lc" };
    let mut cmd = Command::new(&shell);
    cmd.arg(flag).arg(command);
    cmd.current_dir(work_dir(cwd)?);
    // 从访达启动的程序常常没有 LANG，中文会变成转义
    if std::env::var_os("LANG").is_none() {
        cmd.env("LANG", "en_US.UTF-8");
    }
    Ok(cmd)
}

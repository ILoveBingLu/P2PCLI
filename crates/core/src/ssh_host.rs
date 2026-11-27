//! 被连方：给连过来的设备一个能用的 SSH 服务，不用对方去开系统的「远程登录」。
//!
//! Mac、Linux 上用系统自带的 sshd 程序，以当前用户的身份另起一个：只听 127.0.0.1 的随机端口，
//! 配置、主机密钥、授权密钥都放在 P2PCLI 自己的数据目录里，不碰 /etc/ssh，也不用管理员权限。
//! 只认公钥：连接方把 P2PCLI 替它管的公钥发过来，这里记进私有的 authorized_keys，登录就不用密码。
//! 外面连不到这个端口，只能经 P2PCLI 已经认证过的连接进来（[`crate::proto::Msg::OpenSsh`]）。
//!
//! 没有 sshd 程序（Windows）或者起不来时，退回系统自己的 SSH 服务（22 端口），那时按系统的方式登录。

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// 系统 SSH 服务的端口：没有用户态 sshd 时接它
pub const SYSTEM_PORT: u16 = 22;

pub struct UserSshd {
    dir: PathBuf,
    running: Mutex<Option<(Child, u16)>>,
}

impl UserSshd {
    pub fn new(data_dir: &Path) -> Self {
        Self { dir: data_dir.join("sshd"), running: Mutex::new(None) }
    }

    /// 系统的 sshd 程序（只在 Mac、Linux 上用）
    fn binary() -> Option<PathBuf> {
        if !cfg!(unix) {
            return None;
        }
        ["/usr/sbin/sshd", "/usr/local/sbin/sshd", "/opt/homebrew/sbin/sshd"].iter().map(PathBuf::from).find(|p| p.is_file())
    }

    /// 能不能起用户态的 sshd
    pub fn available() -> bool {
        Self::binary().is_some()
    }

    /// 记下连接方的公钥（每台设备一行，换了就替换）。只认一行 OpenSSH 格式的公钥
    pub async fn authorize(&self, peer: &str, pubkey: &str) -> Result<()> {
        let key = valid_key(pubkey)?;
        tokio::fs::create_dir_all(&self.dir).await?;
        let path = self.dir.join("authorized_keys");
        let mark = format!("p2pcli:{peer}");
        let old = tokio::fs::read_to_string(&path).await.unwrap_or_default();
        let mut lines: Vec<String> = old.lines().filter(|l| !l.ends_with(&mark) && !l.trim().is_empty()).map(str::to_string).collect();
        lines.push(format!("{key} {mark}"));
        let text = lines.join("\n") + "\n";
        if text != old {
            crate::store::write_private(&path, text.as_bytes())?;
            info!(peer = &peer[..8.min(peer.len())], "记下了一台设备的 SSH 公钥");
        }
        Ok(())
    }

    /// 用户态 sshd 的端口：没起就起一个，死了就重起
    pub async fn port(&self) -> Result<u16> {
        let mut running = self.running.lock().await;
        if let Some((child, port)) = running.as_mut() {
            if child.try_wait()?.is_none() {
                return Ok(*port);
            }
            warn!("用户态 sshd 退出了，重新起一个");
        }
        let (child, port) = self.start().await?;
        *running = Some((child, port));
        Ok(port)
    }

    async fn start(&self) -> Result<(Child, u16)> {
        let sshd = Self::binary().ok_or_else(|| anyhow!("这台电脑上没有 sshd 程序"))?;
        tokio::fs::create_dir_all(&self.dir).await?;
        let host_key = self.dir.join("host_ed25519");
        if !host_key.exists() {
            let out = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", "p2pcli-host", "-f"])
                .arg(&host_key)
                .output()
                .await
                .context("生成 SSH 主机密钥失败（找不到 ssh-keygen）")?;
            if !out.status.success() {
                bail!("生成 SSH 主机密钥失败：{}", String::from_utf8_lossy(&out.stderr).trim());
            }
        }
        let keys = self.dir.join("authorized_keys");
        if !keys.exists() {
            crate::store::write_private(&keys, b"")?;
        }
        // 先占一个空闲端口再放掉，交给 sshd
        let port = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        let config = self.dir.join("sshd_config");
        crate::store::write_private(&config, sshd_config(port, &host_key, &keys)?.as_bytes())?;
        let mut child = Command::new(&sshd)
            // -D 不进后台，-e 日志写标准错误；sshd 要用绝对路径起（它会重新执行自己）
            .args(["-D", "-e", "-f"])
            .arg(&config)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("起不了 {}", sshd.display()))?;
        if let Some(err) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(err).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("sshd：{line}");
                }
            });
        }
        // 等它开始监听
        for _ in 0..50 {
            if let Some(status) = child.try_wait()? {
                bail!("用户态 sshd 没起来（{status}）");
            }
            if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                info!(port, "用户态 sshd 已经起来了");
                return Ok((child, port));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let _ = child.kill().await;
        bail!("用户态 sshd 一直没开始监听")
    }

    /// 给连过来的设备接一个 SSH 服务：优先用户态的（免密码），起不来就接系统的 22 端口
    pub async fn connect(&self) -> Result<tokio::net::TcpStream> {
        if Self::available() {
            match self.port().await {
                Ok(port) => return crate::ports::connect_local(port).await,
                Err(e) => warn!("用户态 sshd 用不了，改接系统的 SSH 服务：{e:#}"),
            }
        }
        crate::ports::connect_local(SYSTEM_PORT).await.map_err(|_| {
            if cfg!(windows) {
                anyhow!("对方电脑没有开 SSH 服务：在对方电脑上装好「OpenSSH 服务器」（设置 → 系统 → 可选功能），并启动 sshd 服务")
            } else {
                anyhow!("对方电脑上的 SSH 服务起不来，也没有开系统的远程登录")
            }
        })
    }
}

/// 只认一行 OpenSSH 格式的公钥：类型 + base64，备注去掉（由这里加）
fn valid_key(text: &str) -> Result<String> {
    // .pub 文件末尾本来就有换行：首尾的空白去掉，中间夹着换行（多行）才不认
    let text = text.trim();
    let mut parts = text.split_whitespace();
    let (Some(kind), Some(body)) = (parts.next(), parts.next()) else { bail!("公钥格式不对") };
    let kind_ok = matches!(kind, "ssh-ed25519" | "ssh-rsa") || kind.starts_with("ecdsa-sha2-") || kind.starts_with("sk-");
    let body_ok = body.len() >= 40 && body.len() <= 4096 && body.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b));
    if !kind_ok || !body_ok || text.contains('\n') {
        bail!("公钥格式不对");
    }
    Ok(format!("{kind} {body}"))
}

/// sshd_config 里的路径：加双引号（Mac 的数据目录在 `Application Support` 下，带空格，
/// 不加引号 sshd 会把空格后面当成多余的参数，直接起不来）。路径里有引号、换行的不支持
fn quoted(path: &Path) -> Result<String> {
    let text = path.display().to_string();
    if text.contains(['"', '\n', '\r']) {
        bail!("P2PCLI 的数据目录路径里有引号或换行，起不了用户态 sshd：{text}");
    }
    Ok(format!("\"{text}\""))
}

fn sshd_config(port: u16, host_key: &Path, keys: &Path) -> Result<String> {
    // MaxAuthTries 调高：连接方的 ssh 会先把 ssh-agent 里的密钥一把把试，每试一把算一次，
    // 默认 6 次试满就断，轮不到 P2PCLI 管的那把。这里只认 P2PCLI 记下的公钥，多试几次没有风险
    // AuthorizedKeysFile 里的 % 是占位符（%h、%u），路径本身的 % 要写成 %%
    let keys = quoted(keys)?.replace('%', "%%");
    Ok(format!(
        "# 由 P2PCLI 生成：当前用户的 sshd，只听本机回环，只认 P2PCLI 记下的公钥\n\
         Port {port}\n\
         ListenAddress 127.0.0.1\n\
         HostKey {}\n\
         AuthorizedKeysFile {}\n\
         PubkeyAuthentication yes\n\
         MaxAuthTries 30\n\
         PasswordAuthentication no\n\
         KbdInteractiveAuthentication no\n\
         UsePAM no\n\
         StrictModes no\n\
         PidFile none\n\
         AllowTcpForwarding yes\n\
         X11Forwarding no\n\
         PermitTTY yes\n\
         PrintMotd no\n\
         LogLevel ERROR\n\
         Subsystem sftp internal-sftp\n",
        quoted(host_key)?,
        keys,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        let k = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHk2b3Vt5n0Zl1a2b3c4d5e6f7g8h9i0j1k2l3m4n5o6 me@host";
        assert_eq!(valid_key(k).unwrap(), "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHk2b3Vt5n0Zl1a2b3c4d5e6f7g8h9i0j1k2l3m4n5o6");
        assert_eq!(valid_key(&format!("{k}\n")).unwrap(), valid_key(k).unwrap(), "直接读 .pub 文件带着换行也认");
        assert!(valid_key("ssh-ed25519").is_err());
        assert!(valid_key("evil AAAAC3NzaC1lZDI1NTE5AAAAIHk2b3Vt5n0Zl1a2b3c4d5e6f7g8h9i0j1k2").is_err());
        assert!(valid_key("ssh-ed25519 AAAA;rm -rf / AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").is_err());
        assert!(valid_key("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHk2b3Vt5n0Zl1a2b3c4d5e6f7g8h9i0j1k2\nssh-rsa AAAA").is_err());
    }

    /// Mac 的数据目录带空格：路径要加引号，% 要转义；真有 sshd 时让它检查一遍配置
    #[test]
    fn config_quotes_paths() {
        let dir = std::env::temp_dir().join(format!("p2pcli sshd 50% test-{}", std::process::id()));
        let text = sshd_config(2222, &dir.join("host_ed25519"), &dir.join("authorized_keys")).unwrap();
        assert!(text.contains(&format!("HostKey \"{}\"\n", dir.join("host_ed25519").display())), "{text}");
        assert!(text.contains(&format!("AuthorizedKeysFile \"{}\"\n", dir.join("authorized_keys").display().to_string().replace('%', "%%"))), "{text}");
        assert!(sshd_config(2222, Path::new("/a\"b/k"), Path::new("/k")).is_err());
        let Some(sshd) = UserSshd::binary() else { return };
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.join("host_ed25519");
        let made = std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(&key).status();
        if !made.is_ok_and(|s| s.success()) {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let config = dir.join("sshd_config");
        std::fs::write(&config, &text).unwrap();
        let out = std::process::Command::new(&sshd).arg("-t").arg("-f").arg(&config).output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(out.status.success(), "sshd 不认这份配置：{}", String::from_utf8_lossy(&out.stderr));
    }

    #[tokio::test]
    async fn authorize_replaces_per_device() {
        let dir = std::env::temp_dir().join(format!("p2pcli-sshd-keys-{}", std::process::id()));
        let sshd = UserSshd::new(&dir);
        let a = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let b = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
        sshd.authorize("peer1", a).await.unwrap();
        sshd.authorize("peer2", a).await.unwrap();
        sshd.authorize("peer1", b).await.unwrap();
        let text = std::fs::read_to_string(dir.join("sshd/authorized_keys")).unwrap();
        assert_eq!(text.lines().count(), 2, "{text}");
        assert!(text.contains(&format!("{b} p2pcli:peer1")));
        assert!(text.contains(&format!("{a} p2pcli:peer2")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

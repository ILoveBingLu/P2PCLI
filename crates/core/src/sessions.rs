//! Persistent terminals live in tmux (macOS/Linux) or psmux (native Windows).
//! The authenticated remote key is part of every name; clients only supply a random suffix.

use std::path::PathBuf;
use std::process::Output;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use portable_pty::CommandBuilder;
use tokio::process::Command;

use crate::proto::TerminalSession;

pub struct SessionManager {
    namespace: String,
    gate: tokio::sync::Mutex<()>,
}

impl SessionManager {
    pub fn new(local_key: &str) -> Self {
        Self { namespace: format!("p2pcli-{local_key}"), gate: tokio::sync::Mutex::new(()) }
    }

    fn name(owner: &str, id: &str) -> Result<String> {
        if owner.len() != 64 || !owner.bytes().all(|b| b.is_ascii_hexdigit()) ||
            id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("会话标识不合法");
        }
        Ok(format!("p2pcli-{owner}-{id}"))
    }

    async fn command(&self, args: &[&str]) -> Result<Output> {
        let mut cmd = Command::new(executable()?);
        cmd.args(["-L", &self.namespace]).args(args).kill_on_drop(true);
        cmd.env_remove("TMUX").env_remove("PSMUX").env_remove("PSMUX_SESSION_NAME");
        cmd.env("TERM", "xterm-256color").env("COLORTERM", "truecolor").env("TERM_PROGRAM", "P2PCLI");
        if let Some(home) = std::env::home_dir() { cmd.current_dir(home); }
        if std::env::var_os("LANG").is_none() { cmd.env("LANG", "en_US.UTF-8"); }
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW for noninteractive control commands.
        tokio::time::timeout(Duration::from_secs(10), cmd.output()).await
            .context("会话管理器响应超时")?.context("无法运行会话管理器")
    }

    async fn checked(&self, args: &[&str]) -> Result<()> {
        let out = self.command(args).await?;
        if !out.status.success() {
            bail!("会话操作失败：{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }

    pub async fn list(&self, owner: &str) -> Result<Vec<TerminalSession>> {
        let out = self.command(&["list-sessions", "-F", "#{session_name}\t#{session_created}\t#{session_attached}"]).await?;
        if !out.status.success() {
            let error = String::from_utf8_lossy(&out.stderr).to_lowercase();
            if error.contains("no server") || error.contains("no sessions") ||
                (error.contains("error connecting to") && (error.contains("no such file") || error.contains("connection refused"))) {
                return Ok(Vec::new());
            }
            bail!("读取会话列表失败：{}", error.trim());
        }
        let prefix = format!("p2pcli-{owner}-");
        let mut sessions = Vec::new();
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut parts = line.split('\t');
            let Some(id) = parts.next().and_then(|n| n.strip_prefix(&prefix)) else { continue };
            if Self::name(owner, id).is_err() { continue; }
            sessions.push(TerminalSession {
                id: id.to_string(),
                created_at: parts.next().and_then(|v| v.parse().ok()).unwrap_or(0),
                attached: parts.next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0) > 0,
            });
        }
        sessions.sort_by_key(|s| s.created_at);
        Ok(sessions)
    }

    pub async fn attach(&self, owner: &str, requested: Option<&str>, cols: u16, rows: u16) -> Result<(CommandBuilder, String)> {
        let _gate = self.gate.lock().await;
        let id = requested.map(str::to_string).unwrap_or_else(|| format!("{:032x}", rand::random::<u128>()));
        let name = Self::name(owner, &id)?;
        if requested.is_some() {
            self.checked(&["has-session", "-t", &name]).await.context("会话已结束或不属于当前设备")?;
        } else {
            let cols = cols.max(2).to_string();
            let rows = rows.max(2).to_string();
            self.checked(&["new-session", "-d", "-s", &name, "-x", &cols, "-y", &rows]).await?;
            // Per-session options only: never change the user's unrelated tmux sessions.
            self.checked(&["set-option", "-t", &name, "destroy-unattached", "off"]).await?;
        }
        let mut cmd = CommandBuilder::new(executable()?);
        cmd.args(["-L", &self.namespace, "attach-session", "-t", &name]);
        cmd.env_remove("TMUX");
        cmd.env_remove("PSMUX");
        cmd.env_remove("PSMUX_SESSION_NAME");
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "P2PCLI");
        if std::env::var_os("LANG").is_none() { cmd.env("LANG", "en_US.UTF-8"); }
        Ok((cmd, id))
    }

    pub async fn end(&self, owner: &str, id: &str) -> Result<()> {
        let _gate = self.gate.lock().await;
        let name = Self::name(owner, id)?;
        self.checked(&["kill-session", "-t", &name]).await
    }
}

pub fn executable() -> Result<PathBuf> {
    let name = if cfg!(windows) { "psmux.exe" } else { "tmux" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(home) = std::env::home_dir() { dirs.push(home.join(".cargo/bin")); }
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        dirs.push(local.join("Microsoft/WinGet/Links"));
        dirs.push(local.join("P2PCLI/tools/psmux-3.4.0"));
    }
    #[cfg(not(windows))]
    dirs.extend([PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin"), PathBuf::from("/usr/bin")]);
    if let Some(path) = dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file()) { return Ok(path); }
    bail!("{}", if cfg!(windows) {
        "被连设备未安装 psmux，请在被连设备运行 winget install psmux 后重试"
    } else {
        "被连设备未安装 tmux；macOS 可运行 brew install tmux，安装后重试"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_targets_are_bound_to_authenticated_owner() {
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let id = "1".repeat(32);
        assert_ne!(SessionManager::name(&a, &id).unwrap(), SessionManager::name(&b, &id).unwrap());
        for bad in ["", "-a", "work:0", "../session", "x;kill-server"] {
            assert!(SessionManager::name(&a, bad).is_err());
        }
    }
}

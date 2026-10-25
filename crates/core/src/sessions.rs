//! Persistent terminals live in tmux (macOS/Linux) or psmux (native Windows).
//! The authenticated remote key is part of every name; clients only supply a random suffix.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use portable_pty::CommandBuilder;
use tokio::process::Command;

use crate::proto::{TerminalDetails, TerminalSession};

const SESSION_FORMAT: &str = "#{session_name}\t#{session_created}\t#{session_attached}\t#{window_index}\t#{window_name}\t#{session_windows}\t#{pane_current_command}\t#{pane_current_path}";

pub struct SessionManager {
    namespace: String,
    program: Option<PathBuf>,
    gate: tokio::sync::Mutex<()>,
}

impl SessionManager {
    pub fn new(local_key: &str, program: Option<PathBuf>) -> Self {
        Self { namespace: format!("p2pcli-{local_key}"), program, gate: tokio::sync::Mutex::new(()) }
    }

    fn name(owner: &str, id: &str) -> Result<String> {
        if owner.len() != 64 || !owner.bytes().all(|b| b.is_ascii_hexdigit()) ||
            id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("会话标识不合法");
        }
        Ok(format!("p2pcli-{owner}-{id}"))
    }

    async fn command(&self, args: &[&str]) -> Result<Output> {
        let mut cmd = Command::new(executable(self.program.as_deref())?);
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
        let out = self.command(&["list-sessions", "-F", SESSION_FORMAT]).await?;
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
            let mut parts = line.splitn(8, '\t');
            let Some(id) = parts.next().and_then(|n| n.strip_prefix(&prefix)) else { continue };
            if Self::name(owner, id).is_err() { continue; }
            let mut session = TerminalSession {
                id: id.to_string(),
                created_at: parts.next().and_then(|v| v.parse().ok()).unwrap_or(0),
                attached: parts.next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0) > 0,
                details: None,
            };
            let details = TerminalDetails {
                window_index: parts.next().and_then(|v| v.parse().ok()),
                window_name: status_text(parts.next()),
                window_count: parts.next().and_then(|v| v.parse().ok()),
                command: status_text(parts.next()),
                current_path: status_text(parts.next()),
            };
            if details.window_index.is_some() || details.window_name.is_some() || details.command.is_some() || details.current_path.is_some() {
                session.details = Some(details);
            }
            sessions.push(session);
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
        // 新建和恢复旧会话都隐藏原生状态栏；只改当前受管会话，不改全局选项。
        self.checked(&["set-option", "-t", &name, "status", "off"]).await?;
        let mut cmd = CommandBuilder::new(executable(self.program.as_deref())?);
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

fn status_text(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || value.starts_with("#{") { return None; }
    let text: String = value.chars().filter(|c| !c.is_control()).take(2048).collect();
    (!text.is_empty()).then_some(text)
}

fn executable(bundled: Option<&Path>) -> Result<PathBuf> {
    // Desktop builds always supply an absolute sidecar path. Never silently
    // switch to an unrelated executable on PATH if the installation is damaged.
    if let Some(path) = bundled {
        if !path.is_absolute() || !path.is_file() {
            bail!("内置会话组件缺失，请重新安装 P2PCLI；开发环境请恢复仓库的 binaries 组件文件并重新启动开发模式：{}", path.display());
        }
        return Ok(path.to_path_buf());
    }
    // Standalone core/CLI users may still explicitly rely on their own tools.
    let name = if cfg!(windows) { "psmux.exe" } else { "tmux" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(home) = std::env::home_dir() { dirs.push(home.join(".cargo/bin")); }
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        dirs.push(local.join("Microsoft/WinGet/Links"));
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
    fn desktop_uses_only_the_explicit_sidecar_path() {
        let existing = std::env::current_exe().unwrap();
        assert_eq!(executable(Some(&existing)).unwrap(), existing);
        let missing = existing.with_file_name(format!("missing-mux-{:032x}", rand::random::<u128>()));
        assert!(executable(Some(&missing)).is_err());
        assert!(executable(Some(Path::new("psmux.exe"))).is_err());
    }
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

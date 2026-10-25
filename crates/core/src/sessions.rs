//! Persistent terminals live in tmux (macOS/Linux) or psmux (native Windows).
//! The authenticated remote key is part of every name; clients only supply a random suffix.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use portable_pty::CommandBuilder;
use tokio::process::Command;

use crate::agents::{self, AgentInfo, Processes, Quotas, Settings};
use crate::proto::{TerminalDetails, TerminalSession};

// 标题放最后：里面可能有制表符，分割时多出来的都归它
const SESSION_FORMAT: &str = "#{session_name}\t#{session_created}\t#{session_attached}\t#{status}\t#{window_index}\t#{window_name}\t#{session_windows}\t#{pane_current_command}\t#{pane_current_path}\t#{pane_pid}\t#{pane_title}";
const HEALTH_FORMAT: &str = "#{session_name}\t#{pid}\t#{socket_path}";
const LAUNCHER_FORMAT: &str = "#{pane_pid}\t#{pane_current_command}\t#{cursor_x}\t#{cursor_y}\t#{alternate_on}\t#{pane_in_mode}";

#[derive(Default)]
struct Health {
    sessions: BTreeMap<String, String>,
    last_error: Option<String>,
    last_capture: Option<std::time::Instant>,
}

pub struct SessionManager {
    namespace: String,
    program: Option<PathBuf>,
    gate: tokio::sync::Mutex<()>,
    health: std::sync::Mutex<Health>,
    /// 认会话里的 AI 用的进程表
    processes: std::sync::Mutex<Processes>,
    /// 各种 AI 账号的额度
    quotas: std::sync::Mutex<Quotas>,
    /// 会话里 AI 现在的模型、强度、权限模式
    settings: std::sync::Mutex<Settings>,
    installed: tokio::sync::Mutex<Option<(std::time::Instant, Vec<crate::launchers::InstalledAgent>)>>,
}

impl SessionManager {
    pub fn new(local_key: &str, program: Option<PathBuf>) -> Self {
        let namespace = format!("p2pcli-{local_key}");
        tracing::info!(%namespace, ?program, app_pid = std::process::id(), "会话管理器初始化");
        Self {
            namespace,
            program,
            gate: tokio::sync::Mutex::new(()),
            health: Default::default(),
            processes: Default::default(),
            quotas: Default::default(),
            settings: Default::default(),
            installed: Default::default(),
        }
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
        let out = self.command(args).await.inspect_err(|error| {
            tracing::warn!(namespace = %self.namespace, operation = args[0], error = %error, "会话管理命令无法执行");
        })?;
        if !out.status.success() {
            tracing::warn!(namespace = %self.namespace, operation = args[0], status = %out.status,
                stderr = %String::from_utf8_lossy(&out.stderr).trim(), "会话管理命令失败");
            bail!("会话操作失败：{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }

    async fn hide_status(&self, name: &str) -> Result<()> {
        self.checked(&["set-option", "-t", name, "status", "off"]).await?;
        // psmux can return success with a warning when no session was targeted.
        // Read back the effective option before trusting that the bar is hidden.
        let out = self.command(&["show-options", "-t", name, "-v", "status"]).await?;
        if !out.status.success() || !status_hidden(String::from_utf8_lossy(&out.stdout).trim()) {
            bail!("无法确认原生终端状态栏已关闭：{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }

    pub async fn list(&self, owner: &str) -> Result<Vec<TerminalSession>> {
        let out = self.command(&["list-sessions", "-F", SESSION_FORMAT]).await?;
        if !out.status.success() {
            let error = String::from_utf8_lossy(&out.stderr).to_lowercase();
            if server_absent(&error) {
                return Ok(Vec::new());
            }
            bail!("读取会话列表失败：{}", error.trim());
        }
        let prefix = format!("p2pcli-{owner}-");
        let mut sessions = Vec::new();
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut parts = line.splitn(11, '\t');
            let Some(id) = parts.next().and_then(|n| n.strip_prefix(&prefix)) else { continue };
            if Self::name(owner, id).is_err() { continue; }
            let mut session = TerminalSession {
                id: id.to_string(),
                created_at: parts.next().and_then(|v| v.parse().ok()).unwrap_or(0),
                attached: parts.next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0) > 0,
                details: None,
                agent: None,
            };
            // Also repair restored sessions or options changed inside the terminal.
            // Normal polling needs no extra process when status is already off.
            if !status_hidden(parts.next().unwrap_or_default()) {
                self.hide_status(&Self::name(owner, id)?).await?;
            }
            let details = TerminalDetails {
                shell_ready: false,
                window_index: parts.next().and_then(|v| v.parse().ok()),
                window_name: status_text(parts.next()),
                window_count: parts.next().and_then(|v| v.parse().ok()),
                command: status_text(parts.next()),
                current_path: status_text(parts.next()),
            };
            if details.window_index.is_some() || details.window_name.is_some() || details.command.is_some() || details.current_path.is_some() {
                session.details = Some(details);
            }
            let pane_pid = parts.next().and_then(|v| v.trim().parse().ok());
            let title = parts.next().unwrap_or_default();
            session.agent = self.agent(&Self::name(owner, id)?, pane_pid, title).await;
            if session.agent.is_none() {
                if let Some(details) = session.details.as_mut() {
                    if details.command.as_deref().is_some_and(crate::launchers::is_shell) {
                        details.shell_ready = self.shell_ready(&Self::name(owner, id)?, false).await.unwrap_or(false);
                    }
                }
            }
            sessions.push(session);
        }
        sessions.sort_by_key(|s| s.created_at);
        Ok(sessions)
    }

    /// 会话里跑的是不是命令行 AI，是的话再读一次屏幕判断它在干什么。屏幕内容只在这里用，不往外发
    async fn agent(&self, name: &str, pane_pid: Option<u32>, title: &str) -> Option<AgentInfo> {
        let (pid, kind, display) = self.processes.lock().unwrap().agent_under(pane_pid?)?;
        let screen = match self.command(&["capture-pane", "-p", "-t", name]).await {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
            _ => String::new(),
        };
        // 屏幕已经明确显示完成或空闲时立即生效；不能用固定延迟把完成状态重新盖成“干活中”。
        let state = agents::state(title, &screen);
        let quota = self.quotas.lock().unwrap().get(kind);
        let settings = self.settings.lock().unwrap().get(kind, pid, &screen);
        Some(AgentInfo { kind: kind.to_string(), name: display.to_string(), state, quota, settings })
    }

    pub async fn attach(&self, owner: &str, requested: Option<&str>, cols: u16, rows: u16) -> Result<(CommandBuilder, String)> {
        let _gate = self.gate.lock().await;
        let id = requested.map(str::to_string).unwrap_or_else(|| format!("{:032x}", rand::random::<u128>()));
        let name = Self::name(owner, &id)?;
        tracing::info!(namespace = %self.namespace, terminal = %name, restoring = requested.is_some(), "请求附着后台会话");
        if requested.is_some() {
            if let Err(error) = self.checked(&["has-session", "-t", &name]).await {
                let backends = self.health.lock().unwrap().sessions.values().cloned().collect();
                self.capture_diagnostics(backends);
                return Err(error.context("无法恢复后台会话（请查看被控端日志）"));
            }
        } else {
            let cols = cols.max(2).to_string();
            let rows = rows.max(2).to_string();
            self.checked(&["new-session", "-d", "-s", &name, "-x", &cols, "-y", &rows]).await?;
            self.health.lock().unwrap().sessions.insert(name.clone(), String::new());
            tracing::info!(namespace = %self.namespace, terminal = %name, "后台会话已创建");
            // Per-session options only: never change the user's unrelated tmux sessions.
            self.checked(&["set-option", "-t", &name, "destroy-unattached", "off"]).await?;
        }
        // 新建和恢复旧会话都隐藏原生状态栏；只改当前受管会话，不改全局选项。
        self.hide_status(&name).await?;
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
        tracing::info!(namespace = %self.namespace, terminal = %name, "用户请求结束后台会话");
        self.checked(&["kill-session", "-t", &name]).await?;
        self.health.lock().unwrap().sessions.remove(&name);
        tracing::info!(namespace = %self.namespace, terminal = %name, "后台会话已按请求结束");
        Ok(())
    }

    pub async fn installed_agents(&self, refresh: bool) -> Result<Vec<crate::launchers::InstalledAgent>> {
        let mut cache = self.installed.lock().await;
        if !refresh {
            if let Some((at, agents)) = cache.as_ref() {
                if at.elapsed() < Duration::from_secs(300) { return Ok(agents.clone()); }
            }
        }
        let agents = crate::launchers::scan().await?;
        *cache = Some((std::time::Instant::now(), agents.clone()));
        Ok(agents)
    }

    async fn shell_ready(&self, name: &str, fresh: bool) -> Result<bool> {
        let out = self.command(&["display-message", "-p", "-t", name, LAUNCHER_FORMAT]).await?;
        if !out.status.success() { return Ok(false); }
        let text = String::from_utf8_lossy(&out.stdout);
        let fields: Vec<_> = text.trim_end().split('\t').collect();
        if fields.len() != 6 || fields[4] != "0" || fields[5] != "0" { return Ok(false); }
        let (Ok(pid), Ok(x), Ok(y)) = (fields[0].parse::<u32>(), fields[2].parse::<usize>(), fields[3].parse::<usize>()) else { return Ok(false) };
        if !self.processes.lock().unwrap().idle_shell(pid, fields[1], fresh) { return Ok(false); }
        let screen = self.command(&["capture-pane", "-p", "-t", name]).await?;
        if !screen.status.success() { return Ok(false); }
        let text = String::from_utf8_lossy(&screen.stdout);
        let Some(line) = text.lines().nth(y) else { return Ok(false) };
        // 宽字符占多列；光标必须在整行文字之后，不能位于已输入命令的中间。
        Ok(x >= line.trim_end().chars().count() && crate::launchers::empty_prompt(line))
    }

    pub async fn launch_agent(&self, owner: &str, id: &str, kind: &str, allow_bypass: bool) -> Result<()> {
        let name = Self::name(owner, id)?;
        let agents = self.installed_agents(false).await?;
        let agent = agents.iter().find(|agent| agent.kind == kind).context("该 AI 未检测到安装，请刷新列表")?;
        if allow_bypass && kind != "claude" { bail!("该启动选项仅适用于 Claude Code"); }
        let _gate = self.gate.lock().await;
        // 客户端的轮询结果可能过期；必须在被控端临启动前再次检查。
        if !self.shell_ready(&name, true).await? {
            bail!("终端不在空闲提示符，请先完成当前输入或退出正在运行的程序");
        }
        let command = format!("{}{}", agent.command, if allow_bypass { " --allow-dangerously-skip-permissions" } else { "" });
        self.checked(&["send-keys", "-t", &name, "-l", &command]).await?;
        self.checked(&["send-keys", "-t", &name, "Enter"]).await?;
        Ok(())
    }

    /// Observe even detached sessions. Only query; never start, recreate or terminate a server.
    pub async fn check_health(&self) {
        let gate = self.gate.lock().await;
        let output = self.command(&["list-sessions", "-F", HEALTH_FORMAT]).await;
        let (current, error) = match output {
            Ok(out) if out.status.success() => {
                let current = String::from_utf8_lossy(&out.stdout).lines().filter_map(|line| {
                    let (name, details) = line.split_once('\t')?;
                    // Ignore unrelated or malformed names; never log terminal contents.
                    let (owner, id) = name.strip_prefix("p2pcli-")?.split_once('-')?;
                    Self::name(owner, id).ok()?;
                    Some((name.to_string(), details.to_string()))
                }).collect::<BTreeMap<_, _>>();
                (Some(current), None)
            }
            Ok(out) => {
                let error = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let gone = server_absent(&error);
                (gone.then(BTreeMap::new), Some(format!("{}: {error}", out.status)))
            }
            Err(error) => (None, Some(format!("{error:#}"))),
        };
        let missing = {
            let mut health = self.health.lock().unwrap();
            if health.last_error != error {
                if let Some(error) = &error {
                    // An empty namespace before the first session is normal.
                    if !health.sessions.is_empty() || current.is_none() {
                        tracing::warn!(namespace = %self.namespace, %error, "后台会话健康检查失败");
                    }
                } else if health.last_error.is_some() {
                    tracing::info!(namespace = %self.namespace, "后台会话查询恢复正常");
                }
            }
            health.last_error = error;
            let mut missing = Vec::new();
            if let Some(current) = current {
                for (name, details) in &health.sessions {
                    if !current.contains_key(name) {
                        tracing::warn!(namespace = %self.namespace, terminal = %name, last_backend = %details,
                            "后台会话消失（未收到结束请求；可能是 shell 退出、服务退出或连接端点丢失）");
                        missing.push(details.clone());
                    }
                }
                for (name, details) in &current {
                    if health.sessions.get(name) != Some(details) {
                        tracing::info!(namespace = %self.namespace, terminal = %name, backend_pid_socket = %details, "后台会话状态已记录");
                    }
                }
                health.sessions = current;
            }
            missing
        };
        drop(gate);
        if !missing.is_empty() {
            self.capture_diagnostics(missing);
        }
    }

    fn capture_diagnostics(&self, backends: Vec<String>) {
        let mut health = self.health.lock().unwrap();
        if health.last_capture.is_some_and(|at| at.elapsed() < Duration::from_secs(30)) { return; }
        health.last_capture = Some(std::time::Instant::now());
        let namespace = self.namespace.clone();
        // Evidence gathering must not delay the UI's failure response or hold the session gate.
        tokio::spawn(async move { crate::session_diagnostics::capture(&namespace, &backends).await; });
    }
}

fn server_absent(error: &str) -> bool {
    let error = error.to_lowercase();
    error.contains("no server") || error.contains("no sessions") ||
        (error.contains("error connecting to") && (error.contains("no such file") || error.contains("connection refused")))
}

fn status_hidden(value: &str) -> bool {
    matches!(value, "off" | "0")
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

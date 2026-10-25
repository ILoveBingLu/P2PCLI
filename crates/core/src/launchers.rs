//! AI 启动器：在被连接的电脑上查找命令，不启动 AI 来探测版本。
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::process::Command;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstalledAgent {
    pub kind: String,
    pub name: String,
    pub command: String,
}

pub fn is_shell(command: &str) -> bool {
    let name = command.rsplit(['/', '\\']).next().unwrap_or(command).trim_start_matches('-').to_ascii_lowercase();
    matches!(name.trim_end_matches(".exe"), "sh" | "bash" | "zsh" | "fish" | "pwsh" | "powershell" | "cmd")
}

/// 保守识别空提示符：不能把续行提示符或已输入命令当成空行。
pub fn empty_prompt(line: &str) -> bool {
    let line = line.trim();
    let Some(mark) = line.chars().last() else { return false };
    if !"$#%❯➜>".contains(mark) { return false; }
    let prefix = line[..line.len() - mark.len_utf8()].trim();
    if prefix.chars().any(|c| "$#%❯➜>".contains(c)) { return false; }
    if mark == '>' {
        return prefix.starts_with("PS ") || prefix.contains('@')
            || (prefix.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                && prefix.as_bytes().get(1) == Some(&b':') && prefix.as_bytes().get(2) == Some(&b'\\'));
    }
    prefix.is_empty() || prefix.contains('@') || (prefix.starts_with('[') && prefix.ends_with(']'))
        || prefix.starts_with('/') || prefix.starts_with('~')
}

pub async fn scan() -> Result<Vec<InstalledAgent>> {
    let candidates = crate::agents::launcher_candidates();
    let names: Vec<_> = candidates.iter().flat_map(|(_, _, names)| names.iter().copied()).collect();
    #[cfg(windows)]
    let mut cmd = {
        let (_, shell) = crate::pty::shell_command();
        let mut cmd = Command::new(format!("{shell}.exe"));
        // 加载用户 profile，覆盖 npm/pnpm 等在 profile 里扩展的 PATH。
        let script = names.iter().map(|name| format!(
            "if (Get-Command '{name}' -CommandType Application,ExternalScript -ErrorAction SilentlyContinue) {{ Write-Output '__P2PCLI_AI__{name}' }}"
        )).collect::<Vec<_>>().join("; ");
        cmd.args(["-NoLogo", "-NonInteractive", "-Command", &script]);
        cmd.creation_flags(0x08000000);
        cmd
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let shell = portable_pty::CommandBuilder::new_default_prog().get_shell().to_string();
        let mut cmd = Command::new(shell);
        let script = names.iter().map(|name| format!(
            "command -v {name} >/dev/null 2>&1 && printf '__P2PCLI_AI__{name}\\n'"
        )).collect::<Vec<_>>().join("; ") + "; true";
        cmd.args(["-lic", &script]);
        cmd
    };
    if let Some(home) = std::env::home_dir() { cmd.current_dir(home); }
    cmd.kill_on_drop(true).stdin(std::process::Stdio::null());
    let out = tokio::time::timeout(Duration::from_secs(10), cmd.output()).await
        .context("检测 AI 超时，请检查被连接电脑的 Shell 配置")??;
    if !out.status.success() { bail!("被连接电脑的 Shell 无法完成 AI 检测"); }
    let found: std::collections::HashSet<_> = String::from_utf8_lossy(&out.stdout).lines()
        .filter_map(|line| line.trim().strip_prefix("__P2PCLI_AI__").map(str::to_string)).collect();
    Ok(candidates.into_iter().filter_map(|(kind, name, commands)| {
        let command = commands.iter().find(|command| found.contains(**command))?;
        Some(InstalledAgent { kind: kind.into(), name: name.into(), command: (*command).into() })
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_empty_prompts_are_launchable() {
        for line in ["PS C:\\work> ", "C:\\work>", "me@mac project % ", "[me@host dir]$ ", "❯ ", "$ "] {
            assert!(empty_prompt(line), "{line}");
        }
        for line in ["", "> ", ">> ", "PS C:\\work> claude", "me@mac project % echo $", "Password:", "Do you agree?", "❯ draft"] {
            assert!(!empty_prompt(line), "{line}");
        }
        assert!(is_shell("/bin/zsh"));
        assert!(is_shell("powershell.exe"));
        assert!(!is_shell("vim"));
    }
}

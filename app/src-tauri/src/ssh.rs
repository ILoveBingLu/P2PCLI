//! 用本机 VS Code（或 Cursor）的 Remote-SSH 打开别的设备上的文件夹。SSH 走 P2PCLI 的管道：
//! `p2pcli pipe <设备> ssh` 当 ProxyCommand，不用对方开公网端口、不用跳板机。对方是 Mac、Linux 时
//! 由对方的 P2PCLI 起一个只给这条通道用的 sshd，用 P2PCLI 替本机管的密钥登录，不用开远程登录、不用密码。
//!
//! 只维护 `~/.ssh/p2pcli_hosts` 这一个文件（整个重写）；`~/.ssh/config` 只在用户同意后在最前面加一行
//! `Include ~/.ssh/p2pcli_hosts`，不动原有的内容。每台设备一个别名（p2p-设备名），
//! HostKeyAlias 按设备公钥区分，换了设备名、IP 也不会认错主机密钥。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::AppState;

const HOSTS_FILE: &str = "p2pcli_hosts";
/// 加在 ~/.ssh/config 最前面的那一行（Include 放在任何 Host 之前才对所有连接生效）
const INCLUDE_LINE: &str = "Include ~/.ssh/p2pcli_hosts";
/// 记着为哪些设备写过别名
const STORE_FILE: &str = "ssh_hosts.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct SshHost {
    alias: String,
    endpoint_id: String,
    /// 登录对方用的用户名（对方的当前用户）
    user: String,
    name: String,
}

#[derive(Serialize)]
pub(crate) struct SshStatus {
    /// ~/.ssh/config 里已经有 Include 那一行
    include: bool,
    config_path: String,
    hosts_path: String,
}

#[derive(Serialize)]
pub(crate) struct OpenedInEditor {
    /// SSH 别名，终端里也能 ssh 它
    alias: String,
    /// 打开的是对方电脑上的哪个目录
    path: String,
}

fn ssh_dir() -> Result<PathBuf, String> {
    Ok(std::env::home_dir().ok_or("找不到用户目录")?.join(".ssh"))
}

/// ~/.ssh/config 里有没有 Include 了 p2pcli_hosts（不管写成什么路径）
fn has_include(config: &str) -> bool {
    config.lines().any(|l| {
        let l = l.trim().to_lowercase();
        l.starts_with("include") && l.contains(HOSTS_FILE)
    })
}

fn status() -> Result<SshStatus, String> {
    Ok(status_in(&ssh_dir()?))
}

fn status_in(dir: &Path) -> SshStatus {
    let config = dir.join("config");
    let text = std::fs::read_to_string(&config).unwrap_or_default();
    SshStatus { include: has_include(&text), config_path: config.display().to_string(), hosts_path: dir.join(HOSTS_FILE).display().to_string() }
}

/// 别名：p2p- 加设备名里的字母数字（中文等去掉），空了就用公钥开头；和别的设备重了就加公钥开头
fn alias_for(name: &str, endpoint_id: &str, taken: &[SshHost]) -> String {
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    let short = &endpoint_id[..6.min(endpoint_id.len())];
    let base = if slug.is_empty() { format!("p2p-{short}") } else { format!("p2p-{slug}") };
    if taken.iter().any(|h| h.alias == base && h.endpoint_id != endpoint_id) { format!("{base}-{short}") } else { base }
}

/// ProxyCommand 里的程序路径：有空格就加引号
fn quote(path: &str) -> String {
    if path.contains(' ') { format!("\"{path}\"") } else { path.to_string() }
}

/// P2PCLI 替本机管的 SSH 密钥（私钥路径、公钥）：没有就用 ssh-keygen 生成一把。生成不了（没装 ssh-keygen）返回空，
/// 那时只能用对方系统的 SSH 服务、按它的方式登录
fn client_key(data_dir: &Path) -> Option<(PathBuf, String)> {
    let key = data_dir.join("ssh").join("id_ed25519");
    let public = key.with_extension("pub");
    if !key.exists() || !public.exists() {
        std::fs::create_dir_all(key.parent()?).ok()?;
        let _ = std::fs::remove_file(&key);
        let _ = std::fs::remove_file(&public);
        let mut cmd = std::process::Command::new("ssh-keygen");
        cmd.args(["-q", "-t", "ed25519", "-N", "", "-C", "p2pcli", "-f"]).arg(&key);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        match cmd.output() {
            Ok(out) if out.status.success() => tracing::info!("生成了 P2PCLI 用的 SSH 密钥"),
            Ok(out) => {
                tracing::warn!("生成 SSH 密钥失败：{}", String::from_utf8_lossy(&out.stderr).trim());
                return None;
            }
            Err(e) => {
                tracing::warn!("生成 SSH 密钥失败（没有 ssh-keygen）：{e}");
                return None;
            }
        }
    }
    // 私钥只能本人读，不然 ssh 不用它
    if lock_down(&key).is_err() {
        return None;
    }
    let text = std::fs::read_to_string(&public).ok()?.trim().to_string();
    Some((key, text))
}

fn render(hosts: &[SshHost], cli: &str, key: Option<&Path>) -> String {
    let mut out = String::from(
        "# 由 P2PCLI 维护，会被整个重写，请不要手改。\n# SSH 走 P2PCLI 的加密通道（p2pcli pipe），要 P2PCLI 开着。\n\n",
    );
    for h in hosts {
        out.push_str(&format!(
            "# {}\nHost {}\n  HostName {}\n  User {}\n  ProxyCommand {} pipe {} ssh\n  HostKeyAlias p2pcli-{}\n  ServerAliveInterval 30\n{}\n",
            h.name.replace('\n', " "),
            h.alias,
            h.alias,
            h.user,
            quote(cli),
            h.endpoint_id,
            &h.endpoint_id[..16.min(h.endpoint_id.len())],
            key.map(|k| format!("  IdentityFile {}\n", quote(&k.display().to_string()))).unwrap_or_default(),
        ));
    }
    out
}

fn load(data_dir: &Path) -> Vec<SshHost> {
    std::fs::read(data_dir.join(STORE_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建不了 {}：{e}", dir.display()))?;
    }
    p2pcli_core::store::write_private(path, bytes).map_err(|e| format!("写不了 {}：{e:#}", path.display()))?;
    lock_down(path)
}

/// Windows 版 OpenSSH 只读“只有本人、SYSTEM、Administrators 有权限”的配置文件：从上级目录继承来的
/// 别的用户、组（比如沙箱用户组）的权限都要去掉，不然它报 Bad permissions 不读。Unix 上 write_private 已经是 600
#[cfg(windows)]
fn lock_down(path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(d), Ok(u)) => format!("{d}\\{u}"),
        (_, Ok(u)) => u,
        _ => return Err("不知道当前用户名，没法设置 SSH 配置文件的权限".into()),
    };
    let out = std::process::Command::new("icacls")
        .arg(path)
        // 不再继承上级目录的权限，只留本人、SYSTEM（S-1-5-18）、Administrators（S-1-5-32-544）
        .args(["/inheritance:r", "/grant:r", &format!("{user}:F"), "*S-1-5-18:F", "*S-1-5-32-544:F"])
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| format!("设置 {} 的权限失败：{e}", path.display()))?;
    if !out.status.success() {
        return Err(format!("设置 {} 的权限失败：{}", path.display(), String::from_utf8_lossy(&out.stdout).trim()));
    }
    Ok(())
}

#[cfg(not(windows))]
fn lock_down(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// 记下这台设备的别名，重写 ssh_dir 里的 p2pcli_hosts
fn upsert(data_dir: &Path, ssh_dir: &Path, name: &str, endpoint_id: &str, user: &str, cli: &str, key: Option<&Path>) -> Result<String, String> {
    let mut hosts = load(data_dir);
    let alias = hosts.iter().find(|h| h.endpoint_id == endpoint_id).map(|h| h.alias.clone()).unwrap_or_else(|| alias_for(name, endpoint_id, &hosts));
    hosts.retain(|h| h.endpoint_id != endpoint_id);
    hosts.push(SshHost { alias: alias.clone(), endpoint_id: endpoint_id.into(), user: user.into(), name: name.into() });
    hosts.sort_by(|a, b| a.alias.cmp(&b.alias));
    write_private(&data_dir.join(STORE_FILE), &serde_json::to_vec_pretty(&hosts).map_err(|e| e.to_string())?)?;
    write_private(&ssh_dir.join(HOSTS_FILE), render(&hosts, cli, key).as_bytes())?;
    Ok(alias)
}

/// 编辑器的远程链接：vscode://vscode-remote/ssh-remote+别名/路径。Windows 的路径写成 /C:/Users/...
fn editor_url(editor: &str, alias: &str, path: &str) -> Result<String, String> {
    let scheme = match editor {
        "vscode" => "vscode",
        "cursor" => "cursor",
        _ => return Err("不认识的编辑器".into()),
    };
    let path = path.replace('\\', "/");
    let path = if path.starts_with('/') { path } else { format!("/{path}") };
    let mut encoded = String::new();
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~:".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    Ok(format!("{scheme}://vscode-remote/ssh-remote+{alias}{encoded}"))
}

#[tauri::command]
pub(crate) fn ssh_status() -> Result<SshStatus, String> {
    status()
}

/// 在 ~/.ssh/config 最前面加一行 Include（用户在界面上同意过才调）
#[tauri::command]
pub(crate) fn ssh_add_include() -> Result<SshStatus, String> {
    let dir = ssh_dir()?;
    add_include_in(&dir)?;
    Ok(status_in(&dir))
}

fn add_include_in(dir: &Path) -> Result<(), String> {
    let config = dir.join("config");
    let text = std::fs::read_to_string(&config).unwrap_or_default();
    if !has_include(&text) {
        let new = format!("# P2PCLI：用 P2PCLI 连别的设备的 SSH 配置\n{INCLUDE_LINE}\n\n{text}");
        write_private(&config, new.as_bytes())?;
        tracing::info!(config = %config.display(), "在 SSH 配置里加了 Include");
    }
    Ok(())
}

/// 用本机 VS Code / Cursor 打开对方电脑上的文件夹。~/.ssh/config 里还没有 Include 时报 ssh_include_missing，
/// 界面问过用户、加好以后再调一次
#[tauri::command]
pub(crate) async fn open_in_editor(state: State<'_, AppState>, conn_id: u64, path: String, editor: String) -> Result<OpenedInEditor, String> {
    let remote = state.remote(conn_id)?;
    if remote.is_local() {
        return Err("这是本机，直接用编辑器打开就行".into());
    }
    let dir = state.data_dir.clone();
    let cli = tauri::async_runtime::spawn_blocking(move || crate::cli::status(&dir)).await.map_err(|e| e.to_string())?;
    let cli = match (cli.installed, cli.path) {
        (true, Some(p)) => p,
        _ => return Err("先在「设置 → 文件传输」里安装命令行工具 p2pcli：SSH 要用它接通对方".into()),
    };
    let data_dir = state.data_dir.clone();
    let key = tauri::async_runtime::spawn_blocking(move || client_key(&data_dir)).await.map_err(|e| e.to_string())?;
    // 直连优先：SSH、IDE 是持续流量
    remote.wait_direct(p2pcli_core::path::DIRECT_WAIT).await;
    // 先接一次对方的 SSH 服务：确认有能用的，顺便把本机的公钥交给对方记下
    drop(remote.open_ssh(key.as_ref().map(|(_, public)| public.clone())).await.map_err(|e| format!("{e:#}"))?);
    // 目录：~ 和相对路径都换成对方电脑上的绝对路径，顺便确认它存在
    let listing = remote.list_entries(path).await.map_err(|e| format!("{e:#}"))?;
    let data_dir = state.data_dir.clone();
    let (name, endpoint_id, user) = (remote.info.name.clone(), remote.info.endpoint_id.clone(), remote.info.user.clone());
    let alias = upsert(&data_dir, &ssh_dir()?, &name, &endpoint_id, &user, &cli, key.as_ref().map(|(k, _)| k.as_path()))?;
    if !status()?.include {
        return Err("ssh_include_missing".into());
    }
    let url = editor_url(&editor, &alias, &listing.path)?;
    tracing::info!(%alias, editor, "用本机编辑器打开对方的文件夹");
    crate::links::open_external(&url)?;
    Ok(OpenedInEditor { alias, path: listing.path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases() {
        let id = "80bc69b4aaaa";
        assert_eq!(alias_for("Forrest", id, &[]), "p2p-forrest");
        assert_eq!(alias_for("Forrest的Mac mini", id, &[]), "p2p-forrest-mac-mini");
        assert_eq!(alias_for("吉吉的电脑", id, &[]), "p2p-80bc69");
        let taken = [SshHost { alias: "p2p-forrest".into(), endpoint_id: "other".into(), user: "u".into(), name: "x".into() }];
        assert_eq!(alias_for("Forrest", id, &taken), "p2p-forrest-80bc69");
    }

    #[test]
    fn include_detection() {
        assert!(has_include("Include ~/.ssh/p2pcli_hosts\nHost x\n"));
        assert!(has_include("  include /Users/a/.ssh/p2pcli_hosts\n"));
        assert!(!has_include("Host x\n  User y\n"));
    }

    #[test]
    fn urls() {
        assert_eq!(editor_url("vscode", "p2p-a", "/Users/a/my proj").unwrap(), "vscode://vscode-remote/ssh-remote+p2p-a/Users/a/my%20proj");
        assert_eq!(editor_url("cursor", "p2p-a", "C:\\Users\\a").unwrap(), "cursor://vscode-remote/ssh-remote+p2p-a/C:/Users/a");
        assert!(editor_url("vim", "p2p-a", "/").is_err());
    }

    /// 在临时目录里：写别名、加 Include（原有内容保留、只加一次），再让本机的 ssh 解析一遍
    #[test]
    fn writes_config_that_openssh_understands() {
        let base = std::env::temp_dir().join(format!("p2pcli-ssh-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (data, ssh) = (base.join("data"), base.join(".ssh"));
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::write(ssh.join("config"), "Host old\n  User me\n").unwrap();
        let id = "80bc69b4".repeat(8);
        let key = base.join("my key").join("id_ed25519");
        let alias = upsert(&data, &ssh, "Forrest", &id, "30402", "/opt/p2p cli/p2pcli", Some(&key)).unwrap();
        assert_eq!(alias, "p2p-forrest");
        // 同一台设备再写一次：别名不变，不重复
        assert_eq!(upsert(&data, &ssh, "Forrest 改名了", &id, "30402", "/opt/p2p cli/p2pcli", Some(&key)).unwrap(), "p2p-forrest");
        assert_eq!(load(&data).len(), 1);
        assert!(!status_in(&ssh).include);
        add_include_in(&ssh).unwrap();
        add_include_in(&ssh).unwrap();
        let config = std::fs::read_to_string(ssh.join("config")).unwrap();
        assert_eq!(config.matches("Include").count(), 1);
        assert!(config.starts_with("# P2PCLI") && config.ends_with("Host old\n  User me\n"), "{config}");
        assert!(status_in(&ssh).include);
        // OpenSSH 自己解析：用临时目录里的配置（Include 换成绝对路径）
        let hosts = ssh.join(HOSTS_FILE);
        let tmp_config = base.join("config");
        write_private(&tmp_config, format!("Include {}\n", hosts.display()).as_bytes()).unwrap();
        let Ok(out) = std::process::Command::new("ssh").arg("-G").arg("-F").arg(&tmp_config).arg("p2p-forrest").output() else { return };
        let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(text.contains("user 30402"), "{text}");
        assert!(text.contains(&format!("proxycommand \"/opt/p2p cli/p2pcli\" pipe {id} ssh")), "{text}");
        assert!(text.contains("identityfile") && text.contains("my key"), "{text}");
        assert!(text.contains("port 22"), "{text}");
        assert!(text.contains(&format!("hostkeyalias p2pcli-{}", &id[..16])), "{text}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn config_entry() {
        let hosts = [SshHost { alias: "p2p-a".into(), endpoint_id: "0123456789abcdef0123".into(), user: "sam".into(), name: "A".into() }];
        let text = render(&hosts, "/Applications/My App/p2pcli", None);
        assert!(text.contains("Host p2p-a\n"));
        assert!(text.contains("ProxyCommand \"/Applications/My App/p2pcli\" pipe 0123456789abcdef0123 ssh"));
        assert!(!text.contains("IdentityFile"));
        assert!(text.contains("HostKeyAlias p2pcli-0123456789abcdef"));
        assert!(text.contains("User sam"));
    }
}

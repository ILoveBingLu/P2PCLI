//! 状态栏的目录选择：列出被连电脑上某个目录的子目录，再把终端切过去。
//! 只列目录名，不读文件内容；能连上来的设备本来就能在终端里随便 ls，这里不额外放权。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// 一次最多回这么多项，防止 node_modules 这种目录把一帧撑爆；再多前端一次画不过来，靠筛选找
const MAX_ENTRIES: usize = 1000;
/// 路径长度上限：比各系统的 PATH_MAX 宽松，挡住明显异常的输入
const MAX_PATH: usize = 4096;

/// Windows 上“所有磁盘”这一层：在某个盘的根目录（`C:\`）点上一级到这里，列出电脑上所有的盘（`C:`、`D:`…）。
/// 路径用 `\`，是为了老版本的手机、桌面不用改也能用：它们照常把盘名拼在后面得到 `\D:`，这边认成 `D:\`
pub const DRIVES: &str = "\\";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DirListing {
    /// 实际列出的目录（绝对路径）
    pub path: String,
    /// 上一级；已经是根目录时没有
    pub parent: Option<String>,
    /// 子目录名，按名字排好，点开头的隐藏目录放后面
    pub dirs: Vec<String>,
    /// 子目录太多只回了一部分
    pub truncated: bool,
}

/// “所有磁盘”那层拼出来的盘路径（`\D:`、`\D:\work`），或者只写了盘名（`D:`），整理成正常的 `D:\…`；别的返回 None
#[cfg_attr(not(windows), allow(dead_code))]
fn drive_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix(['\\', '/']).unwrap_or(path);
    let b = rest.as_bytes();
    if b.len() < 2 || !b[0].is_ascii_alphabetic() || b[1] != b':' { return None; }
    if b.len() == 2 { return Some(format!("{rest}\\")); }
    // 原本就是 `D:\…` 的不用动
    if !matches!(b[2], b'\\' | b'/') || rest.len() == path.len() { return None; }
    Some(rest.to_string())
}

fn is_drives(path: &str) -> bool {
    matches!(path.trim(), "\\" | "/")
}

/// 校验并整理连接方给的路径：空着或 `~` 开头按用户目录算，其余必须是绝对路径
pub fn resolve(path: &str) -> Result<PathBuf> {
    if path.len() > MAX_PATH || path.chars().any(char::is_control) {
        bail!("路径过长或包含控制字符");
    }
    let path = path.trim();
    #[cfg(windows)]
    let fixed = drive_path(path);
    #[cfg(windows)]
    let path = fixed.as_deref().unwrap_or(path);
    if cfg!(windows) && is_drives(path) {
        bail!("先选一个盘");
    }
    let home = || std::env::home_dir().context("找不到用户目录");
    let resolved = if path.is_empty() || path == "~" {
        home()?
    } else if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        home()?.join(rest)
    } else {
        PathBuf::from(path)
    };
    if !resolved.is_absolute() {
        bail!("只能使用绝对路径：{path}");
    }
    // 去掉 `.`、多余的分隔符；不解析符号链接，路径和终端里看到的保持一致
    Ok(resolved.components().collect())
}

pub async fn list(path: &str) -> Result<DirListing> {
    if cfg!(windows) && is_drives(path) {
        return tokio::task::spawn_blocking(|| DirListing { path: DRIVES.into(), parent: None, dirs: drives(), truncated: false })
            .await
            .map_err(Into::into);
    }
    let path = resolve(path)?;
    tokio::task::spawn_blocking(move || list_blocking(&path)).await?
}

/// 电脑上能打开的盘：`C:`、`D:`…（没插卡的读卡器、断开的网络盘打不开，不列）
fn drives() -> Vec<String> {
    (b'A'..=b'Z')
        .map(|c| format!("{}:", c as char))
        .filter(|d| cfg!(windows) && std::fs::metadata(format!("{d}\\")).is_ok_and(|m| m.is_dir()))
        .collect()
}

fn list_blocking(path: &Path) -> Result<DirListing> {
    let entries = std::fs::read_dir(path).with_context(|| format!("打不开目录 {}", path.display()))?;
    let mut dirs = Vec::new();
    let mut truncated = false;
    for entry in entries.flatten() {
        // 类型直接取自读目录的结果，不用逐项再查一遍（文件多或挂着网络盘时很慢）；
        // 只有符号链接要跟过去看指向的是不是目录，这样指向目录的链接也能进去
        let Ok(kind) = entry.file_type() else { continue };
        if !(kind.is_dir() || kind.is_symlink() && entry.path().is_dir()) { continue; }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
        if dirs.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        dirs.push(name);
    }
    dirs.sort_by_cached_key(|name| (name.starts_with('.'), name.to_lowercase()));
    Ok(DirListing {
        path: path.to_string_lossy().into_owned(),
        // Windows 的盘根目录（C:\\）再往上是“所有磁盘”
        parent: path.parent().map(|p| p.to_string_lossy().into_owned()).or_else(|| cfg!(windows).then(|| DRIVES.to_string())),
        dirs,
        truncated,
    })
}

/// 按终端里的 shell 拼一条切目录的命令；路径整个加引号，不让 shell 展开里面的任何东西
pub fn cd_command(shell: &str, path: &str) -> Result<String> {
    let name = shell.rsplit(['/', '\\']).next().unwrap_or(shell).trim_start_matches('-').to_ascii_lowercase();
    Ok(match name.trim_end_matches(".exe") {
        "pwsh" | "powershell" => format!("Set-Location -LiteralPath '{}'", path.replace('\'', "''")),
        "cmd" => {
            // cmd 的双引号里照样展开 %变量%，也没法转义双引号，遇到就不切
            if path.contains(['"', '%']) { bail!("cmd 终端不支持切换到带 \" 或 % 的路径"); }
            format!("cd /d \"{path}\"")
        }
        "fish" => format!("cd '{}'", path.replace('\\', "\\\\").replace('\'', "\\'")),
        _ => format!("cd '{}'", path.replace('\'', r"'\''")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cd_commands_quote_for_each_shell() {
        assert_eq!(cd_command("zsh", "/a b/it's").unwrap(), r"cd '/a b/it'\''s'");
        assert_eq!(cd_command("-bash", "/tmp/$(x)").unwrap(), "cd '/tmp/$(x)'");
        assert_eq!(cd_command("/usr/bin/fish", r"/a\b/it's").unwrap(), r"cd '/a\\b/it\'s'");
        assert_eq!(cd_command("pwsh.exe", r"C:\it's").unwrap(), r"Set-Location -LiteralPath 'C:\it''s'");
        assert_eq!(cd_command("cmd.exe", r"C:\work").unwrap(), r#"cd /d "C:\work""#);
        assert!(cd_command("cmd.exe", r"C:\100%").is_err());
    }

    #[test]
    fn drive_paths_from_the_drives_level() {
        assert_eq!(drive_path("\\D:").as_deref(), Some("D:\\"));
        assert_eq!(drive_path("D:").as_deref(), Some("D:\\"));
        assert_eq!(drive_path("\\D:\\work").as_deref(), Some("D:\\work"));
        assert_eq!(drive_path("/e:/x").as_deref(), Some("e:/x"));
        assert_eq!(drive_path("C:\\Users"), None);
        assert_eq!(drive_path("/usr/bin"), None);
        assert_eq!(drive_path("\\"), None);
        assert!(is_drives("\\") && is_drives("/") && !is_drives(""));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_drive_roots_lead_to_the_drives_level() {
        let root = list("C:\\").await.unwrap();
        assert_eq!(root.parent.as_deref(), Some(DRIVES));
        let all = list(DRIVES).await.unwrap();
        assert!(all.dirs.contains(&"C:".to_string()) && all.parent.is_none());
        assert_eq!(list("\\C:").await.unwrap().path, "C:\\");
        assert!(resolve(DRIVES).is_err());
    }

    #[test]
    fn resolve_rejects_relative_and_control_characters() {
        assert!(resolve("relative/dir").is_err());
        assert!(resolve("/tmp\nrm -rf ~").is_err());
        assert_eq!(resolve("").unwrap(), std::env::home_dir().unwrap());
        assert_eq!(resolve("~/x").unwrap(), std::env::home_dir().unwrap().join("x"));
        #[cfg(unix)]
        assert_eq!(resolve("/usr/./bin/").unwrap(), PathBuf::from("/usr/bin"));
    }

    #[tokio::test]
    async fn lists_only_directories_sorted_with_hidden_last() {
        let root = std::env::temp_dir().join(format!("p2pcli-dirs-{:x}", rand::random::<u64>()));
        for dir in ["b", "A", ".hidden", "c"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("file.txt"), "x").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("c"), root.join("d-link")).unwrap();
            std::os::unix::fs::symlink(root.join("file.txt"), root.join("file-link")).unwrap();
        }
        let listing = list(root.to_str().unwrap()).await.unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        #[cfg(unix)]
        assert_eq!(listing.dirs, ["A", "b", "c", "d-link", ".hidden"]);
        #[cfg(not(unix))]
        assert_eq!(listing.dirs, ["A", "b", "c", ".hidden"]);
        assert_eq!(listing.parent.as_deref(), root.parent().and_then(Path::to_str));
        assert!(!listing.truncated);
    }
}

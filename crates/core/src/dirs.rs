//! 状态栏的目录选择：列出被连电脑上某个目录的子目录，再把终端切过去。
//! 只列目录名，不读文件内容；能连上来的设备本来就能在终端里随便 ls，这里不额外放权。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// 一次最多回这么多项，防止 node_modules 这种目录把一帧撑爆；再多前端一次画不过来，靠筛选找
const MAX_ENTRIES: usize = 1000;
/// 路径长度上限：比各系统的 PATH_MAX 宽松，挡住明显异常的输入
const MAX_PATH: usize = 4096;

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

/// 校验并整理连接方给的路径：空着或 `~` 开头按用户目录算，其余必须是绝对路径
pub fn resolve(path: &str) -> Result<PathBuf> {
    if path.len() > MAX_PATH || path.chars().any(char::is_control) {
        bail!("路径过长或包含控制字符");
    }
    let path = path.trim();
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
    let path = resolve(path)?;
    tokio::task::spawn_blocking(move || list_blocking(&path)).await?
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
        parent: path.parent().map(|p| p.to_string_lossy().into_owned()),
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

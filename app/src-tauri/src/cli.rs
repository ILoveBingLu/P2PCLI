//! 命令行工具 p2pcli：安装包里带着一份，软件启动时放到终端找得到的地方。
//! Mac 在 ~/.local/bin 建一个指向安装包里那份的链接，软件更新以后链接自动指向新版；
//! Windows 拷一份到数据目录的 bin 里，再把这个目录加进用户的 PATH（安装目录里还有别的程序，不直接加）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CliStatus {
    /// 终端里用的那份在哪
    pub path: Option<String>,
    /// 装好了，指向这个版本
    pub installed: bool,
    /// 所在目录在 PATH 里，终端里直接敲 p2pcli 就行
    pub on_path: bool,
    /// 开发版没带命令行工具
    pub bundled: bool,
}

const NAME: &str = if cfg!(windows) { "p2pcli.exe" } else { "p2pcli" };

/// 安装包里带的那份，和主程序在同一个目录
fn bundled() -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?.with_file_name(NAME);
    p.is_file().then_some(p)
}

/// 正式安装的软件才自动装；开发时从 target 目录跑，不去动终端里的 p2pcli
pub fn auto_install(data_dir: &Path) {
    if cfg!(debug_assertions) || bundled().is_none() {
        return;
    }
    match install(data_dir, false) {
        Ok(s) => tracing::info!(?s, "命令行工具已就绪"),
        Err(e) => tracing::warn!("命令行工具没装上：{e:#}"),
    }
}

#[cfg(unix)]
fn target(_data_dir: &Path) -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/bin").join(NAME))
}

#[cfg(windows)]
fn target(data_dir: &Path) -> Option<PathBuf> {
    Some(data_dir.join("bin").join(NAME))
}

pub fn status(data_dir: &Path) -> CliStatus {
    let path = target(data_dir);
    let src = bundled();
    let installed = match (&path, &src) {
        (Some(path), Some(src)) => same_install(path, src),
        _ => false,
    };
    CliStatus {
        on_path: path.as_deref().and_then(Path::parent).is_some_and(on_path),
        path: path.map(|p| p.display().to_string()),
        installed,
        bundled: src.is_some(),
    }
}

/// 装命令行工具；add_path 为真时顺带把目录加进 PATH（Mac 写进 shell 配置，要用户点按钮才做）
pub fn install(data_dir: &Path, add_path: bool) -> Result<CliStatus> {
    let src = bundled().ok_or_else(|| anyhow!("开发版没有带命令行工具"))?;
    let path = target(data_dir).ok_or_else(|| anyhow!("找不到用户目录"))?;
    let dir = path.parent().expect("目标路径有上级目录");
    std::fs::create_dir_all(dir).with_context(|| format!("建 {} 失败", dir.display()))?;
    if !same_install(&path, &src) {
        place(&src, &path)?;
    }
    // Windows 的 PATH 是安装器该管的事，自动加；Mac 要改用户的 shell 配置，等用户点
    if (cfg!(windows) || add_path) && !on_path(dir) {
        add_to_path(dir)?;
    }
    Ok(status(data_dir))
}

#[cfg(unix)]
fn same_install(path: &Path, src: &Path) -> bool {
    std::fs::read_link(path).is_ok_and(|t| t == src)
}

#[cfg(windows)]
fn same_install(path: &Path, src: &Path) -> bool {
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
    size(path).is_some() && size(path) == size(src) && std::fs::read(path).ok() == std::fs::read(src).ok()
}

#[cfg(unix)]
fn place(src: &Path, path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::remove_file(path)?,
        Ok(_) => bail!("{} 已经有别的文件，没有覆盖", path.display()),
        Err(_) => {}
    }
    std::os::unix::fs::symlink(src, path).with_context(|| format!("建链接 {} 失败", path.display()))
}

#[cfg(windows)]
fn place(src: &Path, path: &Path) -> Result<()> {
    // 先拷到旁边再换名，正在用的旧版换不掉时不留半截文件
    let tmp = path.with_extension("exe.new");
    std::fs::copy(src, &tmp).with_context(|| format!("拷到 {} 失败", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("换成新版失败，可能有 p2pcli 正在运行：{}", path.display()))
}

/// Mac 上软件拿不到终端的 PATH，看 shell 配置文件里有没有加过这个目录
#[cfg(unix)]
fn on_path(dir: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return false };
    let short = dir.strip_prefix(&home).map(|p| p.display().to_string()).unwrap_or_default();
    let full = dir.display().to_string();
    [".zshrc", ".zprofile", ".zshenv", ".bash_profile", ".bashrc", ".profile", ".config/fish/config.fish"]
        .iter()
        .filter_map(|f| std::fs::read_to_string(home.join(f)).ok())
        .any(|text| text.lines().filter(|l| !l.trim_start().starts_with('#')).any(|l| l.contains(&full) || (!short.is_empty() && l.contains(&short))))
}

#[cfg(unix)]
fn add_to_path(dir: &Path) -> Result<()> {
    use std::io::Write;
    let home = PathBuf::from(std::env::var_os("HOME").ok_or_else(|| anyhow!("找不到用户目录"))?);
    let shell = std::env::var("SHELL").unwrap_or_default();
    let rc = if shell.ends_with("bash") { ".bash_profile" } else if shell.ends_with("fish") { ".config/fish/config.fish" } else { ".zshrc" };
    let shown = match dir.strip_prefix(&home) {
        Ok(rel) => format!("$HOME/{}", rel.display()),
        Err(_) => dir.display().to_string(),
    };
    let line = if rc.ends_with("fish") { format!("fish_add_path {shown}") } else { format!("export PATH=\"{shown}:$PATH\"") };
    let path = home.join(rc);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).with_context(|| format!("打不开 {}", path.display()))?;
    writeln!(f, "\n# P2PCLI 命令行工具\n{line}")?;
    Ok(())
}

#[cfg(windows)]
fn powershell(script: &str) -> Result<String> {
    use std::os::windows::process::CommandExt;
    /// 不弹黑窗口
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .context("运行 PowerShell 失败")?;
    if !out.status.success() {
        bail!("PowerShell 出错：{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 用户 PATH 原样读（不展开 %USERPROFILE% 这类写法），按分号拆开比
#[cfg(windows)]
fn on_path(dir: &Path) -> bool {
    let script = "[string][Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment').GetValue('Path', '', 'DoNotExpandEnvironmentNames')";
    let want = dir.display().to_string().to_lowercase();
    powershell(script).is_ok_and(|p| p.split(';').any(|d| d.trim().trim_end_matches('\\').to_lowercase() == want))
}

/// 加进用户 PATH，保留原来的类型；最后设一下又删掉一个临时变量，让系统通知别的程序 PATH 变了，新开的终端就能用
#[cfg(windows)]
fn add_to_path(dir: &Path) -> Result<()> {
    let dir = dir.display().to_string().replace('\'', "''");
    let script = format!(
        r#"$d = '{dir}'
$k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
$p = [string]$k.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
if (-not (($p -split ';') -contains $d)) {{
  $n = if ($p.Trim()) {{ $p.TrimEnd(';') + ';' + $d }} else {{ $d }}
  $k.SetValue('Path', $n, [Microsoft.Win32.RegistryValueKind]::ExpandString)
  [Environment]::SetEnvironmentVariable('P2PCLI_PATH_REFRESH', '1', 'User')
  [Environment]::SetEnvironmentVariable('P2PCLI_PATH_REFRESH', $null, 'User')
}}"#
    );
    powershell(&script).map(|_| ())
}

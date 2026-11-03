//! 远程编辑的代码补全、跳转、报错：在被连方找到 rust-analyzer、gopls 这类语言服务并启动它，
//! 把它的标准输入输出原样转给连接方的编辑器。
//!
//! 被连方只当一根管子：不解析语言服务的消息，按 Content-Length 分消息的事由连接方做。
//! 语言服务按用户自己的 PATH 找（和终端里一样），另外看几个常见的安装位置，
//! 包括 VS Code、Cursor 自带的 rust-analyzer，用户不用另外装或配置。
//! 流关了（编辑器关掉、断线）语言服务和它启动的子进程（比如 cargo check）一起结束。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::OnceCell;
use tokio::time::timeout;

use crate::exec::{AbortOnDrop, Group};
use crate::proto::{Frame, Msg, read_frame, write_data, write_msg};

/// 找不到语言服务时错误信息的开头，后面跟语言；连接方据此显示怎么安装
pub const MISSING: &str = "lsp_missing:";
/// 一条消息最大多少字节，防内存被撑爆
pub const MAX_MESSAGE: usize = 64 * 1024 * 1024;
/// 转发输出时每帧最多多少字节
const CHUNK: usize = 64 * 1024;
/// 语言服务提前退出时，把标准错误最后这么多字节带回去说明原因
const STDERR_TAIL: usize = 4096;

struct Candidate {
    program: &'static str,
    args: &'static [&'static str],
    /// 先跑一次 --version 确认能用：rustup 装的 rust-analyzer 只是个壳，组件没装时一跑就报错
    check: bool,
}

const fn c(program: &'static str, args: &'static [&'static str]) -> Candidate {
    Candidate { program, args, check: false }
}

struct Server {
    /// 连接方按这个名字要语言服务
    family: &'static str,
    candidates: &'static [Candidate],
    /// 项目根目录的标志文件：从文件所在目录往上找，最近的一个就是根目录
    markers: &'static [&'static str],
}

const SERVERS: &[Server] = &[
    Server { family: "rust", candidates: &[Candidate { program: "rust-analyzer", args: &[], check: true }], markers: &["Cargo.toml"] },
    Server { family: "go", candidates: &[c("gopls", &[])], markers: &["go.work", "go.mod"] },
    Server {
        family: "python",
        candidates: &[
            c("basedpyright-langserver", &["--stdio"]), c("pyright-langserver", &["--stdio"]),
            c("pylsp", &[]), c("jedi-language-server", &[]),
        ],
        markers: &["pyproject.toml", "pyrightconfig.json", "setup.py", "setup.cfg", "requirements.txt"],
    },
    Server {
        family: "typescript",
        candidates: &[c("typescript-language-server", &["--stdio"])],
        markers: &["tsconfig.json", "jsconfig.json", "package.json"],
    },
    Server { family: "c", candidates: &[c("clangd", &[])], markers: &["compile_commands.json", "compile_flags.txt", ".clangd", "CMakeLists.txt", "Makefile"] },
    Server { family: "swift", candidates: &[c("sourcekit-lsp", &[])], markers: &["Package.swift"] },
    Server { family: "lua", candidates: &[c("lua-language-server", &[])], markers: &[".luarc.json", ".luarc.jsonc"] },
    Server { family: "ruby", candidates: &[c("ruby-lsp", &[]), c("solargraph", &["stdio"])], markers: &["Gemfile"] },
    Server { family: "php", candidates: &[c("intelephense", &["--stdio"]), c("phpactor", &["language-server"])], markers: &["composer.json"] },
    Server { family: "shell", candidates: &[c("bash-language-server", &["start"])], markers: &[] },
    Server { family: "csharp", candidates: &[c("csharp-ls", &[])], markers: &["*.sln", "*.csproj"] },
    Server { family: "java", candidates: &[c("jdtls", &[])], markers: &["pom.xml", "build.gradle", "build.gradle.kts"] },
    Server { family: "kotlin", candidates: &[c("kotlin-language-server", &[])], markers: &["build.gradle.kts", "build.gradle", "pom.xml"] },
    // 测试用：cat 把收到的原样吐回去，只在调试构建里有
    #[cfg(all(debug_assertions, unix))]
    Server { family: "echo", candidates: &[c("cat", &[])], markers: &[] },
];

fn server(family: &str) -> Result<&'static Server> {
    SERVERS.iter().find(|s| s.family == family).ok_or_else(|| anyhow!("不支持这种语言的代码补全：{family}"))
}

/// 在被连方启动语言服务，之后在 recv/send 上来回转发它的标准输入输出，直到一边结束。
/// 先回 LspStarted；语言服务自己退出时最后回 LspExited。
pub async fn serve<R, W>(recv: R, send: &mut W, family: &str, file: &str, root: &str) -> Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    let server = server(family)?;
    let (file, root) = (PathBuf::from(file), PathBuf::from(root));
    if !file.is_absolute() || !root.is_absolute() { bail!("路径必须是绝对路径"); }
    let workspace = tokio::task::spawn_blocking(move || workspace_root(server, &file, &root)).await?;
    let path = search_path().await;
    let (program, args) = locate(server, path).await?;
    let name = program.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| server.family.into());

    let mut cmd = Command::new(&program);
    cmd.args(args).current_dir(&workspace).env("PATH", std::env::join_paths(path)?)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(unix)]
    {
        cmd.process_group(0);
        if std::env::var_os("LANG").is_none() { cmd.env("LANG", "en_US.UTF-8"); }
    }
    #[cfg(windows)]
    cmd.creation_flags(crate::exec::CREATE_NO_WINDOW);
    let mut child = cmd.spawn().with_context(|| format!("启动 {name} 失败"))?;
    // 不管怎么结束，语言服务连同它启动的进程一起结束
    let _group = Group { pid: child.id(), armed: true };
    tracing::debug!(server = %name, root = %workspace.display(), "启动语言服务");

    let (Some(mut stdin), Some(mut stdout), Some(mut stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
        bail!("语言服务的输入输出没接上");
    };
    write_msg(send, &Msg::LspStarted { server: name.clone(), root: workspace.to_string_lossy().into_owned() }).await?;

    let tail = Arc::new(Mutex::new(VecDeque::<u8>::new()));
    let errors = {
        let tail = tail.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 { break; }
                let mut tail = tail.lock().unwrap();
                tail.extend(&buf[..n]);
                let extra = tail.len().saturating_sub(STDERR_TAIL);
                tail.drain(..extra);
            }
        })
    };
    // 读对面的帧单独放一个任务：读帧读到一半不能被打断
    let mut input = tokio::spawn(async move {
        let mut recv = recv;
        while let Ok(Some(Frame::Data(data))) = read_frame(&mut recv).await {
            if stdin.write_all(&data).await.is_err() { break; }
        }
    });
    let _abort = AbortOnDrop(vec![errors]);

    let mut buf = vec![0u8; CHUNK];
    loop {
        tokio::select! {
            n = stdout.read(&mut buf) => match n {
                Ok(0) | Err(_) => break,
                Ok(n) => write_data(send, &buf[..n]).await?,
            },
            // 对面关了流：编辑器关掉或断线，语言服务跟着结束
            _ = &mut input => return Ok(()),
        }
    }
    input.abort();
    let status = timeout(Duration::from_secs(2), child.wait()).await.ok().and_then(|s| s.ok());
    let stderr = String::from_utf8_lossy(&tail.lock().unwrap().iter().copied().collect::<Vec<_>>()).trim().to_string();
    let mut message = match status.and_then(|s| s.code()) {
        Some(code) => format!("{name} 已退出（退出码 {code}）"),
        None => format!("{name} 已退出"),
    };
    if !stderr.is_empty() { message = format!("{message}：{stderr}"); }
    tracing::info!("{message}");
    write_msg(send, &Msg::LspExited { message }).await?;
    Ok(())
}

/// 语言服务的工作目录：从文件往上找项目的标志文件，找不到就用编辑器打开的目录
fn workspace_root(server: &Server, file: &Path, fallback: &Path) -> PathBuf {
    let start = file.parent().unwrap_or(file);
    let fallback = if file.starts_with(fallback) && fallback.is_dir() { fallback.to_path_buf() } else { start.to_path_buf() };
    let has = |dir: &Path, marker: &str| match marker.strip_prefix('*') {
        Some(ext) => std::fs::read_dir(dir).map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().ends_with(ext))).unwrap_or(false),
        None => dir.join(marker).is_file(),
    };
    let home = std::env::home_dir();
    let ancestors = || start.ancestors().take(64).take_while(|d| home.as_deref() != Some(*d));
    match server.family {
        // Cargo 工作区：同一个工作区的各个 crate 共用一个 rust-analyzer，不要每个 crate 开一个
        "rust" => {
            let Some(krate) = ancestors().find(|d| has(d, "Cargo.toml")) else { return fallback };
            ancestors().skip_while(|d| *d != krate)
                .find(|d| std::fs::read_to_string(d.join("Cargo.toml")).is_ok_and(|t| t.lines().any(|l| l.trim() == "[workspace]")))
                .unwrap_or(krate).to_path_buf()
        }
        _ => server.markers.iter()
            // 标志文件按优先级排：go.work 在 go.mod 前面，找到优先级高的就不看后面的
            .find_map(|m| if server.family == "go" { ancestors().find(|d| has(d, m)) } else { None })
            .or_else(|| ancestors().find(|d| server.markers.iter().any(|m| has(d, m))))
            .map(Path::to_path_buf)
            .unwrap_or(fallback),
    }
}

/// 找语言服务的目录：用户 Shell 里的 PATH（从访达启动的程序拿不到），加上常见的安装位置
async fn search_path() -> &'static Vec<PathBuf> {
    static PATH: OnceCell<Vec<PathBuf>> = OnceCell::const_new();
    PATH.get_or_init(|| async {
        let mut dirs = login_path().await.unwrap_or_default();
        if let Some(path) = std::env::var_os("PATH") { dirs.extend(std::env::split_paths(&path)); }
        dirs.extend(extra_dirs());
        let mut seen = std::collections::HashSet::new();
        dirs.retain(|d| d.is_absolute() && seen.insert(d.clone()));
        dirs
    }).await
}

#[cfg(not(windows))]
async fn login_path() -> Option<Vec<PathBuf>> {
    let shell = portable_pty::CommandBuilder::new_default_prog().get_shell().to_string();
    let name = shell.rsplit('/').next().unwrap_or_default();
    // 和远程执行一样：zsh、fish 的 PATH 常在交互配置里（nvm、pnpm）
    let flag = if matches!(name, "zsh" | "fish") { "-lic" } else { "-lc" };
    let script = if name == "fish" { "printf '__P2PCLI_PATH__%s\\n' (string join : $PATH)" } else { "printf '__P2PCLI_PATH__%s\\n' \"$PATH\"" };
    let mut cmd = Command::new(&shell);
    cmd.args([flag, script]).stdin(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    if let Some(home) = std::env::home_dir() { cmd.current_dir(home); }
    let out = timeout(Duration::from_secs(10), cmd.output()).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find_map(|l| l.strip_prefix("__P2PCLI_PATH__"))?;
    Some(std::env::split_paths(line).collect())
}

#[cfg(windows)]
async fn login_path() -> Option<Vec<PathBuf>> {
    None
}

fn extra_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::home_dir();
    if let Some(home) = &home {
        for sub in [".cargo/bin", "go/bin", ".local/bin", ".bun/bin", ".deno/bin"] { dirs.push(home.join(sub)); }
    }
    if let Some(gopath) = std::env::var_os("GOPATH") { dirs.push(PathBuf::from(gopath).join("bin")); }
    #[cfg(unix)]
    for dir in [
        "/opt/homebrew/bin", "/usr/local/bin", "/opt/homebrew/opt/llvm/bin", "/usr/local/opt/llvm/bin", "/usr/local/go/bin",
        // Mac 上装了 Xcode 或命令行工具就有 clangd、sourcekit-lsp
        "/Library/Developer/CommandLineTools/usr/bin",
        "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin",
    ] {
        dirs.push(PathBuf::from(dir));
    }
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") { dirs.push(PathBuf::from(appdata).join("npm")); }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") { dirs.push(PathBuf::from(local).join("Programs").join("Python").join("Scripts")); }
        dirs.push(PathBuf::from(r"C:\Program Files\LLVM\bin"));
        dirs.push(PathBuf::from(r"C:\Program Files\Go\bin"));
    }
    dirs
}

/// 编辑器插件自带的语言服务：VS Code、Cursor 这些装了 rust-analyzer 插件就有现成的
fn bundled(program: &str) -> Vec<PathBuf> {
    let Some(home) = std::env::home_dir() else { return Vec::new() };
    let (prefix, sub) = match program {
        "rust-analyzer" => ("rust-lang.rust-analyzer-", "server"),
        _ => return Vec::new(),
    };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for editor in [".vscode", ".vscode-insiders", ".cursor", ".windsurf", ".vscode-oss", ".vscode-server", ".cursor-server"] {
        let Ok(entries) = std::fs::read_dir(home.join(editor).join("extensions")) else { continue };
        for entry in entries.flatten() {
            if !entry.file_name().to_string_lossy().starts_with(prefix) { continue; }
            let exe = entry.path().join(sub).join(if cfg!(windows) { format!("{program}.exe") } else { program.to_string() });
            if exe.is_file() {
                // 装过好几个版本时用最新的
                found.push((entry.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), exe));
            }
        }
    }
    found.sort_by_key(|a| std::cmp::Reverse(a.0));
    found.into_iter().map(|(_, p)| p).collect()
}

fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    path.is_file()
}

fn which(program: &str, dirs: &[PathBuf]) -> Vec<PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        [".exe", ".cmd", ".bat"].iter().map(|ext| format!("{program}{ext}")).collect()
    } else {
        vec![program.to_string()]
    };
    dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).filter(|p| executable(p)).collect()
}

/// 找到的语言服务：程序路径和启动参数
type Located = (PathBuf, &'static [&'static str]);
/// 用过的语言服务记下来，第二次不用再找、再试
static FOUND: Mutex<Option<HashMap<&'static str, Located>>> = Mutex::new(None);

async fn locate(server: &'static Server, dirs: &[PathBuf]) -> Result<Located> {
    if let Some(found) = FOUND.lock().unwrap().as_ref().and_then(|m| m.get(server.family)).filter(|(p, _)| p.is_file()) {
        return Ok(found.clone());
    }
    for candidate in server.candidates {
        let mut paths = which(candidate.program, dirs);
        paths.extend(bundled(candidate.program));
        for path in paths {
            if candidate.check && !works(&path, dirs).await { continue; }
            FOUND.lock().unwrap().get_or_insert_default().insert(server.family, (path.clone(), candidate.args));
            return Ok((path, candidate.args));
        }
    }
    bail!("{MISSING}{}", server.family)
}

async fn works(path: &Path, dirs: &[PathBuf]) -> bool {
    let mut cmd = Command::new(path);
    cmd.arg("--version").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    if let Ok(joined) = std::env::join_paths(dirs) { cmd.env("PATH", joined); }
    #[cfg(windows)]
    cmd.creation_flags(crate::exec::CREATE_NO_WINDOW);
    matches!(timeout(Duration::from_secs(10), cmd.status()).await, Ok(Ok(s)) if s.success())
}

/// 给一条消息加上 Content-Length 头
pub fn encode(body: &[u8]) -> Vec<u8> {
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body);
    out
}

/// 从攒下的字节里取出一条完整的消息；还不够一条时返回 None
pub fn take_message(buf: &mut Vec<u8>) -> Result<Option<String>> {
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        if buf.len() > 8192 { bail!("语言服务的输出格式不对"); }
        return Ok(None);
    };
    let head = std::str::from_utf8(&buf[..end]).context("语言服务的输出格式不对")?;
    let len: usize = head.split("\r\n")
        .find_map(|line| line.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-length")).map(|(_, v)| v.trim()))
        .ok_or_else(|| anyhow!("语言服务的输出缺少 Content-Length"))?
        .parse().context("语言服务的输出格式不对")?;
    if len > MAX_MESSAGE { bail!("语言服务的一条消息太大"); }
    let start = end + 4;
    if buf.len() < start + len { return Ok(None); }
    let body = String::from_utf8_lossy(&buf[start..start + len]).into_owned();
    buf.drain(..start + len);
    Ok(Some(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_round_trip() {
        let mut buf = Vec::new();
        buf.extend(encode(br#"{"a":1}"#));
        buf.extend(b"content-length: 11\r\nContent-Type: x\r\n\r\n{\"b\":\"\xe4\xb8\xad\"}");
        assert_eq!(take_message(&mut buf).unwrap().as_deref(), Some(r#"{"a":1}"#));
        // 第二条还差几个字节
        let cut = buf.split_off(buf.len() - 3);
        assert_eq!(take_message(&mut buf).unwrap(), None);
        buf.extend(cut);
        assert_eq!(take_message(&mut buf).unwrap().as_deref(), Some("{\"b\":\"中\"}"));
        assert!(buf.is_empty());
        let mut bad = b"Content-Type: x\r\n\r\n{}".to_vec();
        assert!(take_message(&mut bad).is_err());
    }

    #[test]
    fn workspace_roots() {
        let dir = std::env::temp_dir().join(format!("p2pcli-lsp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let krate = dir.join("ws/crates/a/src");
        std::fs::create_dir_all(&krate).unwrap();
        std::fs::write(dir.join("ws/Cargo.toml"), "[workspace]\nmembers = [\"crates/*\"]\n").unwrap();
        std::fs::write(dir.join("ws/crates/a/Cargo.toml"), "[package]\nname = \"a\"\n").unwrap();
        let file = krate.join("lib.rs");
        assert_eq!(workspace_root(server("rust").unwrap(), &file, &dir), dir.join("ws"));
        // 不在工作区里的单个 crate
        std::fs::write(dir.join("ws/Cargo.toml"), "[package]\nname = \"ws\"\n").unwrap();
        assert_eq!(workspace_root(server("rust").unwrap(), &file, &dir), dir.join("ws/crates/a"));

        let go = dir.join("go/mod/pkg");
        std::fs::create_dir_all(&go).unwrap();
        std::fs::write(dir.join("go/mod/go.mod"), "module x\n").unwrap();
        assert_eq!(workspace_root(server("go").unwrap(), &go.join("a.go"), &dir), dir.join("go/mod"));
        std::fs::write(dir.join("go/go.work"), "go 1.22\n").unwrap();
        assert_eq!(workspace_root(server("go").unwrap(), &go.join("a.go"), &dir), dir.join("go"));

        // 没有标志文件：用编辑器打开的目录
        let plain = dir.join("plain/sub");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(workspace_root(server("python").unwrap(), &plain.join("a.py"), &dir.join("plain")), dir.join("plain"));
        std::fs::write(dir.join("plain/sub/pyproject.toml"), "").unwrap();
        assert_eq!(workspace_root(server("python").unwrap(), &plain.join("a.py"), &dir.join("plain")), plain);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

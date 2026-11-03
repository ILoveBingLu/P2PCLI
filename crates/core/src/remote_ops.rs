//! 远程编辑第二阶段：全项目搜索、按名字找文件、看文件有没有变、新建/重命名/删除、Git 状态。
//! 都是一问一答，打包成一条 Edit 请求；和读写文件一样，不比在终端里能做的多放权。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::dirs::resolve;

/// 搜索最多回这么多处匹配；每个文件最多这么多处，免得一个大文件占满
const MAX_MATCHES: usize = 1000;
const MAX_MATCHES_PER_FILE: usize = 100;
/// 超过这么大的文件不搜：多半是数据或打包产物
const MAX_SEARCH_FILE: u64 = 2 * 1024 * 1024;
/// 搜索和列文件的时间上限，要比会话请求的 20 秒超时短，超时就回已经找到的
const SEARCH_DEADLINE: Duration = Duration::from_secs(12);
/// 文件列表最多这么多字节的路径，留在单帧上限以内
const MAX_FILE_LIST_BYTES: usize = 700 * 1024;
/// 预览一行最多这么多字
const PREVIEW_CHARS: usize = 200;
const MAX_STAT: usize = 200;
const MAX_GIT_ENTRIES: usize = 5000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EditRequest {
    /// 在 root 下面的文件内容里搜索；认 .gitignore，跳过 .git、二进制和大文件
    Search {
        root: String,
        query: String,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default)]
        whole_word: bool,
    },
    /// root 下面所有文件的相对路径，按名字快速打开用
    ListFiles { root: String },
    /// 这些文件现在的大小和修改时间；不存在的是空
    Stat { paths: Vec<String> },
    CreateFile { path: String },
    CreateDir { path: String },
    Rename { from: String, to: String },
    /// 移到废纸篓/回收站
    Delete { path: String },
    /// root 所在 Git 仓库的改动；不在仓库里或没装 git 时回空
    GitStatus { root: String },
    /// 下面这些是 Git 面板的操作，需要 git_v1；paths 相对仓库根目录，和 GitStatus 给的一样
    /// 暂存；paths 空着是全部
    GitStage { root: String, paths: Vec<String> },
    /// 取消暂存；paths 空着是全部
    GitUnstage { root: String, paths: Vec<String> },
    /// 放弃修改：paths 换回暂存区的版本，untracked（没跟踪的）移到废纸篓
    GitDiscard { root: String, paths: Vec<String>, #[serde(default)] untracked: Vec<String> },
    GitCommit { root: String, message: String },
    GitPush { root: String },
    GitPull { root: String },
    GitLog { root: String, limit: usize },
}

impl EditRequest {
    /// 需要 git_v1 的操作
    pub fn needs_git_v1(&self) -> bool {
        matches!(self, EditRequest::GitStage { .. } | EditRequest::GitUnstage { .. } | EditRequest::GitDiscard { .. }
            | EditRequest::GitCommit { .. } | EditRequest::GitPush { .. } | EditRequest::GitPull { .. } | EditRequest::GitLog { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EditReply {
    Search { result: SearchResult },
    Files { list: FileList },
    Stats { stats: Vec<Option<FileStat>> },
    Done,
    Git { status: Option<GitStatus> },
    /// git 命令的输出（提交、推送、拉取的结果）
    Output { text: String },
    Log { commits: Vec<crate::git_ops::GitCommit> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SearchResult {
    pub files: Vec<SearchFile>,
    /// 匹配太多或超时，只回了一部分
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchFile {
    pub path: String,
    pub matches: Vec<SearchMatch>,
}

/// 列号、长度按 UTF-16 算（编辑器就是这么数的），列号从 1 开始
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchMatch {
    pub line: u32,
    pub col: u32,
    pub len: u32,
    /// 这一行（太长时截取匹配附近）
    pub preview: String,
    /// 匹配在 preview 里从第几个 UTF-16 单位开始，从 0 数
    pub preview_col: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileList {
    pub root: String,
    /// 相对 root 的路径，分隔符统一用 /
    pub files: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileStat {
    pub size: u64,
    pub mtime: Option<u64>,
    pub dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitStatus {
    /// 仓库根目录（绝对路径，符号链接已解开，可能和文件树里的写法不同）
    pub top: String,
    /// 请求的目录相对仓库根目录在哪，比如 "app/src/"，就在根目录时是空；
    /// 文件树按它把 entries 换算成自己的路径，不依赖 top 的写法
    pub prefix: String,
    pub branch: Option<String>,
    pub entries: Vec<GitEntry>,
    /// 上游分支，比如 origin/main；没有时为空。旧版本不给
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    /// 比上游多几个提交（要推送）、少几个（要拉取）
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
}

/// status 是 git status --porcelain 的两个字母，比如 " M"、"A "、"??"
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitEntry {
    /// 相对仓库根目录，分隔符是 /
    pub path: String,
    pub status: String,
}

pub async fn handle(request: EditRequest) -> Result<EditReply> {
    Ok(match request {
        EditRequest::Search { root, query, regex, case_sensitive, whole_word } => {
            let root = resolve(&root)?;
            let matcher = build_matcher(&query, regex, case_sensitive, whole_word)?;
            let result = tokio::task::spawn_blocking(move || search(&root, &matcher)).await?;
            EditReply::Search { result }
        }
        EditRequest::ListFiles { root } => {
            let root = resolve(&root)?;
            let list = tokio::task::spawn_blocking(move || list_files(&root)).await?;
            EditReply::Files { list }
        }
        EditRequest::Stat { paths } => {
            if paths.len() > MAX_STAT { bail!("一次最多查 {MAX_STAT} 个文件"); }
            let stats = tokio::task::spawn_blocking(move || paths.iter().map(|p| stat(p)).collect()).await?;
            EditReply::Stats { stats }
        }
        EditRequest::CreateFile { path } => {
            let path = resolve(&path)?;
            std::fs::OpenOptions::new().write(true).create_new(true).open(&path)
                .with_context(|| format!("新建文件失败 {}", path.display()))?;
            EditReply::Done
        }
        EditRequest::CreateDir { path } => {
            let path = resolve(&path)?;
            std::fs::create_dir(&path).with_context(|| format!("新建文件夹失败 {}", path.display()))?;
            EditReply::Done
        }
        EditRequest::Rename { from, to } => {
            let (from, to) = (resolve(&from)?, resolve(&to)?);
            // rename 遇到同名文件会直接盖掉，先挡住；只改大小写时两个路径指的是同一个文件，放行
            if to.symlink_metadata().is_ok() && !same_file(&from, &to) {
                bail!("已经有同名的文件或文件夹：{}", to.display());
            }
            std::fs::rename(&from, &to).with_context(|| format!("重命名失败 {}", from.display()))?;
            EditReply::Done
        }
        EditRequest::Delete { path } => {
            let path = resolve(&path)?;
            if path.parent().is_none() { bail!("不能删除根目录"); }
            if path.symlink_metadata().is_err() { bail!("文件不存在：{}", path.display()); }
            tokio::task::spawn_blocking(move || move_to_trash(&path)).await??;
            EditReply::Done
        }
        EditRequest::GitStatus { root } => EditReply::Git { status: git_status(&resolve(&root)?).await },
        EditRequest::GitStage { root, paths } => { crate::git_ops::stage(&root, &paths).await?; EditReply::Done }
        EditRequest::GitUnstage { root, paths } => { crate::git_ops::unstage(&root, &paths).await?; EditReply::Done }
        EditRequest::GitDiscard { root, paths, untracked } => { crate::git_ops::discard(&root, &paths, &untracked).await?; EditReply::Done }
        EditRequest::GitCommit { root, message } => EditReply::Output { text: crate::git_ops::commit(&root, &message).await? },
        EditRequest::GitPush { root } => EditReply::Output { text: crate::git_ops::push(&root).await? },
        EditRequest::GitPull { root } => EditReply::Output { text: crate::git_ops::pull(&root).await? },
        EditRequest::GitLog { root, limit } => EditReply::Log { commits: crate::git_ops::log(&root, limit).await? },
    })
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

pub(crate) fn move_to_trash(path: &Path) -> Result<()> {
    #[allow(unused_mut)]
    let mut ctx = trash::TrashContext::default();
    // 默认的做法是让 Finder 来删，会在被连的电脑上弹“想控制 Finder”的授权框
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        ctx.set_delete_method(DeleteMethod::NsFileManager);
    }
    ctx.delete(path).map_err(|e| anyhow::anyhow!("移到废纸篓失败（{e}），可以在终端里删除"))
}

fn stat(path: &str) -> Option<FileStat> {
    let meta = std::fs::metadata(resolve(path).ok()?).ok()?;
    Some(FileStat {
        size: if meta.is_dir() { 0 } else { meta.len() },
        mtime: meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64),
        dir: meta.is_dir(),
    })
}

fn build_matcher(query: &str, regex: bool, case_sensitive: bool, whole_word: bool) -> Result<regex::Regex> {
    if query.is_empty() { bail!("搜索内容不能为空"); }
    if query.len() > 1000 { bail!("搜索内容太长"); }
    let mut pattern = if regex { query.to_string() } else { regex::escape(query) };
    if whole_word { pattern = format!(r"\b(?:{pattern})\b"); }
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        // 一行一行地搜，^ $ 按行算；限制编译后的大小，挡住写坏的正则
        .multi_line(true)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| anyhow::anyhow!("正则表达式有误：{e}"))
}

fn walker(root: &Path) -> ignore::WalkBuilder {
    let mut builder = ignore::WalkBuilder::new(root);
    // 点开头的文件也搜（.env、.github 这些常要改），.gitignore 不在 Git 仓库里也认；.git 目录本身跳过
    builder.hidden(false).require_git(false).filter_entry(|e| e.file_name() != ".git");
    builder
}

fn search(root: &Path, matcher: &regex::Regex) -> SearchResult {
    let started = Instant::now();
    let found = Mutex::new(Vec::new());
    let total = AtomicUsize::new(0);
    let truncated = AtomicBool::new(false);
    walker(root).build_parallel().run(|| {
        Box::new(|entry| {
            use ignore::WalkState;
            if total.load(Ordering::Relaxed) >= MAX_MATCHES || started.elapsed() > SEARCH_DEADLINE {
                truncated.store(true, Ordering::Relaxed);
                return WalkState::Quit;
            }
            let Ok(entry) = entry else { return WalkState::Continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) { return WalkState::Continue; }
            if entry.metadata().map(|m| m.len() > MAX_SEARCH_FILE).unwrap_or(true) { return WalkState::Continue; }
            let Ok(bytes) = std::fs::read(entry.path()) else { return WalkState::Continue };
            // 和编辑器一样：带 NUL 的是二进制，不是 UTF-8 的不搜
            if bytes.contains(&0) { return WalkState::Continue; }
            let Ok(text) = std::str::from_utf8(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes)) else { return WalkState::Continue };
            let matches = search_text(text, matcher);
            if matches.is_empty() { return WalkState::Continue; }
            let before = total.fetch_add(matches.len(), Ordering::Relaxed);
            let keep = MAX_MATCHES.saturating_sub(before).min(matches.len());
            if keep < matches.len() { truncated.store(true, Ordering::Relaxed); }
            if keep > 0 {
                let path = entry.path().to_string_lossy().into_owned();
                found.lock().unwrap().push(SearchFile { path, matches: matches.into_iter().take(keep).collect() });
            }
            WalkState::Continue
        })
    });
    let mut files = found.into_inner().unwrap();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    SearchResult { files, truncated: truncated.into_inner() }
}

fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

fn search_text(text: &str, matcher: &regex::Regex) -> Vec<SearchMatch> {
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        for m in matcher.find_iter(line) {
            // 空匹配（比如只写了 ^）没有意义，跳过
            if m.start() == m.end() { continue; }
            let (preview, preview_start) = preview(line, m.start());
            out.push(SearchMatch {
                line: index as u32 + 1,
                col: utf16_len(&line[..m.start()]) + 1,
                len: utf16_len(m.as_str()),
                preview_col: utf16_len(&line[preview_start..m.start()]),
                preview,
            });
            if out.len() >= MAX_MATCHES_PER_FILE { return out; }
        }
    }
    out
}

/// 一行太长时只取匹配前后一段；返回预览和它在原行里从哪个字节开始
fn preview(line: &str, start: usize) -> (String, usize) {
    let lead = line[..start].trim_start();
    let begin = start - lead.len();
    // 匹配前面最多留 40 个字，后面截到 PREVIEW_CHARS
    let begin = line[begin..start].char_indices().rev().nth(39).map(|(i, _)| begin + i).unwrap_or(begin);
    let preview: String = line[begin..].chars().take(PREVIEW_CHARS).collect();
    (preview, begin)
}

fn list_files(root: &Path) -> FileList {
    let started = Instant::now();
    let mut files = Vec::new();
    let mut bytes = 0;
    let mut truncated = false;
    for entry in walker(root).build().flatten() {
        if started.elapsed() > SEARCH_DEADLINE || bytes > MAX_FILE_LIST_BYTES {
            truncated = true;
            break;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) { continue; }
        let Ok(rel) = entry.path().strip_prefix(root) else { continue };
        let rel = rel.to_string_lossy().replace('\\', "/");
        bytes += rel.len() + 4;
        files.push(rel);
    }
    files.sort();
    FileList { root: root.to_string_lossy().into_owned(), files, truncated }
}

async fn git(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C").arg(dir).args(args)
        // 定时轮询 git status，别去抢仓库的 index 锁，免得终端里的 git 命令报“index.lock 已存在”
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let out = tokio::time::timeout(Duration::from_secs(8), cmd.output()).await.ok()?.ok()?;
    out.status.success().then_some(out.stdout)
}

async fn git_status(root: &Path) -> Option<GitStatus> {
    let top = git(root, &["rev-parse", "--show-toplevel"]).await?;
    let top = String::from_utf8(top).ok()?.trim().to_string();
    let prefix = String::from_utf8(git(root, &["rev-parse", "--show-prefix"]).await?).ok()?.trim().to_string();
    let out = git(root, &["status", "--porcelain=v1", "-z", "--branch", "--untracked-files=normal"]).await?;
    let (branch, entries, (upstream, ahead, behind)) = parse_status(&out);
    // git 在 Windows 上回的是 C:/a/b 这种写法，换成系统自己的分隔符，和文件树里的路径对得上
    let top = PathBuf::from(top).components().collect::<PathBuf>().to_string_lossy().into_owned();
    Some(GitStatus { top, prefix, branch, entries, upstream, ahead, behind })
}

/// 上游分支、领先几个、落后几个
type Tracking = (Option<String>, u32, u32);

fn parse_status(out: &[u8]) -> (Option<String>, Vec<GitEntry>, Tracking) {
    let mut branch = None;
    let mut tracking: Tracking = (None, 0, 0);
    let mut entries = Vec::new();
    let mut parts = out.split(|b| *b == 0).filter(|p| !p.is_empty());
    while let Some(part) = parts.next() {
        let text = String::from_utf8_lossy(part);
        if let Some(head) = text.strip_prefix("## ") {
            // “main...origin/main [ahead 1]”、“No commits yet on main”、“HEAD (no branch)”
            let name = head.strip_prefix("No commits yet on ").unwrap_or(head);
            let name = name.split("...").next().unwrap_or(name).split(' ').next().unwrap_or(name);
            branch = Some(name.to_string());
            if let Some((_, rest)) = head.split_once("...") {
                let upstream = rest.split(' ').next().unwrap_or(rest);
                let count = |word: &str| rest.split(word).nth(1).and_then(|s| s.trim().split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()).unwrap_or(0);
                tracking = (Some(upstream.to_string()), count("ahead "), count("behind "));
            }
            continue;
        }
        if text.len() < 4 { continue; }
        let status = text[..2].to_string();
        // 改名和复制后面还跟着一项原来的路径
        if status.starts_with('R') || status.starts_with('C') { parts.next(); }
        if entries.len() < MAX_GIT_ENTRIES {
            entries.push(GitEntry { path: text[3..].to_string(), status });
        }
    }
    (branch, entries, tracking)
}

/// Git 最新提交里的这个文件，按文本读出来，和编辑器里打开时做同样的检查
pub async fn read_head(path: &str) -> Result<(crate::remote_fs::FileMeta, String)> {
    let path = resolve(path)?;
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { bail!("路径不对") };
    let spec = format!("HEAD:./{}", name.to_string_lossy());
    let bytes = git(dir, &["show", &spec]).await.context("这个文件不在 Git 的最新提交里")?;
    if bytes.len() as u64 > crate::remote_fs::MAX_OPEN_BYTES { bail!("文件太大，不能对比"); }
    let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    if body.contains(&0) { bail!("这是二进制文件，不能在编辑器里打开"); }
    let text = std::str::from_utf8(body).map_err(|_| anyhow::anyhow!("文件不是 UTF-8 编码（可能是 GBK 等），暂不支持在编辑器里打开"))?.to_string();
    let meta = crate::remote_fs::FileMeta {
        path: path.to_string_lossy().into_owned(),
        hash: blake3::hash(&bytes).to_hex().to_string(),
        size: bytes.len() as u64,
        bom: bytes.starts_with(b"\xEF\xBB\xBF"),
        readonly: true,
        mtime: None,
    };
    Ok((meta, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("p2pcli-ops-{:x}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn search_respects_gitignore_and_reports_utf16_columns() {
        let root = temp_dir();
        std::fs::write(root.join(".gitignore"), "dist/\n").unwrap();
        std::fs::create_dir_all(root.join("dist")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("dist/out.js"), "needle\n").unwrap();
        std::fs::write(root.join(".git/config"), "needle\n").unwrap();
        std::fs::write(root.join("bin.dat"), b"needle\0").unwrap();
        std::fs::write(root.join("src/a.rs"), "// 中文 Needle here\nlet needle = 1;\n").unwrap();
        std::fs::write(root.join(".env"), "NEEDLE=1\n").unwrap();
        let reply = handle(EditRequest::Search { root: root.to_string_lossy().into(), query: "needle".into(), regex: false, case_sensitive: false, whole_word: false }).await.unwrap();
        let EditReply::Search { result } = reply else { panic!() };
        let names: Vec<_> = result.files.iter().map(|f| Path::new(&f.path).file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, [".env", "a.rs"], "{result:?}");
        let a = &result.files[1].matches;
        // “// 中文 ” 是 6 个 UTF-16 单位，匹配从第 7 列开始
        assert_eq!((a[0].line, a[0].col, a[0].len, a[0].preview_col), (1, 7, 6, 6));
        assert_eq!(a[0].preview, "// 中文 Needle here");
        assert_eq!((a[1].line, a[1].col), (2, 5));

        let EditReply::Search { result } = handle(EditRequest::Search { root: root.to_string_lossy().into(), query: "needle".into(), regex: false, case_sensitive: true, whole_word: true }).await.unwrap() else { panic!() };
        assert_eq!(result.files.len(), 1);
        assert!(handle(EditRequest::Search { root: root.to_string_lossy().into(), query: "(".into(), regex: true, case_sensitive: false, whole_word: false }).await.is_err());

        let EditReply::Files { list } = handle(EditRequest::ListFiles { root: root.to_string_lossy().into() }).await.unwrap() else { panic!() };
        assert_eq!(list.files, [".env", ".gitignore", "bin.dat", "src/a.rs"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn long_lines_are_trimmed_around_the_match() {
        let line = format!("{}needle{}", "x".repeat(500), "y".repeat(500));
        let m = search_text(&line, &build_matcher("needle", false, false, false).unwrap());
        assert_eq!(m[0].col, 501);
        assert_eq!(m[0].preview_col, 40);
        assert_eq!(&m[0].preview[40..46], "needle");
        assert_eq!(m[0].preview.chars().count(), PREVIEW_CHARS);
    }

    #[tokio::test]
    async fn create_rename_stat_and_refuse_overwrite() {
        let root = temp_dir();
        let a = root.join("a.txt").to_string_lossy().into_owned();
        let b = root.join("b.txt").to_string_lossy().into_owned();
        handle(EditRequest::CreateFile { path: a.clone() }).await.unwrap();
        assert!(handle(EditRequest::CreateFile { path: a.clone() }).await.is_err(), "已存在的不能再建");
        handle(EditRequest::CreateFile { path: b.clone() }).await.unwrap();
        assert!(handle(EditRequest::Rename { from: a.clone(), to: b.clone() }).await.is_err(), "改名不能盖掉别的文件");
        let c = root.join("dir").to_string_lossy().into_owned();
        handle(EditRequest::CreateDir { path: c.clone() }).await.unwrap();
        let moved = root.join("dir/a.txt").to_string_lossy().into_owned();
        handle(EditRequest::Rename { from: a.clone(), to: moved.clone() }).await.unwrap();
        let EditReply::Stats { stats } = handle(EditRequest::Stat { paths: vec![a, moved, c] }).await.unwrap() else { panic!() };
        assert!(stats[0].is_none());
        assert!(stats[1].as_ref().is_some_and(|s| !s.dir && s.size == 0 && s.mtime.is_some()));
        assert!(stats[2].as_ref().is_some_and(|s| s.dir));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn parses_porcelain_status() {
        let out = b"## main...origin/main [ahead 1]\0 M src/a.rs\0?? new.txt\0R  b.rs\0old.rs\0A  \xE4\xB8\xAD.md\0";
        let (branch, entries, _) = parse_status(out);
        assert_eq!(branch.as_deref(), Some("main"));
        let got: Vec<_> = entries.iter().map(|e| (e.status.as_str(), e.path.as_str())).collect();
        assert_eq!(got, [(" M", "src/a.rs"), ("??", "new.txt"), ("R ", "b.rs"), ("A ", "中.md")]);
        assert_eq!(parse_status(b"## No commits yet on dev\0").0.as_deref(), Some("dev"));
    }

    #[test]
    fn parses_branch_tracking() {
        let (branch, entries, tracking) = parse_status(b"## main...origin/main [ahead 2, behind 13]\0 M src/a.rs\0?? new.txt\0");
        assert_eq!(branch.as_deref(), Some("main"));
        assert_eq!(entries.len(), 2);
        assert_eq!(tracking, (Some("origin/main".into()), 2, 13));
        assert_eq!(parse_status(b"## feature...origin/feature [behind 1]\0").2, (Some("origin/feature".into()), 0, 1));
        assert_eq!(parse_status(b"## main...origin/main\0").2, (Some("origin/main".into()), 0, 0));
        assert_eq!(parse_status(b"## No commits yet on main\0").2, (None, 0, 0));
        assert_eq!(parse_status(b"## dev\0").2, (None, 0, 0));
    }

    #[tokio::test]
    async fn git_status_and_head_content() {
        if std::process::Command::new("git").arg("--version").output().is_err() { return; }
        let root = temp_dir();
        let run = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&root).args(args).output().unwrap().status.success(), "{args:?}");
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("a.txt"), "v1\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        std::fs::write(root.join("a.txt"), "v2\n").unwrap();
        std::fs::write(root.join("new.txt"), "x").unwrap();
        let EditReply::Git { status: Some(status) } = handle(EditRequest::GitStatus { root: root.to_string_lossy().into() }).await.unwrap() else { panic!("应该认出仓库") };
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.prefix, "");
        assert_eq!(std::fs::canonicalize(&status.top).unwrap(), std::fs::canonicalize(&root).unwrap());
        let got: Vec<_> = status.entries.iter().map(|e| (e.status.as_str(), e.path.as_str())).collect();
        assert_eq!(got, [(" M", "a.txt"), ("??", "new.txt")]);
        let (meta, text) = read_head(&root.join("a.txt").to_string_lossy()).await.unwrap();
        assert_eq!(text, "v1\n");
        assert!(meta.readonly);
        assert!(read_head(&root.join("new.txt").to_string_lossy()).await.is_err());
        let outside = temp_dir();
        assert!(matches!(handle(EditRequest::GitStatus { root: outside.to_string_lossy().into() }).await.unwrap(), EditReply::Git { status: None }));
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }
}

//! 远程编辑：列目录（含文件）、读文本文件、写回文本文件。
//! 编辑在连接方本机内存里进行，这里只负责一次读、一次写；写的时候核对读取时的哈希，
//! 文件在被连电脑上被别的程序（比如终端里的 AI）改过就报冲突，不悄悄盖掉。
//! 能连上来的设备本来就能在终端里读写任何文件，这里不额外放权。

use std::io::Write;
use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// 编辑器最多打开这么大的文件；更大的多半是日志或数据，不适合在编辑器里改
pub const MAX_OPEN_BYTES: u64 = 5 * 1024 * 1024;
/// 保存时允许的最大内容：打开以后还会再写，留些余量
pub const MAX_SAVE_BYTES: u64 = 16 * 1024 * 1024;
/// 一个目录最多回这么多项；再多前端也画不过来
const MAX_ENTRIES: usize = 5000;
/// 冲突错误的前缀，前端按它弹“覆盖还是重新加载”
pub const CONFLICT: &str = "conflict:";
const BOM: &[u8] = b"\xEF\xBB\xBF";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FsEntry {
    pub name: String,
    pub dir: bool,
    /// 文件大小；目录是 0
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntryListing {
    pub path: String,
    pub parent: Option<String>,
    /// 目录在前、文件在后，各自按名字排好，点开头的放在各自后面
    pub entries: Vec<FsEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileMeta {
    pub path: String,
    /// 文件原始字节的 BLAKE3，保存时用来判断有没有被别人改过
    pub hash: String,
    /// 文件字节数（含 BOM）
    pub size: u64,
    /// 文件开头有 UTF-8 BOM；内容里不带，保存时原样加回去
    pub bom: bool,
    /// 当前用户没有写权限
    pub readonly: bool,
    /// 修改时间（毫秒）；读不到时没有
    pub mtime: Option<u64>,
}

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn mtime_ms(meta: &std::fs::Metadata) -> Option<u64> {
    meta.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as u64)
}

pub async fn list_entries(path: &str) -> Result<EntryListing> {
    let path = crate::dirs::resolve(path)?;
    tokio::task::spawn_blocking(move || list_entries_blocking(&path)).await?
}

fn list_entries_blocking(path: &Path) -> Result<EntryListing> {
    let read = std::fs::read_dir(path).with_context(|| format!("打不开目录 {}", path.display()))?;
    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in read.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
        let Ok(kind) = entry.file_type() else { continue };
        // 符号链接跟过去看指向什么；指向不存在的东西就当文件，打开时再报错
        let (dir, size) = if kind.is_symlink() {
            match std::fs::metadata(entry.path()) {
                Ok(meta) => (meta.is_dir(), if meta.is_dir() { 0 } else { meta.len() }),
                Err(_) => (false, 0),
            }
        } else if kind.is_dir() {
            (true, 0)
        } else {
            (false, entry.metadata().map(|m| m.len()).unwrap_or(0))
        };
        if entries.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        entries.push(FsEntry { name, dir, size });
    }
    entries.sort_by_cached_key(|e| (!e.dir, e.name.starts_with('.'), e.name.to_lowercase()));
    Ok(EntryListing {
        path: path.to_string_lossy().into_owned(),
        parent: path.parent().map(|p| p.to_string_lossy().into_owned()),
        entries,
        truncated,
    })
}

pub async fn read_text(path: &str) -> Result<(FileMeta, String)> {
    let path = crate::dirs::resolve(path)?;
    tokio::task::spawn_blocking(move || read_text_blocking(&path)).await?
}

fn read_text_blocking(path: &Path) -> Result<(FileMeta, String)> {
    let meta = std::fs::metadata(path).with_context(|| format!("打不开文件 {}", path.display()))?;
    if meta.is_dir() { bail!("这是一个目录：{}", path.display()); }
    if !meta.is_file() { bail!("不是普通文件，不能在编辑器里打开：{}", path.display()); }
    if meta.len() > MAX_OPEN_BYTES {
        bail!("文件太大（{:.1} MB），编辑器最多打开 5 MB 的文件", meta.len() as f64 / 1048576.0);
    }
    let bytes = std::fs::read(path).with_context(|| format!("读取文件失败 {}", path.display()))?;
    let body = bytes.strip_prefix(BOM).unwrap_or(&bytes);
    // 文本文件里不会有 NUL；有就是二进制，强行当文本改会把文件写坏
    if body.contains(&0) { bail!("这是二进制文件，不能在编辑器里打开"); }
    let text = std::str::from_utf8(body)
        .map_err(|_| anyhow::anyhow!("文件不是 UTF-8 编码（可能是 GBK 等），暂不支持在编辑器里打开"))?
        .to_string();
    Ok((file_meta(path, &bytes, &meta), text))
}

fn file_meta(path: &Path, bytes: &[u8], meta: &std::fs::Metadata) -> FileMeta {
    FileMeta {
        path: path.to_string_lossy().into_owned(),
        hash: hash(bytes),
        size: bytes.len() as u64,
        bom: bytes.starts_with(BOM),
        readonly: meta.permissions().readonly(),
        mtime: mtime_ms(meta),
    }
}

/// 保存。`expected` 是打开时的哈希：对不上说明被别人改过，`force` 为假时报冲突。
/// `expected` 为空表示新建文件，文件已经存在时同样报冲突。
pub async fn write_text(path: &str, text: String, bom: bool, expected: Option<String>, force: bool) -> Result<FileMeta> {
    let path = crate::dirs::resolve(path)?;
    tokio::task::spawn_blocking(move || write_text_blocking(&path, &text, bom, expected.as_deref(), force)).await?
}

fn write_text_blocking(path: &Path, text: &str, bom: bool, expected: Option<&str>, force: bool) -> Result<FileMeta> {
    let mut bytes = Vec::with_capacity(text.len() + 3);
    if bom { bytes.extend_from_slice(BOM); }
    bytes.extend_from_slice(text.as_bytes());
    if bytes.len() as u64 > MAX_SAVE_BYTES { bail!("内容太大，最多保存 16 MB"); }
    if !force {
        match (std::fs::read(path), expected) {
            (Ok(current), Some(expected)) if hash(&current) != expected => bail!("{CONFLICT} 文件在对方电脑上已被修改"),
            (Ok(_), None) => bail!("{CONFLICT} 文件已存在"),
            (Err(e), Some(_)) if e.kind() == std::io::ErrorKind::NotFound => bail!("{CONFLICT} 文件在对方电脑上已被删除"),
            _ => {}
        }
    }
    if path.is_dir() { bail!("这是一个目录：{}", path.display()); }
    // 原地覆盖而不是写临时文件再改名：保留文件的权限、属主、硬链接，符号链接也照常写到指向的文件
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(path)
        .with_context(|| format!("无法写入 {}", path.display()))?;
    file.write_all(&bytes).with_context(|| format!("写入失败 {}", path.display()))?;
    file.sync_all().ok();
    let meta = std::fs::metadata(path)?;
    Ok(file_meta(path, &bytes, &meta))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("p2pcli-remote-fs-{:x}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn lists_directories_first_then_files() {
        let root = temp_dir();
        for dir in ["src", ".git", "Docs"] { std::fs::create_dir(root.join(dir)).unwrap(); }
        for file in ["b.rs", "A.md", ".env"] { std::fs::write(root.join(file), "12345").unwrap(); }
        let listing = list_entries(root.to_str().unwrap()).await.unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        let names: Vec<_> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Docs", "src", ".git", "A.md", "b.rs", ".env"]);
        assert!(listing.entries[0].dir && !listing.entries[3].dir);
        assert_eq!(listing.entries[3].size, 5);
    }

    #[tokio::test]
    async fn round_trips_text_and_keeps_bom_and_line_endings() {
        let root = temp_dir();
        let file = root.join("a.txt");
        std::fs::write(&file, b"\xEF\xBB\xBFline1\r\n\xE4\xB8\xAD\r\n").unwrap();
        let path = file.to_str().unwrap();
        let (meta, text) = read_text(path).await.unwrap();
        assert!(meta.bom);
        assert_eq!(text, "line1\r\n中\r\n");
        let saved = write_text(path, text.replace("line1", "line2"), meta.bom, Some(meta.hash.clone()), false).await.unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"\xEF\xBB\xBFline2\r\n\xE4\xB8\xAD\r\n");
        assert_ne!(saved.hash, meta.hash);
        // 用旧哈希再存一次就是冲突；force 才能覆盖
        let err = write_text(path, "x".into(), false, Some(meta.hash.clone()), false).await.unwrap_err();
        assert!(err.to_string().starts_with(CONFLICT));
        write_text(path, "x".into(), false, Some(meta.hash), true).await.unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"x");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn refuses_binary_non_utf8_large_and_deleted_files() {
        let root = temp_dir();
        let bin = root.join("a.bin");
        std::fs::write(&bin, b"ab\0cd").unwrap();
        assert!(read_text(bin.to_str().unwrap()).await.unwrap_err().to_string().contains("二进制"));
        let gbk = root.join("gbk.txt");
        std::fs::write(&gbk, b"\xD6\xD0\xCE\xC4").unwrap();
        assert!(read_text(gbk.to_str().unwrap()).await.unwrap_err().to_string().contains("UTF-8"));
        let big = root.join("big.log");
        std::fs::write(&big, vec![b'a'; MAX_OPEN_BYTES as usize + 1]).unwrap();
        assert!(read_text(big.to_str().unwrap()).await.unwrap_err().to_string().contains("太大"));
        let gone = root.join("gone.txt");
        let err = write_text(gone.to_str().unwrap(), "x".into(), false, Some("h".into()), false).await.unwrap_err();
        assert!(err.to_string().contains("已被删除"));
        // 新建：不存在时可以建，已存在时冲突
        write_text(gone.to_str().unwrap(), "x".into(), false, None, false).await.unwrap();
        assert!(write_text(gone.to_str().unwrap(), "y".into(), false, None, false).await.is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn saving_keeps_permissions_and_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir();
        let script = root.join("run.sh");
        std::fs::write(&script, "echo 1\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = root.join("link.sh");
        std::os::unix::fs::symlink(&script, &link).unwrap();
        let (meta, _) = read_text(link.to_str().unwrap()).await.unwrap();
        write_text(link.to_str().unwrap(), "echo 2\n".into(), false, Some(meta.hash), false).await.unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&script).unwrap(), "echo 2\n");
        assert_eq!(std::fs::metadata(&script).unwrap().permissions().mode() & 0o777, 0o755);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

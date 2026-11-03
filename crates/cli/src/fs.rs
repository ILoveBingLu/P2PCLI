//! 直接读写另一台设备上的文件：不用先拉过来、改完再发回去。
//! 和 AI 自带的读、写、改、搜工具一一对应，AI 在终端里调这些命令就能改对方电脑上的代码。
//! 对方路径：绝对路径、`~` 开头，或者相对路径（从对方的用户目录算起）。

use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use p2pcli_core::control::{Reply, Request};
use p2pcli_core::remote_ops::{EditReply, EditRequest};

use crate::{ask, print_json};

#[derive(Subcommand)]
pub enum FsCmd {
    /// 读对方电脑上的文本文件（UTF-8，不超过 5 MB），原样打印
    Cat {
        /// 设备名（可以只写一部分）、9 位设备码或公钥开头几位
        device: String,
        /// 对方电脑上的文件
        path: String,
        /// 每行前面加行号（和 cat -n 一样）
        #[arg(short = 'n', long)]
        number: bool,
        /// 从第几行开始（从 1 算）
        #[arg(long, default_value_t = 1)]
        offset: usize,
        /// 最多打印几行
        #[arg(long)]
        limit: Option<usize>,
    },
    /// 把标准输入的内容整个写进对方电脑上的文件：没有就新建，有就整个替换（原来的 BOM 保留）
    Write {
        /// 设备名（可以只写一部分）、9 位设备码或公钥开头几位
        device: String,
        /// 对方电脑上的文件
        path: String,
    },
    /// 按原文替换对方电脑上文件里的一段：原文要和文件里一字不差，并且只出现一次（或者加 --all）。
    /// 写回时核对文件没被别人改过，改过就报错不盖掉
    Edit {
        /// 设备名（可以只写一部分）、9 位设备码或公钥开头几位
        device: String,
        /// 对方电脑上的文件
        path: String,
        /// 要替换掉的原文（包括缩进和换行）
        #[arg(long, allow_hyphen_values = true)]
        old: String,
        /// 换成的新内容，可以是空的（等于删掉原文）
        #[arg(long, allow_hyphen_values = true)]
        new: String,
        /// 原文出现多次时全部替换
        #[arg(long)]
        all: bool,
    },
    /// 列出对方电脑上一个目录里的东西，目录后面带 /
    Ls {
        /// 设备名（可以只写一部分）、9 位设备码或公钥开头几位
        device: String,
        /// 不写就是对方的用户目录
        #[arg(default_value = "~")]
        path: String,
    },
    /// 在对方电脑上一个目录下按名字找文件（认 .gitignore）。模式可以用 * ? **；不带 / 的只看文件名；不带通配符时按包含找
    Find {
        /// 设备名（可以只写一部分）、9 位设备码或公钥开头几位
        device: String,
        /// 在哪个目录下找
        root: String,
        /// 名字模式，比如 *.rs、src/**/*.ts、config；不写列出全部
        pattern: Option<String>,
    },
    /// 在对方电脑上一个目录下搜文件内容（认 .gitignore，跳过二进制和超过 2 MB 的文件）。
    /// 每行打印 路径:行号:内容；没找到时退出码是 1
    Grep {
        /// 设备名（可以只写一部分）、9 位设备码或公钥开头几位
        device: String,
        /// 要找的内容，默认是正则表达式
        pattern: String,
        /// 在哪个目录下搜
        root: String,
        /// 按普通文字找，不当正则
        #[arg(short = 'F', long)]
        fixed: bool,
        /// 不区分大小写
        #[arg(short = 'i', long)]
        ignore_case: bool,
        /// 全字匹配
        #[arg(short = 'w', long)]
        word: bool,
    },
    /// 在对方电脑上新建目录
    Mkdir {
        /// 设备名（可以只写一部分）、9 位设备码或公钥开头几位
        device: String,
        /// 要建的目录
        path: String,
        /// 上级目录不存在时一起建
        #[arg(short = 'p', long)]
        parents: bool,
    },
    /// 在对方电脑上改名或移动文件、目录（不会盖掉已有的同名文件）
    Mv {
        /// 设备名（可以只写一部分）、9 位设备码或公钥开头几位
        device: String,
        /// 原来的路径
        from: String,
        /// 新的路径
        to: String,
    },
    /// 把对方电脑上的文件或目录移到废纸篓（回收站），需要时可以从那里找回
    Rm {
        /// 设备名（可以只写一部分）、9 位设备码或公钥开头几位
        device: String,
        /// 要移到废纸篓的文件或目录
        path: String,
    },
}

/// 对方路径：相对路径从对方的用户目录算起（和 pull、exec 一样）
pub fn remote_path(path: &str) -> String {
    let path = path.trim();
    let absolute = path.starts_with('/') || path.starts_with('~') || path.starts_with("\\\\")
        || (path.len() >= 3 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':' && matches!(path.as_bytes()[2], b'\\' | b'/'));
    if path.is_empty() { "~".into() } else if absolute { path.into() } else { format!("~/{path}") }
}

pub async fn run(cmd: FsCmd, dir: &Path, json: bool) -> Result<ExitCode> {
    match cmd {
        FsCmd::Cat { device, path, number, offset, limit } => {
            let (meta, text) = ask(dir, Request::ReadFile { target: device, path: remote_path(&path) }, |r| match r {
                Reply::File { meta, text } => Some((meta, text)),
                _ => None,
            }).await?;
            let lines: Vec<&str> = text.split_inclusive('\n').collect();
            let start = offset.max(1) - 1;
            let end = limit.map_or(lines.len(), |l| (start + l).min(lines.len()));
            let picked = lines.get(start..end).unwrap_or_default();
            if json {
                print_json(&serde_json::json!({
                    "path": meta.path, "hash": meta.hash, "size": meta.size, "readonly": meta.readonly,
                    "total_lines": lines.len(), "offset": start + 1, "text": picked.concat(),
                }));
            } else {
                let mut out = std::io::stdout().lock();
                for (i, line) in picked.iter().enumerate() {
                    if number { write!(out, "{:>6}\t", start + i + 1)?; }
                    out.write_all(line.as_bytes())?;
                }
                out.flush()?;
            }
        }
        FsCmd::Write { device, path } => {
            let mut bytes = Vec::new();
            std::io::stdin().read_to_end(&mut bytes).context("读标准输入失败")?;
            let text = String::from_utf8(bytes).context("写入的内容不是 UTF-8 文本")?;
            let meta = ask(dir, Request::WriteFile { target: device, path: remote_path(&path), text }, saved).await?.0;
            if json { print_json(&meta); } else { println!("已写入 {}（{} 字节）", meta.path, meta.size); }
        }
        FsCmd::Edit { device, path, old, new, all } => {
            let (meta, replaced) = ask(dir, Request::EditFile { target: device, path: remote_path(&path), old, new, all }, saved).await?;
            if json { print_json(&serde_json::json!({ "path": meta.path, "replaced": replaced, "hash": meta.hash, "size": meta.size })); }
            else { println!("已替换 {replaced} 处：{}", meta.path); }
        }
        FsCmd::Ls { device, path } => {
            let listing = ask(dir, Request::ListEntries { target: device, path: remote_path(&path) }, |r| match r {
                Reply::Entries { listing } => Some(listing),
                _ => None,
            }).await?;
            if json { print_json(&listing); } else {
                for e in &listing.entries { println!("{}{}", e.name, if e.dir { "/" } else { "" }); }
                if listing.truncated { eprintln!("（目录里东西太多，只列出了前 {} 项）", listing.entries.len()); }
            }
        }
        FsCmd::Find { device, root, pattern } => {
            let EditReply::Files { list } = edit(dir, device, EditRequest::ListFiles { root: remote_path(&root) }).await? else { bail!("P2PCLI 回的结果不对") };
            let files: Vec<&String> = list.files.iter().filter(|f| pattern.as_deref().is_none_or(|p| name_matches(p, f))).collect();
            if json { print_json(&serde_json::json!({ "root": list.root, "files": files, "truncated": list.truncated })); } else {
                for f in &files { println!("{f}"); }
                if list.truncated { eprintln!("（文件太多，只看了一部分）"); }
            }
        }
        FsCmd::Grep { device, pattern, root, fixed, ignore_case, word } => {
            let request = EditRequest::Search { root: remote_path(&root), query: pattern, regex: !fixed, case_sensitive: !ignore_case, whole_word: word };
            let EditReply::Search { result } = edit(dir, device, request).await? else { bail!("P2PCLI 回的结果不对") };
            let found = result.files.iter().map(|f| f.matches.len()).sum::<usize>();
            if json { print_json(&result); } else {
                for file in &result.files {
                    for m in &file.matches { println!("{}:{}:{}", file.path, m.line, m.preview); }
                }
                if result.truncated { eprintln!("（结果太多或搜索超时，只列出了一部分）"); }
            }
            if found == 0 { return Ok(ExitCode::from(1)); }
        }
        FsCmd::Mkdir { device, path, parents } => {
            let path = remote_path(&path);
            let targets = if parents { ancestors(&path) } else { vec![path.clone()] };
            for (i, target) in targets.iter().enumerate() {
                let last = i + 1 == targets.len();
                if parents && !last {
                    // 已经有的上级目录跳过
                    let EditReply::Stats { stats } = edit(dir, device.clone(), EditRequest::Stat { paths: vec![target.clone()] }).await? else { bail!("P2PCLI 回的结果不对") };
                    if stats.first().is_some_and(|s| s.as_ref().is_some_and(|s| s.dir)) { continue; }
                }
                let result = edit(dir, device.clone(), EditRequest::CreateDir { path: target.clone() }).await;
                // -p 时最后一级已经存在也不算错
                if let Err(e) = result && !(parents && format!("{e:#}").contains("exists")) { return Err(e); }
            }
            if json { print_json(&serde_json::json!({ "path": path })); } else { println!("已建好 {path}"); }
        }
        FsCmd::Mv { device, from, to } => {
            let (from, to) = (remote_path(&from), remote_path(&to));
            edit(dir, device, EditRequest::Rename { from: from.clone(), to: to.clone() }).await?;
            if json { print_json(&serde_json::json!({ "from": from, "to": to })); } else { println!("已移动 {from} → {to}"); }
        }
        FsCmd::Rm { device, path } => {
            let path = remote_path(&path);
            edit(dir, device, EditRequest::Delete { path: path.clone() }).await?;
            if json { print_json(&serde_json::json!({ "path": path, "trash": true })); } else { println!("已移到废纸篓：{path}"); }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn saved(reply: Reply) -> Option<(p2pcli_core::remote_fs::FileMeta, usize)> {
    match reply {
        Reply::Saved { meta, replaced } => Some((meta, replaced)),
        _ => None,
    }
}

async fn edit(dir: &Path, target: String, request: EditRequest) -> Result<EditReply> {
    ask(dir, Request::Edit { target, request }, |r| match r {
        Reply::EditDone { reply } => Some(reply),
        _ => None,
    }).await
}

/// ~/a/b/c → [~/a, ~/a/b, ~/a/b/c]；/a/b → [/a, /a/b]；C:\a\b → [C:\a, C:\a\b]
fn ancestors(path: &str) -> Vec<String> {
    let sep = if path.contains('\\') && !path.contains('/') { '\\' } else { '/' };
    let parts: Vec<&str> = path.split(['/', '\\']).collect();
    let mut out = Vec::new();
    let mut current = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 { current.push(sep); }
        current.push_str(part);
        // 根（""、"~"、"C:"）本身不建
        if i == 0 || part.is_empty() { continue; }
        out.push(current.clone());
    }
    out
}

/// 按名字找文件：带通配符时按通配符（* 不跨目录，** 跨目录，? 一个字），不带 / 的只看文件名；
/// 不带通配符时不分大小写按包含找
fn name_matches(pattern: &str, rel: &str) -> bool {
    let target = if pattern.contains('/') { rel } else { rel.rsplit('/').next().unwrap_or(rel) };
    if !pattern.contains(['*', '?']) {
        return target.to_lowercase().contains(&pattern.to_lowercase());
    }
    glob(pattern.as_bytes(), target.as_bytes())
}

fn glob(p: &[u8], s: &[u8]) -> bool {
    match p {
        [] => s.is_empty(),
        [b'*', b'*', rest @ ..] => {
            // **/ 可以一层都不匹配
            let rest = rest.strip_prefix(b"/").unwrap_or(rest);
            (0..=s.len()).any(|i| glob(rest, &s[i..]))
        }
        [b'*', rest @ ..] => (0..=s.len()).take_while(|&i| i == 0 || s[i - 1] != b'/').any(|i| glob(rest, &s[i..])),
        [b'?', rest @ ..] => !s.is_empty() && s[0] != b'/' && glob(rest, &s[1..]),
        [c, rest @ ..] => !s.is_empty() && s[0] == *c && glob(rest, &s[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths() {
        assert_eq!(remote_path("src/a.rs"), "~/src/a.rs");
        assert_eq!(remote_path("/etc/hosts"), "/etc/hosts");
        assert_eq!(remote_path("~/x"), "~/x");
        assert_eq!(remote_path("C:\\Users\\a"), "C:\\Users\\a");
        assert_eq!(remote_path(""), "~");
    }

    #[test]
    fn globs() {
        assert!(name_matches("*.rs", "src/core/lib.rs"));
        assert!(!name_matches("*.rs", "src/core/lib.rs.bak"));
        assert!(name_matches("src/**/*.ts", "src/a/b/c.ts"));
        assert!(name_matches("src/**/*.ts", "src/c.ts"));
        assert!(!name_matches("src/*.ts", "src/a/c.ts"));
        assert!(name_matches("Cargo", "crates/cli/Cargo.toml"));
        assert!(name_matches("lib?.rs", "x/lib2.rs"));
    }

    #[test]
    fn mkdir_parents() {
        assert_eq!(ancestors("~/a/b"), ["~/a", "~/a/b"]);
        assert_eq!(ancestors("/a/b"), ["/a", "/a/b"]);
        assert_eq!(ancestors("C:\\a\\b"), ["C:\\a", "C:\\a\\b"]);
    }
}

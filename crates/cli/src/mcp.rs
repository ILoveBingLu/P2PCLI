//! `p2pcli mcp`：标准 MCP 服务（stdio），让 Cursor、Claude Desktop、Cline 这类讲 MCP 的 AI 直接操作别的设备：
//! 列设备、跑命令、读写改文件、列目录、找文件、搜内容、建目录、移动、移到废纸篓、发文件、拉文件。
//!
//! 只是一层薄壳：每个工具都换成控制口的请求，交给正在运行的 P2PCLI 去办，和命令行的子命令走的是同一条路。
//! 协议是一行一条的 JSON-RPC 2.0；标准输出只写协议消息，别的都写标准错误。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow, bail};
use p2pcli_core::control::{self, Reply, Request};
use p2pcli_core::files::{Transfer, TransferState};
use p2pcli_core::remote_ops::{EditReply, EditRequest};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::fs::{ancestors, edit, name_matches, remote_path, saved};
use crate::{ask, device_line, size};

/// 认得的协议版本，新的在前；对方要的我们认得就用它，不认得就用最新的
const VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
/// 读文件默认最多给多少行
const READ_LINES: usize = 2000;
/// 一次回给 AI 的文字最多这么多字符，再多截掉并说明
const MAX_TEXT: usize = 200_000;

const INSTRUCTIONS: &str = "Tools for working on other computers paired in P2PCLI (the P2PCLI app must be running on this computer). \
Name the target with `device`: a device name (a unique part is enough), a 9-digit device code, or the first characters of its public key; \
call p2p_devices to see them. Remote paths are absolute, start with `~`, or are relative to the remote user's home directory. \
Prefer p2p_read_file / p2p_edit_file / p2p_write_file over shell commands for files.";

pub async fn run(dir: &Path) -> Result<()> {
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Value>();
    // 只有这个任务写标准输出：几个工具同时在跑时，回复不会交错
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(msg) = out_rx.recv().await {
            let mut line = serde_json::to_vec(&msg).unwrap_or_default();
            line.push(b'\n');
            if stdout.write_all(&line).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });
    let running: Arc<Mutex<HashMap<String, JoinHandle<()>>>> = Arc::default();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let _ = out_tx.send(error(Value::Null, -32700, &format!("看不懂的 JSON：{e}")));
                continue;
            }
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = id else {
            // 通知：只管取消
            if method == "notifications/cancelled"
                && let Some(req) = params.get("requestId")
                && let Some(task) = running.lock().unwrap().remove(&req.to_string())
            {
                task.abort();
            }
            continue;
        };
        match method.as_str() {
            "initialize" => {
                let wanted = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
                let version = VERSIONS.iter().find(|v| **v == wanted).copied().unwrap_or(VERSIONS[0]);
                let _ = out_tx.send(result(id, json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "p2pcli", "title": "P2PCLI", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                })));
            }
            "ping" => {
                let _ = out_tx.send(result(id, json!({})));
            }
            "tools/list" => {
                let _ = out_tx.send(result(id, json!({ "tools": tools() })));
            }
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                // 键和取消通知里的 requestId 对得上：都是 id 的 JSON 写法
                let key = id.to_string();
                let (dir, out, running2, key2) = (dir.to_path_buf(), out_tx.clone(), running.clone(), key.clone());
                let task = tokio::spawn(async move {
                    let reply = match call(&dir, &name, &args).await {
                        Ok(text) => json!({ "content": [{ "type": "text", "text": clip(text) }], "isError": false }),
                        Err(e) => json!({ "content": [{ "type": "text", "text": format!("{e:#}") }], "isError": true }),
                    };
                    running2.lock().unwrap().remove(&key2);
                    let _ = out.send(result(id, reply));
                });
                // 已经跑完的（很快的工具）就不记了
                let mut map = running.lock().unwrap();
                if !task.is_finished() {
                    map.insert(key, task);
                }
            }
            _ => {
                let _ = out_tx.send(error(id, -32601, &format!("不支持的方法：{method}")));
            }
        }
    }
    // 对方关掉了标准输入：等还在跑的工具回完再走
    let tasks: Vec<JoinHandle<()>> = running.lock().unwrap().drain().map(|(_, t)| t).collect();
    for t in tasks {
        let _ = t.await;
    }
    drop(out_tx);
    let _ = writer.await;
    Ok(())
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// 太长的结果截掉，告诉 AI 截了多少
fn clip(text: String) -> String {
    if text.chars().count() <= MAX_TEXT {
        return text;
    }
    let kept: String = text.chars().take(MAX_TEXT).collect();
    format!("{kept}\n…（结果太长，后面截掉了；请缩小范围，比如读文件时用 offset、limit）")
}

// ---------------------------------------------------------------- 工具清单

fn device_prop() -> Value {
    json!({ "type": "string", "description": "Target device: name (a unique part is enough), 9-digit device code, or the first characters of its public key" })
}

fn tools() -> Value {
    let dev = device_prop();
    let path = |d: &str| json!({ "type": "string", "description": d });
    json!([
        {
            "name": "p2p_devices",
            "title": "List devices",
            "description": "List the devices P2PCLI knows: online state, connection path (direct or relay) and latency, whether sending files needs approval, device code.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true },
        },
        {
            "name": "p2p_exec",
            "title": "Run a command",
            "description": "Run a shell command on another device and return stdout, stderr and the exit code. The command is parsed by the remote shell (zsh/bash on macOS/Linux, PowerShell on Windows). Stdin is empty; it cannot be interactive.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device": dev,
                    "command": { "type": "string", "description": "Command line to run" },
                    "cwd": path("Working directory on the remote device; defaults to the remote user's home directory"),
                    "timeout_secs": { "type": "integer", "minimum": 1, "description": "Kill the command after this many seconds" },
                },
                "required": ["device", "command"],
            },
            "annotations": { "destructiveHint": true, "openWorldHint": true },
        },
        {
            "name": "p2p_read_file",
            "title": "Read a file",
            "description": "Read a UTF-8 text file (up to 5 MB) on another device. Returns lines prefixed with line numbers (like cat -n), plus the total line count and the file hash.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device": dev,
                    "path": path("File on the remote device"),
                    "offset": { "type": "integer", "minimum": 1, "description": "First line to return (1-based)" },
                    "limit": { "type": "integer", "minimum": 1, "description": "Maximum number of lines to return (default 2000)" },
                },
                "required": ["device", "path"],
            },
            "annotations": { "readOnlyHint": true },
        },
        {
            "name": "p2p_write_file",
            "title": "Write a file",
            "description": "Create or overwrite a text file on another device with the given content (an existing BOM is kept).",
            "inputSchema": {
                "type": "object",
                "properties": { "device": dev, "path": path("File on the remote device"), "content": { "type": "string", "description": "The whole new content" } },
                "required": ["device", "path", "content"],
            },
            "annotations": { "destructiveHint": true },
        },
        {
            "name": "p2p_edit_file",
            "title": "Edit a file",
            "description": "Replace exact text in a file on another device. old_string must match the file exactly (including indentation and line breaks) and be unique unless replace_all is true. Fails instead of overwriting if someone else changed the file meanwhile.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device": dev,
                    "path": path("File on the remote device"),
                    "old_string": { "type": "string", "description": "Text to replace" },
                    "new_string": { "type": "string", "description": "Replacement text (may be empty)" },
                    "replace_all": { "type": "boolean", "description": "Replace every occurrence" },
                },
                "required": ["device", "path", "old_string", "new_string"],
            },
            "annotations": { "destructiveHint": true },
        },
        {
            "name": "p2p_list_dir",
            "title": "List a directory",
            "description": "List the entries of a directory on another device; directories end with /.",
            "inputSchema": { "type": "object", "properties": { "device": dev, "path": path("Directory; defaults to the remote home directory") }, "required": ["device"] },
            "annotations": { "readOnlyHint": true },
        },
        {
            "name": "p2p_find_files",
            "title": "Find files",
            "description": "Find files by name under a directory on another device (respects .gitignore). Patterns support * ? **; a pattern without / matches the file name only; without wildcards it matches names containing it (case-insensitive).",
            "inputSchema": {
                "type": "object",
                "properties": { "device": dev, "root": path("Directory to search in"), "pattern": { "type": "string", "description": "Name pattern, e.g. *.rs, src/**/*.ts; omit to list all files" } },
                "required": ["device", "root"],
            },
            "annotations": { "readOnlyHint": true },
        },
        {
            "name": "p2p_grep",
            "title": "Search file contents",
            "description": "Search file contents under a directory on another device (respects .gitignore, skips binary files and files over 2 MB). Returns path:line:text.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device": dev,
                    "pattern": { "type": "string", "description": "Regular expression (or plain text with fixed=true)" },
                    "root": path("Directory to search in"),
                    "fixed": { "type": "boolean", "description": "Treat pattern as plain text" },
                    "ignore_case": { "type": "boolean" },
                    "word": { "type": "boolean", "description": "Match whole words only" },
                },
                "required": ["device", "pattern", "root"],
            },
            "annotations": { "readOnlyHint": true },
        },
        {
            "name": "p2p_mkdir",
            "title": "Create a directory",
            "description": "Create a directory on another device.",
            "inputSchema": {
                "type": "object",
                "properties": { "device": dev, "path": path("Directory to create"), "parents": { "type": "boolean", "description": "Also create missing parent directories" } },
                "required": ["device", "path"],
            },
        },
        {
            "name": "p2p_move",
            "title": "Move or rename",
            "description": "Move or rename a file or directory on another device. Never overwrites an existing target.",
            "inputSchema": { "type": "object", "properties": { "device": dev, "from": path("Current path"), "to": path("New path") }, "required": ["device", "from", "to"] },
        },
        {
            "name": "p2p_trash",
            "title": "Move to trash",
            "description": "Move a file or directory on another device to its Trash / Recycle Bin (it can be restored from there).",
            "inputSchema": { "type": "object", "properties": { "device": dev, "path": path("File or directory") }, "required": ["device", "path"] },
            "annotations": { "destructiveHint": true },
        },
        {
            "name": "p2p_send_files",
            "title": "Send files",
            "description": "Send files or folders from this computer to another device. Without `to` they go to the remote inbox; with `to` they go to that remote directory (created if missing, same-name files are overwritten). Returns the paths on the remote device.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device": dev,
                    "paths": { "type": "array", "items": { "type": "string" }, "minItems": 1, "description": "Absolute paths on this computer" },
                    "to": path("Directory on the remote device"),
                },
                "required": ["device", "paths"],
            },
            "annotations": { "openWorldHint": true },
        },
        {
            "name": "p2p_pull_files",
            "title": "Pull files",
            "description": "Copy files or folders from another device to a directory on this computer (same-name files are overwritten). Returns the local paths.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device": dev,
                    "paths": { "type": "array", "items": { "type": "string" }, "minItems": 1, "description": "Paths on the remote device" },
                    "to": { "type": "string", "description": "Absolute directory on this computer (created if missing)" },
                },
                "required": ["device", "paths", "to"],
            },
        },
    ])
}

// ---------------------------------------------------------------- 工具

fn s(args: &Value, key: &str) -> Result<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string).ok_or_else(|| anyhow!("缺少参数 {key}"))
}

fn opt_s(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).filter(|v| !v.trim().is_empty()).map(str::to_string)
}

fn opt_u(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(Value::as_u64)
}

fn flag(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn list(args: &Value, key: &str) -> Result<Vec<String>> {
    let v: Vec<String> = args.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if v.is_empty() {
        bail!("缺少参数 {key}");
    }
    Ok(v)
}

async fn call(dir: &Path, name: &str, args: &Value) -> Result<String> {
    match name {
        "p2p_devices" => {
            let devices = ask(dir, Request::Devices, |r| match r {
                Reply::Devices { devices } => Some(devices),
                _ => None,
            })
            .await?;
            if devices.is_empty() {
                return Ok("还没有设备，先在 P2PCLI 里连一台".into());
            }
            Ok(devices.iter().map(device_line).collect::<Vec<_>>().join("\n"))
        }
        "p2p_exec" => exec(dir, Request::Exec { target: s(args, "device")?, command: s(args, "command")?, cwd: opt_s(args, "cwd"), timeout_secs: opt_u(args, "timeout_secs") }).await,
        "p2p_read_file" => {
            let (meta, text) = ask(dir, Request::ReadFile { target: s(args, "device")?, path: remote_path(&s(args, "path")?) }, |r| match r {
                Reply::File { meta, text } => Some((meta, text)),
                _ => None,
            })
            .await?;
            let lines: Vec<&str> = text.split_inclusive('\n').collect();
            let start = opt_u(args, "offset").unwrap_or(1).max(1) as usize - 1;
            let limit = opt_u(args, "limit").map_or(READ_LINES, |l| l as usize);
            let end = (start + limit).min(lines.len());
            let mut out = format!("{}（共 {} 行，{}，hash {}{}）\n", meta.path, lines.len(), size(meta.size), meta.hash, if meta.readonly { "，只读" } else { "" });
            for (i, line) in lines.get(start..end).unwrap_or_default().iter().enumerate() {
                out.push_str(&format!("{:>6}\t{}", start + i + 1, line));
                if !line.ends_with('\n') {
                    out.push('\n');
                }
            }
            if end < lines.len() {
                out.push_str(&format!("…（还有 {} 行，用 offset={} 接着读）\n", lines.len() - end, end + 1));
            }
            Ok(out)
        }
        "p2p_write_file" => {
            let (meta, _) = ask(dir, Request::WriteFile { target: s(args, "device")?, path: remote_path(&s(args, "path")?), text: s(args, "content")? }, saved).await?;
            Ok(format!("已写入 {}（{}）", meta.path, size(meta.size)))
        }
        "p2p_edit_file" => {
            let request = Request::EditFile {
                target: s(args, "device")?,
                path: remote_path(&s(args, "path")?),
                old: s(args, "old_string")?,
                new: s(args, "new_string")?,
                all: flag(args, "replace_all"),
            };
            let (meta, replaced) = ask(dir, request, saved).await?;
            Ok(format!("已替换 {replaced} 处：{}", meta.path))
        }
        "p2p_list_dir" => {
            let path = remote_path(&opt_s(args, "path").unwrap_or_else(|| "~".into()));
            let listing = ask(dir, Request::ListEntries { target: s(args, "device")?, path }, |r| match r {
                Reply::Entries { listing } => Some(listing),
                _ => None,
            })
            .await?;
            let mut out = format!("{}\n", listing.path);
            for e in &listing.entries {
                out.push_str(&format!("{}{}\n", e.name, if e.dir { "/" } else { "" }));
            }
            if listing.truncated {
                out.push_str(&format!("…（目录里东西太多，只列出了前 {} 项）\n", listing.entries.len()));
            }
            Ok(out)
        }
        "p2p_find_files" => {
            let pattern = opt_s(args, "pattern");
            let EditReply::Files { list } = edit(dir, s(args, "device")?, EditRequest::ListFiles { root: remote_path(&s(args, "root")?) }).await? else {
                bail!("P2PCLI 回的结果不对")
            };
            let files: Vec<&String> = list.files.iter().filter(|f| pattern.as_deref().is_none_or(|p| name_matches(p, f))).collect();
            let mut out = format!("{}（找到 {} 个）\n", list.root, files.len());
            for f in files {
                out.push_str(f);
                out.push('\n');
            }
            if list.truncated {
                out.push_str("…（文件太多，只看了一部分）\n");
            }
            Ok(out)
        }
        "p2p_grep" => {
            let request = EditRequest::Search {
                root: remote_path(&s(args, "root")?),
                query: s(args, "pattern")?,
                regex: !flag(args, "fixed"),
                case_sensitive: !flag(args, "ignore_case"),
                whole_word: flag(args, "word"),
            };
            let EditReply::Search { result } = edit(dir, s(args, "device")?, request).await? else { bail!("P2PCLI 回的结果不对") };
            let mut out = String::new();
            for file in &result.files {
                for m in &file.matches {
                    out.push_str(&format!("{}:{}:{}\n", file.path, m.line, m.preview));
                }
            }
            if out.is_empty() {
                out.push_str("没有找到\n");
            }
            if result.truncated {
                out.push_str("…（结果太多或搜索超时，只列出了一部分）\n");
            }
            Ok(out)
        }
        "p2p_mkdir" => {
            let device = s(args, "device")?;
            let path = remote_path(&s(args, "path")?);
            let parents = flag(args, "parents");
            let targets = if parents { ancestors(&path) } else { vec![path.clone()] };
            for (i, target) in targets.iter().enumerate() {
                let last = i + 1 == targets.len();
                if parents && !last {
                    let EditReply::Stats { stats } = edit(dir, device.clone(), EditRequest::Stat { paths: vec![target.clone()] }).await? else { bail!("P2PCLI 回的结果不对") };
                    if stats.first().is_some_and(|s| s.as_ref().is_some_and(|s| s.dir)) {
                        continue;
                    }
                }
                let result = edit(dir, device.clone(), EditRequest::CreateDir { path: target.clone() }).await;
                if let Err(e) = result
                    && !(parents && format!("{e:#}").contains("exists"))
                {
                    return Err(e);
                }
            }
            Ok(format!("已建好 {path}"))
        }
        "p2p_move" => {
            let (from, to) = (remote_path(&s(args, "from")?), remote_path(&s(args, "to")?));
            edit(dir, s(args, "device")?, EditRequest::Rename { from: from.clone(), to: to.clone() }).await?;
            Ok(format!("已移动 {from} → {to}"))
        }
        "p2p_trash" => {
            let path = remote_path(&s(args, "path")?);
            edit(dir, s(args, "device")?, EditRequest::Delete { path: path.clone() }).await?;
            Ok(format!("已移到废纸篓：{path}"))
        }
        "p2p_send_files" => {
            let mut paths = Vec::new();
            for p in list(args, "paths")? {
                let abs = std::path::absolute(&p)?;
                if !abs.exists() {
                    bail!("找不到 {p}");
                }
                paths.push(abs);
            }
            let t = transfer(dir, Request::Send { target: s(args, "device")?, paths, to: opt_s(args, "to") }).await?;
            Ok(format!("已发到 {}，{} 个文件，{}。在对方电脑上的位置：\n{}", t.peer_name, t.files, size(t.total), t.paths.join("\n")))
        }
        "p2p_pull_files" => {
            let to = PathBuf::from(s(args, "to")?);
            if !to.is_absolute() {
                bail!("to 要写本机的绝对路径");
            }
            let t = transfer(dir, Request::Pull { target: s(args, "device")?, paths: list(args, "paths")?, to }).await?;
            Ok(format!("已从 {} 拉到本机，{} 个文件，{}。在本机的位置：\n{}", t.peer_name, t.files, size(t.total), t.paths.join("\n")))
        }
        _ => bail!("没有这个工具：{name}"),
    }
}

async fn exec(dir: &Path, request: Request) -> Result<String> {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (mut done, mut failed) = (None, None);
    control::request(dir, &request, |reply| match reply {
        Reply::ExecOut { stderr, data } => {
            let bytes = control::decode_output(&data);
            if stderr { err.extend_from_slice(&bytes) } else { out.extend_from_slice(&bytes) }
        }
        Reply::ExecDone { code, signal, timed_out } => done = Some((code, signal, timed_out)),
        Reply::Error { message } => failed = Some(message),
        _ => {}
    })
    .await?;
    if let Some(message) = failed {
        bail!(message);
    }
    let (code, signal, timed_out) = done.ok_or_else(|| anyhow!("P2PCLI 没有回结果，命令可能没跑完"))?;
    let status = if timed_out {
        "超时，已被结束".to_string()
    } else if let Some(sig) = signal {
        format!("被信号 {sig} 结束")
    } else {
        format!("退出码 {}", code.unwrap_or(-1))
    };
    let mut text = format!("[{status}]\n");
    if !out.is_empty() {
        text.push_str(&String::from_utf8_lossy(&out));
        if !text.ends_with('\n') {
            text.push('\n');
        }
    }
    if !err.is_empty() {
        text.push_str("[stderr]\n");
        text.push_str(&String::from_utf8_lossy(&err));
    }
    Ok(text)
}

/// 发文件、拉文件：等到结束，失败就报错
async fn transfer(dir: &Path, request: Request) -> Result<Transfer> {
    let (mut done, mut failed) = (None, None);
    control::request(dir, &request, |reply| match reply {
        Reply::Done { transfer } => done = Some(transfer),
        Reply::Error { message } => failed = Some(message),
        _ => {}
    })
    .await?;
    if let Some(message) = failed {
        bail!(message);
    }
    let t = done.ok_or_else(|| anyhow!("P2PCLI 没有回结果"))?;
    if t.state != TransferState::Done {
        bail!(t.error.unwrap_or_else(|| "传输失败".into()));
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_list_is_well_formed() {
        let tools = tools();
        let tools = tools.as_array().unwrap();
        let mut names = std::collections::HashSet::new();
        for t in tools {
            let name = t["name"].as_str().unwrap();
            assert!(names.insert(name), "重名的工具：{name}");
            assert!(name.starts_with("p2p_"));
            assert!(!t["description"].as_str().unwrap().is_empty());
            let schema = &t["inputSchema"];
            assert_eq!(schema["type"], "object");
            for req in schema["required"].as_array().into_iter().flatten() {
                assert!(schema["properties"].get(req.as_str().unwrap()).is_some(), "{name} 的必填参数 {req} 没定义");
            }
        }
        assert!(names.contains("p2p_exec") && names.contains("p2p_edit_file") && names.contains("p2p_pull_files"));
    }

    #[test]
    fn long_results_are_clipped() {
        assert_eq!(clip("短的".into()), "短的");
        let long = "字".repeat(MAX_TEXT + 10);
        let out = clip(long);
        assert!(out.chars().count() < MAX_TEXT + 100);
        assert!(out.contains("截掉"));
    }

    #[tokio::test]
    async fn tools_report_errors_instead_of_failing() {
        let dir = std::env::temp_dir().join(format!("p2pcli-mcp-none-{}", std::process::id()));
        let err = call(&dir, "p2p_devices", &json!({})).await.unwrap_err();
        assert!(format!("{err:#}").contains("没在运行"), "{err:#}");
        let err = call(&dir, "p2p_exec", &json!({ "device": "x" })).await.unwrap_err();
        assert!(format!("{err:#}").contains("command"), "{err:#}");
        assert!(call(&dir, "nope", &json!({})).await.is_err());
    }
}

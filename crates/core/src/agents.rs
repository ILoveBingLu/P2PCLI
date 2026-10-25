//! 认出终端会话里跑的命令行 AI，以及它在干活、等你确认还是空闲。
//!
//! 认是哪个 AI：从会话的 shell 开始顺着子进程往下找，看程序名和完整命令行。只看程序名不够：
//! Claude Code 的可执行文件名是版本号（2.1.310），Gemini CLI、Qwen Code 是 Node 写的，Aider、Kimi 是 Python 写的，
//! 程序名都是 node、python，要看命令行里的名字和脚本路径。
//!
//! 看状态：读会话标题和屏幕最后几十行。屏幕内容只在本机判断状态用，不发给任何人。
//! 规则参考了 tmuxcc（MIT）对各家界面的解析；Claude Code、CodeBuddy 的界面用本机真实截下来的屏幕核对过，见下面的测试。
//!
//! 看额度：只读 AI 自己留在本机的文件，不联网、不碰登录凭据，见 quota()。

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// AI 在干什么
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// 在想、在写、在跑工具
    Working,
    /// 停下来等你：批准操作、选选项
    Waiting,
    /// 干完了或者还没开始，等你输入下一句
    Idle,
}

/// 会话里认出来的 AI
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentInfo {
    /// 稳定的标识，比如 claude、codex；界面按它配图标
    pub kind: String,
    /// 显示的名字，比如 Claude Code
    pub name: String,
    pub state: AgentState,
    /// 这台电脑上这种 AI 账号的额度；读不到就没有
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<AgentQuota>,
}

/// AI 账号的额度：几个滚动窗口各用了多少
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentQuota {
    pub windows: Vec<QuotaWindow>,
    /// 这份数据是什么时候记下的（秒）
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaWindow {
    /// 窗口多长，单位分钟：5 小时是 300，7 天是 10080
    pub minutes: u64,
    /// 用了百分之几，0 到 100
    pub used_percent: f64,
    /// 什么时候重置（秒）
    pub resets_at: Option<u64>,
}

struct Known {
    kind: &'static str,
    name: &'static str,
    /// 程序名或脚本名，去掉 .exe、.js 这些后缀后完全相同才算
    names: &'static [&'static str],
    /// 命令行里出现这些路径片段也算，比如 npm 包名
    markers: &'static [&'static str],
}

/// 认识的命令行 AI。新出一个往这里加一行
const KNOWN: &[Known] = &[
    Known { kind: "claude", name: "Claude Code", names: &["claude"], markers: &["@anthropic-ai/claude-code"] },
    Known { kind: "codex", name: "Codex", names: &["codex"], markers: &["@openai/codex"] },
    Known { kind: "gemini", name: "Gemini CLI", names: &["gemini"], markers: &["@google/gemini-cli"] },
    Known { kind: "opencode", name: "OpenCode", names: &["opencode"], markers: &["opencode-ai"] },
    Known { kind: "cursor", name: "Cursor Agent", names: &["cursor-agent"], markers: &["cursor-agent"] },
    Known { kind: "copilot", name: "Copilot CLI", names: &["copilot"], markers: &["@github/copilot"] },
    Known { kind: "qwen", name: "Qwen Code", names: &["qwen"], markers: &["@qwen-code/qwen-code"] },
    Known { kind: "kimi", name: "Kimi CLI", names: &["kimi", "kimi-cli"], markers: &["kimi-cli"] },
    Known { kind: "codebuddy", name: "CodeBuddy", names: &["codebuddy"], markers: &["@tencent-ai/codebuddy-code"] },
    Known { kind: "qoder", name: "Qoder CLI", names: &["qodercli"], markers: &["@qoder-ai/qodercli"] },
    Known { kind: "iflow", name: "iFlow CLI", names: &["iflow"], markers: &["@iflow-ai/iflow-cli"] },
    Known { kind: "kiro", name: "Kiro CLI", names: &["kiro-cli"], markers: &[] },
    Known { kind: "trae", name: "Trae CLI", names: &["trae-cli", "traecli"], markers: &[] },
    Known { kind: "amp", name: "Amp", names: &["amp"], markers: &["@sourcegraph/amp"] },
    Known { kind: "aider", name: "Aider", names: &["aider"], markers: &["aider-chat"] },
    Known { kind: "crush", name: "Crush", names: &["crush"], markers: &[] },
    Known { kind: "droid", name: "Droid", names: &["droid"], markers: &[] },
    Known { kind: "goose", name: "Goose", names: &["goose"], markers: &[] },
    Known { kind: "auggie", name: "Auggie", names: &["auggie"], markers: &["@augmentcode/auggie"] },
];

/// 进程表刷新一次后几秒内的查询都用它，几个会话、几个查看方一起问时不重复扫
const PROCESS_TTL: Duration = Duration::from_millis(1500);
/// 从 shell 往下最多看这么多个进程，防止异常的进程树拖慢查询
const MAX_VISIT: usize = 256;

/// 本机进程表，用来顺着会话的 shell 找子进程
pub struct Processes {
    sys: System,
    children: HashMap<Pid, Vec<Pid>>,
    at: Option<Instant>,
}

impl Default for Processes {
    fn default() -> Self {
        Self { sys: System::new(), children: HashMap::new(), at: None }
    }
}

impl Processes {
    fn refresh(&mut self) {
        if self.at.is_some_and(|at| at.elapsed() < PROCESS_TTL) {
            return;
        }
        // 命令行和程序路径一个进程只读一次，之后只刷新有哪些进程
        let kind = ProcessRefreshKind::nothing()
            .without_tasks()
            .with_cmd(UpdateKind::OnlyIfNotSet)
            .with_exe(UpdateKind::OnlyIfNotSet);
        self.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
        self.children.clear();
        for (pid, process) in self.sys.processes() {
            if let Some(parent) = process.parent() {
                self.children.entry(parent).or_default().push(*pid);
            }
        }
        self.at = Some(Instant::now());
    }

    /// 从会话的 shell（tmux 的 pane_pid）往下找，离 shell 近的优先，返回第一个认识的 AI 的标识和名字
    pub fn agent_under(&mut self, root: u32) -> Option<(&'static str, &'static str)> {
        self.refresh();
        let mut queue = VecDeque::from([Pid::from_u32(root)]);
        let mut visited = 0;
        while let Some(pid) = queue.pop_front() {
            visited += 1;
            if visited > MAX_VISIT {
                break;
            }
            if let Some(process) = self.sys.process(pid) {
                let cmd: Vec<String> = process.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect();
                if let Some(known) = identify(&process.name().to_string_lossy(), &cmd) {
                    return Some((known.kind, known.name));
                }
            }
            if let Some(kids) = self.children.get(&pid) {
                queue.extend(kids.iter().copied());
            }
        }
        None
    }
}

/// 按程序名和命令行认是不是哪个 AI
fn identify(program: &str, cmd: &[String]) -> Option<&'static Known> {
    let mut words = vec![stem(program)];
    if let Some(first) = cmd.first() {
        words.push(stem(first));
    }
    // Node、Python 这类解释器跑的：真正的名字在后面的脚本路径里
    if words.iter().any(|w| is_runner(w)) {
        words.extend(cmd.iter().skip(1).filter(|a| !a.starts_with('-')).take(2).map(|a| stem(a)));
    }
    let args: Vec<String> = cmd.iter().take(4).map(|a| a.replace('\\', "/").to_lowercase()).collect();
    KNOWN.iter().find(|k| {
        words.iter().any(|w| k.names.contains(&w.as_str()))
            || k.markers.iter().any(|m| args.iter().any(|a| a.contains(m)))
    })
}

/// 路径的最后一段，小写，去掉 .exe、.js 这些后缀
fn stem(path: &str) -> String {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path).to_lowercase();
    for ext in [".exe", ".cmd", ".js", ".mjs", ".cjs", ".ts", ".py"] {
        if let Some(name) = base.strip_suffix(ext) {
            return name.to_string();
        }
    }
    base
}

fn is_runner(word: &str) -> bool {
    matches!(word, "node" | "bun" | "deno" | "npx" | "pnpm" | "uv" | "uvx" | "ruby" | "pypy3") || word.starts_with("python")
}

/// 确认框底下的操作提示，出现在屏幕最后几行就说明在等你选
const CONFIRM_HINTS: &[&str] = &["enter to confirm", "esc to exit", "press enter to confirm", "enter to select", "to navigate"];
/// 正在干活时屏幕上的打断提示（Codex、Gemini 用这个；老版本 Claude Code 也有）
const BUSY_HINTS: &[&str] = &["esc to interrupt", "esc to cancel", "esc interrupt", "ctrl+c to interrupt", "esc to stop"];
/// 选项前面的选中标记
const CURSORS: &[&str] = &["❯", "›", ">", "▶", "▸", "➤", "●", "◉", "→"];
/// Claude Code 干活时那一行开头轮换的符号
const SPINNERS: &[char] = &['✻', '✶', '✳', '✢', '·', '✽', '*', '∗', '◐', '◓', '◑', '◒'];

/// 按会话标题和屏幕内容判断 AI 的状态。
/// 先看是不是在等你，再看是不是在干活：确认框里也有“Esc to cancel”这种字
pub fn state(title: &str, screen: &str) -> AgentState {
    let mut lines: Vec<String> = screen.lines().rev().map(clean).filter(|l| !l.is_empty()).take(18).collect();
    lines.reverse();
    if waiting(&lines) {
        AgentState::Waiting
    } else if working(title, &lines) {
        AgentState::Working
    } else {
        AgentState::Idle
    }
}

/// 去掉首尾空白和框线（CodeBuddy 的确认框整个包在 │ 里）
fn clean(line: &str) -> String {
    line.trim().trim_matches(['│', '┃', '║']).trim().to_string()
}

fn waiting(lines: &[String]) -> bool {
    let tail = &lines[lines.len().saturating_sub(12)..];
    let lower: Vec<String> = tail.iter().map(|l| l.to_lowercase()).collect();
    let bottom = &lower[lower.len().saturating_sub(5)..];
    // 选择框：底下有“Enter to confirm”之类的提示
    if bottom.iter().any(|l| CONFIRM_HINTS.iter().any(|h| l.contains(h))) {
        return true;
    }
    // 编号选项 1. 2. …：带选中标记，或者底下有取消提示
    let options: Vec<(bool, u32)> = tail.iter().filter_map(|l| option(l)).collect();
    let numbered = options.iter().any(|o| o.1 == 1) && options.iter().any(|o| o.1 == 2);
    if numbered && (options.iter().any(|o| o.0) || bottom.iter().any(|l| l.contains("esc to cancel"))) {
        return true;
    }
    // [y/n] 式的问题，或者 OpenCode 的权限请求
    bottom.iter().any(|l| ["[y/n]", "(y/n)", "[yes/no]", "(yes/no)"].iter().any(|p| l.contains(p)))
        || lower.iter().any(|l| l.contains("permission required"))
}

/// 选项行：前面可能有选中标记，然后是“1. ”或“1) ”。返回是否带选中标记和编号
fn option(line: &str) -> Option<(bool, u32)> {
    let mut rest = line.trim_start();
    let mut cursor = false;
    if let Some(after) = CURSORS.iter().find_map(|c| rest.strip_prefix(c)) {
        rest = after.trim_start();
        cursor = true;
    }
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || digits > 2 {
        return None;
    }
    let after = &rest[digits..];
    if !(after.starts_with(". ") || after.starts_with(") ")) {
        return None;
    }
    Some((cursor, rest[..digits].parse().ok()?))
}

fn working(title: &str, lines: &[String]) -> bool {
    // 标题里有盲文转圈符号
    if title.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)) {
        return true;
    }
    let tail = &lines[lines.len().saturating_sub(14)..];
    tail.iter().any(|l| {
        let lower = l.to_lowercase();
        BUSY_HINTS.iter().any(|h| lower.contains(h)) || spinner_line(l)
    })
}

/// “✻ Cogitating…”这种：开头一个转圈符号，第一个词以省略号结尾。干完会变成“✻ Worked for 2s”，没有省略号
fn spinner_line(line: &str) -> bool {
    let mut chars = line.chars();
    let Some(first) = chars.next() else { return false };
    if !(SPINNERS.contains(&first) || ('\u{2801}'..='\u{28ff}').contains(&first)) || chars.next() != Some(' ') {
        return false;
    }
    chars.as_str().split_whitespace().next().is_some_and(|word| word.ends_with('…') || word.ends_with("..."))
}

/// 额度读一次能用这么久，几个会话、几个查看方一起问时不重复读盘
const QUOTA_TTL: Duration = Duration::from_secs(10);
/// 太久以前的额度快照不显示
const QUOTA_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// Codex 的会话日志只看最后这么多字节，额度每轮都会记一次
const CODEX_TAIL_BYTES: u64 = 512 * 1024;

/// 各种 AI 额度的缓存
#[derive(Default)]
pub struct Quotas {
    cache: HashMap<String, (Instant, Option<AgentQuota>)>,
}

impl Quotas {
    pub fn get(&mut self, kind: &str) -> Option<AgentQuota> {
        if let Some((at, quota)) = self.cache.get(kind)
            && at.elapsed() < QUOTA_TTL
        {
            return quota.clone();
        }
        let quota = quota(kind);
        self.cache.insert(kind.to_string(), (Instant::now(), quota.clone()));
        quota
    }
}

/// 这台电脑上某种 AI 账号的额度，只读它们自己留在本机的文件：
/// Claude Code 只把额度交给状态栏脚本，读状态栏脚本存下的快照 ~/.claude/.statusline-ratelimits.json；
/// Codex 读最近一个会话日志里最后一次记下的 rate_limits
pub fn quota(kind: &str) -> Option<AgentQuota> {
    let home = std::env::home_dir()?;
    let now = now_secs();
    let quota = match kind {
        "claude" => claude_quota(&home.join(".claude/.statusline-ratelimits.json")),
        "codex" => {
            let codex_home = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".codex"));
            codex_quota(&codex_home.join("sessions"))
        }
        _ => None,
    }?;
    current(quota, now)
}

/// 去掉已经过了重置时间的窗口（重置以后用了多少不知道），太旧的快照整个不要
fn current(mut quota: AgentQuota, now: u64) -> Option<AgentQuota> {
    if now.saturating_sub(quota.at) > QUOTA_MAX_AGE.as_secs() {
        return None;
    }
    quota.windows.retain(|w| w.resets_at.is_none_or(|at| at > now));
    (!quota.windows.is_empty()).then_some(quota)
}

/// {"rate_limits":{"five_hour":{"used_percentage":34,"resets_at":…},"seven_day":{…}},"at":"…"}；时间用文件的修改时间
fn claude_quota(path: &Path) -> Option<AgentQuota> {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let limits = value.get("rate_limits")?;
    let windows = [("five_hour", 300), ("seven_day", 10080)]
        .into_iter()
        .filter_map(|(key, minutes)| {
            let w = limits.get(key)?;
            Some(QuotaWindow {
                minutes,
                used_percent: w.get("used_percentage")?.as_f64()?,
                resets_at: w.get("resets_at").and_then(|v| v.as_u64()),
            })
        })
        .collect::<Vec<_>>();
    (!windows.is_empty()).then(|| AgentQuota { windows, at: modified_secs(path) })
}

/// 在 sessions/年/月/日/ 里找最新的会话日志，从后往前找最后一次记下的 rate_limits
fn codex_quota(sessions: &Path) -> Option<AgentQuota> {
    let file = newest_rollout(sessions)?;
    let mut f = std::fs::File::open(&file).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(CODEX_TAIL_BYTES))).ok()?;
    let mut text = String::new();
    f.take(CODEX_TAIL_BYTES).read_to_string(&mut text).ok()?;
    text.lines().rev().find_map(|line| {
        if !line.contains("rate_limits") {
            return None;
        }
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        let limits = find_key(&value, "rate_limits")?;
        let windows = ["primary", "secondary"]
            .into_iter()
            .filter_map(|key| {
                let w = limits.get(key)?;
                Some(QuotaWindow {
                    minutes: w.get("window_minutes").and_then(|v| v.as_u64()).unwrap_or(0),
                    used_percent: w.get("used_percent")?.as_f64()?,
                    resets_at: w.get("resets_at").and_then(|v| v.as_u64()),
                })
            })
            .collect::<Vec<_>>();
        (!windows.is_empty()).then(|| AgentQuota { windows, at: modified_secs(&file) })
    })
}

/// 日志外层的包装以后可能变，直接找 rate_limits 这一段（非空的）
fn find_key<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    match value {
        serde_json::Value::Object(map) => map
            .get(key)
            .filter(|v| v.is_object())
            .or_else(|| map.values().find_map(|v| find_key(v, key))),
        _ => None,
    }
}

/// 年、月、日的目录名都是数字，倒着找到第一个有 .jsonl 的日子，取里面最新改过的
fn newest_rollout(sessions: &Path) -> Option<PathBuf> {
    fn dirs(path: &Path) -> Vec<PathBuf> {
        let mut list: Vec<PathBuf> = std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        list.sort();
        list.reverse();
        list
    }
    for year in dirs(sessions) {
        for month in dirs(&year) {
            for day in dirs(&month) {
                let newest = std::fs::read_dir(&day)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
                    .max_by_key(|p| modified_secs(p));
                if newest.is_some() {
                    return newest;
                }
            }
        }
    }
    None
}

fn modified_secs(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn identifies_agents_by_command_line() {
        let kind = |program: &str, args: &[&str]| identify(program, &cmd(args)).map(|k| k.kind);
        // Claude Code 的可执行文件名是版本号，靠命令行第一个参数认
        assert_eq!(kind("2.1.310", &["claude", "--effort", "xhigh"]), Some("claude"));
        // Node 写的：程序名是 node，看脚本路径
        assert_eq!(kind("node", &["node", "/opt/homebrew/bin/gemini"]), Some("gemini"));
        assert_eq!(kind("node", &["node", "/usr/lib/node_modules/@openai/codex/bin/codex.js"]), Some("codex"));
        assert_eq!(kind("node.exe", &[r"C:\Program Files\nodejs\node.exe", r"C:\Users\a\AppData\Roaming\npm\node_modules\@google\gemini-cli\dist\index.js"]), Some("gemini"));
        // 改过进程标题的 Node 程序，命令行第一个参数就是名字
        assert_eq!(kind("node", &["gemini"]), Some("gemini"));
        // Python 写的
        assert_eq!(kind("python3.12", &["/usr/bin/python3.12", "/Users/a/.local/bin/aider", "--model", "x"]), Some("aider"));
        // Windows 的 exe
        assert_eq!(kind("claude.exe", &[r"C:\Users\a\.local\bin\claude.exe"]), Some("claude"));
        assert_eq!(kind("codebuddy", &["codebuddy"]), Some("codebuddy"));
        // 不是 AI 的
        assert_eq!(kind("zsh", &["-zsh"]), None);
        assert_eq!(kind("vim", &["vim", "claude.md"]), None);
        assert_eq!(kind("node", &["node", "server.js"]), None);
    }

    // 下面几块屏幕是本机真实跑 Claude Code 2.1.310 和 CodeBuddy 截下来的
    const CLAUDE_TRUST: &str = "
 Accessing workspace:
 /private/tmp/at/work
 Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open
 source project, or work from your team). If not, take a moment to review what's in this folder first.
 Claude Code'll be able to read, edit, and execute files here.
 Security guide
 ❯ No, exit
   Yes, I trust this folder
 Enter to confirm · Esc to cancel
";
    const CLAUDE_PERMISSION: &str = "
❯ Create a file named hello.txt containing the word hi. Do it with the Write tool, nothing else.
⏺ Write(hello.txt)
──────────────────────────────────────────────────────
 Create file
 hello.txt
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
  1 hi
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 Do you want to create hello.txt?
 ❯ 1. Yes
   2. Yes, and switch to accept edits (auto-approve file edits and common file commands) for this session
      (shift+tab)
   3. No
 Esc to cancel · Tab to amend
";
    const CLAUDE_WORKING: &str = "
❯ Without tools, write a 250-word story about a lighthouse keeper.

✽ Cogitating… (4s · thinking with xhigh effort)

──────────────────────────────────────────────────────
❯
──────────────────────────────────────────────────────
  work · Opus 5.5 · [░░░░░░░░░░░░░░░░] 5%  53k/1M
  ⏸ manual mode on
";
    const CLAUDE_DONE: &str = "
  thirty-nine
  forty
✻ Worked for 2s · done 6:54
──────────────────────────────────────────────────────
❯
──────────────────────────────────────────────────────
  work · Opus 5.5 · [░░░░░░░░░░░░░░░░] 5%  53k/1M
  ⏸ manual mode on
";
    const CLAUDE_IDLE: &str = "
 ▐▛███▛█   Claude Code v2.1.310
▝▜██████▀  Opus 5.5 with xhigh effort · Claude Max
──────────────────────────────────────────────────────
❯ Try \"edit <filepath> to...\"
──────────────────────────────────────────────────────
  work · Opus 5.5 · [░░░░░░░░░░░░░░░░] 0%  0/1M
  ⏵⏵ auto mode on (shift+tab to cycle)
";
    const CODEBUDDY_TRUST: &str = "
│ Learn more ( https://copilot.tencent.com/docs/cli )                                                        │
│                                                                                                            │
│   > 1. Trust folder only (work)                                                                            │
│     2. Trust parent folder (at/**)                                                                         │
│     3. Trust folder and all subdirectories (work/**)                                                       │
│     4. No, exit (escape)                                                                                   │
│                                                                                                            │
│ Enter to confirm • Esc to exit                                                                             │
╰────────────────────────────────────────────────────────────────────────────────────────────────────────────╯
";
    // 下面几块按 Codex、Gemini CLI、OpenCode 的界面写的
    const CODEX_APPROVAL: &str = "
  Would you like to run the following command?
  $ cargo test
› 1. Yes, proceed (y)
  2. Yes, and don't ask again for this command (a)
  3. No, and tell Codex what to do differently (esc)
  Press enter to confirm or esc to cancel
";
    const CODEX_WORKING: &str = "
• Working (12s • esc to interrupt)
▌ Ask Codex to do anything
  ⏎ send   ⌃J newline   ⌃T transcript   ⌃C quit
";
    const GEMINI_APPROVAL: &str = "
 Allow execution of: 'ls'?
 ● 1. Yes, allow once
   2. Yes, allow always ...
   3. No, suggest changes (esc)
";
    const GEMINI_WORKING: &str = "
⠋ Thinking... (esc to cancel, 3s)
> Type your message or @path/to/file
";
    const OPENCODE_PERMISSION: &str = "
 △ Permission required
 → Edit src/main.rs
 Allow once   Allow always   Reject
";
    const CHAT_LIST: &str = "
⏺ 有两种做法：
  1. 改配置
  2. 改代码
  你想用哪种？
──────────────────────────────────────────────────────
❯
──────────────────────────────────────────────────────
";

    #[test]
    fn reads_quotas_from_local_files() {
        let dir = std::env::temp_dir().join(format!("p2pcli-quota-{:x}", rand::random::<u64>()));
        let now = now_secs();
        // Claude Code 状态栏快照，格式照本机真实存下来的
        let claude = dir.join("ratelimits.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&claude, format!(
            r#"{{"rate_limits":{{"five_hour":{{"used_percentage":34,"resets_at":{}}},"seven_day":{{"used_percentage":74,"resets_at":{}}}}},"at":"2026-10-25T14:42:54Z"}}"#,
            now + 3600, now + 86400)).unwrap();
        let q = current(claude_quota(&claude).unwrap(), now).unwrap();
        assert_eq!(q.windows.iter().map(|w| (w.minutes, w.used_percent)).collect::<Vec<_>>(), vec![(300, 34.0), (10080, 74.0)]);
        // 5 小时窗口已经过了重置时间，只剩 7 天的
        std::fs::write(&claude, format!(
            r#"{{"rate_limits":{{"five_hour":{{"used_percentage":95,"resets_at":{}}},"seven_day":{{"used_percentage":74,"resets_at":{}}}}}}}"#,
            now - 60, now + 86400)).unwrap();
        let q = current(claude_quota(&claude).unwrap(), now).unwrap();
        assert_eq!(q.windows.len(), 1);
        assert_eq!(q.windows[0].minutes, 10080);

        // Codex 会话日志：取最新那天最新的文件里最后一次记下的额度
        let old = dir.join("sessions/2026/10/24");
        let new = dir.join("sessions/2026/10/25");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        let line = |used: f64| format!(
            r#"{{"timestamp":"2026-10-25T10:00:00Z","type":"event_msg","payload":{{"type":"token_count","info":null,"rate_limits":{{"limit_id":null,"primary":{{"used_percent":{used},"window_minutes":300,"resets_at":{}}},"secondary":{{"used_percent":12.0,"window_minutes":10080,"resets_at":{}}}}}}}}}"#,
            now + 600, now + 90000);
        std::fs::write(old.join("rollout-a.jsonl"), line(1.0)).unwrap();
        std::fs::write(new.join("rollout-b.jsonl"), format!("{}\n{}\n{{\"type\":\"response_item\"}}\n", line(40.0), line(42.5))).unwrap();
        let q = current(codex_quota(&dir.join("sessions")).unwrap(), now).unwrap();
        assert_eq!(q.windows.iter().map(|w| (w.minutes, w.used_percent)).collect::<Vec<_>>(), vec![(300, 42.5), (10080, 12.0)]);
        assert_eq!(codex_quota(&dir.join("nothing")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_state_from_real_screens() {
        use AgentState::*;
        assert_eq!(state("", CLAUDE_TRUST), Waiting);
        assert_eq!(state("✳ Hello.txt file creation", CLAUDE_PERMISSION), Waiting);
        assert_eq!(state("✳ Hello.txt file creation", CLAUDE_WORKING), Working);
        assert_eq!(state("✳ Hello.txt file creation", CLAUDE_DONE), Idle);
        assert_eq!(state("✳ Claude Code", CLAUDE_IDLE), Idle);
        assert_eq!(state("", CODEBUDDY_TRUST), Waiting);
        assert_eq!(state("", CODEX_APPROVAL), Waiting);
        assert_eq!(state("", CODEX_WORKING), Working);
        assert_eq!(state("", GEMINI_APPROVAL), Waiting);
        assert_eq!(state("", GEMINI_WORKING), Working);
        assert_eq!(state("", OPENCODE_PERMISSION), Waiting);
        // 标题里有盲文转圈符号
        assert_eq!(state("⠙ gemini", "> Type your message"), Working);
        // AI 回复里普通的编号列表不算在等你确认
        assert_eq!(state("", CHAT_LIST), Idle);
        assert_eq!(state("", ""), Idle);
    }
}

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

use std::collections::{HashMap, HashSet, VecDeque};
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
    /// 这个会话里的 AI 现在用的模型、思考强度、权限模式；目前只认 Claude Code
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<AgentSettings>,
}

/// AI 现在的设置，读不到的项留空
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSettings {
    /// 显示用的模型名，比如 Opus 5.5
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 思考强度，用 AI 自己的叫法：low、medium、high、xhigh、max、auto
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// 权限模式，用各家自己的叫法。Claude Code：default、acceptEdits、plan、auto、bypassPermissions、dontAsk；
    /// ZCode：build、edit、yolo、plan
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// 能切到的模型。Claude Code 的别名是固定的，写在界面里；ZCode 按那台电脑上的配置来
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelChoice>,
    /// 当前模型能用的思考强度，同样只有按配置来的才有
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChoice {
    /// 切换时用的名字，比如 bigmodel/GLM-5.3
    pub id: String,
    /// 显示的名字，比如 GLM-5.3
    pub name: String,
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
    Known { kind: "claude", name: "Claude Code", names: &["claude"], markers: &["@anthropic-ai/claude-code", "/claude/versions/"] },
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
    Known { kind: "zcode", name: "ZCode", names: &["zcode"], markers: &["zcode-app-cli"] },
];

/// 安装检测复用识别名单，只探测固定命令名，不运行 AI、不读取登录信息。
pub fn launcher_candidates() -> Vec<(&'static str, &'static str, &'static [&'static str])> {
    KNOWN.iter().map(|a| (a.kind, a.name, a.names)).collect()
}

/// 进程表刷新一次后几秒内的查询都用它，几个会话、几个查看方一起问时不重复扫
const PROCESS_TTL: Duration = Duration::from_millis(1500);
/// 从 shell 往下最多看这么多个进程，防止异常的进程树拖慢查询
const MAX_VISIT: usize = 256;

/// 本机进程表，用来顺着会话的 shell 找子进程
pub struct Processes {
    sys: System,
    children: HashMap<Pid, Vec<Pid>>,
    /// 这一轮重读过命令行的进程
    fresh: HashSet<Pid>,
    at: Option<Instant>,
}

impl Default for Processes {
    fn default() -> Self {
        Self { sys: System::new(), children: HashMap::new(), fresh: HashSet::new(), at: None }
    }
}

impl Processes {
    pub fn idle_shell(&mut self, root: u32, command: &str, fresh: bool) -> bool {
        if fresh { self.at = None; }
        self.refresh();
        let pid = Pid::from_u32(root);
        let Some(process) = self.sys.process(pid) else { return false };
        crate::launchers::is_shell(command)
            && crate::launchers::is_shell(&process.name().to_string_lossy())
            && self.children.get(&pid).is_none_or(|children| children.is_empty())
    }

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
        self.fresh.clear();
        for (pid, process) in self.sys.processes() {
            if let Some(parent) = process.parent() {
                self.children.entry(parent).or_default().push(*pid);
            }
        }
        self.at = Some(Instant::now());
    }

    /// 从会话的 shell（tmux 的 pane_pid）往下找，离 shell 近的优先，返回第一个认识的 AI 的进程号、标识和名字
    pub fn agent_under(&mut self, root: u32) -> Option<(u32, &'static str, &'static str)> {
        self.refresh();
        let mut queue = VecDeque::from([Pid::from_u32(root)]);
        let mut visited = 0;
        while let Some(pid) = queue.pop_front() {
            visited += 1;
            if visited > MAX_VISIT {
                break;
            }
            // shell 用 exec 换成 AI 时进程号不变，缓存里还是原来 shell 的命令行；顺路经过的这几个每轮重读一次
            if self.fresh.insert(pid) {
                let kind = ProcessRefreshKind::nothing().without_tasks().with_cmd(UpdateKind::Always).with_exe(UpdateKind::Always);
                self.sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), false, kind);
            }
            if let Some(process) = self.sys.process(pid) {
                let cmd: Vec<String> = process.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect();
                if let Some(known) = identify(&process.name().to_string_lossy(), &cmd) {
                    return Some((pid.as_u32(), known.kind, known.name));
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
const CONFIRM_HINTS: &[&str] = &["enter to confirm", "esc to exit", "press enter to confirm", "enter to select", "to navigate", "enter confirm", "up/down choose"];
/// 正在干活时屏幕上的打断提示（Codex、Gemini 用这个；老版本 Claude Code 也有）
const BUSY_HINTS: &[&str] = &["esc to interrupt", "esc to cancel", "esc interrupt", "ctrl+c to interrupt", "esc to stop"];
/// 选项前面的选中标记
const CURSORS: &[&str] = &["❯", "›", ">", "▶", "▸", "➤", "●", "◉", "→"];
/// Claude Code 干活时那一行开头轮换的符号
const SPINNERS: &[char] = &['✻', '✶', '✳', '✢', '·', '✽', '*', '∗', '◐', '◓', '◑', '◒'];

/// 按会话标题和屏幕内容判断 AI 的状态。
/// 确认框优先；忙碌和完成提示按屏幕从下往上的顺序判断，避免上一轮状态盖过新一轮。
pub fn state(title: &str, screen: &str) -> AgentState {
    let mut lines: Vec<String> = screen.lines().rev().map(clean).filter(|l| !l.is_empty()).take(18).collect();
    lines.reverse();
    if waiting(&lines) {
        AgentState::Waiting
    } else {
        for (depth, line) in lines.iter().rev().take(14).enumerate() {
            if depth < 8 && completed_line(line) {
                return AgentState::Idle;
            }
            if working_line(line) {
                return AgentState::Working;
            }
        }
        // 没有明确的屏幕状态时才使用标题；完成后标题里的转圈符号可能还没清掉。
        if title.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)) {
            AgentState::Working
        } else {
            AgentState::Idle
        }
    }
}

/// Claude Code、Codex 等结束后会留下 “Worked for …” 一行，ZCode 是“[ ✓ 7s ]”。
/// 只看靠下的完成标记，避免把回复正文里提到的同类文字误认为已经完成。
fn completed_line(line: &str) -> bool {
    if line.starts_with("[ ✓ ") {
        return true;
    }
    let mut chars = line.chars();
    let marked = chars.next().is_some_and(|c| SPINNERS.contains(&c) || matches!(c, '•' | '✦' | '✓'))
        && chars.next() == Some(' ');
    let lower = chars.as_str().to_lowercase();
    marked && ["worked for ", "completed in ", "finished in "].iter().any(|prefix| lower.starts_with(prefix))
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

fn working_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    BUSY_HINTS.iter().any(|h| lower.contains(h)) || spinner_line(line) || timer_line(line)
}

/// ZCode 干活时编辑框上面那行“working… ── [ 🕐 1s ]”“thinking… ── [ 🕔 5s ]”
fn timer_line(line: &str) -> bool {
    line.split_once(" ── [").is_some_and(|(head, _)| !head.contains(' ') && (head.ends_with('…') || head.ends_with("...")))
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
    let text = tail(&file, CODEX_TAIL_BYTES)?;
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

/// 文件最后这么多字节；切在半个字符上的地方换成替换符，切断的第一行解析不了会被跳过
fn tail(path: &Path, bytes: u64) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(bytes))).ok()?;
    let mut buf = Vec::new();
    f.take(bytes).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Claude Code 的对话记录只看最后这么多字节：切模型、切强度和每轮回复都会记一笔
const TRANSCRIPT_TAIL_BYTES: u64 = 256 * 1024;

/// 各会话里 AI 设置的缓存：对话记录没变长、配置文件没改过就不重新读
#[derive(Default)]
pub struct Settings {
    transcripts: HashMap<PathBuf, (u64, Option<String>, Option<String>)>,
    zcode: Option<(u64, ZcodeCatalog)>,
}

impl Settings {
    /// 读进程号是 pid 的 AI 现在的设置。屏幕内容只在这里看，不往外发
    pub fn get(&mut self, kind: &str, pid: u32, screen: &str) -> Option<AgentSettings> {
        let settings = match kind {
            "claude" => self.claude(pid, screen),
            "zcode" => self.zcode(screen),
            _ => return None,
        };
        (settings != AgentSettings::default()).then_some(settings)
    }

    /// ZCode 三样都写在最底下的状态栏里；能切哪些模型、每个模型有哪些强度在它的配置里
    fn zcode(&mut self, screen: &str) -> AgentSettings {
        let mut settings = zcode_footer(screen);
        let Some(path) = std::env::home_dir().map(|home| home.join(".zcode/cli/config.json")) else { return settings };
        let modified = modified_secs(&path);
        if self.zcode.as_ref().is_none_or(|(seen, _)| *seen != modified) {
            // 配置里有各家的 API key，这里只取模型名和强度，别的不往外带
            let catalog = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).map(|c| zcode_catalog(&c)).unwrap_or_default();
            self.zcode = Some((modified, catalog));
        }
        let catalog = &self.zcode.as_ref().unwrap().1;
        settings.models = catalog.models.clone();
        if let Some(model) = &settings.model {
            settings.efforts = catalog.levels.get(&model.to_lowercase()).cloned().unwrap_or_default();
        }
        settings
    }

    fn claude(&mut self, pid: u32, screen: &str) -> AgentSettings {
        // 模型和强度以对话记录为准，记录里没有（刚开的会话、记录关了）再看屏幕
        let config = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| Some(std::env::home_dir()?.join(".claude")));
        let (model, effort) = config
            .and_then(|config| claude_transcript(&config, pid))
            .map(|path| self.transcript(path))
            .unwrap_or_default();
        AgentSettings {
            model: model.or_else(|| claude_screen_model(screen)),
            effort: effort.or_else(|| claude_screen_effort(screen)),
            mode: claude_mode(screen).map(str::to_string),
            ..Default::default()
        }
    }

    fn transcript(&mut self, path: PathBuf) -> (Option<String>, Option<String>) {
        let len = std::fs::metadata(&path).map_or(0, |m| m.len());
        if let Some((seen, model, effort)) = self.transcripts.get(&path)
            && *seen == len
        {
            return (model.clone(), effort.clone());
        }
        let (model, effort) = tail(&path, TRANSCRIPT_TAIL_BYTES).map(|t| claude_transcript_settings(&t)).unwrap_or_default();
        self.transcripts.insert(path, (len, model.clone(), effort.clone()));
        (model, effort)
    }
}

/// Claude Code 每个进程在 sessions/进程号.json 里记着会话号和目录，对话记录在 projects/目录改写/会话号.jsonl
fn claude_transcript(config: &Path, pid: u32) -> Option<PathBuf> {
    let info: serde_json::Value = serde_json::from_slice(&std::fs::read(config.join(format!("sessions/{pid}.json"))).ok()?).ok()?;
    let id = info.get("sessionId")?.as_str()?;
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let file = format!("{id}.jsonl");
    let projects = config.join("projects");
    // 目录名是把路径里字母数字以外的字符都换成 -；很长的路径会被截短，对不上就挨个目录找
    if let Some(cwd) = info.get("cwd").and_then(|v| v.as_str()) {
        let slug: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let path = projects.join(slug).join(&file);
        if path.is_file() {
            return Some(path);
        }
    }
    std::fs::read_dir(&projects).ok()?.flatten().map(|e| e.path().join(&file)).find(|p| p.is_file())
}

/// 从对话记录末尾往前找最近一次的模型和强度：/model、/effort 命令的输出，或者回复上记的模型和强度
fn claude_transcript_settings(text: &str) -> (Option<String>, Option<String>) {
    let (mut model, mut effort) = (None, None);
    for line in text.lines().rev() {
        if model.is_some() && effort.is_some() {
            break;
        }
        // 先粗筛再解析，大部分行是工具调用和结果；子代理用的模型不算
        if line.contains("\"isSidechain\":true") || !(line.contains("<local-command-stdout>") || line.contains("\"type\":\"assistant\"")) {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        match value.get("type").and_then(|v| v.as_str()) {
            // 命令的输出是一条用户记录，内容整个包在 <local-command-stdout> 里；工具结果里提到这几个字不算
            Some("user") => {
                let Some(text) = value.pointer("/message/content").and_then(|v| v.as_str()) else { continue };
                let Some(text) = text.strip_prefix("<local-command-stdout>") else { continue };
                if model.is_none() {
                    model = after(text, &["Set model to ", "Kept model as "]).map(|m| m.trim_matches('`').to_string());
                }
                if effort.is_none() {
                    effort = after(text, &["Set effort level to ", "Effort level set to "]).map(str::to_string);
                }
            }
            Some("assistant") => {
                if model.is_none() {
                    model = value
                        .pointer("/message/model")
                        .and_then(|v| v.as_str())
                        .filter(|m| !m.starts_with('<'))
                        .map(model_name);
                }
                if effort.is_none() {
                    effort = value.get("effort").and_then(|v| v.as_str()).map(str::to_string);
                }
            }
            _ => {}
        }
    }
    (model, effort.filter(|e| is_effort(e)))
}

/// 命令输出里某个前缀后面的值：模型名到“ and saved”或行尾，强度是一个词
fn after<'a>(text: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    let rest = prefixes.iter().find_map(|p| text.split_once(p).map(|(_, r)| r))?;
    let end = [" and saved", " (", ":", "<", "\""].iter().filter_map(|s| rest.find(s)).min().unwrap_or(rest.len());
    let value = rest[..end].trim();
    (!value.is_empty()).then_some(value)
}

fn is_effort(word: &str) -> bool {
    matches!(word, "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "auto")
}

/// 模型 ID 换成显示的名字：claude-opus-5-5 → Opus 5.5，claude-sonnet-4-5-20250929 → Sonnet 4.5；
/// 不是 Claude 的名字（中转服务自己起的）原样显示
fn model_name(id: &str) -> String {
    let (base, long) = match id.strip_suffix("[1m]") {
        Some(base) => (base, " 1M"),
        None => (id, ""),
    };
    let Some(rest) = base.strip_prefix("claude-") else { return id.to_string() };
    let mut family = Vec::new();
    let mut version = Vec::new();
    for part in rest.split('-') {
        if part.len() == 8 && part.chars().all(|c| c.is_ascii_digit()) {
            continue; // 发布日期
        }
        if part.chars().all(|c| c.is_ascii_digit()) {
            version.push(part);
        } else {
            let mut chars = part.chars();
            family.extend(chars.next().map(|c| c.to_ascii_uppercase().to_string() + chars.as_str()));
        }
    }
    if family.is_empty() {
        return id.to_string();
    }
    let version = if version.is_empty() { String::new() } else { format!(" {}", version.join(".")) };
    format!("{}{version}{long}", family.join(" "))
}

/// 屏幕上最近一次切模型的结果，或者开头横幅 logo 右边那行：订阅是“Opus 5.5 with xhigh effort · Claude Max”，
/// 用 API key 是“Opus 5.5 · API Usage Billing”
fn claude_screen_model(screen: &str) -> Option<String> {
    screen.lines().rev().find_map(|line| {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('⎿') {
            return after(rest, &["Set model to ", "Kept model as "]).map(str::to_string);
        }
        if !(line.starts_with('▝') && line.contains('█')) {
            return None;
        }
        let text = line.trim_start_matches(|c: char| !c.is_alphanumeric());
        let end = [" with ", " · "].iter().filter_map(|s| text.find(s)).min().unwrap_or(text.len());
        let model = text[..end].trim();
        (!model.is_empty()).then(|| model.to_string())
    })
}

/// 屏幕上的强度：切强度的结果、输入框上方的“● high · /effort”、干活时的“thinking with xhigh effort”、开头横幅
fn claude_screen_effort(screen: &str) -> Option<String> {
    screen.lines().rev().find_map(|line| {
        let line = line.trim();
        let word = if let Some(rest) = line.strip_prefix('⎿') {
            after(rest, &["Set effort level to ", "Effort level set to "])?
        } else if let Some(rest) = line.strip_suffix("· /effort") {
            rest.split_whitespace().last()?
        } else {
            line.split_once(" with ")?.1.split_once(" effort")?.0
        };
        is_effort(word).then(|| word.to_string())
    })
}

/// ZCode 配置里的模型：键是“提供方/模型”，强度在 reasoning.levels 里，按从低到高排
#[derive(Debug, Default, Clone, PartialEq)]
struct ZcodeCatalog {
    models: Vec<ModelChoice>,
    /// 小写的模型 ID → 能用的强度
    levels: HashMap<String, Vec<String>>,
}

fn zcode_catalog(config: &serde_json::Value) -> ZcodeCatalog {
    const ORDER: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "max"];
    let mut catalog = ZcodeCatalog::default();
    let providers = config.get("provider").and_then(|v| v.as_object()).into_iter().flatten();
    for (provider, p) in providers {
        for (model, m) in p.get("models").and_then(|v| v.as_object()).into_iter().flatten() {
            let id = format!("{provider}/{model}");
            let name = m.get("name").and_then(|v| v.as_str()).unwrap_or(model).to_string();
            let reasoning = m.get("reasoning");
            if reasoning.and_then(|r| r.get("enabled")).and_then(|v| v.as_bool()) != Some(false) {
                let mut levels: Vec<String> = reasoning
                    .and_then(|r| r.get("levels"))
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
                levels.sort_by_key(|l| ORDER.iter().position(|o| o == l).unwrap_or(ORDER.len()));
                if !levels.is_empty() {
                    catalog.levels.insert(id.to_lowercase(), levels);
                }
            }
            catalog.models.push(ModelChoice { id, name });
        }
    }
    catalog
}

/// ZCode 最底下的状态栏：“◈ bigmodel/GLM-5.3-Flash ─ ◉ build ─ ⚡ high ─ ctx 99% left”；
/// 窗口窄的时候后面几项会被挤掉，等你确认时整行被确认框盖住
fn zcode_footer(screen: &str) -> AgentSettings {
    let mut settings = AgentSettings::default();
    let Some(line) = screen.lines().rev().map(str::trim).filter(|l| !l.is_empty()).take(3).find(|l| l.starts_with("◈ ")) else {
        return settings;
    };
    for field in line.split(" ─ ").map(str::trim) {
        let value = |prefix: &str| field.strip_prefix(prefix).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        settings.model = settings.model.take().or_else(|| value("◈ "));
        settings.mode = settings.mode.take().or_else(|| value("◉ "));
        settings.effort = settings.effort.take().or_else(|| value("⚡ "));
    }
    settings
}

/// 屏幕最底下的权限模式提示：“⏸ manual mode on”“⏵⏵ auto mode on (shift+tab to cycle)”；
/// 老版本默认模式什么都不写，只有“? for shortcuts”。等你确认时底下换成了确认框，读不到
fn claude_mode(screen: &str) -> Option<&'static str> {
    const MODES: &[(&str, &str)] = &[
        ("manual mode on", "default"),
        ("accept edits on", "acceptEdits"),
        ("plan mode on", "plan"),
        ("auto mode on", "auto"),
        ("bypass permissions on", "bypassPermissions"),
        ("don't ask on", "dontAsk"),
    ];
    let bottom: Vec<&str> = screen.lines().rev().map(str::trim).filter(|l| !l.is_empty()).take(3).collect();
    for line in &bottom {
        if line.starts_with(['⏵', '⏸']) {
            let lower = line.to_lowercase();
            if let Some((_, mode)) = MODES.iter().find(|(text, _)| lower.contains(text)) {
                return Some(mode);
            }
        }
    }
    bottom.iter().any(|l| l.starts_with("? for shortcuts")).then_some("default")
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
        // 直接按安装目录里的版本文件起的，命令行第一个参数也是版本号，看路径
        assert_eq!(kind("2.1.310", &["/Users/me/.local/share/claude/versions/2.1.310", "--bare"]), Some("claude"));
        assert_eq!(kind("2.1.310.exe", &[r"C:\Users\a\.local\share\claude\versions\2.1.310.exe"]), Some("claude"));
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
        // ZCode 的终端客户端是 Node 写的，npm 包叫 zcode-app-cli
        assert_eq!(kind("node", &["node", "/opt/homebrew/bin/zcode"]), Some("zcode"));
        assert_eq!(kind("node.exe", &[r"C:\Program Files\nodejs\node.exe", r"C:\Users\a\AppData\Roaming\npm\node_modules\zcode-app-cli\bin\zcode.js"]), Some("zcode"));
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
        assert_eq!(state("ZC | ⠇ working…", ZCODE_WORKING), Working);
        assert_eq!(state("", ZCODE_WORKING), Working);
        assert_eq!(state("", ZCODE_DONE), Idle);
        // 标题里的转圈晚一步清掉也以屏幕上的完成标记为准
        assert_eq!(state("ZC | ⠇ thinking…", ZCODE_DONE), Idle);
        assert_eq!(state("ZC | ⠧ running Write…", ZCODE_PERMISSION), Waiting);
        // 标题里有盲文转圈符号
        assert_eq!(state("⠙ gemini", "> Type your message"), Working);
        // AI 回复里普通的编号列表不算在等你确认
        assert_eq!(state("", CHAT_LIST), Idle);
        assert_eq!(state("", ""), Idle);
    }

    #[test]
    fn completion_overrides_stale_busy_evidence() {
        use AgentState::*;
        // 完成行上方还留着忙碌提示时，不能继续把历史文本当成当前状态。
        let with_history = format!("✽ Cogitating… (4s · thinking)\n{CLAUDE_DONE}");
        assert_eq!(state("", &with_history), Idle);
        // 某些 CLI 的 pane 标题会晚一步清掉 spinner，也不能盖过明确的完成行。
        assert_eq!(state("⠙ claude", CLAUDE_DONE), Idle);
        assert_eq!(state("", "• Worked for 3s\n› Ask Codex to do anything"), Idle);
        // 没有完成标记时，标题 spinner 仍然是有效的忙碌信号。
        assert_eq!(state("⠙ gemini", "> Type your message"), Working);
    }

    #[test]
    fn latest_screen_state_wins_across_turns() {
        use AgentState::*;
        // 上一轮完成行还在，但新一轮已经开始；不能误报空闲或发送完成通知。
        assert_eq!(state("", "✻ Worked for 2s\n❯ Continue\n✽ Cogitating… (1s)\n❯"), Working);
        assert_eq!(state("", "• Worked for 3s\n› Continue\nWorking (esc to interrupt)"), Working);
        // 新一轮也完成后，以最下面的完成标记为准。
        assert_eq!(state("⠙ claude", "✻ Worked for 2s\n✽ Cogitating… (1s)\n✓ Completed in 4s\n❯"), Idle);
        // 确认框仍然优先，里面的取消提示不是忙碌信号。
        assert_eq!(state("", "✻ Worked for 2s\n❯ 1. Yes\n2. No\nEnter to confirm · Esc to cancel"), Waiting);
        // 正文中引用完成短语不能覆盖有效的标题忙碌信号。
        assert_eq!(state("⠙ claude", "• The message says worked for 3s"), Working);
    }

    // Claude Code 2.1.310 里打 /effort high、/model sonnet 之后真实截下来的屏幕（目录换掉了）
    const CLAUDE_SWITCHED: &str = "
 ▐▛███▛█   Claude Code v2.1.310
▝▜██████▀  Opus 5.5 with high effort · Claude Max
 ▝▝   ▝▝   ~/work
❯ /effort high
  ⎿  Set effort level to high (saved as your default for new sessions): Comprehensive implementation with extensive
     testing and documentation
❯ /model sonnet
  ⎿  Set model to Sonnet 5.5 and saved as your default for new sessions
                                                                                                      ● high · /effort
──────────────────────────────────────────────────────
❯
──────────────────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle)
";

    // 用 API key 刚开的 Claude Code 2.1.310，真实截屏（目录换掉了）
    const CLAUDE_API_KEY: &str = "
 ▐▛███▛█   Claude Code v2.1.310
▝▜██████▀  Opus 5.5 · API Usage Billing
 ▝▝   ▝▝   ~/work
──────────────────────────────────────────────────────
❯ Try \"fix lint errors\"
──────────────────────────────────────────────────────
  ⏸ manual mode on · ? for shortcuts · ← for agents                                                 ◐ medium · /effort
";

    #[test]
    fn reads_claude_settings_from_real_screens() {
        let read = |screen: &str| (claude_screen_model(screen), claude_screen_effort(screen), claude_mode(screen));
        let s = |v: &str| Some(v.to_string());
        // 最近一次切换的结果比开头横幅新
        assert_eq!(read(CLAUDE_SWITCHED), (s("Sonnet 5.5"), s("high"), Some("auto")));
        assert_eq!(read(CLAUDE_IDLE), (s("Opus 5.5"), s("xhigh"), Some("auto")));
        // 用 API key 登录时横幅不写强度，强度在底下那行右边
        assert_eq!(read(CLAUDE_API_KEY), (s("Opus 5.5"), s("medium"), Some("default")));
        // 干活时转圈那行写着强度；底下是他自己配的状态栏，不认
        assert_eq!(read(CLAUDE_WORKING), (None, s("xhigh"), Some("default")));
        // 等你确认时底下是确认框，读不到权限模式
        assert_eq!(read(CLAUDE_PERMISSION).2, None);
        // Shift+Tab 依次切出来的几种（真实截屏的最后一行）
        let mode = |footer: &str| claude_mode(&format!("❯\n──────\n  {footer}"));
        assert_eq!(mode("⏸ manual mode on · ? for shortcuts"), Some("default"));
        assert_eq!(mode("⏵⏵ accept edits on (shift+tab to cycle)"), Some("acceptEdits"));
        assert_eq!(mode("⏸ plan mode on (shift+tab to cycle)"), Some("plan"));
        assert_eq!(mode("⏵⏵ auto mode on (shift+tab to cycle) · ← for agents"), Some("auto"));
        assert_eq!(mode("⏵⏵ bypass permissions on (shift+tab to cycle)"), Some("bypassPermissions"));
        // 老版本默认模式不写，只有快捷键提示
        assert_eq!(mode("? for shortcuts"), Some("default"));
        // 正文里聊到这几个字不算
        assert_eq!(claude_mode("我们先开 plan mode on 试试\n❯\n──────\n  work · Opus 5.5"), None);
    }

    #[test]
    fn reads_claude_settings_from_transcript() {
        // 每行的结构照 Claude Code 2.1.310 真实记下的，会话号、目录这些换掉了
        let reply = |model: &str, effort: &str, side: bool| format!(
            r#"{{"parentUuid":"a","isSidechain":{side},"message":{{"model":"{model}","type":"message","role":"assistant","content":[{{"type":"text","text":"好"}}]}},"type":"assistant","effort":"{effort}","perTurnEffort":"{effort}","uuid":"b"}}"#);
        let command = |out: &str| format!(
            r#"{{"parentUuid":"a","isSidechain":false,"type":"user","message":{{"role":"user","content":"<local-command-stdout>{out}</local-command-stdout>"}},"uuid":"c"}}"#);
        let s = |v: &str| Some(v.to_string());

        assert_eq!(claude_transcript_settings(&reply("claude-opus-5-5", "xhigh", false)), (s("Opus 5.5"), s("xhigh")));
        // 回复之后又切了强度和模型：以最后一次为准；最前面切断的半行跳过
        let text = [
            "ted\":\"x\"}".to_string(),
            reply("claude-opus-5-5", "xhigh", false),
            command("Set effort level to high (saved as your default for new sessions): Comprehensive implementation with extensive testing and documentation"),
            command("Set model to `Sonnet 5.5` and saved as your default for new sessions"),
        ].join("\n");
        assert_eq!(claude_transcript_settings(&text), (s("Sonnet 5.5"), s("high")));
        // 子代理的回复、工具结果里出现的同样字样都不算
        let tool = r#"{"isSidechain":false,"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"<local-command-stdout>Set model to `Haiku 5.5`</local-command-stdout>"}]},"note":"\"type\":\"assistant\""}"#;
        let text = [reply("claude-opus-5-5", "xhigh", false), reply("claude-haiku-5-5", "low", true), tool.to_string()].join("\n");
        assert_eq!(claude_transcript_settings(&text), (s("Opus 5.5"), s("xhigh")));
        assert_eq!(claude_transcript_settings(&command("Effort level set to auto")), (None, s("auto")));
        assert_eq!(claude_transcript_settings(""), (None, None));

        assert_eq!(model_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(model_name("claude-sonnet-4-5-20250929"), "Sonnet 4.5");
        assert_eq!(model_name("claude-3-5-haiku-20241022"), "Haiku 3.5");
        assert_eq!(model_name("claude-fable-5-1[1m]"), "Fable 5.1 1M");
        assert_eq!(model_name("glm-4.6"), "glm-4.6");
    }

    #[test]
    fn finds_claude_transcript_by_process() {
        let dir = std::env::temp_dir().join(format!("p2pcli-claude-{:x}", rand::random::<u64>()));
        let id = "0b6f3c1e-5a7d-4e2b-9c41-8d2f6a1b3e57";
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        std::fs::create_dir_all(dir.join("projects/-Users-me-My-Work")).unwrap();
        std::fs::write(dir.join("sessions/4242.json"), format!(r#"{{"pid":4242,"sessionId":"{id}","cwd":"/Users/me/My Work","status":"idle"}}"#)).unwrap();
        // 还没有对话记录（刚开的会话）
        assert_eq!(claude_transcript(&dir, 4242), None);
        let path = dir.join(format!("projects/-Users-me-My-Work/{id}.jsonl"));
        std::fs::write(&path, "").unwrap();
        assert_eq!(claude_transcript(&dir, 4242), Some(path.clone()));
        // 目录名对不上（很长的路径会被截短）就挨个目录找
        std::fs::create_dir_all(dir.join("projects/-Users-me-My-W-1a2b")).unwrap();
        let moved = dir.join(format!("projects/-Users-me-My-W-1a2b/{id}.jsonl"));
        std::fs::rename(&path, &moved).unwrap();
        assert_eq!(claude_transcript(&dir, 4242), Some(moved));
        assert_eq!(claude_transcript(&dir, 999), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // ZCode（zcode-app-cli 3.11.2）真实截下来的屏幕，目录换掉了
    const ZCODE_WORKING: &str = "
╰─ /help commands · /status details ───────────────────────────────────────╯
 › 用一句话介绍你自己，不要用任何工具
 ◇ Thinking · active
  The user is asking me to introduce myself in one sentence, in Chinese, and
 thinking… ── [ 🕔 5s ]
──────────────────────────────────────────────────────
──────────────────────────────────────────────────────
 ◈ bigmodel/GLM-5.3-Flash ─ ◉ plan ─ ⚡ high ─ ctx 100% left ─ session 0 tokens
";
    const ZCODE_DONE: &str = "
 › 用一句话介绍你自己，不要用任何工具
 ◇ Thought · Ctrl+O to expand
 我是 ZCode，一个由 GLM（Z.ai 训练）驱动的交互式编程助手，能帮你阅读代码、编写实现、调试问题并执行各类软件开发任务。
 [ ✓ 7s ]
──────────────────────────────────────────────────────
──────────────────────────────────────────────────────
 ◈ bigmodel/GLM-5.3-Flash ─ ◉ plan ─ ⚡ high ─ ctx 99% left ─ cache 50% hit ─ session 13.8K tokens
";
    const ZCODE_PERMISSION: &str = "
 ◆ Write ~/work/hi.txt · waiting
 for permission
  hi
 Activity · 1 active tool
  ○ Write
Permission · Write
Tool has side effects and requires approval
 Risk: medium
 ● Write ~/work/hi.txt +1 -0
       │ @@ -0,0 +1,1 @@
     1 │+ hi
Filter: type to search
→ 1. Allow once                   Approve only this request
  2. Always allow in this projec  Allow matching Write requests
  3. Deny and tell ZCode why      Return feedback so the agent can adjust
  4. Deny
Type to filter · number selects · Up/Down choose · Ctrl+O details · ←/→ or PgUp/PgDn scroll · Enter confirm · Esc cancel
";

    #[test]
    fn reads_zcode_settings() {
        let s = |v: &str| Some(v.to_string());
        let footer = zcode_footer(ZCODE_DONE);
        assert_eq!((footer.model, footer.mode, footer.effort), (s("bigmodel/GLM-5.3-Flash"), s("plan"), s("high")));
        // 刚开、还没配模型时
        let footer = zcode_footer("──────\n──────\n ◈ default ─ ◉ build ─ ctx 100% left");
        assert_eq!((footer.model, footer.mode, footer.effort), (s("default"), s("build"), None));
        // 确认框盖住了状态栏
        assert_eq!(zcode_footer(ZCODE_PERMISSION), AgentSettings::default());

        // 配置的结构照真实的 ~/.zcode/cli/config.json，key 是假的；只取模型名和强度
        let config: serde_json::Value = serde_json::from_str(r#"{
            "model": {"main": "bigmodel/GLM-5.3-Flash", "lite": "bigmodel/GLM-5.3"},
            "provider": {
                "bigmodel": {"kind": "anthropic", "name": "BigModel Coding Plan",
                    "options": {"apiKeyRequired": true, "baseURL": "https://open.bigmodel.cn/api/anthropic", "apiKey": "not-a-real-key"},
                    "models": {
                        "GLM-5.3": {"name": "GLM-5.3", "reasoning": {"enabled": true, "levels": ["low", "max", "high"], "defaultLevel": "max"}},
                        "glm-5-turbo": {"name": "GLM-5-Turbo", "reasoning": {"enabled": false, "levels": ["low"]}}
                    }},
                "zai": {"models": {"glm-5.2": {"name": "GLM-5.2"}}}
            }}"#).unwrap();
        let catalog = zcode_catalog(&config);
        let ids: Vec<(&str, &str)> = catalog.models.iter().map(|m| (m.id.as_str(), m.name.as_str())).collect();
        assert_eq!(ids, [("bigmodel/GLM-5.3", "GLM-5.3"), ("bigmodel/glm-5-turbo", "GLM-5-Turbo"), ("zai/glm-5.2", "GLM-5.2")]);
        assert_eq!(catalog.levels.get("bigmodel/glm-5.3"), Some(&vec!["low".to_string(), "high".to_string(), "max".to_string()]));
        // 关掉了推理的、没写强度的都没有
        assert_eq!(catalog.levels.len(), 1);
        assert!(!format!("{catalog:?}").contains("not-a-real-key"), "配置里的 key 不能带出来");
    }
}

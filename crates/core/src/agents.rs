//! 认出终端会话里跑的命令行 AI，以及它在干活、等你确认还是空闲。
//!
//! 认是哪个 AI：从会话的 shell 开始顺着子进程往下找，看程序名和完整命令行。只看程序名不够：
//! Claude Code 的可执行文件名是版本号（2.1.310），Gemini CLI、Qwen Code 是 Node 写的，Aider、Kimi 是 Python 写的，
//! 程序名都是 node、python，要看命令行里的名字和脚本路径。
//!
//! 看状态：读会话标题和屏幕最后几十行。屏幕内容只在本机判断状态用，不发给任何人。
//! 规则参考了 tmuxcc（MIT）对各家界面的解析；Claude Code、CodeBuddy 的界面用本机真实截下来的屏幕核对过，见下面的测试。

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

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

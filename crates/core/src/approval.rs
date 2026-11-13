//! Conservative adapters for visible, single-choice AI prompts. Never invent choices.
//!
//! 同一个确认框要有稳定的 id：连接方拿着它来点选项，被控端重新抓屏核对。终端改尺寸会让 AI 重新排版
//! （折行位置变、长路径被截断），所以 id 只看稳定的部分：选项个数、每个选项开头一段、紧挨着选项的问题末尾一段，
//! 都去掉空白再算。
use std::collections::HashMap;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Approval {
    pub id: String,
    pub context: String,
    pub options: Vec<Choice>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub id: usize,
    pub label: String,
}

#[derive(Debug)]
pub struct Parsed {
    pub context: String,
    pub labels: Vec<String>,
    pub selected: usize,
    pub horizontal: bool,
    signature: String,
}

impl Parsed {
    pub fn keys(&self, choice: usize) -> Option<Vec<&'static str>> {
        if choice >= self.labels.len() { return None; }
        let key = match (self.horizontal, choice < self.selected) {
            (true, true) => "Left", (true, false) => "Right",
            (false, true) => "Up", (false, false) => "Down",
        };
        let mut keys = vec![key; choice.abs_diff(self.selected)];
        keys.push("Enter");
        Some(keys)
    }
}

/// 点过的确认框过了这么久还在，就当成新的一轮重新给出来：可能按键没生效，也可能 AI 又问了一个一样的问题
const CONSUMED_HOLD: Duration = Duration::from_secs(4);

struct Entry { signature: String, id: String, consumed: Option<Instant> }

#[derive(Default)]
pub struct Tracker { entries: HashMap<String, Entry> }

impl Tracker {
    pub fn observe(&mut self, session: &str, parsed: Option<&Parsed>) -> Option<Approval> {
        self.observe_at(session, parsed, Instant::now())
    }

    fn observe_at(&mut self, session: &str, parsed: Option<&Parsed>, now: Instant) -> Option<Approval> {
        let Some(parsed) = parsed else { self.entries.remove(session); return None; };
        let fresh = || Entry { signature: parsed.signature.clone(), id: format!("{:032x}", rand::random::<u128>()), consumed: None };
        let entry = self.entries.entry(session.into()).or_insert_with(fresh);
        let expired = entry.consumed.is_some_and(|at| now.duration_since(at) >= CONSUMED_HOLD);
        if entry.signature != parsed.signature || expired {
            *entry = fresh();
        }
        if entry.consumed.is_some() { return None; }
        Some(Approval {
            id: entry.id.clone(), context: parsed.context.clone(),
            options: parsed.labels.iter().enumerate().map(|(id, label)| Choice { id, label: label.clone() }).collect(),
        })
    }

    pub fn consume(&mut self, session: &str, id: &str) -> bool {
        self.consume_at(session, id, Instant::now())
    }

    fn consume_at(&mut self, session: &str, id: &str, now: Instant) -> bool {
        let Some(entry) = self.entries.get_mut(session) else { return false; };
        if entry.id != id || entry.consumed.is_some() { return false; }
        entry.consumed = Some(now);
        true
    }
}

fn clean(line: &str) -> &str { line.trim().trim_matches(['│', '┃', '║']).trim() }
fn footer(line: &str) -> bool {
    let lower = line.to_lowercase();
    ["enter to confirm", "enter to select", "esc to cancel", "enter confirm", "esc cancel", "to navigate"]
        .iter().any(|hint| lower.contains(hint))
}

fn numbered(line: &str) -> Option<(bool, usize, &str)> {
    let mut text = clean(line);
    let selected = ['❯', '›', '>', '▶', '▸', '●', '◉'].iter().any(|c| text.starts_with(*c));
    if selected || text.starts_with('○') { text = text.chars().next().map(|c| &text[c.len_utf8()..])?.trim_start(); }
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    if digits != 1 { return None; }
    let rest = text.get(digits..)?;
    let label = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))?.trim();
    if label.is_empty() || label.len() > 700 { return None; }
    Some((selected, text[..digits].parse().ok()?, label))
}

fn vertical(lines: &[&str]) -> Option<Parsed> {
    let indexed: Vec<_> = lines.iter().enumerate().filter_map(|(row, line)| numbered(line).map(|option| (row, option))).collect();
    // Only the bottom menu, beginning at 1, is eligible. No truncated/paged lists.
    let start = indexed.iter().rposition(|(_, (_, number, _))| *number == 1)?;
    let rows = &indexed[start..];
    if !(2..=9).contains(&rows.len()) || rows[0].0 + 30 < lines.len() { return None; }
    if rows.iter().enumerate().any(|(i, (_, (_, n, _)))| *n != i + 1) { return None; }
    let selected: Vec<_> = rows.iter().enumerate().filter(|(_, (_, (selected, _, _)))| *selected).map(|(i, _)| i).collect();
    if selected.len() != 1 { return None; }
    let last = rows.last()?.0;
    let tail = &lines[last + 1..];
    if tail.len() > 10 || tail.iter().any(|line| matches!(*line, "❯" | "›" | "$" | ">")) { return None; }
    let before = &lines[..rows[0].0];
    let header = before.iter().rev().take(8).copied().collect::<Vec<_>>().join("\n").to_lowercase();
    // 底下没有“回车确认”这类提示时，上面得明明白白是在问你（Gemini、Qwen 这类有时不写提示）
    let asking = ["allow execution", "allow ", "apply this", "do you want", "approve", "permission", "proceed?", "would you like"].iter().any(|hint| header.contains(hint));
    if !tail.iter().any(|line| footer(line)) && !asking { return None; }
    let mut labels = Vec::new();
    for (i, (row, (_, _, label))) in rows.iter().enumerate() {
        let end = rows.get(i + 1).map(|(row, _)| *row)
            .unwrap_or_else(|| (last + 1..lines.len()).find(|&row| footer(lines[row])).unwrap_or(lines.len()));
        let continuation = &lines[row + 1..end];
        if continuation.iter().any(|line| footer(line) || numbered(line).is_some()) { return None; }
        let full = std::iter::once(*label).chain(continuation.iter().copied()).collect::<Vec<_>>().join(" ");
        if full.len() > 900 { return None; }
        labels.push(full);
    }
    Some(Parsed { context: context(before)?, labels, selected: selected[0], horizontal: false, signature: String::new() })
}

/// 没有编号的单选框，比如 Claude Code 问“信任这个文件夹吗”：
///  ❯ No, exit
///    Yes, I trust this folder
///
///  Enter to confirm · Esc to cancel
/// 只认紧挨着底部提示、对齐整齐、正好一个光标的几行；碰到右边界的行可能是折行，认不准就不认
fn unnumbered(raw: &[String]) -> Option<Parsed> {
    let footer_row = raw.iter().rposition(|line| footer(line))?;
    if raw[footer_row + 1..].iter().filter(|line| !line.trim().is_empty()).count() > 10 { return None; }
    let width = raw.iter().map(|line| line.chars().count()).max()?;
    let mut end = footer_row;
    while end > 0 && raw[end - 1].trim().is_empty() {
        end -= 1;
        if footer_row - end > 1 { return None; }
    }
    let cursor = |line: &str| -> Option<(usize, String)> {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let rest = &line[indent..];
        ['❯', '›', '>', '▶', '▸'].iter()
            .find_map(|c| rest.strip_prefix(*c).and_then(|r| r.strip_prefix(' ')))
            .map(|r| (indent, r.trim().to_string()))
    };
    // 光标那一行就在提示上面几行里；以它为准，上下只收和它文字对齐的行（光标一格加一个空格）
    let row = (end.saturating_sub(6)..end).rev().find(|&row| cursor(&raw[row]).is_some())?;
    let (indent, _) = cursor(&raw[row])?;
    let aligned = |line: &str| !line.trim().is_empty() && line.len() - line.trim_start_matches(' ').len() == indent + 2;
    let mut start = row;
    while start > 0 && aligned(&raw[start - 1]) { start -= 1; }
    if !raw[row + 1..end].iter().all(|line| aligned(line)) { return None; }
    let block = &raw[start..end];
    let selected = row - start;
    if !(2..=6).contains(&block.len()) { return None; }
    // 上面紧挨着的一行如果也像选项（有光标）就是两个光标，认不准
    if start > 0 && cursor(&raw[start - 1]).is_some() { return None; }
    let mut labels = Vec::new();
    for (i, line) in block.iter().enumerate() {
        let label = if i == selected { cursor(line)?.1 } else { line.trim().to_string() };
        if label.is_empty() || label.chars().count() > 120 || numbered(&label).is_some() || line.chars().count() + 1 >= width { return None; }
        labels.push(label);
    }
    let before: Vec<&str> = raw[..start].iter().map(|line| clean(line)).filter(|line| !line.is_empty()).collect();
    Some(Parsed { context: context(&before)?, labels, selected, horizontal: false, signature: String::new() })
}

/// 确认框的指纹：只看改尺寸也不变的部分（见文件开头）
fn signature(kind: &str, pid: u32, parsed: &Parsed) -> String {
    let squash = |text: &str| text.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let context = squash(&parsed.context);
    let context: String = context.chars().skip(context.chars().count().saturating_sub(80)).collect();
    let labels: Vec<String> = parsed.labels.iter().map(|label| squash(label).chars().take(24).collect()).collect();
    blake3::hash(format!("{kind}\0{pid}\0{}\0{}\0{context}", labels.len(), labels.join("\u{1}")).as_bytes()).to_hex().to_string()
}

fn context(lines: &[&str]) -> Option<String> {
    let lines = &lines[lines.len().saturating_sub(8)..];
    let text = lines.iter().filter(|line| !line.chars().all(|c| "─━╌═╭╮╰╯┌┐└┘ ".contains(c))).copied().collect::<Vec<_>>().join("\n");
    if text.is_empty() || text.len() > 1800 { None } else { Some(text) }
}

// OpenCode renders the selected permission as a different background, with no text cursor.
// Retain SGR backgrounds while stripping escape sequences; never infer selection from order.
fn styled_text(screen: &str) -> (String, Vec<String>) {
    let mut text = String::new();
    let mut styles = Vec::new();
    let mut background = String::new();
    let mut inverse = false;
    let mut chars = screen.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() != Some('[') { continue; }
            let mut codes = String::new();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    if c == 'm' {
                        let values: Vec<_> = codes.split(';').collect();
                        let mut i = 0;
                        while i < values.len() {
                            match values[i] {
                                "" | "0" => { background.clear(); inverse = false; }
                                "7" => inverse = true,
                                "27" => inverse = false,
                                "49" => background.clear(),
                                "48" | "38" => {
                                    let length = match values.get(i + 1) { Some(&"2") => 5, Some(&"5") => 3, _ => 1 };
                                    let end = (i + length).min(values.len());
                                    if values[i] == "48" { background = values[i..end].join(";"); }
                                    i = end;
                                    continue;
                                }
                                value => { if let Ok(n) = value.parse::<u16>() { if (40..=47).contains(&n) || (100..=107).contains(&n) { background = value.into(); } } }
                            }
                            i += 1;
                        }
                    }
                    break;
                }
                codes.push(c);
            }
            continue;
        }
        let style = format!("{background}/{inverse}");
        styles.extend(std::iter::repeat_n(style, c.len_utf8()));
        text.push(c);
    }
    (text, styles)
}

fn horizontal(screen: &str) -> Option<Parsed> {
    let (text, styles) = styled_text(screen);
    let lines: Vec<_> = text.lines().map(clean).filter(|line| !line.is_empty()).collect();
    let header = lines.iter().rposition(|line| line.contains("Permission required"))?;
    if lines.len() - header > 35 { return None; }
    let offset = text.rfind("Permission required")?;
    let footer_start = text[offset..].find("Allow once")? + offset;
    let footer_text = &text[footer_start..];
    let second = if footer_text.contains("Allow always") { "Allow always" } else { "Always allow" };
    let labels = ["Allow once", second, "Reject"];
    let mut starts = Vec::new();
    let mut pos = footer_start;
    for label in labels {
        let start = text[pos..].find(label)? + pos;
        if text[pos..start].chars().any(|c| !c.is_whitespace() && c != '│') { return None; }
        starts.push(start);
        pos = start + label.len();
    }
    let tail = &text[pos..];
    let lower = tail.to_lowercase();
    if tail.lines().count() > 8 || tail.contains("Permission required")
        || !lower.contains("enter") || !(lower.contains("confirm") || lower.contains("select"))
        || tail.lines().any(|line| matches!(clean(line), "❯" | "›" | ">" | "$")) { return None; }
    let colors: Vec<_> = starts.iter().map(|&start| styles.get(start)).collect::<Option<Vec<_>>>()?;
    let selected: Vec<_> = (0..3).filter(|&i| colors[(i+1)%3] == colors[(i+2)%3] && colors[i] != colors[(i+1)%3]).collect();
    if selected.len() != 1 { return None; }
    let before: Vec<_> = text[offset..footer_start].lines().map(clean).filter(|line| !line.is_empty()).collect();
    Some(Parsed { context: context(&before)?, labels: labels.map(str::to_string).to_vec(), selected: selected[0], horizontal: true, signature: String::new() })
}

/// 认出终端里 AI 正在问的单选题。所有认出来的 AI 都试：编号的选项最常见，没编号的（Claude Code 问信任文件夹）也认，
/// OpenCode 是横着排的；认不准就不认，交给用户在终端里选
pub fn parse(kind: &str, pid: u32, screen: &str) -> Option<Parsed> {
    if screen.len() > 200_000 { return None; }
    let (plain, _) = styled_text(screen);
    let lines: Vec<_> = plain.lines().map(clean).filter(|line| !line.is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(20)..].join("\n").to_lowercase();
    if ["space to select", "space to toggle", "multi-select", "type your answer", "type something"].iter().any(|hint| tail.contains(hint)) { return None; }
    let mut parsed = if kind == "opencode" {
        horizontal(screen)?
    } else if let Some(parsed) = vertical(&lines) {
        parsed
    } else {
        // 没编号的要看缩进，用没去掉行首空白的行；框线去掉
        let raw: Vec<String> = plain.lines().map(|line| {
            let line = line.trim_end().trim_end_matches(['│', '┃', '║']).trim_end();
            let body = line.trim_start();
            match body.chars().next() {
                Some(c @ ('│' | '┃' | '║')) => body[c.len_utf8()..].to_string(),
                _ => line.to_string(),
            }
        }).collect();
        unnumbered(&raw)?
    };
    parsed.signature = signature(kind, pid, &parsed);
    Some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numbered_agents_and_navigation() {
        for (kind, cursor) in [("claude", "❯"), ("codex", "›"), ("gemini", "●")] {
            let screen = format!("Run command: echo hi\nDo you want to continue?\n  1. Yes\n{cursor} 2. Yes, for this session\n  3. No\nEnter to confirm · Esc to cancel");
            let parsed = parse(kind, 7, &screen).unwrap();
            assert_eq!(parsed.labels.len(), 3);
            assert_eq!(parsed.keys(0).unwrap(), ["Up", "Enter"]);
            assert_eq!(parsed.keys(2).unwrap(), ["Down", "Enter"]);
        }
        let gemini = parse("gemini", 8, "Allow execution of: echo?\n● 1. Allow once\n  2. Allow for this session\n  3. No, suggest changes (esc)").unwrap();
        assert_eq!(gemini.keys(2).unwrap(), ["Down", "Down", "Enter"]);
        let wrapped = parse("claude", 8, "Do you want to create hello.txt?\n❯ 1. Yes\n2. Yes, and allow edits\n   for this session (shift+tab)\n3. No\nEsc to cancel · Tab to amend").unwrap();
        assert_eq!(wrapped.labels[1], "Yes, and allow edits for this session (shift+tab)");
        assert!(wrapped.keys(9).is_none());
    }
    #[test]
    fn ambiguous_and_stale_menus_fail_closed() {
        assert!(parse("claude", 1, "1. Yes\n2. No\nEnter to confirm").is_none());
        assert!(parse("claude", 1, "Question?\n❯ 2. Yes\n3. No\nEnter to confirm").is_none());
        assert!(parse("codex", 1, "Question?\n› 1. Yes\n2. No\nEnter to confirm\n›").is_none());
        assert!(parse("gemini", 1, "Question?\n● 1. Yes\n2. No\nSpace to select · Enter to confirm").is_none());
        let parsed = parse("claude", 1, "Question?\n❯ 1. Yes\n2. No\nEnter to confirm").unwrap();
        let mut tracker = Tracker::default();
        let first = tracker.observe("s", Some(&parsed)).unwrap();
        assert!(!tracker.consume("s", "wrong"));
        assert!(tracker.consume("s", &first.id));
        assert!(!tracker.consume("s", &first.id));
        assert!(tracker.observe("s", Some(&parsed)).is_none());
        tracker.observe("s", None);
        let next = tracker.observe("s", Some(&parsed)).unwrap();
        assert_ne!(first.id, next.id);
        assert!(!tracker.consume("s", &first.id));
        let changed = parse("claude", 1, "A different command?\n❯ 1. Yes\n2. No\nEnter to confirm").unwrap();
        let changed_menu = tracker.observe("s", Some(&changed)).unwrap();
        assert_ne!(changed_menu.id, next.id);
        assert!(!tracker.consume("s", &next.id));
    }
    /// 真的 Claude Code 2.1 画面（tmux capture-pane 抓的，路径换成了示例路径）
    #[test]
    fn real_claude_screens() {
        let wide = parse("claude", 9, include_str!("approval_screens/claude-bash.txt")).expect("认不出 Claude 的命令确认框");
        assert_eq!(wide.labels.len(), 4);
        assert_eq!(wide.selected, 0);
        assert_eq!(wide.labels[0], "Yes");
        assert!(wide.labels[1].starts_with("Yes, and always allow access to /Users/demo/work/project"), "{:?}", wide.labels[1]);
        assert_eq!(wide.labels[3], "No");
        assert!(wide.context.contains("Do you want to proceed?") && wide.context.contains("echo second-probe > probe.txt"));
        assert_eq!(wide.keys(3).unwrap(), ["Down", "Down", "Down", "Enter"]);
        // 终端变窄后重新排版（长路径在单词中间折行、末尾不再截断）：还是同一个确认框
        let narrow = parse("claude", 9, include_str!("approval_screens/claude-bash-narrow.txt")).expect("窄的认不出");
        assert_eq!(narrow.labels.len(), 4);
        assert_eq!(wide.signature, narrow.signature);
        // 换了进程（另一个 AI）就是另一个确认框
        assert_ne!(wide.signature, parse("claude", 10, include_str!("approval_screens/claude-bash.txt")).unwrap().signature);

        let trust = parse("claude", 9, include_str!("approval_screens/claude-trust.txt")).expect("认不出没编号的信任确认框");
        assert_eq!(trust.labels, ["No, exit", "Yes, I trust this folder"]);
        assert_eq!(trust.selected, 0);
        assert_eq!(trust.keys(1).unwrap(), ["Down", "Enter"]);
        assert!(trust.context.contains("Security guide"));
    }

    #[test]
    fn other_agents_and_unnumbered_fail_closed() {
        // Qwen、CodeBuddy 这类没有专门适配的，格式对上也认
        assert!(parse("qwen", 1, "Allow execution of: ls?\n● 1. Yes, allow once\n  2. No (esc)").is_some());
        assert!(parse("codebuddy", 1, "│ Do you want to proceed? │\n│ ❯ 1. Yes │\n│   2. No  │\n│ Esc to cancel │").is_some());
        // 没编号的：没有底部提示、两个光标、没对齐、碰到右边界（可能折行）都不认
        assert!(parse("claude", 1, "Question?\n ❯ No\n   Yes\n").is_none());
        assert!(parse("claude", 1, "Question?\n ❯ No\n ❯ Yes\n\n Enter to confirm").is_none());
        assert!(parse("claude", 1, "Question?\n ❯ No\n  Yes\n\n Enter to confirm").is_none());
        assert!(parse("claude", 1, "Question?\n ❯ No\n   Yes this label runs all the way to the right edge\n Enter to confirm · Esc to cancel......").is_none());
        let ok = parse("claude", 1, "Question?\n ❯ No\n   Yes\n\n Enter to confirm · Esc to cancel");
        assert_eq!(ok.unwrap().labels, ["No", "Yes"]);
    }

    #[test]
    fn consumed_menu_comes_back_after_a_while() {
        let parsed = parse("claude", 1, "Question?\n❯ 1. Yes\n2. No\nEnter to confirm").unwrap();
        let mut tracker = Tracker::default();
        let start = Instant::now();
        let first = tracker.observe_at("s", Some(&parsed), start).unwrap();
        assert!(tracker.consume_at("s", &first.id, start));
        // 刚点过：AI 还没反应过来，确认框还在，先不再给
        assert!(tracker.observe_at("s", Some(&parsed), start + Duration::from_secs(1)).is_none());
        // 过了一会儿还在（按键没生效，或者又问了一个一样的问题）：当成新的一轮
        let again = tracker.observe_at("s", Some(&parsed), start + CONSUMED_HOLD).unwrap();
        assert_ne!(again.id, first.id);
        assert!(!tracker.consume_at("s", &first.id, start + CONSUMED_HOLD));
        assert!(tracker.consume_at("s", &again.id, start + CONSUMED_HOLD));
    }

    #[test]
    fn opencode_requires_unambiguous_highlight() {
        let plain = "Permission required\n$ echo hi\nAllow once  Allow always  Reject\nenter confirm";
        assert!(parse("opencode", 1, plain).is_none());
        let screen = "Permission required\n$ echo hi\n\x1b[48;2;20;20;20m Allow once \x1b[48;2;255;160;0m Allow always \x1b[48;2;20;20;20m Reject \x1b[0m\nenter confirm";
        let parsed = parse("opencode", 1, screen).unwrap();
        assert_eq!(parsed.selected, 1);
        assert_eq!(parsed.keys(0).unwrap(), ["Left", "Enter"]);
        let changed = screen.replace("Allow always", "Always allow");
        assert!(parse("opencode", 1, &changed).is_some());
        assert!(parse("opencode", 1, &format!("{screen}\n›")).is_none());
    }
}

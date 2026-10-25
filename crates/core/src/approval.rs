//! Conservative adapters for visible, single-choice AI prompts. Never invent choices.
use std::collections::HashMap;
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

struct Entry { signature: String, id: String, consumed: bool }

#[derive(Default)]
pub struct Tracker { entries: HashMap<String, Entry> }

impl Tracker {
    pub fn observe(&mut self, session: &str, parsed: Option<&Parsed>) -> Option<Approval> {
        let Some(parsed) = parsed else { self.entries.remove(session); return None; };
        let entry = self.entries.entry(session.into()).or_insert_with(|| Entry {
            signature: parsed.signature.clone(), id: format!("{:032x}", rand::random::<u128>()), consumed: false,
        });
        if entry.signature != parsed.signature {
            *entry = Entry { signature: parsed.signature.clone(), id: format!("{:032x}", rand::random::<u128>()), consumed: false };
        }
        if entry.consumed { return None; }
        Some(Approval {
            id: entry.id.clone(), context: parsed.context.clone(),
            options: parsed.labels.iter().enumerate().map(|(id, label)| Choice { id, label: label.clone() }).collect(),
        })
    }

    pub fn consume(&mut self, session: &str, id: &str) -> bool {
        let Some(entry) = self.entries.get_mut(session) else { return false; };
        if entry.id != id || entry.consumed { return false; }
        entry.consumed = true;
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

fn vertical(kind: &str, lines: &[&str]) -> Option<Parsed> {
    let indexed: Vec<_> = lines.iter().enumerate().filter_map(|(row, line)| numbered(line).map(|option| (row, option))).collect();
    // Only the bottom menu, beginning at 1, is eligible. No truncated/paged lists.
    let start = indexed.iter().rposition(|(_, (_, number, _))| *number == 1)?;
    let rows = &indexed[start..];
    if !(2..=9).contains(&rows.len()) || rows[0].0 + 24 < lines.len() { return None; }
    if rows.iter().enumerate().any(|(i, (_, (_, n, _)))| *n != i + 1) { return None; }
    let selected: Vec<_> = rows.iter().enumerate().filter(|(_, (_, (selected, _, _)))| *selected).map(|(i, _)| i).collect();
    if selected.len() != 1 { return None; }
    let last = rows.last()?.0;
    let tail = &lines[last + 1..];
    if tail.len() > 7 || tail.iter().any(|line| matches!(*line, "❯" | "›" | "$" | ">")) { return None; }
    let before = &lines[..rows[0].0];
    let header = before.iter().rev().take(8).copied().collect::<Vec<_>>().join("\n").to_lowercase();
    let gemini_prompt = kind == "gemini" && ["allow execution", "allow ", "apply this", "do you want", "approve", "permission"].iter().any(|hint| header.contains(hint));
    if !tail.iter().any(|line| footer(line)) && !gemini_prompt { return None; }
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

pub fn parse(kind: &str, pid: u32, screen: &str) -> Option<Parsed> {
    if !matches!(kind, "claude" | "codex" | "gemini" | "opencode") || screen.len() > 200_000 { return None; }
    let (plain, _) = styled_text(screen);
    let lines: Vec<_> = plain.lines().map(clean).filter(|line| !line.is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(20)..].join("\n").to_lowercase();
    if ["space to select", "space to toggle", "multi-select", "type your answer", "type something"].iter().any(|hint| tail.contains(hint)) { return None; }
    let mut parsed = if kind == "opencode" { horizontal(screen)? } else { vertical(kind, &lines)? };
    parsed.signature = blake3::hash(format!("{kind}\0{pid}\0{screen}").as_bytes()).to_hex().to_string();
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

//! 给本机的命令行 AI 装“怎么用 p2pcli”的使用说明（Agent Skills 格式的 SKILL.md）。
//! 软件启动时自动装，之后 AI 新开的会话就知道可以用 p2pcli 操作其他设备，不用用户教。
//!
//! 只往已经存在的 AI 配置目录里写，不替没装的 AI 建目录；每家写一个独立的文件，
//! 不碰用户自己的 CLAUDE.md、AGENTS.md。文件里带标记，软件升级时只覆盖带标记的，
//! 同名但不是软件装的（没有标记）一律不动。

use std::path::{Path, PathBuf};

use serde::Serialize;
use tracing::{info, warn};

const SKILL: &str = include_str!("p2pcli-skill.md");
/// 技能目录名，也是 SKILL.md 里的 name
const NAME: &str = "p2pcli";
/// 写在文件里的标记：带它的是软件装的，可以覆盖和删除；没有的是别人的，不动
const MARK: &str = "managed-by: p2pcli";

/// 一个要写的技能目录，以及读它的有哪些 AI
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub ais: Vec<&'static str>,
    pub skills_dir: PathBuf,
}

impl Target {
    fn file(&self) -> PathBuf {
        self.skills_dir.join(NAME).join("SKILL.md")
    }
}

/// 这个目标里使用说明文件的完整路径
pub fn file_of(target: &Target) -> PathBuf {
    target.file()
}

/// 一个目标现在的情况
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// 还没装
    Missing,
    /// 已装，是最新的
    Current,
    /// 软件以前装的，内容不是最新的
    Outdated,
    /// 那里已有同名的、不是软件装的技能
    Foreign,
}

/// 界面里的一行：给哪些 AI 用、装在哪、现在什么情况
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Row {
    pub ais: Vec<String>,
    /// SKILL.md 的完整路径，界面用它指定要操作哪一行
    pub path: String,
    pub state: State,
}

/// 本机有哪些 AI，各自该往哪个目录放。按官方文档里的用户级技能目录：
/// Claude Code 读 ~/.claude/skills（CLAUDE_CONFIG_DIR 可以换）；Codex、Gemini CLI、Cursor、OpenCode、
/// Copilot CLI、Kimi CLI 都读通用的 ~/.agents/skills；Qwen Code 读 ~/.qwen/skills；CodeBuddy 读 ~/.codebuddy/skills。
/// 以 AI 自己的配置目录在不在来判断装没装它。
pub fn targets(home: &Path, claude_dir: Option<&Path>) -> Vec<Target> {
    let mut out = Vec::new();
    let claude = claude_dir.map_or_else(|| home.join(".claude"), Path::to_path_buf);
    if claude.is_dir() {
        out.push(Target { ais: vec!["Claude Code"], skills_dir: claude.join("skills") });
    }
    const GENERIC: [(&str, &str); 6] = [
        ("Codex", ".codex"),
        ("Gemini CLI", ".gemini"),
        ("Cursor", ".cursor"),
        ("OpenCode", ".config/opencode"),
        ("Copilot CLI", ".copilot"),
        ("Kimi CLI", ".kimi"),
    ];
    let ais: Vec<&'static str> = GENERIC.iter().filter(|(_, dir)| home.join(dir).is_dir()).map(|(ai, _)| *ai).collect();
    if !ais.is_empty() {
        out.push(Target { ais, skills_dir: home.join(".agents").join("skills") });
    }
    for (ai, dir) in [("Qwen Code", ".qwen"), ("CodeBuddy", ".codebuddy")] {
        if home.join(dir).is_dir() {
            out.push(Target { ais: vec![ai], skills_dir: home.join(dir).join("skills") });
        }
    }
    out
}

/// 本机真实的目标：用户目录和 CLAUDE_CONFIG_DIR
pub fn local_targets() -> Vec<Target> {
    let Some(home) = std::env::home_dir() else { return Vec::new() };
    let claude = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()).map(PathBuf::from);
    targets(&home, claude.as_deref())
}

pub fn state(target: &Target) -> State {
    match std::fs::read_to_string(target.file()) {
        Ok(existing) if existing == SKILL => State::Current,
        Ok(existing) if existing.contains(MARK) => State::Outdated,
        Ok(_) => State::Foreign,
        Err(_) => State::Missing,
    }
}

pub fn rows(targets: &[Target]) -> Vec<Row> {
    targets
        .iter()
        .map(|t| Row { ais: t.ais.iter().map(|s| s.to_string()).collect(), path: t.file().display().to_string(), state: state(t) })
        .collect()
}

/// 装或更新一个目标。那里有别人放的同名技能时不覆盖，报错
pub fn install_target(target: &Target) -> Result<(), String> {
    let file = target.file();
    if state(target) == State::Foreign {
        return Err(format!("{} 已经有一个同名技能，不是 P2PCLI 装的，没有覆盖", file.display()));
    }
    if state(target) == State::Current {
        return Ok(());
    }
    let write = || -> std::io::Result<()> {
        let dir = file.parent().expect("技能文件一定有目录");
        std::fs::create_dir_all(dir)?;
        // 先写临时文件再改名，AI 正好在读的时候不会读到半截
        let tmp = dir.join(".SKILL.md.tmp");
        std::fs::write(&tmp, SKILL)?;
        std::fs::rename(&tmp, &file)
    };
    write().map_err(|e| format!("写 {} 失败：{e}", file.display()))
}

/// 删一个目标里软件装的使用说明；不是软件装的不动，报错
pub fn remove_target(target: &Target) -> Result<(), String> {
    let file = target.file();
    match state(target) {
        State::Missing => Ok(()),
        State::Foreign => Err(format!("{} 不是 P2PCLI 装的，没有删", file.display())),
        State::Current | State::Outdated => {
            std::fs::remove_file(&file).map_err(|e| format!("删 {} 失败：{e}", file.display()))?;
            // 目录是空的才删，里面有别的文件就留着
            let _ = std::fs::remove_dir(file.parent().expect("技能文件一定有目录"));
            Ok(())
        }
    }
}

/// 全部装或更新；有别人放的同名技能的跳过，不算错。返回失败的原因
pub fn install(targets: &[Target]) -> Vec<String> {
    let mut errors = Vec::new();
    for t in targets {
        match state(t) {
            State::Foreign => info!(path = %t.file().display(), "那里已有同名技能，不是 P2PCLI 装的，不动它"),
            _ => {
                if let Err(e) = install_target(t) {
                    warn!("给 AI 装使用说明失败：{e}");
                    errors.push(e);
                }
            }
        }
    }
    errors
}

/// 全部删掉软件装的；别人的同名技能不动，不算错。返回失败的原因
pub fn remove(targets: &[Target]) -> Vec<String> {
    let mut errors = Vec::new();
    for t in targets {
        if state(t) != State::Foreign
            && let Err(e) = remove_target(t)
        {
            warn!("删除 AI 使用说明失败：{e}");
            errors.push(e);
        }
    }
    errors
}

/// 软件启动时调用：自动安装开着就装或更新。开发版不去动开发者自己的 AI 配置，和命令行工具的自动安装一样
pub fn auto_install(enabled: bool) {
    if cfg!(debug_assertions) || !enabled {
        return;
    }
    let targets = local_targets();
    install(&targets);
    let ready = rows(&targets).iter().filter(|r| r.state == State::Current).count();
    if ready > 0 {
        info!(ready, "本机的 AI 已经有 p2pcli 使用说明");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("p2pcli-hint-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn states(targets: &[Target]) -> Vec<State> {
        rows(targets).iter().map(|r| r.state).collect()
    }

    #[test]
    fn skill_file_is_valid_for_the_agent_skills_format() {
        let front = SKILL.strip_prefix("---\n").unwrap().split("\n---\n").next().unwrap();
        let field = |key: &str| front.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).unwrap();
        assert_eq!(field("name"), NAME);
        let description = field("description");
        // 规范：描述 1 到 1024 个字符
        assert!((1..=1024).contains(&description.chars().count()), "描述 {} 个字符", description.chars().count());
        assert!(front.contains(MARK));
        // 文档里写的每条命令和选项，命令行里都得真有
        for needle in ["p2pcli devices", "p2pcli exec", "--cwd", "--timeout", "p2pcli pull", "p2pcli send", "--to", "p2pcli inbox", "--json"] {
            assert!(SKILL.contains(needle), "缺 {needle}");
        }
    }

    #[test]
    fn only_existing_ai_dirs_get_targets() {
        let home = temp("targets");
        assert!(targets(&home, None).is_empty(), "什么 AI 都没装就什么都不写");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        std::fs::create_dir_all(home.join(".qwen")).unwrap();
        let t = targets(&home, None);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0], Target { ais: vec!["Claude Code"], skills_dir: home.join(".claude/skills") });
        assert_eq!(t[1], Target { ais: vec!["Codex", "Gemini CLI"], skills_dir: home.join(".agents/skills") });
        assert_eq!(t[2], Target { ais: vec!["Qwen Code"], skills_dir: home.join(".qwen/skills") });
        // 没装的 AI 的目录不会被建出来
        assert!(install(&t).is_empty());
        assert!(!home.join(".cursor").exists() && !home.join(".codebuddy").exists() && !home.join(".config").exists());
        // CLAUDE_CONFIG_DIR 换了位置就跟着换
        let other = home.join("my-claude");
        std::fs::create_dir_all(&other).unwrap();
        assert_eq!(targets(&home, Some(&other))[0].skills_dir, other.join("skills"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn rows_report_each_target_state() {
        let home = temp("rows");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let t = targets(&home, None);
        assert_eq!(states(&t), [State::Missing, State::Missing]);

        install_target(&t[0]).unwrap();
        assert_eq!(states(&t), [State::Current, State::Missing]);
        let row = &rows(&t)[0];
        assert_eq!(row.ais, ["Claude Code"]);
        assert_eq!(row.path, home.join(".claude/skills/p2pcli/SKILL.md").display().to_string());

        // 软件以前装的旧版本
        std::fs::write(t[0].file(), format!("---\nname: p2pcli\nmetadata:\n  {MARK}\n---\n旧版本")).unwrap();
        assert_eq!(state(&t[0]), State::Outdated);
        install_target(&t[0]).unwrap();
        assert_eq!(std::fs::read_to_string(t[0].file()).unwrap(), SKILL);
        assert!(!t[0].file().parent().unwrap().join(".SKILL.md.tmp").exists(), "临时文件要清掉");

        // 别人的同名技能
        std::fs::create_dir_all(t[1].file().parent().unwrap()).unwrap();
        std::fs::write(t[1].file(), "---\nname: p2pcli\ndescription: 用户自己写的\n---\n").unwrap();
        assert_eq!(state(&t[1]), State::Foreign);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn foreign_skills_are_never_overwritten_or_deleted() {
        let home = temp("foreign");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let t = targets(&home, None);
        install_target(&t[0]).unwrap();
        let mine = "---\nname: p2pcli\ndescription: 用户自己写的\n---\n";
        std::fs::create_dir_all(t[1].file().parent().unwrap()).unwrap();
        std::fs::write(t[1].file(), mine).unwrap();

        // 单个操作：报错，文件原样
        assert!(install_target(&t[1]).unwrap_err().contains("没有覆盖"));
        assert!(remove_target(&t[1]).unwrap_err().contains("没有删"));
        assert_eq!(std::fs::read_to_string(t[1].file()).unwrap(), mine);

        // 全部操作：别人的跳过，不算错，软件装的照常处理
        assert!(install(&t).is_empty());
        assert!(remove(&t).is_empty());
        assert_eq!(std::fs::read_to_string(t[1].file()).unwrap(), mine);
        assert_eq!(state(&t[0]), State::Missing);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn remove_cleans_empty_dir_but_keeps_other_files() {
        let home = temp("remove");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let t = targets(&home, None);
        install(&t);
        assert_eq!(states(&t), [State::Current, State::Current]);

        std::fs::write(t[1].file().parent().unwrap().join("notes.txt"), "别的文件").unwrap();
        remove_target(&t[0]).unwrap();
        assert!(!home.join(".claude/skills/p2pcli").exists());
        assert!(home.join(".claude").exists(), "AI 自己的配置目录不能动");
        remove_target(&t[1]).unwrap();
        assert!(!t[1].file().exists());
        assert!(t[1].file().parent().unwrap().join("notes.txt").exists(), "目录里有别的文件就不删目录");
        // 没装的再删也不报错
        remove_target(&t[0]).unwrap();
        let _ = std::fs::remove_dir_all(home);
    }
}

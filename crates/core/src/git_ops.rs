//! 编辑器的 Git 面板：暂存、取消暂存、放弃修改、提交、推送、拉取、最近的提交。
//!
//! 都是在被连方跑 git 命令。会改仓库的命令用用户 Shell 里的 PATH（提交钩子要用 node、npm，
//! 推送要用 gh 这类凭据助手），不弹密码框（GIT_TERMINAL_PROMPT=0）：要输密码时直接报错，
//! 提示去终端里做，不会卡住。路径都相对仓库根目录（和 git status 给的一样）。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::time::timeout;

/// 一次最多操作这么多个文件，命令行太长在 Windows 上会失败
const MAX_PATHS: usize = 500;
/// 提交说明最长多少字节
const MAX_MESSAGE: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitCommit {
    pub hash: String,
    pub short: String,
    pub author: String,
    /// 秒级时间戳
    pub time: u64,
    pub subject: String,
}

/// 跑一条会改仓库的 git 命令：成功回标准输出和标准错误合起来的文字（push、pull 的结果在标准错误里），
/// 失败把 git 的报错原样带回去
async fn run(top: &Path, args: &[&str], wait: Duration) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(top).args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        // Git Credential Manager 也别弹窗
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Ok(path) = std::env::join_paths(crate::lsp::search_path().await) { cmd.env("PATH", path); }
    #[cfg(windows)]
    cmd.creation_flags(crate::exec::CREATE_NO_WINDOW);
    let out = timeout(wait, cmd.output()).await.map_err(|_| anyhow::anyhow!("git 运行超时（可能在等输入密码），请在终端里执行"))?
        .context("运行 git 失败，对方电脑上装了 git 吗")?;
    let text = [String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)]
        .iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
    if !out.status.success() {
        bail!("{}", if text.is_empty() { "git 命令失败".to_string() } else { text });
    }
    Ok(text)
}

/// 仓库根目录
async fn top(root: &str) -> Result<PathBuf> {
    let root = crate::dirs::resolve(root)?;
    let out = run(&root, &["rev-parse", "--show-toplevel"], Duration::from_secs(10)).await.context("这里不是 Git 仓库")?;
    Ok(PathBuf::from(out.lines().next().unwrap_or_default().trim()))
}

fn check_paths(paths: &[String]) -> Result<()> {
    if paths.len() > MAX_PATHS { bail!("一次最多操作 {MAX_PATHS} 个文件"); }
    // 相对仓库根目录，不能跳出去；git 也会拦，这里先挡一下
    if paths.iter().any(|p| p.is_empty() || p.starts_with('/') || p.starts_with('-') || p.split(['/', '\\']).any(|s| s == "..")) {
        bail!("路径不对");
    }
    Ok(())
}

/// 用 :(literal) 把路径当成字面路径，名字里有 * ? [ 也不会被当成通配符
fn pathspecs(paths: &[String]) -> Vec<String> {
    paths.iter().map(|p| format!(":(literal){p}")).collect()
}

fn with_paths<'a>(args: &[&'a str], specs: &'a [String]) -> Vec<&'a str> {
    let mut all = args.to_vec();
    all.push("--");
    all.extend(specs.iter().map(String::as_str));
    all
}

/// 暂存；paths 空着是全部
pub async fn stage(root: &str, paths: &[String]) -> Result<()> {
    check_paths(paths)?;
    let top = top(root).await?;
    if paths.is_empty() {
        run(&top, &["add", "-A"], Duration::from_secs(60)).await?;
    } else {
        let specs = pathspecs(paths);
        run(&top, &with_paths(&["add", "-A"], &specs), Duration::from_secs(60)).await?;
    }
    Ok(())
}

/// 取消暂存；paths 空着是全部。还没有任何提交的仓库没有 HEAD，改用 rm --cached
pub async fn unstage(root: &str, paths: &[String]) -> Result<()> {
    check_paths(paths)?;
    let top = top(root).await?;
    let specs = if paths.is_empty() { vec![":/".to_string()] } else { pathspecs(paths) };
    let has_head = run(&top, &["rev-parse", "--verify", "-q", "HEAD"], Duration::from_secs(10)).await.is_ok();
    if has_head {
        run(&top, &with_paths(&["restore", "--staged"], &specs), Duration::from_secs(60)).await?;
    } else {
        run(&top, &with_paths(&["rm", "-r", "-q", "--cached"], &specs), Duration::from_secs(60)).await?;
    }
    Ok(())
}

/// 放弃修改：跟踪的文件换回暂存区里的版本（暂存的修改留着），没跟踪的移到废纸篓（能找回）
pub async fn discard(root: &str, paths: &[String], untracked: &[String]) -> Result<()> {
    check_paths(paths)?;
    check_paths(untracked)?;
    let top = top(root).await?;
    if !paths.is_empty() {
        let specs = pathspecs(paths);
        run(&top, &with_paths(&["restore", "--worktree"], &specs), Duration::from_secs(60)).await?;
    }
    for rel in untracked {
        let path = top.join(rel);
        if path.symlink_metadata().is_err() { continue; }
        tokio::task::spawn_blocking(move || crate::remote_ops::move_to_trash(&path)).await??;
    }
    Ok(())
}

/// 提交暂存的修改。提交钩子（lint、格式化）可能要跑一会儿
pub async fn commit(root: &str, message: &str) -> Result<String> {
    let message = message.trim();
    if message.is_empty() { bail!("提交说明不能为空"); }
    if message.len() > MAX_MESSAGE { bail!("提交说明太长"); }
    let top = top(root).await?;
    run(&top, &["commit", "-m", message], Duration::from_secs(300)).await
}

/// 推送。还没有上游分支时推到 origin 的同名分支并设为上游
pub async fn push(root: &str) -> Result<String> {
    let top = top(root).await?;
    let upstream = run(&top, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"], Duration::from_secs(10)).await.is_ok();
    if upstream {
        return run(&top, &["push"], Duration::from_secs(180)).await;
    }
    let branch = run(&top, &["symbolic-ref", "--short", "HEAD"], Duration::from_secs(10)).await.context("现在不在任何分支上，没法推送")?;
    let remotes = run(&top, &["remote"], Duration::from_secs(10)).await?;
    let remote = if remotes.lines().any(|r| r == "origin") { "origin" } else {
        remotes.lines().next().context("这个仓库没有配置远程仓库")?
    };
    run(&top, &["push", "-u", remote, branch.trim()], Duration::from_secs(180)).await
}

/// 拉取：只快进，有分叉时报错，让用户在终端里决定怎么合并
pub async fn pull(root: &str) -> Result<String> {
    let top = top(root).await?;
    run(&top, &["pull", "--ff-only"], Duration::from_secs(180)).await
}

/// 最近的提交
pub async fn log(root: &str, limit: usize) -> Result<Vec<GitCommit>> {
    let top = top(root).await?;
    let limit = limit.clamp(1, 200).to_string();
    // 还没有任何提交：git log 会报错，当成没有
    let Ok(out) = run(&top, &["log", "-n", &limit, "--format=%H%x1f%h%x1f%an%x1f%at%x1f%s%x1e"], Duration::from_secs(20)).await else {
        return Ok(Vec::new());
    };
    Ok(parse_log(&out))
}

fn parse_log(out: &str) -> Vec<GitCommit> {
    out.split('\x1e').filter_map(|record| {
        let mut f = record.trim().split('\x1f');
        Some(GitCommit {
            hash: f.next()?.to_string(),
            short: f.next()?.to_string(),
            author: f.next()?.to_string(),
            time: f.next()?.parse().ok()?,
            subject: f.next().unwrap_or_default().to_string(),
        })
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_outside_the_repo() {
        assert!(check_paths(&["src/a.rs".into(), "中文 名字.md".into()]).is_ok());
        for bad in ["../x", "/etc/passwd", "a/../../b", "-rf", ""] {
            assert!(check_paths(&[bad.to_string()]).is_err(), "{bad}");
        }
        assert_eq!(pathspecs(&["a*b.rs".into()]), [":(literal)a*b.rs"]);
    }

    #[tokio::test]
    async fn stage_commit_unstage_discard_and_log() {
        if std::process::Command::new("git").arg("--version").output().is_err() { return; }
        let dir = std::env::temp_dir().join(format!("p2pcli-git-ops-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let root = dir.to_string_lossy().into_owned();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap().status.success(), "{args:?}");
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.name", "t"]);
        git(&["config", "user.email", "t@e"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("src/a*b.rs"), "one\n").unwrap();
        std::fs::write(dir.join("readme.md"), "hi\n").unwrap();
        // 还没有提交时取消暂存走 rm --cached
        stage(&root, &[]).await.unwrap();
        unstage(&root, &["readme.md".into()]).await.unwrap();
        stage(&root, &["readme.md".into()]).await.unwrap();
        assert!(log(&root, 10).await.unwrap().is_empty());
        let out = commit(&root, "第一次提交").await.unwrap();
        assert!(out.contains("第一次提交"), "{out}");
        let commits = log(&root, 10).await.unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].subject, "第一次提交");
        assert_eq!(commits[0].author, "t");

        // 改了再放弃；新文件移到废纸篓不在这里测（会真的动废纸篓），只测跟踪的
        std::fs::write(dir.join("readme.md"), "changed\n").unwrap();
        stage(&root, &["readme.md".into()]).await.unwrap();
        unstage(&root, &["readme.md".into()]).await.unwrap();
        discard(&root, &["readme.md".into()], &[]).await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("readme.md")).unwrap(), "hi\n");
        // 通配符字符的文件名按字面处理
        std::fs::write(dir.join("src/a*b.rs"), "two\n").unwrap();
        discard(&root, &["src/a*b.rs".into()], &[]).await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("src/a*b.rs")).unwrap(), "one\n");

        assert!(commit(&root, "  ").await.is_err());
        // 没有远程仓库：推送报错而不是卡住
        assert!(push(&root).await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

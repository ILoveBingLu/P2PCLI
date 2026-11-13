//! 编辑器的 Git 面板：暂存、取消暂存、放弃修改、提交、推送、拉取、最近的提交。
//!
//! 都是在被连方跑 git 命令。会改仓库的命令用用户 Shell 里的 PATH（提交钩子要用 node、npm，
//! 推送要用 gh 这类凭据助手），不弹密码框（GIT_TERMINAL_PROMPT=0）：要输密码时直接报错，
//! 提示去终端里做，不会卡住。路径都相对仓库根目录（和 git status 给的一样）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitCommit {
    pub hash: String,
    pub short: String,
    pub author: String,
    /// 秒级时间戳
    pub time: u64,
    pub subject: String,
}

// 真正跑 git 命令的那部分只有被连的一方用，手机端（关掉 host）不编
#[cfg(feature = "host")]
mod run;
#[cfg(feature = "host")]
pub use run::{commit, discard, log, pull, push, stage, unstage};

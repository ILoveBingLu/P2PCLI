//! 远程编辑第二阶段：全项目搜索、按名字找文件、看文件有没有变、新建/重命名/删除、Git 状态。
//! 都是一问一答，打包成一条 Edit 请求；和读写文件一样，不比在终端里能做的多放权。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EditRequest {
    /// 在 root 下面的文件内容里搜索；认 .gitignore，跳过 .git、二进制和大文件
    Search {
        root: String,
        query: String,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default)]
        whole_word: bool,
    },
    /// root 下面所有文件的相对路径，按名字快速打开用
    ListFiles { root: String },
    /// 这些文件现在的大小和修改时间；不存在的是空
    Stat { paths: Vec<String> },
    CreateFile { path: String },
    CreateDir { path: String },
    Rename { from: String, to: String },
    /// 移到废纸篓/回收站
    Delete { path: String },
    /// root 所在 Git 仓库的改动；不在仓库里或没装 git 时回空
    GitStatus { root: String },
    /// 下面这些是 Git 面板的操作，需要 git_v1；paths 相对仓库根目录，和 GitStatus 给的一样
    /// 暂存；paths 空着是全部
    GitStage { root: String, paths: Vec<String> },
    /// 取消暂存；paths 空着是全部
    GitUnstage { root: String, paths: Vec<String> },
    /// 放弃修改：paths 换回暂存区的版本，untracked（没跟踪的）移到废纸篓
    GitDiscard { root: String, paths: Vec<String>, #[serde(default)] untracked: Vec<String> },
    GitCommit { root: String, message: String },
    GitPush { root: String },
    GitPull { root: String },
    GitLog { root: String, limit: usize },
}

impl EditRequest {
    /// 需要 git_v1 的操作
    pub fn needs_git_v1(&self) -> bool {
        matches!(self, EditRequest::GitStage { .. } | EditRequest::GitUnstage { .. } | EditRequest::GitDiscard { .. }
            | EditRequest::GitCommit { .. } | EditRequest::GitPush { .. } | EditRequest::GitPull { .. } | EditRequest::GitLog { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EditReply {
    Search { result: SearchResult },
    Files { list: FileList },
    Stats { stats: Vec<Option<FileStat>> },
    Done,
    Git { status: Option<GitStatus> },
    /// git 命令的输出（提交、推送、拉取的结果）
    Output { text: String },
    Log { commits: Vec<crate::git_ops::GitCommit> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SearchResult {
    pub files: Vec<SearchFile>,
    /// 匹配太多或超时，只回了一部分
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchFile {
    pub path: String,
    pub matches: Vec<SearchMatch>,
}

/// 列号、长度按 UTF-16 算（编辑器就是这么数的），列号从 1 开始
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchMatch {
    pub line: u32,
    pub col: u32,
    pub len: u32,
    /// 这一行（太长时截取匹配附近）
    pub preview: String,
    /// 匹配在 preview 里从第几个 UTF-16 单位开始，从 0 数
    pub preview_col: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileList {
    pub root: String,
    /// 相对 root 的路径，分隔符统一用 /
    pub files: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileStat {
    pub size: u64,
    pub mtime: Option<u64>,
    pub dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitStatus {
    /// 仓库根目录（绝对路径，符号链接已解开，可能和文件树里的写法不同）
    pub top: String,
    /// 请求的目录相对仓库根目录在哪，比如 "app/src/"，就在根目录时是空；
    /// 文件树按它把 entries 换算成自己的路径，不依赖 top 的写法
    pub prefix: String,
    pub branch: Option<String>,
    pub entries: Vec<GitEntry>,
    /// 上游分支，比如 origin/main；没有时为空。旧版本不给
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    /// 比上游多几个提交（要推送）、少几个（要拉取）
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
}

/// status 是 git status --porcelain 的两个字母，比如 " M"、"A "、"??"
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitEntry {
    /// 相对仓库根目录，分隔符是 /
    pub path: String,
    pub status: String,
}

// 真正去搜索、列文件、改文件、跑 git 的那部分只有被连的一方用，手机端（关掉 host）不编
#[cfg(feature = "host")]
mod ops;
#[cfg(feature = "host")]
pub use ops::{handle, read_head};
#[cfg(feature = "host")]
pub(crate) use ops::move_to_trash;

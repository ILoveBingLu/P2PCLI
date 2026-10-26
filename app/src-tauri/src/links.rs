//! 在默认浏览器里打开项目的 GitHub 页面。界面传来的链接不当真：只认本项目的地址，
//! 免得界面里哪天混进别的链接就被拿去打开任意网址。

/// 项目主页；界面里的“GitHub 项目”“反馈问题”“版本发布”都从这里出发
pub const REPO_URL: &str = "https://github.com/ILoveBingLu/P2PCLI";

/// 是本项目 GitHub 地址本身，或者它下面的页面（/issues、/releases……）
pub fn is_project_url(url: &str) -> bool {
    url.strip_prefix(REPO_URL)
        .is_some_and(|rest| (rest.is_empty() || rest.starts_with('/')) && !rest.chars().any(|c| c.is_whitespace() || c.is_control()))
}

/// 用系统默认浏览器打开
pub fn open(url: &str) -> Result<(), String> {
    if !is_project_url(url) {
        return Err("不是这个项目的链接，不打开".into());
    }
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(windows)]
    let r = std::process::Command::new("explorer").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    r.map(|_| ()).map_err(|e| format!("打不开浏览器：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_projects_github_pages_are_allowed() {
        for ok in [REPO_URL, "https://github.com/ILoveBingLu/P2PCLI/issues", "https://github.com/ILoveBingLu/P2PCLI/releases"] {
            assert!(is_project_url(ok), "{ok}");
        }
        for bad in [
            "https://github.com/ILoveBingLu/P2PCLI-evil",
            "https://github.com/ILoveBingLu/P2PCLIx/issues",
            "https://github.com/someone-else/P2PCLI",
            "http://github.com/ILoveBingLu/P2PCLI",
            "https://github.com.evil.com/ILoveBingLu/P2PCLI",
            "https://evil.com/?https://github.com/ILoveBingLu/P2PCLI",
            "https://github.com/ILoveBingLu/P2PCLI/issues x",
            "https://github.com/ILoveBingLu/P2PCLI/\n--flag",
            "file:///etc/passwd",
            "",
        ] {
            assert!(!is_project_url(bad), "{bad:?}");
        }
        assert!(open("https://example.com").is_err());
    }
}

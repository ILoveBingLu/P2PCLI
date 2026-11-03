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
    launch(url)
}

/// 打开本机端口上的网页（端口转发过来的开发服务器）
pub fn open_localhost(port: u16, path: &str, https: bool) -> Result<(), String> {
    launch(&localhost_url(port, path, https)?)
}

/// 只认 http://localhost:端口 加一个干净的路径，界面传来的其他东西都不要
fn localhost_url(port: u16, path: &str, https: bool) -> Result<String, String> {
    let path = if path.is_empty() { "/" } else { path };
    let bad = |c: char| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '<' | '>' | '\\' | '`');
    if port == 0 || !path.starts_with('/') || path.starts_with("//") || path.len() > 2048 || path.chars().any(bad) {
        return Err("地址不对，不打开".into());
    }
    Ok(format!("{}://localhost:{port}{path}", if https { "https" } else { "http" }))
}

fn launch(url: &str) -> Result<(), String> {
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
    fn localhost_urls_only_take_clean_paths() {
        assert_eq!(localhost_url(5173, "", false).unwrap(), "http://localhost:5173/");
        assert_eq!(localhost_url(5173, "/src/main.ts?t=1#x", false).unwrap(), "http://localhost:5173/src/main.ts?t=1#x");
        assert_eq!(localhost_url(8443, "/", true).unwrap(), "https://localhost:8443/");
        for bad in ["src", "//evil.com/x", "/a b", "/a\"b", "/a\nb", "/<script>"] {
            assert!(localhost_url(5173, bad, false).is_err(), "{bad}");
        }
        assert!(localhost_url(0, "/", false).is_err());
    }

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

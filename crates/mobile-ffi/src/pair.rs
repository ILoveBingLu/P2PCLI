//! 桌面端标题栏手机图标里的二维码：p2pcli://pair?code=设备码&pw=配对密码&id=公钥&name=设备名。
//! 设备码要牵线服务器连着才有；没有时手机按公钥连（同一个局域网里能找到）。
//! 和 app/src/lib/pairLink.ts 的格式一致。

use url::Url;

#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PairInfo {
    /// 9 位设备码；牵线服务器没连上时没有
    pub code: Option<String>,
    pub password: String,
    /// 设备公钥，设备码不在时用它连
    pub id: String,
    pub name: String,
}

impl PairInfo {
    /// 连接时交给 connect 的目标：有设备码用设备码，没有用公钥
    pub fn target(&self) -> String {
        self.code.clone().unwrap_or_else(|| self.id.clone())
    }
}

/// 扫出来的不是 P2PCLI 的连接二维码时返回空
#[uniffi::export]
pub fn parse_pair_link(text: String) -> Option<PairInfo> {
    let url = Url::parse(text.trim()).ok()?;
    let host = url.host_str().map(str::to_string).unwrap_or_else(|| url.path().trim_start_matches('/').to_string());
    if url.scheme() != "p2pcli" || host != "pair" {
        return None;
    }
    let get = |key: &str| url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned());
    let code: String = get("code").unwrap_or_default().chars().filter(char::is_ascii_digit).collect();
    let id = get("id").unwrap_or_default().trim().to_string();
    let code_ok = code.len() == 9;
    let id_ok = id.len() == 64 && id.chars().all(|c| c.is_ascii_hexdigit());
    if !code_ok && !id_ok {
        return None;
    }
    Some(PairInfo {
        code: code_ok.then_some(code),
        password: get("pw").unwrap_or_default(),
        id,
        name: get("name").unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_desktop_link() {
        let id = "a".repeat(64);
        let p = parse_pair_link(format!("p2pcli://pair?code=123456789&pw=ab%2Bc&id={id}&name=%E5%B7%A5%E4%BD%9C%E5%AE%A4")).unwrap();
        assert_eq!(p.code.as_deref(), Some("123456789"));
        assert_eq!(p.password, "ab+c");
        assert_eq!(p.name, "工作室");
        assert_eq!(p.target(), "123456789");
    }

    #[test]
    fn falls_back_to_public_key() {
        let id = "0f".repeat(32);
        let p = parse_pair_link(format!("p2pcli://pair?pw=x&id={id}")).unwrap();
        assert_eq!(p.code, None);
        assert_eq!(p.target(), id);
    }

    #[test]
    fn rejects_other_codes() {
        assert_eq!(parse_pair_link("https://example.com/?code=123456789".into()), None);
        assert_eq!(parse_pair_link("p2pcli://pair?code=123".into()), None);
        assert_eq!(parse_pair_link("not a url".into()), None);
    }
}

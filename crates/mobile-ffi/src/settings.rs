//! 手机端的设置：只留手机用得上的。手机只当连接的一方，所以桌面上那些给“被连的电脑”用的设置
//! （AI 启动命令、多机互动、受信设备、配对密码、局域网里显示本机、系统权限）这里都没有。
//!
//! 存在数据目录的 mobile-settings.json；没改过的项用 App 启动时给的值（MobileConfig）。

use std::path::Path;

use p2pcli_core::node::{Node, NodeConfig};
use p2pcli_core::rendezvous::{DEFAULT_SERVER, SERVER_PRESETS};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::P2pError;

/// 用户改过的设置；空着的项用默认
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct MobileSettings {
    /// 对方电脑上看到的名字
    #[serde(default)]
    pub device_name: Option<String>,
    /// 牵线服务器：wss://域名，或者 公钥@IP:端口
    #[serde(default)]
    pub server: Option<String>,
    /// 中继服务器，多个用逗号分开
    #[serde(default)]
    pub relay: Option<String>,
    /// HTTP 代理：牵线和中继走它，点对点直连不走
    #[serde(default)]
    pub proxy: Option<String>,
}

/// 设置页里可以直接选的牵线服务器
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ServerPreset {
    pub address: String,
    pub label: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SettingsView {
    pub settings: MobileSettings,
    /// 名字留空时用的（手机型号）
    pub default_device_name: String,
    pub default_server: String,
    pub server_presets: Vec<ServerPreset>,
    /// 收到的文件放在哪
    pub inbox_dir: String,
    /// 本机公钥：排查问题时用
    pub endpoint_id: String,
    pub app_version: Option<String>,
}

const FILE: &str = "mobile-settings.json";

pub(crate) fn load(data_dir: &Path) -> MobileSettings {
    match std::fs::read(data_dir.join(FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            warn!("设置文件读不了，用默认设置：{e}");
            MobileSettings::default()
        }),
        Err(_) => MobileSettings::default(),
    }
}

pub(crate) fn store(data_dir: &Path, s: &MobileSettings) -> anyhow::Result<()> {
    p2pcli_core::store::write_private(&data_dir.join(FILE), &serde_json::to_vec_pretty(s)?)
}

/// 去掉空白；格式不对的直接报出来，和桌面端的检查一样
pub(crate) fn validate(s: MobileSettings) -> Result<MobileSettings, P2pError> {
    let clean = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let s = MobileSettings {
        device_name: clean(s.device_name),
        server: clean(s.server),
        relay: clean(s.relay),
        proxy: clean(s.proxy),
    };
    if let Some(proxy) = &s.proxy {
        p2pcli_core::proxy::parse_proxy(proxy).map_err(P2pError::from_anyhow)?;
    }
    if let Some(server) = &s.server {
        p2pcli_core::rendezvous::parse_rdv_server(server).map_err(P2pError::from_anyhow)?;
    }
    if let Some(relay) = &s.relay {
        for u in relay.split(',').map(str::trim).filter(|u| !u.is_empty()) {
            u.parse::<iroh::RelayUrl>().map_err(|e| P2pError::failed(format!("中继地址格式不对：{u}（{e}）")))?;
        }
    }
    Ok(s)
}

/// App 给的启动参数，盖上用户改过的项
pub(crate) fn apply(base: &NodeConfig, s: &MobileSettings) -> NodeConfig {
    let mut cfg = base.clone();
    if let Some(name) = &s.device_name {
        cfg.device_name = Some(name.clone());
    }
    if let Some(server) = &s.server {
        cfg.server = Some(server.clone());
    }
    if let Some(relay) = &s.relay {
        cfg.relays = relay.split(',').map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).collect();
    }
    if let Some(proxy) = &s.proxy {
        cfg.proxy = Some(proxy.clone());
    }
    cfg
}

pub(crate) fn view(base: &NodeConfig, s: &MobileSettings, node: &Node) -> SettingsView {
    SettingsView {
        settings: s.clone(),
        default_device_name: base.device_name.clone().unwrap_or_else(|| node.me.name.clone()),
        default_server: base.server.clone().unwrap_or_else(|| DEFAULT_SERVER.to_string()),
        server_presets: SERVER_PRESETS
            .iter()
            .map(|&(address, label, note)| ServerPreset { address: address.into(), label: label.into(), note: note.into() })
            .collect(),
        inbox_dir: node.files.inbox_root().display().to_string(),
        endpoint_id: node.id().to_string(),
        app_version: base.app_version.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_like_desktop() {
        assert!(validate(MobileSettings { proxy: Some("socks5://x".into()), ..Default::default() }).is_err());
        assert!(validate(MobileSettings { relay: Some("not a url".into()), ..Default::default() }).is_err());
        let ok = validate(MobileSettings {
            device_name: Some("  我的手机 ".into()),
            proxy: Some(" http://127.0.0.1:7890 ".into()),
            server: Some("".into()),
            relay: Some("https://relay.example.com, https://relay2.example.com".into()),
        })
        .unwrap();
        assert_eq!(ok.device_name.as_deref(), Some("我的手机"));
        assert_eq!(ok.server, None);
        let cfg = apply(&NodeConfig { device_name: Some("Pixel 8".into()), ..Default::default() }, &ok);
        assert_eq!(cfg.device_name.as_deref(), Some("我的手机"));
        assert_eq!(cfg.relays.len(), 2);
        assert_eq!(cfg.proxy.as_deref(), Some("http://127.0.0.1:7890"));
    }

    #[test]
    fn saves_and_loads() {
        let dir = std::env::temp_dir().join(format!("p2pcli-mobile-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = MobileSettings { device_name: Some("测试".into()), ..Default::default() };
        store(&dir, &s).unwrap();
        assert_eq!(load(&dir), s);
        let _ = std::fs::remove_dir_all(dir);
    }
}

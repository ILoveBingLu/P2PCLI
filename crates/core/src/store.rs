//! 本机小文件：受信设备、配对过的设备、最近连过的设备和它们的会话。都是数据目录下的 JSON。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use iroh::EndpointId;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::proto::TerminalSession;

/// 写只有本人能读的文件（私钥、受信名单）
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("写 {} 失败", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn load_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrustedDevice {
    pub name: String,
    pub platform: String,
    pub added_at: u64,
}

/// 被连方记的设备名单。trusted.json 是受信设备：来连不用密码、不弹窗；
/// paired.json 是配对过的设备：同意过一次，之后能在后台看延迟和会话列表，开终端照常要同意
pub struct TrustStore {
    path: PathBuf,
    map: Mutex<BTreeMap<String, TrustedDevice>>,
}

impl TrustStore {
    pub fn open(data_dir: &Path) -> Self {
        Self::open_file(data_dir, "trusted.json")
    }

    pub fn open_file(data_dir: &Path, file: &str) -> Self {
        let path = data_dir.join(file);
        let map = load_json(&path);
        Self {
            path,
            map: Mutex::new(map),
        }
    }

    pub fn contains(&self, id: &EndpointId) -> bool {
        self.map.lock().unwrap().contains_key(&id.to_string())
    }

    pub fn add(&self, id: &EndpointId, name: &str, platform: &str) -> Result<()> {
        let mut map = self.map.lock().unwrap();
        map.insert(
            id.to_string(),
            TrustedDevice {
                name: name.into(),
                platform: platform.into(),
                added_at: now_secs(),
            },
        );
        write_private(&self.path, &serde_json::to_vec_pretty(&*map)?)
    }

    pub fn remove(&self, id: &str) -> Result<bool> {
        let mut map = self.map.lock().unwrap();
        let removed = map.remove(id).is_some();
        write_private(&self.path, &serde_json::to_vec_pretty(&*map)?)?;
        Ok(removed)
    }

    pub fn list(&self) -> Vec<(String, TrustedDevice)> {
        self.map
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecentDevice {
    pub code: String,
    /// 第一次连上时记下的设备公钥；之后牵线服务器给的不一样就报警
    pub endpoint_id: String,
    pub name: String,
    pub platform: String,
    /// 固定排序时间，旧记录迁移时沿用原来的最近使用时间
    #[serde(default)]
    pub sort_at: u64,
    pub last_used: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Remarks {
    pub device: String,
    pub sessions: BTreeMap<String, String>,
}

/// 记住的某台设备的会话列表：离线时也能显示，标着是什么时候看到的
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CachedSessions {
    pub sessions: Vec<TerminalSession>,
    pub at: u64,
}

/// 连接方记的最近设备，按设备码索引；连上过的配对密码、看到过的会话列表另存文件，按设备公钥索引
pub struct RecentStore {
    path: PathBuf,
    map: Mutex<BTreeMap<String, RecentDevice>>,
    passwords_path: PathBuf,
    passwords: Mutex<BTreeMap<String, String>>,
    sessions_path: PathBuf,
    sessions: Mutex<BTreeMap<String, CachedSessions>>,
    remarks_path: PathBuf,
    remarks: Mutex<BTreeMap<String, Remarks>>,
    /// 会话列表上次写盘的时间：没变化时最多 5 分钟写一次，只为让“上次看到”的时间别太旧
    sessions_written: Mutex<u64>,
}

impl RecentStore {
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("recent.json");
        let mut map: BTreeMap<String, RecentDevice> = load_json(&path);
        let mut order = BTreeMap::<String, u64>::new();
        for device in map.values() {
            let at = if device.sort_at == 0 { device.last_used } else { device.sort_at };
            order.entry(device.endpoint_id.clone()).and_modify(|value| *value = (*value).max(at)).or_insert(at);
        }
        for device in map.values_mut() {
            device.sort_at = order[&device.endpoint_id];
        }
        let passwords_path = data_dir.join("saved_passwords.json");
        let passwords = load_json(&passwords_path);
        let sessions_path = data_dir.join("sessions_cache.json");
        let sessions = load_json(&sessions_path);
        let remarks_path = data_dir.join("remarks.json");
        let remarks = load_json(&remarks_path);
        Self {
            path,
            map: Mutex::new(map),
            passwords_path,
            passwords: Mutex::new(passwords),
            sessions_path,
            sessions: Mutex::new(sessions),
            remarks_path,
            remarks: Mutex::new(remarks),
            sessions_written: Mutex::new(0),
        }
    }

    pub fn get(&self, code: &str) -> Option<RecentDevice> {
        self.map.lock().unwrap().get(code).cloned()
    }

    pub fn upsert(&self, mut dev: RecentDevice) -> Result<()> {
        let mut map = self.map.lock().unwrap();
        dev.sort_at = map.values().find(|d| d.endpoint_id == dev.endpoint_id)
            .map(|d| d.sort_at).unwrap_or(dev.last_used);
        map.insert(dev.code.clone(), dev);
        write_private(&self.path, &serde_json::to_vec_pretty(&*map)?)
    }

    /// 从历史里删掉；这台设备没有别的记录了，记住的密码也一起删
    pub fn remove(&self, code: &str) -> Result<()> {
        let mut map = self.map.lock().unwrap();
        let removed = map.remove(code);
        write_private(&self.path, &serde_json::to_vec_pretty(&*map)?)?;
        if let Some(dev) = removed
            && !map.values().any(|d| d.endpoint_id == dev.endpoint_id)
        {
            drop(map);
            self.forget_password(&dev.endpoint_id)?;
            self.forget_sessions(&dev.endpoint_id)?;
            let mut remarks = self.remarks.lock().unwrap();
            if remarks.remove(&dev.endpoint_id).is_some() {
                write_private(&self.remarks_path, &serde_json::to_vec_pretty(&*remarks)?)?;
            }
        }
        Ok(())
    }

    /// 记下这台设备现在的会话列表；只存编号、创建时间、在不在用，不存目录和程序
    pub fn save_sessions(&self, endpoint_id: &str, sessions: &[TerminalSession]) -> Result<()> {
        let sessions: Vec<_> = sessions
            .iter()
            .map(|s| TerminalSession { details: None, ..s.clone() })
            .collect();
        let now = now_secs();
        let mut map = self.sessions.lock().unwrap();
        let changed = map.get(endpoint_id).is_none_or(|c| c.sessions != sessions);
        map.insert(endpoint_id.to_string(), CachedSessions { sessions, at: now });
        let mut written = self.sessions_written.lock().unwrap();
        if !changed && now.saturating_sub(*written) < 300 {
            return Ok(());
        }
        *written = now;
        write_private(&self.sessions_path, &serde_json::to_vec_pretty(&*map)?)
    }

    pub fn sessions(&self, endpoint_id: &str) -> Option<CachedSessions> {
        self.sessions.lock().unwrap().get(endpoint_id).cloned()
    }

    fn forget_sessions(&self, endpoint_id: &str) -> Result<()> {
        let mut map = self.sessions.lock().unwrap();
        if map.remove(endpoint_id).is_some() {
            write_private(&self.sessions_path, &serde_json::to_vec_pretty(&*map)?)?;
        }
        Ok(())
    }

    pub fn set_device_remark(&self, endpoint_id: &str, remark: &str) -> Result<String> {
        self.set_remark(endpoint_id, None, remark)
    }

    pub fn set_session_remark(&self, endpoint_id: &str, session_id: &str, remark: &str) -> Result<String> {
        self.set_remark(endpoint_id, Some(session_id), remark)
    }

    fn set_remark(&self, endpoint_id: &str, session_id: Option<&str>, remark: &str) -> Result<String> {
        let remark = clean_remark(remark);
        let mut map = self.remarks.lock().unwrap();
        // Only publish the new value after writing succeeds.
        let mut next = map.clone();
        let entry = next.entry(endpoint_id.to_string()).or_default();
        if let Some(id) = session_id {
            if remark.is_empty() {
                entry.sessions.remove(id);
            } else {
                entry.sessions.insert(id.to_string(), remark.clone());
            }
        } else {
            entry.device = remark.clone();
        }
        if entry.device.is_empty() && entry.sessions.is_empty() {
            next.remove(endpoint_id);
        }
        write_private(&self.remarks_path, &serde_json::to_vec_pretty(&next)?)?;
        *map = next;
        Ok(remark)
    }

    pub fn remarks(&self) -> BTreeMap<String, Remarks> {
        self.remarks.lock().unwrap().clone()
    }

    /// 连上过的配对密码，下次没填密码时自动带上
    pub fn password(&self, endpoint_id: &str) -> Option<String> {
        self.passwords.lock().unwrap().get(endpoint_id).cloned()
    }

    pub fn save_password(&self, endpoint_id: &str, password: &str) -> Result<()> {
        let mut map = self.passwords.lock().unwrap();
        if map.get(endpoint_id).map(String::as_str) == Some(password) {
            return Ok(());
        }
        map.insert(endpoint_id.to_string(), password.to_string());
        write_private(&self.passwords_path, &serde_json::to_vec_pretty(&*map)?)
    }

    pub fn forget_password(&self, endpoint_id: &str) -> Result<()> {
        let mut map = self.passwords.lock().unwrap();
        if map.remove(endpoint_id).is_some() {
            write_private(&self.passwords_path, &serde_json::to_vec_pretty(&*map)?)?;
        }
        Ok(())
    }

    /// 按公钥连上以后，更新同一台设备已有的历史记录；没有记录就不动
    pub fn touch(&self, endpoint_id: &str, name: &str, platform: &str) -> Result<()> {
        let mut map = self.map.lock().unwrap();
        let mut changed = false;
        for dev in map.values_mut().filter(|d| d.endpoint_id == endpoint_id) {
            dev.name = name.to_string();
            dev.platform = platform.to_string();
            dev.last_used = now_secs();
            changed = true;
        }
        if changed {
            write_private(&self.path, &serde_json::to_vec_pretty(&*map)?)?;
        }
        Ok(())
    }

    /// 设备位置固定；同一设备的多个设备码仍优先使用最近的。
    pub fn list(&self) -> Vec<RecentDevice> {
        let mut v: Vec<_> = self.map.lock().unwrap().values().cloned().collect();
        v.sort_by_key(|d| (std::cmp::Reverse(d.sort_at), d.endpoint_id.clone(), std::cmp::Reverse(d.last_used)));
        v
    }
}

fn clean_remark(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(80)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    #[test]
    fn trust_roundtrip() {
        let dir = std::env::temp_dir().join(format!("p2pcli-trust-{}", std::process::id()));
        let id = SecretKey::generate().public();
        {
            let s = TrustStore::open(&dir);
            assert!(!s.contains(&id));
            s.add(&id, "笔记本", "macos").unwrap();
        }
        let s = TrustStore::open(&dir);
        assert!(s.contains(&id));
        assert!(s.remove(&id.to_string()).unwrap());
        assert!(!TrustStore::open(&dir).contains(&id));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

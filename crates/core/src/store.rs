//! 本机小文件：受信设备、最近连过的设备。都是数据目录下的 JSON。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use iroh::EndpointId;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

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

/// 被连方记的受信设备：这些设备来连不用密码、不弹窗
pub struct TrustStore {
    path: PathBuf,
    map: Mutex<BTreeMap<String, TrustedDevice>>,
}

impl TrustStore {
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("trusted.json");
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
    pub last_used: u64,
}

/// 连接方记的最近设备，按设备码索引；连上过的配对密码另存一个文件，按设备公钥索引，只给后端用
pub struct RecentStore {
    path: PathBuf,
    map: Mutex<BTreeMap<String, RecentDevice>>,
    passwords_path: PathBuf,
    passwords: Mutex<BTreeMap<String, String>>,
}

impl RecentStore {
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("recent.json");
        let map = load_json(&path);
        let passwords_path = data_dir.join("saved_passwords.json");
        let passwords = load_json(&passwords_path);
        Self {
            path,
            map: Mutex::new(map),
            passwords_path,
            passwords: Mutex::new(passwords),
        }
    }

    pub fn get(&self, code: &str) -> Option<RecentDevice> {
        self.map.lock().unwrap().get(code).cloned()
    }

    pub fn upsert(&self, dev: RecentDevice) -> Result<()> {
        let mut map = self.map.lock().unwrap();
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
        }
        Ok(())
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

    /// 最近用过的在前
    pub fn list(&self) -> Vec<RecentDevice> {
        let mut v: Vec<_> = self.map.lock().unwrap().values().cloned().collect();
        v.sort_by_key(|d| std::cmp::Reverse(d.last_used));
        v
    }
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

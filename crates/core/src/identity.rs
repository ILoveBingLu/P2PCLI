//! 设备身份：一把长期私钥，公钥就是设备的 EndpointId。

use std::path::Path;

use anyhow::{Context, Result};
use iroh::SecretKey;
use serde::{Deserialize, Serialize};

use crate::store::write_private;

const KEY_FILE: &str = "identity.key";

/// 读本机私钥，没有就生成一把存下来
pub fn load_or_create_key(data_dir: &Path) -> Result<SecretKey> {
    let path = data_dir.join(KEY_FILE);
    if path.exists() {
        let text = std::fs::read_to_string(&path).context("读设备私钥失败")?;
        let bytes = decode_hex32(text.trim()).context("设备私钥文件损坏")?;
        return Ok(SecretKey::from_bytes(&bytes));
    }
    let key = SecretKey::generate();
    write_private(&path, encode_hex(&key.to_bytes()).as_bytes())?;
    Ok(key)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceInfo {
    pub name: String,
    pub platform: String,
    pub user: String,
}

impl DeviceInfo {
    pub fn local(name_override: Option<String>) -> Self {
        let name = name_override
            .or_else(|| whoami::devicename().ok())
            .or_else(|| whoami::hostname().ok())
            .unwrap_or_else(|| "未命名设备".into());
        Self {
            name,
            platform: std::env::consts::OS.to_string(),
            user: whoami::username().unwrap_or_default(),
        }
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_persists() {
        let dir = std::env::temp_dir().join(format!("p2pcli-id-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = load_or_create_key(&dir).unwrap();
        let b = load_or_create_key(&dir).unwrap();
        assert_eq!(a.public(), b.public());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

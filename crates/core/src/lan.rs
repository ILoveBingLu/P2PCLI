//! 局域网发现：同一个局域网里的 P2PCLI 用 mDNS 互相广播、发现，不经过牵线服务器，断了外网也能连。
//!
//! 用 iroh 官方的 iroh-mdns-address-lookup，服务名换成自己的，只发现 P2PCLI 设备。
//! 设备名和系统放在广播的附加数据里，只发到局域网：iroh 自带的附加数据接口会连带发到公共 DNS，
//! 所以这里包一层，只在交给 mDNS 的那份地址里塞进去。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use futures_util::StreamExt;
use iroh::address_lookup::{AddressLookup, EndpointData, Error as AddressLookupError, Item, UserData};
use iroh::{Endpoint, EndpointId};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use serde::Serialize;
use tracing::debug;

use crate::identity::DeviceInfo;

/// mDNS 服务名，和别的 iroh 程序分开
const SERVICE_NAME: &str = "p2pcli";
/// 附加数据的格式标记，以后改格式时能认出旧版本
const DATA_TAG: &str = "p2pcli1";
/// 设备名最多显示这么多字
const MAX_NAME_CHARS: usize = 64;

/// 局域网里发现的一台 P2PCLI 设备
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NearbyDevice {
    pub endpoint_id: String,
    pub name: String,
    pub platform: String,
}

pub struct Lan {
    mdns: MdnsAddressLookup,
    shared: Arc<Shared>,
    devices: Arc<Mutex<HashMap<EndpointId, NearbyDevice>>>,
    task: tokio::task::AbortHandle,
}

/// 本机要广播的附加数据，以及 iroh 最近一次发布的地址（改名时拿它重新发布）
#[derive(Debug, Default)]
struct Shared {
    user_data: Mutex<Option<UserData>>,
    last: Mutex<Option<EndpointData>>,
}

/// 挂到 endpoint 上的那一层：iroh 发布地址时，只给 mDNS 这份带上设备名
#[derive(Debug)]
struct LanLookup {
    mdns: MdnsAddressLookup,
    shared: Arc<Shared>,
}

impl AddressLookup for LanLookup {
    fn publish(&self, data: &EndpointData) {
        *self.shared.last.lock().unwrap() = Some(data.clone());
        let mut data = data.clone();
        data.set_user_data(self.shared.user_data.lock().unwrap().clone());
        self.mdns.publish(&data);
    }

    fn resolve(&self, endpoint_id: EndpointId) -> Option<n0_future::boxed::BoxStream<Result<Item, AddressLookupError>>> {
        self.mdns.resolve(endpoint_id)
    }
}

impl Lan {
    /// 开始发现局域网里的设备；visible 为假时只看别人，不广播本机
    pub async fn start(endpoint: &Endpoint, me: &DeviceInfo, visible: bool) -> Result<Arc<Self>> {
        let mdns = MdnsAddressLookup::builder()
            .service_name(SERVICE_NAME)
            .advertise(visible)
            .build(endpoint.id())
            .context("局域网发现启动失败")?;
        let shared = Arc::new(Shared::default());
        *shared.user_data.lock().unwrap() = encode(me);
        endpoint.address_lookup()?.add(LanLookup {
            mdns: mdns.clone(),
            shared: shared.clone(),
        });
        let devices: Arc<Mutex<HashMap<EndpointId, NearbyDevice>>> = Arc::default();
        let mut events = mdns.subscribe().await;
        let (found, me_id) = (devices.clone(), endpoint.id());
        let task = tokio::spawn(async move {
            while let Some(ev) = events.next().await {
                match ev {
                    DiscoveryEvent::Discovered { endpoint_info, .. } => {
                        let id = endpoint_info.endpoint_id;
                        if id == me_id {
                            continue;
                        }
                        match endpoint_info.data.user_data().and_then(|d| decode(d.as_ref())) {
                            Some((platform, name)) => {
                                let dev = NearbyDevice { endpoint_id: id.to_string(), name, platform };
                                if found.lock().unwrap().insert(id, dev.clone()).as_ref() != Some(&dev) {
                                    debug!(id = %id.fmt_short(), name = %dev.name, "局域网发现设备");
                                }
                            }
                            None => debug!(id = %id.fmt_short(), "局域网里有认不出的设备，跳过"),
                        }
                    }
                    DiscoveryEvent::Expired { endpoint_id } => {
                        let gone = found.lock().unwrap().remove(&endpoint_id);
                        if gone.is_some() {
                            debug!(id = %endpoint_id.fmt_short(), "局域网设备离开");
                        }
                    }
                    _ => {}
                }
            }
        })
        .abort_handle();
        Ok(Arc::new(Self { mdns, shared, devices, task }))
    }

    /// 改了设备名就重新广播
    pub fn set_info(&self, me: &DeviceInfo) {
        let user_data = encode(me);
        let mut current = self.shared.user_data.lock().unwrap();
        if *current == user_data {
            return;
        }
        current.clone_from(&user_data);
        drop(current);
        if let Some(mut data) = self.shared.last.lock().unwrap().clone() {
            data.set_user_data(user_data);
            self.mdns.publish(&data);
        }
    }

    /// 现在能看到的设备，按名字排
    pub fn devices(&self) -> Vec<NearbyDevice> {
        let mut v: Vec<_> = self.devices.lock().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.endpoint_id.cmp(&b.endpoint_id)));
        v
    }
}

impl Drop for Lan {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// 广播内容：`p2pcli1|系统|设备名`，设备名太长就截断，总长不超过 iroh 的上限
fn encode(me: &DeviceInfo) -> Option<UserData> {
    let mut s = format!("{DATA_TAG}|{}|", me.platform);
    for c in clean_name(&me.name).chars() {
        if s.len() + c.len_utf8() > UserData::MAX_LENGTH {
            break;
        }
        s.push(c);
    }
    UserData::try_from(s).ok()
}

fn decode(s: &str) -> Option<(String, String)> {
    let mut parts = s.splitn(3, '|');
    if parts.next()? != DATA_TAG {
        return None;
    }
    let platform = parts.next()?;
    let name = clean_name(parts.next()?);
    if platform.is_empty() || platform.len() > 16 || !platform.chars().all(|c| c.is_ascii_alphanumeric()) || name.is_empty() {
        return None;
    }
    Some((platform.to_string(), name))
}

fn clean_name(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(MAX_NAME_CHARS).collect::<String>().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me(name: &str) -> DeviceInfo {
        DeviceInfo { name: name.into(), platform: "macos".into(), user: "u".into() }
    }

    #[test]
    fn user_data_roundtrip() {
        let d = encode(&me("工作室 Mac mini")).unwrap();
        assert_eq!(decode(d.as_ref()), Some(("macos".into(), "工作室 Mac mini".into())));
        // 名字里有分隔符也不影响
        let d = encode(&me("a|b")).unwrap();
        assert_eq!(decode(d.as_ref()).unwrap().1, "a|b");
        // 太长的名字截断，不超过上限，也不会切坏汉字
        let d = encode(&me(&"很长的名字".repeat(40))).unwrap();
        assert!(d.as_ref().len() <= UserData::MAX_LENGTH);
        assert_eq!(decode(d.as_ref()).unwrap().1.chars().count(), MAX_NAME_CHARS);
        // 别的程序的数据、格式不对的都不认
        for bad in ["", "hello", "p2pcli1|macos|", "p2pcli1||名字", "p2pcli1|mac os|名字", "p2pcli2|macos|名字"] {
            assert_eq!(decode(bad), None, "{bad} 不该认出来");
        }
    }
}

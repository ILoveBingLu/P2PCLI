//! 局域网发现：同一个局域网里的 P2PCLI 用 mDNS 互相广播、发现，不经过牵线服务器，断了外网也能连。
//!
//! 用 iroh 官方的 iroh-mdns-address-lookup，服务名换成自己的，只发现 P2PCLI 设备。
//! 设备名和系统放在广播的附加数据里，只发到局域网：iroh 自带的附加数据接口会连带发到公共 DNS，
//! 所以这里包一层，只在交给 mDNS 的那份地址里塞进去。

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::StreamExt;
use iroh::address_lookup::{AddressLookup, EndpointData, EndpointInfo, Error as AddressLookupError, Item, UserData};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, TransportAddr, endpoint::presets};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use serde::Serialize;
use tracing::{debug, info, warn};

use crate::identity::DeviceInfo;
use crate::ALPN_TERM;

mod interfaces;

/// mDNS 服务名，和别的 iroh 程序分开
const SERVICE_NAME: &str = "p2pcli";
/// 附加数据的格式标记，以后改格式时能认出旧版本
const DATA_TAG: &str = "p2pcli1";
/// 设备名最多显示这么多字
const MAX_NAME_CHARS: usize = 64;
/// 提高同网段多设备时的 mDNS 响应上限，避免超过 2 台设备后其余节点被提前抑制
const RESPONSE_RATE: f32 = 15.0;
/// 刷新时等待常驻 mDNS 完成约两轮交互查询（默认间隔 700ms），不额外重复绑定 :5353
const SCAN_SETTLE_WAIT: Duration = Duration::from_millis(1500);

/// 局域网里发现的一台 P2PCLI 设备
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NearbyDevice {
    pub endpoint_id: String,
    pub name: String,
    pub platform: String,
    pub online: bool,
}

struct KnownDevice {
    info: NearbyDevice,
    addr: EndpointAddr,
    first_seen: Instant,
    /// 每次收到新的 Discovered 事件递增，避免并发 scan 用旧探测结果覆盖刚收到的上线状态
    generation: u64,
}

struct ActiveMdns {
    mdns: MdnsAddressLookup,
    task: tokio::task::AbortHandle,
    interfaces: Vec<Ipv4Addr>,
}

impl Drop for ActiveMdns {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub struct Lan {
    endpoint: Endpoint,
    visible: bool,
    shared: Arc<Shared>,
    mdns_cell: Arc<Mutex<Option<MdnsAddressLookup>>>,
    active: Mutex<Option<ActiveMdns>>,
    mdns_lock: tokio::sync::Mutex<()>,
    devices: Arc<Mutex<HashMap<EndpointId, KnownDevice>>>,
    scan_lock: tokio::sync::Mutex<()>,
}

/// 本机要广播的附加数据，以及 iroh 最近一次发布的地址（改名或延迟启动时拿它重新发布）
#[derive(Debug, Default)]
struct Shared {
    user_data: Mutex<Option<UserData>>,
    last: Mutex<Option<EndpointData>>,
}

/// 挂到 endpoint 上的那一层：iroh 发布地址时，只给 mDNS 这份带上设备名
#[derive(Debug)]
struct LanLookup {
    mdns: Arc<Mutex<Option<MdnsAddressLookup>>>,
    shared: Arc<Shared>,
}

impl AddressLookup for LanLookup {
    fn publish(&self, data: &EndpointData) {
        *self.shared.last.lock().unwrap() = Some(data.clone());
        let mdns = self.mdns.lock().unwrap().clone();
        if let Some(mdns) = mdns {
            let mut data = data.clone();
            data.set_user_data(self.shared.user_data.lock().unwrap().clone());
            mdns.publish(&data);
        }
    }

    fn resolve(&self, endpoint_id: EndpointId) -> Option<n0_future::boxed::BoxStream<Result<Item, AddressLookupError>>> {
        let mdns = self.mdns.lock().unwrap().clone()?;
        mdns.resolve(endpoint_id)
    }
}

impl Lan {
    /// 开始发现局域网里的设备；visible 为假时只看别人，不广播本机。
    /// 若启动瞬间（如权限未授、端口暂被占用）绑定失败，仍保留句柄并在刷新或权限变更时自动重试。
    pub async fn start(endpoint: &Endpoint, me: &DeviceInfo, visible: bool) -> Arc<Self> {
        let shared = Arc::new(Shared::default());
        *shared.user_data.lock().unwrap() = encode(me);
        let mdns_cell = Arc::new(Mutex::new(None));
        if let Ok(lookup) = endpoint.address_lookup() {
            lookup.add(LanLookup {
                mdns: mdns_cell.clone(),
                shared: shared.clone(),
            });
        }
        let lan = Arc::new(Self {
            endpoint: endpoint.clone(),
            visible,
            shared,
            mdns_cell,
            active: Mutex::new(None),
            mdns_lock: tokio::sync::Mutex::new(()),
            devices: Arc::default(),
            scan_lock: tokio::sync::Mutex::new(()),
        });
        let interfaces = interfaces::ipv4();
        info!(?interfaces, "局域网发现使用的 IPv4 网卡");
        if let Err(e) = lan.ensure_and_refresh(&interfaces).await {
            warn!("局域网发现暂未启动，将在后续刷新或权限就绪后重试：{e:#}");
        }
        lan
    }

    /// 确保常驻 mDNS 实例正在运行并同步当前 IPv4 网卡集合；若实例已异常退出则自动重建。
    async fn ensure_and_refresh(&self, interfaces: &[Ipv4Addr]) -> Result<()> {
        let _guard = self.mdns_lock.lock().await;
        let existing = {
            let active = self.active.lock().unwrap();
            active.as_ref().map(|a| (a.mdns.clone(), a.interfaces != interfaces))
        };
        if let Some((mdns, changed)) = existing {
            if !changed {
                return Ok(());
            }
            match mdns.set_multicast_interfaces_v4(interfaces.iter().copied()).await {
                Ok(()) => {
                    if let Some(active) = self.active.lock().unwrap().as_mut() {
                        active.interfaces = interfaces.to_vec();
                    }
                    return Ok(());
                }
                Err(e) => {
                    warn!("更新局域网发现网卡失败，尝试重建 mDNS：{e:#}");
                    *self.mdns_cell.lock().unwrap() = None;
                    *self.active.lock().unwrap() = None;
                }
            }
        }

        let me_id = self.endpoint.id();
        let mdns = MdnsAddressLookup::builder()
            .service_name(SERVICE_NAME)
            .advertise(self.visible)
            .response_rate(RESPONSE_RATE)
            .multicast_interfaces_v4(interfaces.iter().copied())
            .build(me_id)
            .context("局域网发现启动失败")?;
        let mut events = mdns.subscribe().await;
        let found = self.devices.clone();
        let task = tokio::spawn(async move {
            while let Some(ev) = events.next().await {
                match ev {
                    DiscoveryEvent::Discovered { endpoint_info, .. } => {
                        let id = endpoint_info.endpoint_id;
                        if id == me_id {
                            continue;
                        }
                        remember(&mut found.lock().unwrap(), &endpoint_info);
                    }
                    DiscoveryEvent::Expired { endpoint_id } => {
                        if let Some(device) = found.lock().unwrap().get_mut(&endpoint_id) {
                            device.info.online = false;
                            debug!(id = %endpoint_id.fmt_short(), "局域网设备离开");
                        }
                    }
                    _ => {}
                }
            }
        })
        .abort_handle();

        *self.mdns_cell.lock().unwrap() = Some(mdns.clone());
        if let Some(mut data) = self.shared.last.lock().unwrap().clone() {
            data.set_user_data(self.shared.user_data.lock().unwrap().clone());
            mdns.publish(&data);
        }
        *self.active.lock().unwrap() = Some(ActiveMdns {
            mdns,
            task,
            interfaces: interfaces.to_vec(),
        });
        Ok(())
    }

    /// 网卡或网络环境变化时立即刷新组播绑定。
    pub async fn refresh_interfaces(&self) {
        let interfaces = interfaces::ipv4();
        if let Err(e) = self.ensure_and_refresh(&interfaces).await {
            debug!("刷新局域网发现网卡失败：{e:#}");
        }
    }

    /// 刷新网卡并等待常驻 mDNS 收发，随后用独立直连 endpoint 检查局域网设备连通性。
    pub async fn scan(&self, _me: EndpointId) -> Result<Vec<NearbyDevice>> {
        let _guard = match self.scan_lock.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                // 自动与手动刷新共享这一轮检测，等待完成即可，不排队重复扫描。
                let _running = self.scan_lock.lock().await;
                return Ok(self.devices());
            }
        };
        let interfaces = interfaces::ipv4();
        self.ensure_and_refresh(&interfaces).await?;
        tokio::time::sleep(SCAN_SETTLE_WAIT).await;

        // 独立、仅直连的 endpoint：不复用或关闭用户的连接，也不经中继判断局域网在线。
        let addresses: Vec<_> = self.devices.lock().unwrap().iter()
            .map(|(id, device)| (*id, device.addr.clone(), device.generation)).collect();
        if addresses.is_empty() {
            return Ok(self.devices());
        }
        let checker = Endpoint::builder(presets::Minimal)
            .clear_address_lookup()
            .relay_mode(RelayMode::Disabled)
            .bind().await.context("设备连通性检查启动失败")?;
        let checks = futures_util::stream::iter(addresses).map(|(id, addr, generation)| {
            let checker = checker.clone();
            async move {
                let online = if addr.ip_addrs().next().is_none() {
                    false
                } else {
                    match tokio::time::timeout(Duration::from_secs(2), checker.connect(addr, ALPN_TERM)).await {
                        Ok(Ok(conn)) => {
                            // 只完成加密握手，不开流、不发 Hello，不触发配对或创建会话。
                            conn.close(0u32.into(), b"reachability checked");
                            true
                        }
                        _ => false,
                    }
                };
                (id, online, generation)
            }
        }).buffer_unordered(8);
        tokio::pin!(checks);
        while let Some((id, online, generation)) = checks.next().await {
            if let Some(device) = self.devices.lock().unwrap().get_mut(&id)
                && device.generation == generation
            {
                device.info.online = online;
            }
        }
        checker.close().await;
        Ok(self.devices())
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
        let mdns = self.mdns_cell.lock().unwrap().clone();
        if let Some(mdns) = mdns
            && let Some(mut data) = self.shared.last.lock().unwrap().clone()
        {
            data.set_user_data(user_data);
            mdns.publish(&data);
        }
    }

    /// 保留离线设备，并按首次发现顺序显示，刷新时不跳行。
    pub fn devices(&self) -> Vec<NearbyDevice> {
        let devices = self.devices.lock().unwrap();
        let mut v: Vec<_> = devices.values().collect();
        v.sort_by_key(|device| device.first_seen);
        v.into_iter().map(|device| device.info.clone()).collect()
    }
}

fn remember(devices: &mut HashMap<EndpointId, KnownDevice>, endpoint: &EndpointInfo) {
    let Some((platform, name)) = endpoint.data.user_data().and_then(|data| decode(data.as_ref())) else { return };
    let id = endpoint.endpoint_id;
    // 只保留 mDNS 提供的 IP 地址，连通性检查不使用广播中的中继地址。
    let addr = EndpointAddr::from_parts(id, endpoint.data.ip_addrs().copied().map(TransportAddr::Ip));
    if let Some(device) = devices.get_mut(&id) {
        device.info.name = name;
        device.info.platform = platform;
        device.info.online = true;
        device.addr = addr;
        device.generation = device.generation.wrapping_add(1);
    } else {
        devices.insert(id, KnownDevice {
            info: NearbyDevice { endpoint_id: id.to_string(), name, platform, online: true },
            addr,
            first_seen: Instant::now(),
            generation: 0,
        });
    }
}

impl Drop for Lan {
    fn drop(&mut self) {
        *self.mdns_cell.lock().unwrap() = None;
        *self.active.lock().unwrap() = None;
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
        DeviceInfo { name: name.into(), platform: "macos".into(), user: "u".into(), version: None, os: None }
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

    #[test]
    fn remember_restores_online_after_expiry_and_bumps_generation() {
        use iroh_base::SecretKey;
        use std::net::{IpAddr, Ipv4Addr, SocketAddr};

        let id = SecretKey::from_bytes(&[7; 32]).public();
        let make_info = |name: &str, port: u16| {
            let mut data = EndpointData::default();
            data.set_user_data(encode(&me(name)));
            data.add_addrs([TransportAddr::Ip(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
                port,
            ))]);
            EndpointInfo::from_parts(id, data)
        };

        let mut devices = HashMap::new();
        remember(&mut devices, &make_info("旧名字", 4000));
        assert!(devices[&id].info.online);
        assert_eq!(devices[&id].generation, 0);

        // 模拟设备离线（Expired 或 2s 连通性检查超时）
        devices.get_mut(&id).unwrap().info.online = false;
        assert!(!devices[&id].info.online);

        // 对方重新广播上线后，必须恢复 online = true 并递增 generation
        remember(&mut devices, &make_info("新名字", 4001));
        let dev = &devices[&id];
        assert!(dev.info.online);
        assert_eq!(dev.info.name, "新名字");
        assert_eq!(dev.generation, 1);
        assert_eq!(
            dev.addr.ip_addrs().next().unwrap().port(),
            4001
        );
    }
}

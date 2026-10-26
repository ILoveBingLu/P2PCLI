use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionStatus {
    Granted,
    Denied,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemPermission {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub status: PermissionStatus,
    pub can_request: bool,
    pub settings_url: Option<&'static str>,
}

const LOCAL_NETWORK_ID: &str = "local_network";
const LOCAL_NETWORK_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork";

pub fn list() -> Vec<SystemPermission> {
    #[cfg(target_os = "macos")]
    {
        return vec![SystemPermission {
            id: LOCAL_NETWORK_ID,
            name: "局域网访问",
            description: "用于发现同一局域网里的 P2PCLI 设备，并尝试局域网直连。",
            status: macos::local_network_status(),
            can_request: true,
            settings_url: Some(LOCAL_NETWORK_SETTINGS_URL),
        }];
    }
    #[allow(unreachable_code)]
    Vec::new()
}

pub async fn request(id: &str, state: &crate::AppState) -> Result<Vec<SystemPermission>, String> {
    if id != LOCAL_NETWORK_ID {
        return Err(format!("未知的系统权限：{id}"));
    }

    #[cfg(target_os = "macos")]
    {
        let node = state.node();
        if let Some(lan) = &node.lan {
            // mDNS 是这个权限实际保护的操作。首次调用时 macOS 会在这里弹授权框。
            let _ = lan.scan(node.id()).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
    Ok(list())
}

#[cfg(target_os = "macos")]
mod macos {
    use block2::{Block, RcBlock};
    use dispatch2::{DispatchQueue, DispatchQueueGlobalPriority, GlobalQueueIdentifier};
    use std::ffi::c_void;
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    type NwPath = *mut c_void;
    type NwPathMonitor = *mut c_void;

    const NW_PATH_STATUS_SATISFIED: i32 = 1;
    const NW_PATH_UNSATISFIED_REASON_LOCAL_NETWORK_DENIED: i32 = 3;

    #[link(name = "Network", kind = "framework")]
    unsafe extern "C" {
        fn nw_path_monitor_create() -> NwPathMonitor;
        fn nw_path_monitor_set_update_handler(
            monitor: NwPathMonitor,
            handler: &Block<dyn Fn(NwPath)>,
        );
        fn nw_path_monitor_set_queue(monitor: NwPathMonitor, queue: &DispatchQueue);
        fn nw_path_monitor_start(monitor: NwPathMonitor);
        fn nw_path_monitor_cancel(monitor: NwPathMonitor);
        fn nw_path_get_status(path: NwPath) -> i32;
        fn nw_path_get_unsatisfied_reason(path: NwPath) -> i32;
    }

    pub fn local_network_status() -> super::PermissionStatus {
        let result = Arc::new((Mutex::new(None), Condvar::new()));
        let callback_result = Arc::clone(&result);
        let handler: RcBlock<dyn Fn(NwPath)> = RcBlock::new(move |path| {
            let status = unsafe {
                if nw_path_get_status(path) == NW_PATH_STATUS_SATISFIED {
                    super::PermissionStatus::Granted
                } else if nw_path_get_unsatisfied_reason(path)
                    == NW_PATH_UNSATISFIED_REASON_LOCAL_NETWORK_DENIED
                {
                    super::PermissionStatus::Denied
                } else {
                    super::PermissionStatus::Unknown
                }
            };
            let (lock, wake) = &*callback_result;
            *lock.lock().unwrap() = Some(status);
            wake.notify_one();
        });

        let monitor = unsafe { nw_path_monitor_create() };
        if monitor.is_null() {
            return super::PermissionStatus::Unknown;
        }
        let queue = DispatchQueue::global_queue(GlobalQueueIdentifier::Priority(
            DispatchQueueGlobalPriority::Default,
        ));
        unsafe {
            nw_path_monitor_set_update_handler(monitor, &handler);
            nw_path_monitor_set_queue(monitor, &queue);
            nw_path_monitor_start(monitor);
        }

        let (lock, wake) = &*result;
        let mut status = lock.lock().unwrap();
        if status.is_none() {
            let (next, _) = wake
                .wait_timeout(status, Duration::from_millis(250))
                .unwrap();
            status = next;
        }
        let status = status.take().unwrap_or(super::PermissionStatus::Unknown);
        unsafe {
            nw_path_monitor_cancel(monitor);
        }
        status
    }
}

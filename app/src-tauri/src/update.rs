//! 在线更新：查 GitHub 发布页上的 latest.json，有新版就下载、核对签名、安装并重启。
//! 签名公钥写在 tauri.conf.json 里，只认用对应私钥签过的安装包，下载链路被人改了也装不上。

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, Url};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};
use tracing::{info, warn};

/// 查到了、还没装的新版本
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<Update>>);

#[derive(Serialize)]
pub struct UpdateView {
    version: String,
    current_version: String,
    notes: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum UpdateProgress {
    Downloading { downloaded: u64, total: Option<u64> },
    Installing,
}

/// 下载更新包最多等这么久；检查更新还是 30 秒
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// 设置里配了 HTTP 代理就走代理；P2PCLI_UPDATE_URL 是测试用的，换成别的更新地址
fn updater(app: &AppHandle, proxy: Option<&str>) -> Result<Updater, String> {
    let mut builder = app.updater_builder().timeout(Duration::from_secs(30));
    if let Some(proxy) = proxy.and_then(|p| p.parse::<Url>().ok()) {
        builder = builder.proxy(proxy);
    }
    if let Some(url) = std::env::var("P2PCLI_UPDATE_URL").ok().and_then(|u| u.parse::<Url>().ok()) {
        builder = builder.endpoints(vec![url]).map_err(|e| format!("更新地址不对：{e}"))?;
    }
    builder.build().map_err(|e| format!("更新组件没准备好：{e}"))
}

pub async fn check(app: &AppHandle, proxy: Option<String>) -> Result<Option<UpdateView>, String> {
    let update = updater(app, proxy.as_deref())?
        .check()
        .await
        .map_err(|e| {
            warn!("检查更新失败：{e}");
            format!("检查更新失败：{e}")
        })?;
    let view = update.as_ref().map(|u| UpdateView {
        version: u.version.clone(),
        current_version: u.current_version.clone(),
        notes: u.body.clone().filter(|b| !b.trim().is_empty()),
    });
    *app.state::<PendingUpdate>().0.lock().unwrap() = update;
    Ok(view)
}

/// 下载、核对签名、安装，装好就重启；Windows 上安装程序接手后软件会自己退出
pub async fn install(app: &AppHandle, progress: Channel<UpdateProgress>) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Err("开发版不能在线更新，请用安装包装的正式版".into());
    }
    let mut update = app
        .state::<PendingUpdate>()
        .0
        .lock()
        .unwrap()
        .take()
        .ok_or("没有待安装的新版本，请重新检查更新")?;
    // 检查更新的 30 秒超时会原样带到下载，而且算的是整个下载；网慢的时候十几 MB 下不完就报失败。下载放宽到 10 分钟
    update.timeout = Some(DOWNLOAD_TIMEOUT);
    info!(version = %update.version, "开始下载更新");
    let mut downloaded = 0u64;
    let on_chunk = progress.clone();
    update
        .download_and_install(
            |chunk, total| {
                downloaded += chunk as u64;
                let _ = on_chunk.send(UpdateProgress::Downloading { downloaded, total });
            },
            || {
                let _ = progress.send(UpdateProgress::Installing);
            },
        )
        .await
        .map_err(|e| {
            warn!(version = %update.version, downloaded, "更新失败：{e}");
            format!("更新失败：{e}")
        })?;
    info!(version = %update.version, "新版本已装好，重启");
    app.restart();
}

/// 测试用：P2PCLI_UPDATE_AUTO 设了就启动后自己检查并安装，不用点界面。
/// 只装签名对得上的包，和手动点更新走的是同一条路。
pub fn auto_install_for_test(app: &AppHandle) {
    if std::env::var_os("P2PCLI_UPDATE_AUTO").is_none() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        match check(&app, None).await {
            Ok(Some(u)) => {
                info!(version = %u.version, "测试：发现新版本，自动安装");
                if let Err(e) = install(&app, Channel::new(|_| Ok(()))).await {
                    warn!("测试：自动安装失败：{e}");
                }
            }
            Ok(None) => info!("测试：已经是最新版本"),
            Err(e) => warn!("测试：{e}"),
        }
    });
}

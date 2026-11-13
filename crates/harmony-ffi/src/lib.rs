//! 鸿蒙（HarmonyOS NEXT）的 ArkTS 调的 Rust 接口。
//!
//! 连接、协议、会话、传文件都用 mobile-ffi（和安卓、iOS 同一份），这里只把它用 N-API 导出给 ArkTS：
//! - 数据一律转成 JSON 字符串交过去，ArkTS 那边 JSON.parse，字段名和桌面前端一样是下划线命名；
//!   类型声明在 mobile/harmony/entry/src/main/cpp/types/libp2pcli/index.d.ts
//! - 会等网络的方法返回 Promise，在 Rust 自己的线程上跑，不卡界面
//! - 回调（设备列表变了、终端输出）从 Rust 的线程经线程安全函数交回 ArkTS 的主线程
//! - 出错时抛的 Error.message 也是 JSON：{"kind":"rejected","reason":"need_password","detail":"…"}
//!
//! 64 位整数（连接编号、传输编号）在 ArkTS 里都是 number：数值都很小，不会丢精度。

use std::sync::Arc;

use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::{Result, Uint8Array};
use napi_ohos::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_ohos::{Error, Status};
use p2pcli_mobile as ffi;
use serde::Serialize;
use serde_json::json;

/// ArkTS 传进来的回调：收一个字符串（事件的 JSON）
type StrCallback = ThreadsafeFunction<String, (), String, Status, false>;
/// 终端输出：收原始字节
type BytesCallback = ThreadsafeFunction<Uint8Array, (), Uint8Array, Status, false>;

fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

/// Rust 的错误转成 ArkTS 的 Error，message 是 JSON，界面按 kind、reason 决定怎么提示
fn err(e: ffi::P2pError) -> Error {
    let v = match &e {
        ffi::P2pError::Rejected { reason, detail } => json!({ "kind": "rejected", "reason": reason, "detail": detail }),
        ffi::P2pError::Disconnected => json!({ "kind": "disconnected", "detail": e.to_string() }),
        ffi::P2pError::Failed { detail } => json!({ "kind": "failed", "detail": detail }),
    };
    Error::new(Status::GenericFailure, v.to_string())
}

fn bad_input(e: impl std::fmt::Display) -> Error {
    Error::new(Status::InvalidArg, json!({ "kind": "failed", "detail": e.to_string() }).to_string())
}

/// 后台变化交给 ArkTS：都走同一个回调，按 type 区分
struct Events(StrCallback);

impl Events {
    fn send(&self, v: serde_json::Value) {
        self.0.call(v.to_string(), ThreadsafeFunctionCallMode::NonBlocking);
    }
}

impl ffi::AppListener for Events {
    fn on_overview_changed(&self) {
        self.send(json!({ "type": "overview_changed" }));
    }
    fn on_disconnected(&self, conn_id: u64, reason: String) {
        self.send(json!({ "type": "disconnected", "conn_id": conn_id, "reason": reason }));
    }
    fn on_transfers_changed(&self, transfers: Vec<ffi::TransferView>) {
        self.send(json!({ "type": "transfers", "transfers": transfers }));
    }
    fn on_file_request(&self, request: ffi::FileRequestView) {
        self.send(json!({ "type": "file_request", "request": request }));
    }
    fn on_file_request_cancelled(&self, id: u64) {
        self.send(json!({ "type": "file_request_cancelled", "id": id }));
    }
}

/// 一个终端的输出和结束
struct TermEvents {
    output: BytesCallback,
    closed: StrCallback,
}

impl ffi::TerminalListener for TermEvents {
    fn on_output(&self, data: Vec<u8>) {
        self.output.call(Uint8Array::new(data), ThreadsafeFunctionCallMode::NonBlocking);
    }
    fn on_closed(&self, exit_code: Option<u32>, reason: Option<String>) {
        let v = json!({ "exit_code": exit_code, "reason": reason });
        self.closed.call(v.to_string(), ThreadsafeFunctionCallMode::NonBlocking);
    }
}

/// 启动：读身份、连牵线服务器、开始局域网发现。config 是 MobileConfig 的 JSON；
/// on_event 收后台变化（JSON，type 是 overview_changed、disconnected、transfers、file_request、file_request_cancelled）
#[napi]
pub async fn start(config: String, on_event: StrCallback) -> Result<P2p> {
    let config: ffi::MobileConfig = serde_json::from_str(&config).map_err(bad_input)?;
    let listener: Arc<dyn ffi::AppListener> = Arc::new(Events(on_event));
    // 启动会阻塞到网络起来：放到阻塞线程上
    let inner = napi_ohos::tokio::task::spawn_blocking(move || ffi::P2p::start(config, listener))
        .await
        .map_err(bad_input)?
        .map_err(err)?;
    Ok(P2p { inner })
}

/// 扫出来的不是 P2PCLI 的二维码时返回 null；是的话返回 PairInfo 的 JSON
#[napi]
pub fn parse_pair_link(text: String) -> Option<String> {
    ffi::parse_pair_link(text).map(|p| to_json(&p))
}

#[napi]
pub struct P2p {
    inner: Arc<ffi::P2p>,
}

#[napi]
impl P2p {
    /// Overview 的 JSON：本机、牵线服务器状态、设备列表、附近设备
    #[napi]
    pub fn overview(&self) -> String {
        to_json(&self.inner.overview())
    }

    #[napi]
    pub async fn scan_nearby(&self) -> Result<String> {
        self.inner.scan_nearby().await.map(|v| to_json(&v)).map_err(err)
    }

    /// 连一台设备：target 是设备码或公钥。返回 Connected 的 JSON
    #[napi]
    pub async fn connect(&self, target: String, password: Option<String>) -> Result<String> {
        self.inner.connect(target, password).await.map(|v| to_json(&v)).map_err(err)
    }

    #[napi]
    pub fn disconnect(&self, conn_id: i64) {
        self.inner.disconnect(conn_id as u64);
    }

    #[napi]
    pub async fn list_sessions(&self, conn_id: i64) -> Result<String> {
        self.inner.list_sessions(conn_id as u64).await.map(|v| to_json(&v)).map_err(err)
    }

    #[napi]
    pub async fn end_session(&self, conn_id: i64, session_id: String) -> Result<()> {
        self.inner.end_session(conn_id as u64, session_id).await.map_err(err)
    }

    #[napi]
    pub async fn choose_agent_option(&self, conn_id: i64, session_id: String, approval_id: String, option_id: i64) -> Result<()> {
        self.inner
            .choose_agent_option(conn_id as u64, session_id, approval_id, option_id as u64)
            .await
            .map_err(err)
    }

    /// 打开终端：session_id 为空是新建。on_output 收终端输出的原始字节；
    /// on_closed 收 {"exit_code":…,"reason":…}（程序退出时有 exit_code，连接断了时有 reason）
    #[napi]
    pub async fn open_terminal(
        &self,
        conn_id: i64,
        session_id: Option<String>,
        cols: u32,
        rows: u32,
        on_output: BytesCallback,
        on_closed: StrCallback,
    ) -> Result<Terminal> {
        let listener: Arc<dyn ffi::TerminalListener> = Arc::new(TermEvents { output: on_output, closed: on_closed });
        let inner = self
            .inner
            .open_terminal(conn_id as u64, session_id, cols.min(u16::MAX as u32) as u16, rows.min(u16::MAX as u32) as u16, listener)
            .await
            .map_err(err)?;
        Ok(Terminal { inner })
    }

    /// 在后台发文件，返回传输编号；进度从 on_event 的 transfers 来
    #[napi]
    pub fn send_files(&self, target: String, paths: Vec<String>) -> Result<String> {
        self.inner.send_files(target, paths).map_err(err)
    }

    #[napi]
    pub fn transfers(&self) -> String {
        to_json(&self.inner.transfers())
    }

    #[napi]
    pub fn respond_files(&self, id: i64, accept: bool) {
        self.inner.respond_files(id as u64, accept);
    }

    #[napi]
    pub fn set_device_remark(&self, endpoint_id: String, remark: String) -> Result<()> {
        self.inner.set_device_remark(endpoint_id, remark).map_err(err)
    }

    #[napi]
    pub fn forget_device(&self, endpoint_id: String) -> Result<()> {
        self.inner.forget_device(endpoint_id).map_err(err)
    }

    #[napi]
    pub fn refresh_devices(&self) {
        self.inner.refresh_devices();
    }

    /// SettingsView 的 JSON
    #[napi]
    pub fn settings(&self) -> String {
        to_json(&self.inner.settings())
    }

    /// 保存设置并马上生效；settings 是 MobileSettings 的 JSON
    #[napi]
    pub async fn save_settings(&self, settings: String) -> Result<()> {
        let s: ffi::MobileSettings = serde_json::from_str(&settings).map_err(bad_input)?;
        self.inner.save_settings(s).await.map_err(err)
    }

    #[napi]
    pub fn shutdown(&self) {
        self.inner.shutdown();
    }
}

#[napi]
pub struct Terminal {
    inner: Arc<ffi::Terminal>,
}

#[napi]
impl Terminal {
    /// 对方电脑上的会话编号：关掉终端以后还能用它接回来
    #[napi]
    pub fn session_id(&self) -> String {
        self.inner.session_id()
    }

    /// 键盘、按键条打的字节，原样发过去
    #[napi]
    pub fn write(&self, data: Uint8Array) {
        self.inner.write(data.to_vec());
    }

    #[napi]
    pub fn write_text(&self, text: String) {
        self.inner.write(text.into_bytes());
    }

    #[napi]
    pub fn resize(&self, cols: u32, rows: u32) {
        self.inner.resize(cols.min(u16::MAX as u32) as u16, rows.min(u16::MAX as u32) as u16);
    }

    /// 关掉手机上的这个终端；对方电脑上的会话接着跑
    #[napi]
    pub fn detach(&self) {
        self.inner.detach();
    }
}

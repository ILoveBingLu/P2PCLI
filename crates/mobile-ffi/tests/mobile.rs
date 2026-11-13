//! 手机接口走一遍：同一个进程里起一台被连的电脑（core 带 host），用手机接口连它、
//! 开后台会话打字、列会话、结束会话，再试错密码。走真的 iroh 连接，只用本机回环地址，不连外网。
//!
//! 开后台会话要 tmux：P2PCLI_TEST_SESSION_PROGRAM 指定，或者 PATH 里有 tmux；都没有就跳过。

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use p2pcli_core::host::{BoxFuture, Decision, HostDelegate, HostEvent, IncomingRequest};
use p2pcli_core::node::{Node, NodeConfig};
use p2pcli_mobile::{AppListener, FileRequestView, MobileSettings, P2p, P2pError, RejectReason, TerminalListener, TransferView};

/// 被连的电脑：谁来都同意
struct AllowAll;
impl HostDelegate for AllowAll {
    fn approve(&self, _req: IncomingRequest) -> BoxFuture<Decision> {
        Box::pin(async { Decision::Allow { trust: false } })
    }
    fn event(&self, _ev: HostEvent) {}
}

#[derive(Default)]
struct Events {
    disconnected: Mutex<Vec<(u64, String)>>,
}
impl AppListener for Events {
    fn on_overview_changed(&self) {}
    fn on_disconnected(&self, conn_id: u64, reason: String) {
        self.disconnected.lock().unwrap().push((conn_id, reason));
    }
    fn on_transfers_changed(&self, _transfers: Vec<TransferView>) {}
    fn on_file_request(&self, _request: FileRequestView) {}
    fn on_file_request_cancelled(&self, _id: u64) {}
}

/// 收终端输出，等某段文字出现
#[derive(Default)]
struct Screen {
    text: Mutex<String>,
    closed: Mutex<Option<(Option<u32>, Option<String>)>>,
    changed: Condvar,
}
impl TerminalListener for Screen {
    fn on_output(&self, data: Vec<u8>) {
        self.text.lock().unwrap().push_str(&String::from_utf8_lossy(&data));
        self.changed.notify_all();
    }
    fn on_closed(&self, exit_code: Option<u32>, reason: Option<String>) {
        *self.closed.lock().unwrap() = Some((exit_code, reason));
        self.changed.notify_all();
    }
}
impl Screen {
    fn wait_for(&self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut text = self.text.lock().unwrap();
        while !text.contains(needle) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "等不到 {needle:?}，终端里是：{text}");
            text = self.changed.wait_timeout(text, left).unwrap().0;
        }
    }
}

fn session_program() -> Option<PathBuf> {
    std::env::var_os("P2PCLI_TEST_SESSION_PROGRAM").map(PathBuf::from).or_else(|| {
        std::env::var_os("PATH")?.to_str()?.split(':').map(|d| PathBuf::from(d).join("tmux")).find(|p| p.is_file())
    })
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("p2pcli-mobile-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// 这些接口在 Kotlin/Swift 里是挂起函数；测试里随便找个执行器等它们。
/// 不能用 P2p 自己的线程池，也不能在 tokio 里丢掉 P2p（它带着自己的线程池）
fn wait<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(fut)
}

#[test]
fn connect_open_terminal_and_end_session() {
    let Some(program) = session_program() else {
        eprintln!("没有 tmux，跳过");
        return;
    };
    let host_dir = temp_dir("host");
    let phone_dir = temp_dir("phone");

    // 被连的电脑
    let host_rt = tokio::runtime::Runtime::new().unwrap();
    let host = host_rt
        .block_on(Node::start(
            NodeConfig {
                data_dir: host_dir.clone(),
                device_name: Some("被连的电脑".into()),
                offline: true,
                session_program: Some(program),
                ..Default::default()
            },
            Arc::new(AllowAll),
        ))
        .unwrap();
    host.host.set_password("abc234");
    let port = host.endpoint.bound_sockets().into_iter().find(|s| s.is_ipv4()).unwrap().port();
    let target = format!("{}@{}", host.id(), SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port));

    // 手机
    let events = Arc::new(Events::default());
    let phone = P2p::start_with(
        NodeConfig { data_dir: phone_dir.clone(), device_name: Some("测试手机".into()), offline: true, ..Default::default() },
        events.clone(),
    )
    .unwrap();

    // 密码不对要被拒，原因能分出来
    match wait(phone.connect(target.clone(), Some("xxxxxx".into()))) {
        Err(P2pError::Rejected { reason, .. }) => assert_eq!(reason, RejectReason::BadPassword),
        other => panic!("应该因为密码不对被拒：{other:?}"),
    }

    let conn = wait(phone.connect(target, Some("ABC234".into()))).expect("应该连上");
    assert_eq!(conn.name, "被连的电脑");
    assert!(phone.path(conn.conn_id).is_ok());

    // 新建后台会话，打字，看到输出
    let screen = Arc::new(Screen::default());
    let term = wait(phone.open_terminal(conn.conn_id, None, 100, 30, screen.clone())).expect("应该开出终端");
    let session = term.session_id();
    assert!(!session.is_empty());
    term.resize(120, 40);
    term.write(b"echo ffi-$((40+2))\r".to_vec());
    screen.wait_for("ffi-42");

    // 会话列表里有它；关掉手机上的终端，会话还在
    let sessions = wait(phone.list_sessions(conn.conn_id)).unwrap();
    assert!(sessions.iter().any(|s| s.id == session), "会话列表：{sessions:?}");
    term.detach();
    std::thread::sleep(Duration::from_millis(300));
    assert!(wait(phone.list_sessions(conn.conn_id)).unwrap().iter().any(|s| s.id == session), "关掉终端不该结束会话");

    // 接回同一个会话，之前的输出还在
    let again = Arc::new(Screen::default());
    let term2 = wait(phone.open_terminal(conn.conn_id, Some(session.clone()), 100, 30, again.clone())).unwrap();
    assert_eq!(term2.session_id(), session);
    term2.write(b"echo back-$((1+1))\r".to_vec());
    again.wait_for("back-2");

    // 选工作目录：列出上一级能看到它，等终端回到空闲提示符再切过去，pwd 看到新目录
    let workdir = temp_dir("ffi-workdir");
    std::fs::create_dir_all(&workdir).unwrap();
    let name = workdir.file_name().unwrap().to_string_lossy().to_string();
    let listing = wait(phone.list_dir(conn.conn_id, workdir.parent().unwrap().display().to_string())).expect("应该列出目录");
    assert!(listing.dirs.iter().any(|d| *d == name), "目录列表：{listing:?}");
    for _ in 0..50 {
        let ready = wait(phone.list_sessions(conn.conn_id)).unwrap().iter().any(|s| s.id == session && s.shell_ready);
        if ready {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    wait(phone.change_dir(conn.conn_id, session.clone(), workdir.display().to_string())).expect("应该切过去");
    term2.write(b"pwd\r".to_vec());
    again.wait_for(&name);

    // 检测装了的 AI：能调通就行（测试机上可能一个都没装）
    assert!(wait(phone.installed_agents(conn.conn_id, true)).is_ok());
    term2.detach();

    // 结束会话，列表里没了
    wait(phone.end_session(conn.conn_id, session.clone())).unwrap();
    assert!(!wait(phone.list_sessions(conn.conn_id)).unwrap().iter().any(|s| s.id == session));

    // 连接历史里记下了这台电脑
    let overview = phone.overview();
    assert!(overview.devices.iter().any(|d| d.endpoint_id == conn.endpoint_id) || overview.devices.is_empty(),
        "按地址连的不一定进历史，进了就该是这台：{:?}", overview.devices);

    // 改设备名：网络没换，连接不断；设置读回来是新的
    wait(phone.save_settings(MobileSettings { device_name: Some("改了名的手机".into()), ..Default::default() })).unwrap();
    assert_eq!(phone.settings().settings.device_name.as_deref(), Some("改了名的手机"));
    assert!(wait(phone.list_sessions(conn.conn_id)).is_ok(), "改名不该断开连接");
    // 格式不对的设置不保存
    assert!(wait(phone.save_settings(MobileSettings { proxy: Some("socks5://x".into()), ..Default::default() })).is_err());
    assert_eq!(phone.settings().settings.device_name.as_deref(), Some("改了名的手机"));

    // 断开以后编号不能再用
    phone.disconnect(conn.conn_id);
    assert!(matches!(wait(phone.list_sessions(conn.conn_id)), Err(P2pError::Disconnected)));

    phone.shutdown();
    drop(phone);
    host_rt.block_on(host.shutdown());
    let _ = std::fs::remove_dir_all(host_dir);
    let _ = std::fs::remove_dir_all(phone_dir);
}

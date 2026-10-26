//! 在一条模拟的“跨境链路”上比较 QUIC 拥塞控制：延迟、丢包、带宽上限都在内存里重现，
//! 两头是真的 iroh 端点，走真的 QUIC。链路是自定义传输（不经过真实网卡），所以没有别的路能绕过去。
//! 默认不跑（要两分钟左右）：
//!   cargo test -p p2pcli-core --test lossy_link -- --ignored --nocapture

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use iroh::endpoint::transports::{CustomEndpoint, CustomSender, CustomTransport, RecvInfo, Transmit};
use p2pcli_core::host::{BoxFuture, Decision, FileRequest, HostDelegate, HostEvent, IncomingRequest};
use p2pcli_core::node::{Node, NodeConfig};
use iroh::endpoint::{PathId, QuicTransportConfig, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey, TransportAddr};
use iroh_base::CustomAddr;
use noq_proto::congestion::{Bbr3Config, CubicConfig};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep, sleep_until};

const ALPN: &[u8] = b"bench/1";
/// 自定义传输的编号，随便挑一个不和别的撞的
const LINK_TRANSPORT_ID: u64 = 0x7e57;
/// 每组测多久；前面几秒是慢启动，算稳定速率时去掉
const RUN: Duration = Duration::from_secs(15);
const WARMUP: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, Debug)]
struct Link {
    one_way: Duration,
    loss: f64,
    rate_bps: u64,
    /// 队列里最多排多久，再多就丢尾（像路由器的缓冲）
    queue_limit: Duration,
}

#[derive(Clone, Copy, Debug)]
enum Cc {
    Cubic,
    Bbr3,
    /// 产品里实际用的：按链路延迟自动选 Cubic 或 BBR3
    Adaptive,
}

fn addr_of(id: EndpointId) -> CustomAddr {
    CustomAddr::from((LINK_TRANSPORT_ID, &id.as_bytes()[..]))
}

fn id_of(addr: &CustomAddr) -> io::Result<EndpointId> {
    let key: &[u8; 32] = addr.data().try_into().map_err(|_| io::Error::other("地址长度不对"))?;
    EndpointId::from_bytes(key).map_err(|_| io::Error::other("公钥不对"))
}

#[derive(Debug)]
struct Packet {
    data: Bytes,
    from: CustomAddr,
}

/// 排队等着投递的包：什么时候到、包本身、投给谁
type Due = (Instant, Packet, mpsc::Sender<Packet>);

/// 一个方向的链路：这个方向上所有连接共用同一份带宽和队列，包按进队的顺序投递
#[derive(Debug)]
struct Direction {
    /// 链路下一次空出来的时间：包要按带宽一个接一个发
    link_free: Mutex<Instant>,
    delay: mpsc::UnboundedSender<Due>,
}

/// 内存里的网络：谁发给谁，都经过同一条带损伤的链路
#[derive(Debug, Clone)]
struct Net {
    inboxes: Arc<Mutex<BTreeMap<EndpointId, mpsc::Sender<Packet>>>>,
    /// (发件人, 收件人) 各自一个方向
    directions: Arc<Mutex<BTreeMap<(EndpointId, EndpointId), Arc<Direction>>>>,
    link: Link,
}

impl Net {
    fn new(link: Link) -> Self {
        Net { inboxes: Default::default(), directions: Default::default(), link }
    }

    /// 取这个方向的链路；第一次用到时建，并起一个按序投递的任务
    fn direction(&self, from: EndpointId, to: EndpointId) -> Arc<Direction> {
        self.directions
            .lock()
            .unwrap()
            .entry((from, to))
            .or_insert_with(|| {
                // 到期时间只会往后走，所以先进先出，不会把包的顺序打乱
                // （每个包单独起任务延迟投递会乱序，高速率下一堆乱序包会让 QUIC 误以为丢包）
                let (delay, mut due) = mpsc::unbounded_channel::<Due>();
                tokio::spawn(async move {
                    while let Some((at, packet, inbox)) = due.recv().await {
                        sleep_until(at).await;
                        let _ = inbox.try_send(packet);
                    }
                });
                Arc::new(Direction { link_free: Mutex::new(Instant::now()), delay })
            })
            .clone()
    }

    fn transport(&self, id: EndpointId) -> Arc<LinkTransport> {
        let (tx, rx) = mpsc::channel(1 << 17);
        self.inboxes.lock().unwrap().insert(id, tx);
        Arc::new(LinkTransport { id, net: self.clone(), rx: Mutex::new(Some(rx)) })
    }
}

#[derive(Debug)]
struct LinkTransport {
    id: EndpointId,
    net: Net,
    rx: Mutex<Option<mpsc::Receiver<Packet>>>,
}

impl CustomTransport for LinkTransport {
    fn bind(&self) -> io::Result<Box<dyn CustomEndpoint>> {
        let rx = self.rx.lock().unwrap().take().ok_or_else(|| io::Error::other("已经绑过了"))?;
        Ok(Box::new(LinkEndpoint {
            id: self.id,
            net: self.net.clone(),
            rx,
            local: n0_watcher::Watchable::new(vec![addr_of(self.id)]),
        }))
    }
}

#[derive(Debug)]
struct LinkEndpoint {
    id: EndpointId,
    net: Net,
    rx: mpsc::Receiver<Packet>,
    local: n0_watcher::Watchable<Vec<CustomAddr>>,
}

impl CustomEndpoint for LinkEndpoint {
    fn watch_local_addrs(&self) -> n0_watcher::Direct<Vec<CustomAddr>> {
        self.local.watch()
    }

    fn create_sender(&self) -> Arc<dyn CustomSender> {
        Arc::new(LinkSender { id: self.id, net: self.net.clone() })
    }

    fn poll_recv(
        &mut self,
        cx: &mut Context,
        bufs: &mut [io::IoSliceMut<'_>],
        metas: &mut [noq_udp::RecvMeta],
        recv_infos: &mut [RecvInfo],
    ) -> Poll<io::Result<usize>> {
        let n = bufs.len();
        if n == 0 {
            return Poll::Ready(Ok(0));
        }
        let mut packets = Vec::new();
        match self.rx.poll_recv_many(cx, &mut packets, n) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(0) => return Poll::Ready(Err(io::Error::other("收件箱关了"))),
            Poll::Ready(_) => {}
        }
        let mut count = 0;
        for (i, packet) in packets.into_iter().enumerate() {
            if bufs[i].len() < packet.data.len() {
                break;
            }
            bufs[i][..packet.data.len()].copy_from_slice(&packet.data);
            recv_infos[i] = RecvInfo::new(packet.from, Some(addr_of(self.id)));
            metas[i].len = packet.data.len();
            metas[i].stride = packet.data.len();
            count += 1;
        }
        if count > 0 { Poll::Ready(Ok(count)) } else { Poll::Pending }
    }
}

#[derive(Debug)]
struct LinkSender {
    id: EndpointId,
    net: Net,
}

impl CustomSender for LinkSender {
    fn is_valid_send_addr(&self, addr: &CustomAddr) -> bool {
        addr.id() == LINK_TRANSPORT_ID
    }

    fn poll_send(&self, _cx: &mut Context, dst: &CustomAddr, _src: Option<&CustomAddr>, transmit: &Transmit<'_>) -> Poll<io::Result<()>> {
        let to = id_of(dst)?;
        let Some(inbox) = self.net.inboxes.lock().unwrap().get(&to).cloned() else {
            return Poll::Ready(Err(io::Error::other("没有这个收件人")));
        };
        let link = self.net.link;
        let direction = self.net.direction(self.id, to);
        let segment = transmit.segment_size.unwrap_or(transmit.contents.len()).max(1);
        for chunk in transmit.contents.chunks(segment) {
            if rand::random::<f64>() < link.loss {
                continue;
            }
            let now = Instant::now();
            let (start, tx_time) = {
                let mut free = direction.link_free.lock().unwrap();
                let start = (*free).max(now);
                if start - now > link.queue_limit {
                    continue; // 队列满了，丢尾
                }
                let tx_time = Duration::from_secs_f64(chunk.len() as f64 * 8.0 / link.rate_bps as f64);
                *free = start + tx_time;
                (start, tx_time)
            };
            let deliver_at = start + tx_time + link.one_way;
            let packet = Packet { data: Bytes::copy_from_slice(chunk), from: addr_of(self.id) };
            let _ = direction.delay.send((deliver_at, packet, inbox.clone()));
        }
        Poll::Ready(Ok(()))
    }
}

fn transport_config(cc: Cc) -> QuicTransportConfig {
    match cc {
        Cc::Cubic => QuicTransportConfig::builder().congestion_controller_factory(Arc::new(CubicConfig::default())).build(),
        Cc::Bbr3 => QuicTransportConfig::builder().congestion_controller_factory(Arc::new(Bbr3Config::default())).build(),
        Cc::Adaptive => p2pcli_core::quic::transport_config(),
    }
}

async fn endpoint(net: &Net, cc: Cc) -> Endpoint {
    let key = SecretKey::generate();
    Endpoint::builder(presets::N0)
        .secret_key(key.clone())
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .clear_ip_transports()
        .add_custom_transport(net.transport(key.public()))
        .transport_config(transport_config(cc))
        .bind()
        .await
        .unwrap()
}

/// 一组：客户端往服务端灌数据 RUN 这么久，返回稳定阶段的速率（字节每秒）和结束时用的算法
async fn measure(cc: Cc, link: Link) -> (f64, &'static str) {
    let net = Net::new(link);
    let (server, client) = (endpoint(&net, cc).await, endpoint(&net, cc).await);
    let received = Arc::new(AtomicU64::new(0));
    let sink = {
        let (server, received) = (server.clone(), received.clone());
        tokio::spawn(async move {
            let conn = server.accept().await.unwrap().accept().unwrap().await.unwrap();
            let mut recv = conn.accept_uni().await.unwrap();
            let mut buf = vec![0u8; 256 * 1024];
            while let Ok(Some(n)) = recv.read(&mut buf).await {
                received.fetch_add(n as u64, Ordering::Relaxed);
            }
        })
    };

    let addr = EndpointAddr::from_parts(server.id(), [TransportAddr::Custom(addr_of(server.id()))]);
    let conn = client.connect(addr, ALPN).await.unwrap();
    let mut send = conn.open_uni().await.unwrap();
    let chunk = vec![7u8; 256 * 1024];
    let started = Instant::now();
    let (mut at_warmup, mut warm_at) = (None::<u64>, started);
    while started.elapsed() < RUN {
        send.write_all(&chunk).await.unwrap();
        if at_warmup.is_none() && started.elapsed() >= WARMUP {
            at_warmup = Some(received.load(Ordering::Relaxed));
            warm_at = Instant::now();
        }
    }
    let end_bytes = received.load(Ordering::Relaxed);
    let end_at = Instant::now();
    let algorithm = conn
        .congestion_state(PathId::ZERO)
        .and_then(p2pcli_core::quic::algorithm)
        .unwrap_or("?");
    sink.abort();
    conn.close(0u32.into(), b"done");
    sleep(Duration::from_millis(100)).await;
    server.close().await;
    client.close().await;
    ((end_bytes - at_warmup.unwrap_or(0)) as f64 / (end_at - warm_at).as_secs_f64(), algorithm)
}

fn mbps(b: f64) -> String {
    format!("{:.1} Mbps", b * 8.0 / 1e6)
}

/// 丢包率从 0 扫到 3%：同一条跨境链路上三种做法的速度
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "几分钟的对比实验，手动跑，加 --test-threads=1"]
async fn congestion_control_on_a_lossy_long_link() {
    let base = Link { one_way: Duration::from_millis(53), loss: 0.0, rate_bps: 100_000_000, queue_limit: Duration::from_millis(100) };
    println!("\n链路：往返 {} ms，瓶颈带宽 {} Mbps，队列最多 {} ms；每组跑 {} 秒，去掉前 {} 秒慢启动",
        base.one_way.as_millis() * 2, base.rate_bps / 1_000_000, base.queue_limit.as_millis(), RUN.as_secs(), WARMUP.as_secs());
    println!("{:>8}  {:>14}  {:>14}  {:>20}", "丢包率", "Cubic（以前）", "BBR3", "自适应（现在）");
    for loss in [0.0, 0.005, 0.013, 0.03] {
        let link = Link { loss, ..base };
        let (cubic, _) = measure(Cc::Cubic, link).await;
        let (bbr, _) = measure(Cc::Bbr3, link).await;
        let (adaptive, chose) = measure(Cc::Adaptive, link).await;
        println!("{:>7.1}%  {:>14}  {:>14}  {:>14}（{chose}）", loss * 100.0, mbps(cubic), mbps(bbr), mbps(adaptive));
        assert_eq!(chose, "bbr3", "往返 106 ms 的链路应该选 BBR3");
    }
}

/// 常见的几种链路：自适应不能比 Cubic 差，该用 BBR3 的链路上要接近 BBR3
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "几分钟的对比实验，手动跑，加 --test-threads=1"]
async fn congestion_control_on_typical_links() {
    let q = Duration::from_millis(100);
    let cases = [
        ("局域网 2ms 300M 无丢包", Link { one_way: Duration::from_millis(1), loss: 0.0, rate_bps: 300_000_000, queue_limit: q }, "cubic"),
        ("局域网 2ms 300M 丢包0.1%", Link { one_way: Duration::from_millis(1), loss: 0.001, rate_bps: 300_000_000, queue_limit: q }, "cubic"),
        ("同城 10ms 300M 无丢包", Link { one_way: Duration::from_millis(5), loss: 0.0, rate_bps: 300_000_000, queue_limit: q }, "cubic"),
        ("国内宽带 30ms 100M 丢包0.1%", Link { one_way: Duration::from_millis(15), loss: 0.001, rate_bps: 100_000_000, queue_limit: q }, "bbr3"),
        ("跨境 106ms 300M 丢包0.1%", Link { one_way: Duration::from_millis(53), loss: 0.001, rate_bps: 300_000_000, queue_limit: q }, "bbr3"),
        ("跨境 106ms 50M 丢包1.3%", Link { one_way: Duration::from_millis(53), loss: 0.013, rate_bps: 50_000_000, queue_limit: q }, "bbr3"),
    ];
    println!("\n{:<28}{:>14}{:>14}{:>20}", "链路", "Cubic（以前）", "BBR3", "自适应（现在）");
    for (name, link, expect) in cases {
        let (a, _) = measure(Cc::Cubic, link).await;
        let (b, _) = measure(Cc::Bbr3, link).await;
        let (c, chose) = measure(Cc::Adaptive, link).await;
        println!("{name:<28}{:>14}{:>14}{:>14}（{chose}）", mbps(a), mbps(b), mbps(c));
        assert_eq!(chose, expect, "{name} 选错了算法");
    }
}

/// 并发：同一条连接里开多条流（共用一个拥塞窗口），或者开多条独立连接（各有各的窗口），灌数据 RUN 这么久，
/// 返回总的稳定速率。一条链路上所有连接共用同一个带宽和队列。
async fn measure_parallel(cc: Cc, link: Link, connections: usize, streams: usize) -> f64 {
    let net = Net::new(link);
    let (server, client) = (endpoint(&net, cc).await, endpoint(&net, cc).await);
    let received = Arc::new(AtomicU64::new(0));
    let accept = {
        let (server, received) = (server.clone(), received.clone());
        tokio::spawn(async move {
            let mut readers = Vec::new();
            for _ in 0..connections {
                let conn = server.accept().await.unwrap().accept().unwrap().await.unwrap();
                let received = received.clone();
                readers.push(tokio::spawn(async move {
                    let mut tasks = Vec::new();
                    for _ in 0..streams {
                        let mut recv = conn.accept_uni().await.unwrap();
                        let received = received.clone();
                        tasks.push(tokio::spawn(async move {
                            let mut buf = vec![0u8; 256 * 1024];
                            while let Ok(Some(n)) = recv.read(&mut buf).await {
                                received.fetch_add(n as u64, Ordering::Relaxed);
                            }
                        }));
                    }
                    for t in tasks {
                        let _ = t.await;
                    }
                    conn
                }));
            }
            for r in readers {
                let _ = r.await;
            }
        })
    };
    let addr = EndpointAddr::from_parts(server.id(), [TransportAddr::Custom(addr_of(server.id()))]);
    let started = Instant::now();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut senders = Vec::new();
    let mut conns = Vec::new();
    for _ in 0..connections {
        let conn = client.connect(addr.clone(), ALPN).await.unwrap();
        for _ in 0..streams {
            let mut send = conn.open_uni().await.unwrap();
            let stop = stop.clone();
            senders.push(tokio::spawn(async move {
                let chunk = vec![7u8; 256 * 1024];
                while !stop.load(Ordering::Relaxed) {
                    if send.write_all(&chunk).await.is_err() {
                        break;
                    }
                }
            }));
        }
        conns.push(conn);
    }
    sleep(WARMUP).await;
    let at_warmup = received.load(Ordering::Relaxed);
    let warm_at = Instant::now();
    sleep(RUN.saturating_sub(started.elapsed())).await;
    let end_bytes = received.load(Ordering::Relaxed);
    let end_at = Instant::now();
    stop.store(true, Ordering::Relaxed);
    for c in &conns {
        c.close(0u32.into(), b"done");
    }
    accept.abort();
    for s in senders {
        s.abort();
    }
    sleep(Duration::from_millis(100)).await;
    server.close().await;
    client.close().await;
    (end_bytes - at_warmup) as f64 / (end_at - warm_at).as_secs_f64()
}

/// 并发有没有用：一条流、同一条连接多条流、多条连接，各自在几种链路上的总速度
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "几分钟的对比实验，手动跑，加 --test-threads=1"]
async fn parallel_streams_and_connections() {
    let q = Duration::from_millis(100);
    let cases = [
        ("跨境 106ms 100M 丢包1.3%", Link { one_way: Duration::from_millis(53), loss: 0.013, rate_bps: 100_000_000, queue_limit: q }),
        ("跨境 106ms 300M 丢包0.1%", Link { one_way: Duration::from_millis(53), loss: 0.001, rate_bps: 300_000_000, queue_limit: q }),
        ("国内 30ms 100M 丢包0.1%", Link { one_way: Duration::from_millis(15), loss: 0.001, rate_bps: 100_000_000, queue_limit: q }),
    ];
    println!("\n{:<26}{:>6}  {:>12}{:>12}{:>12}{:>12}", "链路", "算法", "1连接1流", "1连接4流", "4连接各1流", "4连接各2流");
    for (name, link) in cases {
        for (cc, label) in [(Cc::Cubic, "Cubic"), (Cc::Adaptive, "自适应")] {
            let a = measure_parallel(cc, link, 1, 1).await;
            let b = measure_parallel(cc, link, 1, 4).await;
            let c = measure_parallel(cc, link, 4, 1).await;
            let d = measure_parallel(cc, link, 4, 2).await;
            println!("{name:<26}{label:>6}  {:>12}{:>12}{:>12}{:>12}", mbps(a), mbps(b), mbps(c), mbps(d));
        }
    }
}

/// 什么都同意的被连方：这个实验只量传文件的速度
struct AllowAll;

impl HostDelegate for AllowAll {
    fn approve(&self, _req: IncomingRequest) -> BoxFuture<Decision> {
        Box::pin(async { Decision::Allow { trust: false } })
    }
    fn event(&self, _ev: HostEvent) {}
    fn approve_files(&self, _req: FileRequest) -> BoxFuture<bool> {
        Box::pin(async { true })
    }
}

/// 在模拟链路上，用真正的 Node 和真正的传文件代码，发一个文件，返回耗时（秒）和用了几条流
async fn transfer_over(link: Link, lanes: Option<u32>, megabytes: usize) -> (f64, u32) {
    let net = Net::new(link);
    let root = std::env::temp_dir().join(format!("p2pcli-lossy-{}-{}", std::process::id(), rand::random::<u32>()));
    let node = |tag: &str, name: &str, lanes: Option<u32>| {
        let dir = root.join(tag);
        std::fs::create_dir_all(&dir).unwrap();
        let key = p2pcli_core::identity::load_or_create_key(&dir).unwrap();
        let transport = net.transport(key.public());
        let (name, inbox) = (name.to_string(), root.join(format!("{tag}-inbox")));
        async move {
            Node::start(
                NodeConfig {
                    data_dir: dir,
                    device_name: Some(name),
                    offline: true,
                    relay_only: true,
                    inbox_dir: Some(inbox),
                    file_lanes: lanes,
                    custom_transport: Some(transport),
                    ..Default::default()
                },
                Arc::new(AllowAll),
            )
            .await
            .unwrap()
        }
    };
    let host = node("host", "收件机", None).await;
    let sender = node("sender", "发件机", lanes).await;
    host.host.set_password("abc234");
    let addr = EndpointAddr::from_parts(host.id(), [TransportAddr::Custom(addr_of(host.id()))]);
    sender.connect_addr(addr.clone(), Some("abc234".into())).await.expect("应该连上").close();

    let file = root.join("big.bin");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&file).unwrap();
        let block: Vec<u8> = (0..1_048_576u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        for _ in 0..megabytes {
            f.write_all(&block).unwrap();
        }
    }
    let started = Instant::now();
    let t = sender.send_files(addr, "收件机", vec![file.clone()]).await;
    let secs = started.elapsed().as_secs_f64();
    assert_eq!(t.state, p2pcli_core::files::TransferState::Done, "{:?}", t.error);
    let got = std::fs::metadata(root.join("host-inbox/发件机/big.bin")).unwrap().len();
    assert_eq!(got as usize, megabytes << 20, "收到的大小不对");
    host.shutdown().await;
    sender.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
    (secs, t.lanes)
}

/// 真实的传文件代码在模拟链路上：一条流（旧版本对端、或者关了并发）对比并发分段
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "几分钟的对比实验，手动跑，加 --test-threads=1"]
async fn file_transfer_over_lossy_links() {
    let q = Duration::from_millis(100);
    let cases = [
        ("跨境 106ms 100M 丢包1.3%", Link { one_way: Duration::from_millis(53), loss: 0.013, rate_bps: 100_000_000, queue_limit: q }, 64),
        ("跨境 106ms 300M 丢包0.1%", Link { one_way: Duration::from_millis(53), loss: 0.001, rate_bps: 300_000_000, queue_limit: q }, 160),
        ("国内 30ms 100M 丢包0.1%", Link { one_way: Duration::from_millis(15), loss: 0.001, rate_bps: 100_000_000, queue_limit: q }, 64),
        ("局域网 2ms 300M 无丢包", Link { one_way: Duration::from_millis(1), loss: 0.0, rate_bps: 300_000_000, queue_limit: q }, 160),
    ];
    println!("\n{:<28}{:>6}  {:>16}{:>22}", "链路", "大小", "一条流", "并发分段");
    for (name, link, mb) in cases {
        let (single, _) = transfer_over(link, Some(0), mb).await;
        let (parallel, lanes) = transfer_over(link, None, mb).await;
        let rate = |secs: f64| format!("{:.1} Mbps", mb as f64 * 8.0 * 1.048576 / secs);
        println!("{name:<28}{:>4}M  {:>16}{:>16}（{lanes} 条流，快 {:.1} 倍）", mb, rate(single), rate(parallel), single / parallel);
    }
}

/// 看一次传输从头到尾的时间线：每半秒记一次“发送方认为发了多少”和“接收方真收到多少”，
/// 用来看收尾阶段有没有卡住、两边的进度差多少
async fn transfer_timeline(link: Link, lanes: Option<u32>, megabytes: usize) -> Vec<(f64, u64, u64)> {
    let net = Net::new(link);
    let root = std::env::temp_dir().join(format!("p2pcli-timeline-{}-{}", std::process::id(), rand::random::<u32>()));
    let node = |tag: &str, name: &str, lanes: Option<u32>| {
        let dir = root.join(tag);
        std::fs::create_dir_all(&dir).unwrap();
        let key = p2pcli_core::identity::load_or_create_key(&dir).unwrap();
        let transport = net.transport(key.public());
        let (name, inbox) = (name.to_string(), root.join(format!("{tag}-inbox")));
        async move {
            Node::start(
                NodeConfig { data_dir: dir, device_name: Some(name), offline: true, relay_only: true, inbox_dir: Some(inbox), file_lanes: lanes, custom_transport: Some(transport), ..Default::default() },
                Arc::new(AllowAll),
            )
            .await
            .unwrap()
        }
    };
    let host = node("host", "收件机", None).await;
    let sender = node("sender", "发件机", lanes).await;
    host.host.set_password("abc234");
    let addr = EndpointAddr::from_parts(host.id(), [TransportAddr::Custom(addr_of(host.id()))]);
    sender.connect_addr(addr.clone(), Some("abc234".into())).await.expect("应该连上").close();
    let file = root.join("big.bin");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&file).unwrap();
        let block: Vec<u8> = (0..1_048_576u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        for _ in 0..megabytes {
            f.write_all(&block).unwrap();
        }
    }
    let started = Instant::now();
    let send = {
        let (sender, file) = (sender.clone(), file.clone());
        tokio::spawn(async move { sender.send_files(addr, "收件机", vec![file]).await })
    };
    let mut timeline = Vec::new();
    while !send.is_finished() {
        sleep(Duration::from_millis(500)).await;
        let sent = sender.files.transfers().first().map_or(0, |t| t.done);
        let got = host.files.transfers().first().map_or(0, |t| t.done);
        timeline.push((started.elapsed().as_secs_f64(), sent, got));
    }
    let t = send.await.unwrap();
    println!("  发送方诊断：{}", sender.files.transfers().first().map_or(String::new(), |t| t.diag.clone()));
    println!("  接收方诊断：{}", host.files.transfers().first().map_or(String::new(), |t| t.diag.clone()));
    timeline.push((started.elapsed().as_secs_f64(), t.done, host.files.transfers().first().map_or(0, |t| t.done)));
    assert_eq!(t.state, p2pcli_core::files::TransferState::Done, "{:?}", t.error);
    host.shutdown().await;
    sender.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
    timeline
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "手动跑，看一次传输的时间线"]
async fn transfer_timeline_on_a_clean_domestic_link() {
    let q = Duration::from_millis(100);
    // 往返 40 ms、几乎不丢包、20 Mbps 带宽：和一次国内家宽之间的传输差不多
    let link = Link { one_way: Duration::from_millis(20), loss: 0.0, rate_bps: 20_000_000, queue_limit: q };
    for (label, lanes) in [("并发分段", None), ("一条流", Some(0))] {
        let tl = transfer_timeline(link, lanes, 56).await;
        println!("\n{label}：56 MiB，往返 40 ms，20 Mbps，无丢包；总用时 {:.1} 秒", tl.last().unwrap().0);
        println!("{:>6}  {:>16}  {:>16}  {:>10}", "时间", "发送方显示已发", "接收方真收到", "差");
        // 发送方显示的进度不能比接收方真收到的多：是对方回报的，最多落后半秒
        if lanes != Some(0) {
            for (t, sent, got) in &tl {
                assert!(*sent <= *got + (1 << 20), "{t:.1}s 发送方显示 {sent}，接收方只收到 {got}，显示跑到了前面");
            }
        }
        for (t, sent, got) in tl.iter().step_by(2) {
            println!("{t:>5.1}s  {:>13.1} MiB  {:>13.1} MiB  {:>7.1} MiB", *sent as f64 / 1048576.0, *got as f64 / 1048576.0, (*sent as f64 - *got as f64) / 1048576.0);
        }
    }
}

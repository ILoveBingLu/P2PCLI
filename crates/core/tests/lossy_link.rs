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
use p2pcli_core::quic::Mode;
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
    /// 偶尔成串丢包：不随机丢，隔一阵子连着丢几个
    burst: Option<Burst>,
}

/// 成串丢包：每隔 every_min 到 every_max 之间的随机时长，连着丢 1 到 max_len 个数据包
/// （只丢大包，确认包这种小包不丢，像真实链路上拥塞的那个方向）
#[derive(Clone, Copy, Debug)]
struct Burst {
    every_min: Duration,
    every_max: Duration,
    max_len: u32,
}

/// 一个方向上成串丢包的进度
#[derive(Debug)]
struct BurstState {
    /// 下一串什么时候开始
    next_at: Instant,
    /// 这一串还要丢几个
    left: u32,
}

impl Burst {
    fn gap(&self) -> Duration {
        self.every_min + (self.every_max - self.every_min).mul_f64(rand::random::<f64>())
    }
}

#[derive(Clone, Copy, Debug)]
enum Cc {
    /// noq 原版的 Cubic，不经过产品里那层包装
    Cubic,
    /// noq 原版的 BBR3
    Bbr3,
    /// 产品里实际用的默认做法（现在固定 Cubic，不看环境变量 P2PCLI_CC）
    Default,
    /// 以前的做法：按链路延迟选，往返 20 ms 以上用 BBR3，留着对比
    Auto,
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
    burst: Mutex<BurstState>,
}

/// 内存里的网络：谁发给谁，都经过同一条带损伤的链路
#[derive(Debug, Clone)]
struct Net {
    inboxes: Arc<Mutex<BTreeMap<EndpointId, mpsc::Sender<Packet>>>>,
    /// (发件人, 收件人) 各自一个方向
    directions: Arc<Mutex<BTreeMap<(EndpointId, EndpointId), Arc<Direction>>>>,
    link: Link,
    /// 成串丢包一共丢了几个（所有方向加起来）
    burst_dropped: Arc<AtomicU64>,
}

impl Net {
    fn new(link: Link) -> Self {
        Net { inboxes: Default::default(), directions: Default::default(), link, burst_dropped: Default::default() }
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
                let first = self.link.burst.map_or(Duration::ZERO, |b| b.gap());
                let burst = Mutex::new(BurstState { next_at: Instant::now() + first, left: 0 });
                Arc::new(Direction { link_free: Mutex::new(Instant::now()), delay, burst })
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
            if let Some(b) = link.burst.filter(|_| chunk.len() >= 1000) {
                let mut state = direction.burst.lock().unwrap();
                if state.left == 0 && now >= state.next_at {
                    state.left = 1 + rand::random::<u32>() % b.max_len;
                    state.next_at = now + b.gap();
                }
                if state.left > 0 {
                    state.left -= 1;
                    self.net.burst_dropped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            }
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
        Cc::Default => p2pcli_core::quic::transport_config_with(Mode::default()),
        Cc::Auto => p2pcli_core::quic::transport_config_with(Mode::Auto),
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

/// 丢包率从 0 扫到 3%：同一条跨境链路上现在的默认（Cubic）、BBR3、以前按延迟选的做法各自的速度。
/// 均匀随机丢包下 BBR3 远比 Cubic 快，但真实跨境链路上 noq 的 BBR3 会越跑越慢，所以默认没用它，
/// 见 src/quic.rs 和下面的 bbr3_collapse_after_sparse_loss
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "几分钟的对比实验，手动跑，加 --test-threads=1"]
async fn congestion_control_on_a_lossy_long_link() {
    let base = Link { one_way: Duration::from_millis(53), loss: 0.0, rate_bps: 100_000_000, queue_limit: Duration::from_millis(100), burst: None };
    println!("\n链路：往返 {} ms，瓶颈带宽 {} Mbps，队列最多 {} ms；每组跑 {} 秒，去掉前 {} 秒慢启动",
        base.one_way.as_millis() * 2, base.rate_bps / 1_000_000, base.queue_limit.as_millis(), RUN.as_secs(), WARMUP.as_secs());
    println!("{:>8}  {:>20}  {:>14}  {:>22}", "丢包率", "默认（现在）", "BBR3", "按延迟选（以前）");
    for loss in [0.0, 0.005, 0.013, 0.03] {
        let link = Link { loss, ..base };
        let (default, now) = measure(Cc::Default, link).await;
        let (bbr, _) = measure(Cc::Bbr3, link).await;
        let (auto, before) = measure(Cc::Auto, link).await;
        println!("{:>7.1}%  {:>14}（{now}）  {:>14}  {:>14}（{before}）", loss * 100.0, mbps(default), mbps(bbr), mbps(auto));
        assert_eq!(now, "cubic", "默认应该一直是 Cubic");
        assert_eq!(before, "bbr3", "以前的做法在往返 106 ms 的链路上会选 BBR3");
    }
}

/// 常见的几种链路：现在的默认（一直 Cubic）、BBR3、以前按延迟选的做法各自的速度；
/// 默认不管什么链路都该是 Cubic，以前的做法在往返 20 ms 以上会换成 BBR3
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "几分钟的对比实验，手动跑，加 --test-threads=1"]
async fn congestion_control_on_typical_links() {
    let q = Duration::from_millis(100);
    let cases = [
        ("局域网 2ms 300M 无丢包", Link { one_way: Duration::from_millis(1), loss: 0.0, rate_bps: 300_000_000, queue_limit: q, burst: None }, "cubic"),
        ("局域网 2ms 300M 丢包0.1%", Link { one_way: Duration::from_millis(1), loss: 0.001, rate_bps: 300_000_000, queue_limit: q, burst: None }, "cubic"),
        ("同城 10ms 300M 无丢包", Link { one_way: Duration::from_millis(5), loss: 0.0, rate_bps: 300_000_000, queue_limit: q, burst: None }, "cubic"),
        ("国内宽带 30ms 100M 丢包0.1%", Link { one_way: Duration::from_millis(15), loss: 0.001, rate_bps: 100_000_000, queue_limit: q, burst: None }, "bbr3"),
        ("跨境 106ms 300M 丢包0.1%", Link { one_way: Duration::from_millis(53), loss: 0.001, rate_bps: 300_000_000, queue_limit: q, burst: None }, "bbr3"),
        ("跨境 106ms 50M 丢包1.3%", Link { one_way: Duration::from_millis(53), loss: 0.013, rate_bps: 50_000_000, queue_limit: q, burst: None }, "bbr3"),
    ];
    println!("\n{:<28}{:>20}{:>14}{:>22}", "链路", "默认（现在）", "BBR3", "按延迟选（以前）");
    for (name, link, expect) in cases {
        let (a, now) = measure(Cc::Default, link).await;
        let (b, _) = measure(Cc::Bbr3, link).await;
        let (c, before) = measure(Cc::Auto, link).await;
        println!("{name:<28}{:>14}（{now}）{:>14}{:>14}（{before}）", mbps(a), mbps(b), mbps(c));
        assert_eq!(now, "cubic", "{name}：默认应该一直是 Cubic");
        assert_eq!(before, expect, "{name}：以前的做法选错了算法");
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
        ("跨境 106ms 100M 丢包1.3%", Link { one_way: Duration::from_millis(53), loss: 0.013, rate_bps: 100_000_000, queue_limit: q, burst: None }),
        ("跨境 106ms 300M 丢包0.1%", Link { one_way: Duration::from_millis(53), loss: 0.001, rate_bps: 300_000_000, queue_limit: q, burst: None }),
        ("国内 30ms 100M 丢包0.1%", Link { one_way: Duration::from_millis(15), loss: 0.001, rate_bps: 100_000_000, queue_limit: q, burst: None }),
    ];
    println!("\n{:<26}{:>6}  {:>12}{:>12}{:>12}{:>12}", "链路", "算法", "1连接1流", "1连接4流", "4连接各1流", "4连接各2流");
    for (name, link) in cases {
        for (cc, label) in [(Cc::Default, "默认"), (Cc::Bbr3, "BBR3")] {
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
        ("跨境 106ms 100M 丢包1.3%", Link { one_way: Duration::from_millis(53), loss: 0.013, rate_bps: 100_000_000, queue_limit: q, burst: None }, 64),
        ("跨境 106ms 300M 丢包0.1%", Link { one_way: Duration::from_millis(53), loss: 0.001, rate_bps: 300_000_000, queue_limit: q, burst: None }, 160),
        ("国内 30ms 100M 丢包0.1%", Link { one_way: Duration::from_millis(15), loss: 0.001, rate_bps: 100_000_000, queue_limit: q, burst: None }, 64),
        ("局域网 2ms 300M 无丢包", Link { one_way: Duration::from_millis(1), loss: 0.0, rate_bps: 300_000_000, queue_limit: q, burst: None }, 160),
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
    let link = Link { one_way: Duration::from_millis(20), loss: 0.0, rate_bps: 20_000_000, queue_limit: q, burst: None };
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

/// 时间线上的一秒（看发送方那一头）
#[derive(Clone, Copy, Debug, Default)]
struct Tick {
    /// 这一秒接收方收到多少字节
    got: u64,
    cwnd: u64,
    /// 带宽估计（bit/s），只有 BBR3 有
    bw: Option<u64>,
    rtt: Duration,
    /// 这一秒发送方判定丢了几个包
    lost: u64,
    /// 这一秒在传大文件（否则在发远程画面那样的小帧）
    bulk: bool,
}

/// 发送方式
#[derive(Clone, Copy, Debug)]
enum Pattern {
    /// 一直灌数据，灌这么久
    Bulk(Duration),
    /// 像远程画面那样每秒 60 个小帧（每帧 frame_kb.0~frame_kb.1 KB）发 app 这么久，再传一个 bulk_mb MB 的大文件，
    /// 重复 rounds 次；file_first 就每轮先传文件再发画面
    ScreenThenFile { app: Duration, frame_kb: (usize, usize), bulk_mb: usize, rounds: usize, file_first: bool },
}

/// 按 pattern 发一遍，返回每秒的时间线、每次传大文件的耗时（秒）、链路成串丢了几个包
async fn timeline(cc: Cc, link: Link, pattern: Pattern) -> (Vec<Tick>, Vec<f64>, u64) {
    let net = Net::new(link);
    let (server, client) = (endpoint(&net, cc).await, endpoint(&net, cc).await);
    let received = Arc::new(AtomicU64::new(0));
    // 每条流收完了报一声（远程画面那条流不会收完）
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<u64>();
    let sink = {
        let (server, received) = (server.clone(), received.clone());
        tokio::spawn(async move {
            let conn = server.accept().await.unwrap().accept().unwrap().await.unwrap();
            while let Ok(mut recv) = conn.accept_uni().await {
                let (received, done_tx) = (received.clone(), done_tx.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 256 * 1024];
                    let mut total = 0u64;
                    while let Ok(Some(n)) = recv.read(&mut buf).await {
                        received.fetch_add(n as u64, Ordering::Relaxed);
                        total += n as u64;
                    }
                    let _ = done_tx.send(total);
                });
            }
        })
    };
    let addr = EndpointAddr::from_parts(server.id(), [TransportAddr::Custom(addr_of(server.id()))]);
    let conn = client.connect(addr, ALPN).await.unwrap();

    // 每秒记一行
    let bulk = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let monitor = {
        let (conn, received, bulk) = (conn.clone(), received.clone(), bulk.clone());
        tokio::spawn(async move {
            let mut ticks = Vec::new();
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.tick().await;
            let (mut last_got, mut last_lost) = (0u64, 0u64);
            loop {
                tick.tick().await;
                let Some(s) = conn.paths().iter().next().map(|p| p.stats()) else { break };
                let got = received.load(Ordering::Relaxed);
                ticks.push(Tick {
                    got: got - last_got,
                    cwnd: s.cwnd,
                    bw: s.bandwidth_estimate,
                    rtt: s.rtt,
                    lost: s.lost_packets - last_lost,
                    bulk: bulk.load(Ordering::Relaxed),
                });
                (last_got, last_lost) = (got, s.lost_packets);
                if conn.close_reason().is_some() {
                    break;
                }
            }
            ticks
        })
    };

    let chunk = vec![7u8; 256 * 1024];
    let mut bulk_secs = Vec::new();
    match pattern {
        Pattern::Bulk(run) => {
            bulk.store(true, Ordering::Relaxed);
            let mut send = conn.open_uni().await.unwrap();
            let started = Instant::now();
            while started.elapsed() < run {
                send.write_all(&chunk).await.unwrap();
            }
        }
        Pattern::ScreenThenFile { app, frame_kb: (min_kb, max_kb), bulk_mb, rounds, file_first } => {
            let mut frames = conn.open_uni().await.unwrap();
            let frame = vec![3u8; max_kb * 1024];
            // 每一步：真是传文件，假是发画面
            let steps = (0..rounds).flat_map(|_| if file_first { [true, false] } else { [false, true] });
            for file in steps {
                if !file {
                    let mut every = tokio::time::interval(Duration::from_micros(16_667));
                    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    let started = Instant::now();
                    while started.elapsed() < app {
                        every.tick().await;
                        let size = min_kb * 1024 + rand::random::<u32>() as usize % ((max_kb - min_kb) * 1024 + 1);
                        frames.write_all(&frame[..size]).await.unwrap();
                    }
                    continue;
                }
                bulk.store(true, Ordering::Relaxed);
                let started = Instant::now();
                let mut send = conn.open_uni().await.unwrap();
                for _ in 0..bulk_mb * 4 {
                    send.write_all(&chunk).await.unwrap();
                }
                send.finish().unwrap();
                let got = done_rx.recv().await.unwrap();
                assert_eq!(got as usize, bulk_mb << 20, "大文件收到的大小不对");
                bulk_secs.push(started.elapsed().as_secs_f64());
                bulk.store(false, Ordering::Relaxed);
            }
        }
    }
    // 让监视再记最后一秒
    sleep(Duration::from_millis(1100)).await;
    conn.close(0u32.into(), b"done");
    let ticks = monitor.await.unwrap();
    sink.abort();
    sleep(Duration::from_millis(100)).await;
    server.close().await;
    client.close().await;
    (ticks, bulk_secs, net.burst_dropped.load(Ordering::Relaxed))
}

/// 一行里一种算法的那几列：速率、窗口、带宽估计、延迟、丢包，画面阶段在前面标“画”
fn tick_cols(t: Option<&Tick>) -> String {
    let Some(t) = t else { return format!("{:>46}", "") };
    format!(
        "{}{:>6.1}M {:>6}K {:>7} {:>4}ms {:>3}",
        if t.bulk { "传" } else { "画" },
        t.got as f64 * 8.0 / 1e6,
        t.cwnd / 1024,
        t.bw.map_or("-".into(), |b| format!("{:.1}M", b as f64 / 1e6)),
        t.rtt.as_millis(),
        t.lost,
    )
}

/// 同一条链路、同一种发法，Cubic 和 BBR3 的时间线并排打出来
async fn compare_timelines(name: &str, link: Link, pattern: Pattern) {
    let (cubic, cubic_bulk, cubic_drops) = timeline(Cc::Cubic, link, pattern).await;
    let (bbr, bbr_bulk, bbr_drops) = timeline(Cc::Bbr3, link, pattern).await;
    println!("\n{name}：往返 {} ms，{} Mbps，队列最多 {} ms，{:?}，{:?}",
        link.one_way.as_millis() * 2, link.rate_bps / 1_000_000, link.queue_limit.as_millis(), link.burst, pattern);
    println!("每秒一行：传/画 = 在传大文件/在发画面小帧；速率 窗口 带宽估计 延迟 这一秒丢包数");
    println!("{:>4}  {:<46}  {:<46}", "秒", "Cubic", "BBR3");
    for i in 0..cubic.len().max(bbr.len()) {
        println!("{:>4}  {}  {}", i + 1, tick_cols(cubic.get(i)), tick_cols(bbr.get(i)));
    }
    let secs = |v: &[f64]| v.iter().map(|s| format!("{s:.1}s")).collect::<Vec<_>>().join(" ");
    if !cubic_bulk.is_empty() {
        println!("每次传大文件的耗时：Cubic {}；BBR3 {}", secs(&cubic_bulk), secs(&bbr_bulk));
    }
    println!("链路成串丢的包：Cubic 那次 {cubic_drops} 个，BBR3 那次 {bbr_drops} 个");
}

/// 想复现真实跨境链路上 BBR3 越跑越慢：往返 80 ms、100 Mbps，不随机丢包，只是隔 5~10 秒连着丢 1~6 个包。
/// 一种一直灌数据，一种像先看远程画面（小帧，app 受限）、再拉一个大文件，来回几次。
/// 实测这两种都没复现：BBR3 一直 100 Mbps 上下，反倒是 Cubic 在画面和文件交替时窗口被零星丢包一路压小。
/// 复现出来的是下面 bbr3_collapse_variations 的第二种
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "几分钟的对比实验，手动跑，加 --release --test-threads=1"]
async fn bbr3_collapse_after_sparse_loss() {
    let link = Link {
        one_way: Duration::from_millis(40),
        loss: 0.0,
        rate_bps: 100_000_000,
        queue_limit: Duration::from_millis(50),
        burst: Some(Burst { every_min: Duration::from_secs(5), every_max: Duration::from_secs(10), max_len: 6 }),
    };
    compare_timelines("一直灌数据", link, Pattern::Bulk(Duration::from_secs(60))).await;
    compare_timelines("画面和大文件交替", link, Pattern::ScreenThenFile { app: Duration::from_secs(20), frame_kb: (2, 20), bulk_mb: 20, rounds: 3, file_first: false }).await;
}

/// 上面那个没复现时试的几种变化：浅队列（零星丢包多半是队列溢出）；先传文件把带宽估计拉满，
/// 再发一分钟大一些的画面帧（1~50 KB），期间成串丢包更频繁；然后再传文件。
///
/// 浅队列：BBR3 一开始的慢启动撞上丢尾以后只有 30~45 Mbps，慢慢往上爬，Cubic 60~90 Mbps；不算“越跑越慢”。
/// 第二种复现了，但不是每次：没改的 noq 1.4.0 跑 7 次有 3 次在发画面时带宽估计从 101 Mbps 一下掉到 10~16 Mbps，
/// 之后丢包不多、延迟不变也一路滑到 2~4 Mbps，接着传 20 MB 用了 40 秒（正常 2.5 秒），画面也跟不上。
/// 怀疑的原因（还没向 noq 报）：bbr3/mod.rs 的 adapt_long_term_model 在 ack_phase 是 ProbeStopping 时
/// 每一轮都推进一次 max_bw 过滤器，而 ack_phase 要到下一次 Refill 才改，于是 DOWN/CRUISE 期间每轮都推进，
/// 本该是“两个探测周期”的窗口缩成了两三轮，画面这种 app 受限的小样本一混进来就把带宽估计冲掉。
/// Linux 的 BBR 推进一次以后就把 ack_phase 改回 ACKS_INIT；本地照这样改一份 noq 再跑 6 次，一次都没掉
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "几分钟的对比实验，手动跑，加 --release --test-threads=1"]
async fn bbr3_collapse_variations() {
    let base = Link {
        one_way: Duration::from_millis(40),
        loss: 0.0,
        rate_bps: 100_000_000,
        queue_limit: Duration::from_millis(50),
        burst: Some(Burst { every_min: Duration::from_secs(5), every_max: Duration::from_secs(10), max_len: 6 }),
    };
    let shallow = Link { queue_limit: Duration::from_millis(8), ..base };
    compare_timelines("浅队列 8 ms，一直灌数据", shallow, Pattern::Bulk(Duration::from_secs(45))).await;
    let frequent = Link { burst: Some(Burst { every_min: Duration::from_secs(2), every_max: Duration::from_secs(5), max_len: 6 }), ..base };
    let pattern = Pattern::ScreenThenFile { app: Duration::from_secs(60), frame_kb: (1, 50), bulk_mb: 20, rounds: 2, file_first: true };
    compare_timelines("先传文件，再发一分钟画面，丢得更勤", frequent, pattern).await;
}

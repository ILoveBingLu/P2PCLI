//! 并发分段传输：把要发的文件切成固定大小的段，开多条单向流（车道）一起发，每段带自己的校验。
//!
//! 为什么要这样：一条流一次只有一个流量窗口，丢一个包整条流就要等重传，窗口被占满发送就停；
//! 往返一百多毫秒、丢包 1% 上下的跨境链路上，这两样加起来把单流压在窗口的一小部分里。
//! 多条流各有各的窗口、各等各的重传，互不拖累（实测见 tests/lossy_link.rs）。
//! 每条流的接收缓冲只能容纳 1024 个不连续的数据块，所以不能靠放大单条流的窗口来提速，多条流才安全。
//!
//! 流程：发送方在 Offer 里说最多想开几条流；接收方（新版）回 LaneAccept：开几条、每段多大、哪些段已经收好；
//! 发送方按段切好放进共享队列，每条车道自己领段发：`Seg` 头、数据帧、`SegEnd` 带这一段的 BLAKE3 哈希；
//! 接收方按偏移写进预先分配好的半截文件，段校验通过才算收好，记进旁边的进度文件，断了能接着发；
//! 发送方所有车道发完，在控制流上发 `Finish`，接收方确认每段都有了，放到正式位置，回 `Saved`。
//! 对方是旧版本（回的是 Accept）就退回原来的一条流，不受影响。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use iroh::endpoint::PathId;
use tokio::task::JoinSet;

use super::*;
use crate::proto::{KIND_MSG, MAX_FRAME};

/// 每段多大：并发和续传的单位
pub(super) const SEGMENT: u64 = 8 << 20;
/// 对方要的分段大小最多多大，防对面塞怪值
const MAX_SEGMENT: u64 = 64 << 20;
/// 接收方这么久没收到任何数据就当对方掉线了
const STALL: Duration = Duration::from_secs(90);
/// 等对方核对完、放到正式位置：大文件跨磁盘挪动可能要很久
const FINAL_WAIT: Duration = Duration::from_secs(15 * 60);

/// 在控制流上读消息。读到一半被别的事打断也不丢数据，所以可以放心在 select! 里反复调 next：
/// 一条条消息陆续来（进度回报、最后的 Saved），不能像 read_json 那样被打断就把半条消息丢了
struct Control<'a> {
    recv: &'a mut RecvStream,
    buf: Vec<u8>,
}

impl<'a> Control<'a> {
    fn new(recv: &'a mut RecvStream) -> Self {
        Control { recv, buf: Vec::new() }
    }

    async fn next(&mut self) -> Result<FileMsg> {
        loop {
            if self.buf.len() >= 5 {
                let len = u32::from_be_bytes(self.buf[1..5].try_into().expect("够 4 字节")) as usize;
                if self.buf[0] != KIND_MSG || len > MAX_FRAME {
                    bail!("控制消息的格式不对");
                }
                if self.buf.len() >= 5 + len {
                    let msg = serde_json::from_slice(&self.buf[5..5 + len])?;
                    self.buf.drain(..5 + len);
                    return Ok(msg);
                }
            }
            let mut chunk = [0u8; 4096];
            // QUIC 流的 read 被打断时不会丢数据，可以放心在 select! 里用
            let Some(n) = self.recv.read(&mut chunk).await.map_err(|e| anyhow!("{e}"))? else {
                bail!("连接已关闭");
            };
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
}

/// 各条车道的用时分布（纳秒），查慢的时候看时间花在哪
#[derive(Default)]
struct Times {
    read: AtomicU64,
    hash: AtomicU64,
    write: AtomicU64,
}

/// 这一段活干了多久，记进对应的计数
fn spent(counter: &AtomicU64, since: Instant) {
    counter.fetch_add(since.elapsed().as_nanos() as u64, Ordering::Relaxed);
}

/// 几项用时各占百分之几
fn shares(parts: &[(&str, u64)]) -> String {
    let total = parts.iter().map(|p| p.1).sum::<u64>().max(1);
    parts.iter().map(|(name, ns)| format!("{name} {}%", ns * 100 / total)).collect::<Vec<_>>().join(" · ")
}

/// 一段要发的数据
struct Segment {
    file: u32,
    offset: u64,
    len: u64,
    path: PathBuf,
    /// 文件大小：发之前核对一下，文件被改了就不发了
    size: u64,
}

fn seg_count(size: u64, segment: u64) -> usize {
    size.div_ceil(segment) as usize
}

fn seg_len(size: u64, segment: u64, index: u64) -> u64 {
    (size - index * segment).min(segment)
}

/// 切段：返回要发的段（已经收好的跳过），以及已经收好的字节数
fn plan_segments(list: &[(PathBuf, FileEntry)], segment: u64, done: &[Vec<u32>]) -> (VecDeque<Segment>, u64) {
    let mut queue = VecDeque::new();
    let mut resumed = 0;
    for (i, (path, entry)) in list.iter().enumerate() {
        let skip: HashSet<u32> = done.get(i).into_iter().flatten().copied().collect();
        for n in 0..seg_count(entry.size, segment) as u64 {
            let len = seg_len(entry.size, segment, n);
            if skip.contains(&(n as u32)) {
                resumed += len;
            } else {
                queue.push_back(Segment { file: i as u32, offset: n * segment, len, path: path.clone(), size: entry.size });
            }
        }
    }
    (queue, resumed)
}

fn done_bytes(sizes: &[u64], segment: u64, done: &[Vec<bool>]) -> u64 {
    sizes
        .iter()
        .zip(done)
        .map(|(size, d)| d.iter().enumerate().filter(|(_, d)| **d).map(|(i, _)| seg_len(*size, segment, i as u64)).sum::<u64>())
        .sum()
}

/// 半截文件旁边的进度文件：哪些段已经校验通过
#[derive(Serialize, Deserialize)]
struct Sidecar {
    size: u64,
    segment: u64,
    done: Vec<u32>,
}

fn sidecar_path(partial: &Path) -> PathBuf {
    let mut s = partial.as_os_str().to_owned();
    s.push(".segs");
    PathBuf::from(s)
}

fn write_sidecar(partial: &Path, size: u64, segment: u64, done: &[bool]) -> std::io::Result<()> {
    let list = done.iter().enumerate().filter(|(_, d)| **d).map(|(i, _)| i as u32).collect();
    let bytes = serde_json::to_vec(&Sidecar { size, segment, done: list })?;
    let side = sidecar_path(partial);
    let tmp = side.with_extension("segs.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &side)
}

/// 备好半截文件：不存在就建，长度对不上就重新分配；读进度文件，知道哪些段已经收好。
/// 半截文件长度不对、或者进度文件对不上（大小或分段变了），就当什么都没收到
fn prepare_partial(path: &Path, size: u64, segment: u64) -> Result<Vec<bool>> {
    let mut done = vec![false; seg_count(size, segment)];
    let side = sidecar_path(path);
    let right_len = std::fs::metadata(path).is_ok_and(|m| m.len() == size);
    let saved = right_len
        .then(|| std::fs::read(&side).ok())
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<Sidecar>(&bytes).ok())
        .filter(|s| s.size == size && s.segment == segment);
    match saved {
        Some(s) => {
            for i in s.done {
                if let Some(d) = done.get_mut(i as usize) {
                    *d = true;
                }
            }
        }
        None => {
            let _ = std::fs::remove_file(&side);
        }
    }
    let file = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(path)?;
    if !right_len {
        file.set_len(size)?;
    }
    Ok(done)
}

/// 收的一方：这次并发传输的状态，各条车道共用
pub(super) struct LaneSession {
    peer: EndpointId,
    segment: u64,
    sizes: Vec<u64>,
    partials: Vec<PathBuf>,
    /// 每个文件的每一段收好了没有
    done: Mutex<Vec<Vec<bool>>>,
    /// 写进文件的字节数，算进度和看有没有卡住
    received: AtomicU64,
    failed: Mutex<Option<String>>,
    wake: tokio::sync::Notify,
    /// 写进度文件要一个一个来，免得写丢
    persist: tokio::sync::Mutex<()>,
    /// 各条车道的用时（纳秒）：等对方的数据、写盘、落盘
    wait_ns: AtomicU64,
    write_ns: AtomicU64,
    sync_ns: AtomicU64,
}

impl LaneSession {
    fn fail(&self, message: String) {
        {
            let mut failed = self.failed.lock().unwrap();
            if failed.is_none() {
                *failed = Some(message);
            }
        }
        self.wake.notify_one();
    }

    fn failure(&self) -> Option<String> {
        self.failed.lock().unwrap().clone()
    }

    fn missing(&self) -> usize {
        self.done.lock().unwrap().iter().flatten().filter(|d| !**d).count()
    }

    fn done_lists(&self) -> Vec<Vec<u32>> {
        self.done
            .lock()
            .unwrap()
            .iter()
            .map(|d| d.iter().enumerate().filter(|(_, d)| **d).map(|(i, _)| i as u32).collect())
            .collect()
    }

    /// 一条车道上的东西：一段接一段，流结束就是这条车道发完了
    async fn lane(&self, recv: &mut RecvStream) -> Result<()> {
        loop {
            match read_frame_as::<_, FileMsg>(recv).await? {
                None => return Ok(()),
                Some(AnyFrame::Msg(FileMsg::Seg { file, offset, len })) => self.segment(recv, file, offset, len).await?,
                Some(_) => bail!("车道上收到意外的东西"),
            }
        }
    }

    async fn segment(&self, recv: &mut RecvStream, file: u32, offset: u64, len: u64) -> Result<()> {
        let file = file as usize;
        let size = *self.sizes.get(file).ok_or_else(|| anyhow!("段所属的文件编号不对"))?;
        let valid = len > 0
            && offset.is_multiple_of(self.segment)
            && offset.checked_add(len).is_some_and(|end| end <= size)
            && len == seg_len(size, self.segment, offset / self.segment);
        if !valid {
            bail!("段的范围不对");
        }
        let index = (offset / self.segment) as usize;
        let mut out = tokio::fs::OpenOptions::new().write(true).open(&self.partials[file]).await?;
        out.seek(SeekFrom::Start(offset)).await?;
        let mut hasher = blake3::Hasher::new();
        let mut left = len;
        loop {
            let waited = Instant::now();
            let frame = read_frame_as::<_, FileMsg>(recv).await?;
            spent(&self.wait_ns, waited);
            match frame {
                Some(AnyFrame::Data(d)) => {
                    let n = d.len() as u64;
                    if n > left {
                        bail!("对方发来的比这一段的大小多");
                    }
                    let wrote = Instant::now();
                    out.write_all(&d).await?;
                    spent(&self.write_ns, wrote);
                    hasher.update(&d);
                    left -= n;
                    self.received.fetch_add(n, Ordering::Relaxed);
                }
                Some(AnyFrame::Msg(FileMsg::SegEnd { hash })) => {
                    if left != 0 {
                        bail!("这一段没收全");
                    }
                    out.flush().await?;
                    // 落盘了才记“收好了”，不然断电以后进度文件说收好、文件里其实是空的
                    let synced = Instant::now();
                    out.sync_data().await?;
                    spent(&self.sync_ns, synced);
                    drop(out);
                    if hasher.finalize().to_hex().as_str() != hash {
                        bail!("校验没通过，文件可能在传输中损坏，请重发");
                    }
                    return self.mark(file, index).await;
                }
                Some(AnyFrame::Msg(other)) => bail!("意外的消息：{other:?}"),
                None => bail!("对方中途断开"),
            }
        }
    }

    /// 记下这一段收好了：先写进度文件，写完才把这一段标成收好。
    /// 顺序不能反：标成收好以后别处就可能认为全收齐了、放好文件并删掉进度文件，迟到的写入会把它又建回来
    async fn mark(&self, file: usize, index: usize) -> Result<()> {
        let _one_at_a_time = self.persist.lock().await;
        let mut snapshot = self.done.lock().unwrap()[file].clone();
        snapshot[index] = true;
        let (path, size, segment) = (self.partials[file].clone(), self.sizes[file], self.segment);
        let saved = snapshot.clone();
        tokio::task::spawn_blocking(move || write_sidecar(&path, size, segment, &saved)).await??;
        self.done.lock().unwrap()[file][index] = true;
        // 叫醒在等“收齐了没有”的那头，不用等下一次心跳
        self.wake.notify_one();
        Ok(())
    }
}

/// 这次传输登记在 Files 里，收完或出错时撤掉
struct Registered<'a> {
    map: &'a Mutex<HashMap<String, Arc<LaneSession>>>,
    key: String,
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        self.map.lock().unwrap().remove(&self.key);
    }
}

/// 发一条车道：从共享队列里一段段领，发完队列就结束这条流
async fn send_lane(mut uni: SendStream, id: String, queue: Arc<Mutex<VecDeque<Segment>>>, sent: Arc<AtomicU64>, times: Arc<Times>) -> Result<()> {
    write_json(&mut uni, &FileMsg::Lane { id }).await?;
    let mut buf = vec![0u8; CHUNK];
    loop {
        let next = queue.lock().unwrap().pop_front();
        let Some(seg) = next else { break };
        let opened = Instant::now();
        let mut file = tokio::fs::File::open(&seg.path).await.with_context(|| format!("打不开 {}", seg.path.display()))?;
        if file.metadata().await?.len() != seg.size {
            bail!("文件在发送过程中被改动了");
        }
        file.seek(SeekFrom::Start(seg.offset)).await?;
        spent(&times.read, opened);
        write_json(&mut uni, &FileMsg::Seg { file: seg.file, offset: seg.offset, len: seg.len }).await?;
        let mut hasher = blake3::Hasher::new();
        let mut left = seg.len;
        while left > 0 {
            let want = left.min(CHUNK as u64) as usize;
            let read_at = Instant::now();
            let n = file.read(&mut buf[..want]).await?;
            spent(&times.read, read_at);
            if n == 0 {
                bail!("文件在发送过程中被改动了");
            }
            let hash_at = Instant::now();
            hasher.update(&buf[..n]);
            spent(&times.hash, hash_at);
            let write_at = Instant::now();
            write_data(&mut uni, &buf[..n]).await?;
            spent(&times.write, write_at);
            left -= n as u64;
            sent.fetch_add(n as u64, Ordering::Relaxed);
        }
        write_json(&mut uni, &FileMsg::SegEnd { hash: hasher.finalize().to_hex().to_string() }).await?;
    }
    let _ = uni.finish();
    Ok(())
}

/// 每隔两秒把这条连接和各车道的情况整理成一行：往返延迟、丢包、窗口、时间花在读盘还是等网络上。
/// 传得慢的时候看这一行，就知道慢在发送方的磁盘、网络，还是对方收得慢
struct DiagClock {
    at: Instant,
    sent_packets: u64,
    lost_packets: u64,
    app_sent: u64,
    times: [u64; 3],
}

impl DiagClock {
    fn new(conn: &Connection) -> Self {
        let stats = conn.stats();
        DiagClock { at: Instant::now(), sent_packets: stats.sent_packets, lost_packets: stats.lost_packets, app_sent: 0, times: [0; 3] }
    }

    /// 到点了就返回一行诊断，没到点返回 None
    fn sample(&mut self, conn: &Connection, app_sent: u64, delivered: Option<u64>, times: &Times) -> Option<String> {
        let elapsed = self.at.elapsed();
        if elapsed < Duration::from_secs(2) {
            return None;
        }
        let stats = conn.stats();
        let (sent, lost) = (stats.sent_packets.saturating_sub(self.sent_packets), stats.lost_packets.saturating_sub(self.lost_packets));
        let now = [times.read.load(Ordering::Relaxed), times.hash.load(Ordering::Relaxed), times.write.load(Ordering::Relaxed)];
        let spent: Vec<u64> = now.iter().zip(self.times).map(|(n, o)| n.saturating_sub(o)).collect();
        let rate = app_sent.saturating_sub(self.app_sent) as f64 / elapsed.as_secs_f64();
        let rtt = conn.paths().iter().find(|p| p.is_selected()).map(|p| p.rtt().as_millis());
        let controller = conn.congestion_state(PathId::ZERO);
        let window = controller.as_ref().map(|c| c.window() / 1024);
        let algorithm = controller.and_then(crate::quic::algorithm);
        let mut parts = vec![
            format!("发出 {:.0} KB/s", rate / 1024.0),
            format!("往返 {} ms", rtt.map_or("?".into(), |r| r.to_string())),
            format!("近 {:.0} 秒丢包 {:.1}%（{lost}/{sent}）", elapsed.as_secs_f64(), if sent > 0 { lost as f64 * 100.0 / sent as f64 } else { 0.0 }),
            format!("窗口 {} KB", window.map_or("?".into(), |w| w.to_string())),
            algorithm.unwrap_or("?").to_string(),
        ];
        if let Some(d) = delivered {
            parts.push(format!("在途 {:.1} MiB", app_sent.saturating_sub(d) as f64 / 1048576.0));
        }
        parts.push(format!("车道用时 {}", shares(&[("读盘", spent[0]), ("校验", spent[1]), ("发送", spent[2])])));
        *self = DiagClock { at: Instant::now(), sent_packets: stats.sent_packets, lost_packets: stats.lost_packets, app_sent, times: now };
        Some(parts.join(" · "))
    }
}

impl Files {
    /// 发的一方：按对方说的分段大小切段，开车道一起发，最后在控制流上说发完了，等对方放好。
    /// 进度显示对方回报的“真正收到了多少”；对方是 0.1.5（不回报）就退回显示写进发送缓冲区的字节
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn send_lanes(
        &self,
        id: &str,
        conn: &Connection,
        send: &mut SendStream,
        recv: &mut RecvStream,
        list: &[(PathBuf, FileEntry)],
        lanes: u32,
        segment: u64,
        done: Vec<Vec<u32>>,
        feedback: bool,
    ) -> Result<(String, Vec<String>)> {
        if segment == 0 || segment > MAX_SEGMENT {
            bail!("对方要的分段大小不对");
        }
        let (queue, resumed) = plan_segments(list, segment, &done);
        let total: u64 = list.iter().map(|(_, e)| e.size).sum();
        self.update(id, |t| {
            t.state = TransferState::Running;
            t.done = resumed;
        });
        let pending = queue.len();
        let lanes = (lanes.max(1)).min(self.lanes_max.max(1)).min(pending.max(1) as u32);
        self.update(id, |t| t.lanes = lanes);
        let queue = Arc::new(Mutex::new(queue));
        let sent = Arc::new(AtomicU64::new(0));
        let times = Arc::new(Times::default());
        let mut set = JoinSet::new();
        if pending > 0 {
            for _ in 0..lanes {
                let uni = conn.open_uni().await.map_err(|e| anyhow!("开车道失败：{e}"))?;
                set.spawn(send_lane(uni, id.to_string(), queue.clone(), sent.clone(), times.clone()));
            }
        }
        let mut control = Control::new(&mut *recv);
        // 对方回报的已收字节；没收到过回报就是 None，显示退回“已写进发送缓冲区”
        let mut delivered: Option<u64> = None;
        // 对方会回报：只显示它回报的（还没收到第一次回报时是已经收好的部分）；对方是 0.1.5 不回报：退回已写进发送缓冲区的字节
        let shown = |delivered: Option<u64>| {
            if feedback { delivered.unwrap_or(resumed) } else { (resumed + sent.load(Ordering::Relaxed)).min(total) }
        };
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        let mut diag = DiagClock::new(conn);
        // 等车道发完；期间刷新进度和诊断，对方中途报错就停下
        while !set.is_empty() {
            tokio::select! {
                msg = control.next() => match msg.map_err(|e| anyhow!("对方中途断开：{e:#}"))? {
                    FileMsg::Progress { done } => delivered = Some(done.min(total)),
                    FileMsg::Error { message } => bail!("对方出错：{message}"),
                    FileMsg::Reject { message } => bail!("{message}"),
                    other => bail!("对方回了意外的消息：{other:?}"),
                },
                joined = set.join_next() => match joined {
                    Some(Ok(Ok(()))) | None => {}
                    Some(Ok(Err(e))) => return Err(e),
                    Some(Err(e)) => bail!("车道出了问题：{e}"),
                },
                _ = tick.tick() => {}
            }
            let now = shown(delivered);
            self.update(id, |t| t.done = now);
            if let Some(text) = diag.sample(conn, sent.load(Ordering::Relaxed), delivered.map(|d| d.saturating_sub(resumed)), &times) {
                info!("传输诊断（发） {text}");
                self.update(id, |t| t.diag = text);
            }
        }
        write_json(send, &FileMsg::Finish).await?;
        let _ = send.finish();
        // 对方核对、放到正式位置之前，数据可能还有一小段在路上，继续收它的进度回报，直到 Saved
        loop {
            match timeout(FINAL_WAIT, control.next()).await.map_err(|_| anyhow!("对方收完以后没有回应"))?? {
                FileMsg::Progress { done } => {
                    self.update(id, |t| t.done = done.min(total));
                }
                FileMsg::Saved { dir, paths } => return Ok((dir, paths)),
                FileMsg::Error { message } => bail!("对方出错：{message}"),
                other => bail!("对方回了意外的消息：{other:?}"),
            }
        }
    }

    /// 收的一方：一条车道。第一条消息说明属于哪次传输，核对是同一个设备开的
    pub(super) async fn receive_lane(&self, conn: &Connection, mut recv: RecvStream) -> Result<()> {
        let FileMsg::Lane { id } = timeout(Duration::from_secs(10), read_json(&mut recv)).await?? else {
            bail!("车道的开场消息不对");
        };
        let key = format!("{}:{id}", conn.remote_id());
        let session = self.lane_sessions.lock().unwrap().get(&key).cloned().ok_or_else(|| anyhow!("没有这次传输"))?;
        if session.peer != conn.remote_id() {
            bail!("车道不是这次传输的发送方开的");
        }
        let result = session.lane(&mut recv).await;
        if let Err(e) = &result {
            session.fail(format!("{e:#}"));
        }
        result
    }

    /// 收的一方：对面想并发分段发，本机也开着。备好半截文件、告诉对方哪些段已经有了，
    /// 等车道把所有段收齐、对面说发完了，核对后放到收件目录
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn receive_lanes(
        &self,
        remote: EndpointId,
        offer_id: String,
        name: &str,
        plan: Plan,
        entries: Vec<FileEntry>,
        offered: u32,
        feedback: bool,
        send: &mut SendStream,
        recv: &mut RecvStream,
    ) -> Result<(String, Vec<String>)> {
        let name = clean_name(name);
        let total: u64 = entries.iter().map(|e| e.size).sum();
        let tops: Vec<String> = plan.tops.iter().map(|t| t.name.clone()).collect();

        let partial_dir = self.inbox_root.join(PARTIAL_DIR);
        if let Err(e) = tokio::fs::create_dir_all(&partial_dir).await {
            let message = format!("建收件目录失败：{}（{e}）", partial_dir.display());
            let _ = reply(send, FileMsg::Error { message: message.clone() }).await;
            bail!("{message}");
        }
        // 半截文件名带 .seg，和一条流收的半截文件分开，两种方式的进度不会互相弄混
        let keys: Vec<String> = entries.iter().map(|e| format!("{}.seg", partial_key(&remote, e))).collect();
        let Some(_claim) = Claim::take(&self.receiving, &keys) else {
            let message = "这批文件正在传，等它传完再发".to_string();
            let _ = reply(send, FileMsg::Reject { message: message.clone() }).await;
            bail!("{message}");
        };
        let busy = self.receiving.lock().unwrap().clone();
        let dir = partial_dir.clone();
        let _ = tokio::task::spawn_blocking(move || clean_partials(&dir, &busy)).await;

        let paths: Vec<PathBuf> = keys.iter().map(|k| partial_dir.join(k)).collect();
        let sizes: Vec<u64> = entries.iter().map(|e| e.size).collect();
        let prepared = {
            let (paths, sizes) = (paths.clone(), sizes.clone());
            tokio::task::spawn_blocking(move || -> Result<Vec<Vec<bool>>> {
                paths.iter().zip(&sizes).map(|(p, s)| prepare_partial(p, *s, SEGMENT)).collect()
            })
            .await?
        };
        let done = match prepared {
            Ok(done) => done,
            Err(e) => {
                let message = format!("备半截文件失败：{e:#}");
                let _ = reply(send, FileMsg::Error { message: message.clone() }).await;
                bail!("{message}");
            }
        };
        let resumed = done_bytes(&sizes, SEGMENT, &done);
        let lanes = offered.clamp(1, self.lanes_max.max(1));
        let session = Arc::new(LaneSession {
            peer: remote,
            segment: SEGMENT,
            sizes,
            partials: paths.clone(),
            done: Mutex::new(done),
            received: AtomicU64::new(0),
            failed: Mutex::new(None),
            wake: tokio::sync::Notify::new(),
            persist: tokio::sync::Mutex::new(()),
            wait_ns: AtomicU64::new(0),
            write_ns: AtomicU64::new(0),
            sync_ns: AtomicU64::new(0),
        });
        let key = format!("{remote}:{offer_id}");
        let duplicate = {
            let mut sessions = self.lane_sessions.lock().unwrap();
            let duplicate = sessions.contains_key(&key);
            if !duplicate {
                sessions.insert(key.clone(), session.clone());
            }
            duplicate
        };
        if duplicate {
            let message = "这次传输已经在进行".to_string();
            let _ = reply(send, FileMsg::Reject { message: message.clone() }).await;
            bail!("{message}");
        }
        let _registered = Registered { map: &self.lane_sessions, key };

        let id = new_id();
        self.track(Transfer {
            id: id.clone(),
            direction: Direction::Receive,
            peer: remote.to_string(),
            peer_name: name.clone(),
            label: label(&tops),
            files: entries.len(),
            total,
            done: resumed,
            state: TransferState::Running,
            error: None,
            dir: None,
            paths: Vec::new(),
            started_at: now_secs(),
            finished_at: None,
            lanes,
            diag: String::new(),
        });
        info!(remote = %remote.fmt_short(), %name, files = entries.len(), total, lanes, resumed, "开始并发分段收文件");

        let result: Result<(String, Vec<String>)> = async {
            write_json(send, &FileMsg::LaneAccept { lanes, segment: SEGMENT, done: session.done_lists(), progress: feedback }).await?;
            self.await_lanes(&id, &session, send, recv, resumed, total, feedback).await?;
            let missing = session.missing();
            if missing > 0 {
                bail!("还有 {missing} 段没收到");
            }
            let dest = self.inbox_root.join(safe_component(&name).unwrap_or_else(|| "未命名设备".into()));
            let shown = dest.display().to_string();
            let (entries, keys) = (entries.clone(), keys.clone());
            let placed = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
                let placed = place(&dest, &partial_dir, &plan, &keys, &entries, false)?;
                // 收齐放好了，进度文件不再需要
                for p in &paths {
                    let _ = std::fs::remove_file(sidecar_path(p));
                }
                Ok(placed)
            })
            .await??;
            Ok((shown, placed))
        }
        .await;
        self.conclude(&id, true, remote, name, entries.len(), total, Target::Inbox, result, send).await
    }

    /// 等车道把所有段收齐：对面在控制流上说发完了，并且每一段都校验通过才算。
    /// 控制流和车道是不同的流，“发完了”可能比最后几段数据先到，所以收到它以后还要接着等段收齐；
    /// 期间更新进度（对方看得懂就定期回报给它），车道出错或者很久没数据就停
    #[allow(clippy::too_many_arguments)]
    async fn await_lanes(
        &self,
        id: &str,
        session: &LaneSession,
        send: &mut SendStream,
        recv: &mut RecvStream,
        resumed: u64,
        total: u64,
        feedback: bool,
    ) -> Result<()> {
        let mut control = std::pin::pin!(read_json::<_, FileMsg>(recv));
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        let (mut seen, mut quiet_since) = (0u64, Instant::now());
        let (mut reported, mut reported_at) = (u64::MAX, Instant::now());
        let (mut diag_at, mut diag_times, mut diag_bytes) = (Instant::now(), [0u64; 3], 0u64);
        let mut finished = false;
        loop {
            if finished && session.missing() == 0 {
                if feedback {
                    // 最后再报一次，让对方的进度走到头
                    let _ = write_json(send, &FileMsg::Progress { done: total }).await;
                }
                return Ok(());
            }
            tokio::select! {
                msg = &mut control, if !finished => match msg {
                    Ok(FileMsg::Finish) => finished = true,
                    Ok(FileMsg::Error { message }) => bail!("对方出错：{message}"),
                    Ok(other) => bail!("对方发来意外的消息：{other:?}"),
                    Err(e) => bail!("对方中途断开：{e:#}"),
                },
                _ = session.wake.notified() => {
                    if let Some(message) = session.failure() {
                        bail!("{message}");
                    }
                }
                _ = tick.tick() => {
                    let got = session.received.load(Ordering::Relaxed);
                    let now = (resumed + got).min(total);
                    self.update(id, |t| t.done = now);
                    if got != seen {
                        seen = got;
                        quiet_since = Instant::now();
                    } else if quiet_since.elapsed() > STALL {
                        bail!("对方 {} 秒没有发数据，当作掉线", STALL.as_secs());
                    }
                    // 回报真正收到了多少，半秒一次，没变化不发
                    if feedback && now != reported && reported_at.elapsed() >= Duration::from_millis(500) {
                        write_json(send, &FileMsg::Progress { done: now }).await?;
                        (reported, reported_at) = (now, Instant::now());
                    }
                    // 每两秒一行诊断：时间花在等对方的数据、写盘还是落盘上
                    if diag_at.elapsed() >= Duration::from_secs(2) {
                        let times = [session.wait_ns.load(Ordering::Relaxed), session.write_ns.load(Ordering::Relaxed), session.sync_ns.load(Ordering::Relaxed)];
                        let spent: Vec<u64> = times.iter().zip(diag_times).map(|(n, o)| n.saturating_sub(o)).collect();
                        let rate = got.saturating_sub(diag_bytes) as f64 / diag_at.elapsed().as_secs_f64();
                        let text = format!("收到 {:.0} KB/s · 车道用时 {}", rate / 1024.0, shares(&[("等对方的数据", spent[0]), ("写盘", spent[1]), ("落盘", spent[2])]));
                        info!("传输诊断（收） {text}");
                        self.update(id, |t| t.diag = text);
                        (diag_at, diag_times, diag_bytes) = (Instant::now(), times, got);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, size: u64) -> (PathBuf, FileEntry) {
        (PathBuf::from(path), FileEntry { path: path.into(), size, modified: 0, exec: false })
    }

    #[test]
    fn segments_cover_each_file_exactly_and_skip_finished_ones() {
        let seg = 10;
        let list = vec![entry("a", 25), entry("empty", 0), entry("b", 10), entry("c", 3)];
        let (queue, resumed) = plan_segments(&list, seg, &[]);
        let all: Vec<(u32, u64, u64)> = queue.iter().map(|s| (s.file, s.offset, s.len)).collect();
        assert_eq!(all, [(0, 0, 10), (0, 10, 10), (0, 20, 5), (2, 0, 10), (3, 0, 3)]);
        assert_eq!(resumed, 0);
        // 每个文件的段加起来正好是它的大小，空文件没有段
        for (i, (_, e)) in list.iter().enumerate() {
            let sum: u64 = queue.iter().filter(|s| s.file == i as u32).map(|s| s.len).sum();
            assert_eq!(sum, e.size);
        }
        // 第一个文件的第 1 段和最后一个文件已经收好：跳过，字节数记进已完成
        let (queue, resumed) = plan_segments(&list, seg, &[vec![1], vec![], vec![], vec![0]]);
        let all: Vec<(u32, u64)> = queue.iter().map(|s| (s.file, s.offset)).collect();
        assert_eq!(all, [(0, 0), (0, 20), (2, 0)]);
        assert_eq!(resumed, 10 + 3);
    }

    #[test]
    fn partial_files_resume_only_when_everything_matches() {
        let dir = std::env::temp_dir().join(format!("p2pcli-lanes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.seg");

        // 第一次：新建，什么都没收到，文件被预先分配到完整大小
        assert_eq!(prepare_partial(&path, 25, 10).unwrap(), [false, false, false]);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 25);

        // 记下第 0、2 段收好了：下次接着用
        write_sidecar(&path, 25, 10, &[true, false, true]).unwrap();
        assert_eq!(prepare_partial(&path, 25, 10).unwrap(), [true, false, true]);

        // 分段大小变了、文件大小变了：进度作废，从头来
        assert_eq!(prepare_partial(&path, 25, 5).unwrap(), [false; 5]);
        write_sidecar(&path, 25, 10, &[true, true, true]).unwrap();
        assert_eq!(prepare_partial(&path, 30, 10).unwrap(), [false, false, false]);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 30);
        assert!(!sidecar_path(&path).exists(), "对不上的进度文件要删掉");

        // 半截文件被截短过：就算进度文件说收好了也不信
        write_sidecar(&path, 30, 10, &[true, true, true]).unwrap();
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(7).unwrap();
        assert_eq!(prepare_partial(&path, 30, 10).unwrap(), [false, false, false]);

        // 进度文件里有越界的段编号：忽略，不崩
        std::fs::write(sidecar_path(&path), br#"{"size":30,"segment":10,"done":[0,99]}"#).unwrap();
        assert_eq!(prepare_partial(&path, 30, 10).unwrap(), [true, false, false]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finished_bytes_follow_the_last_short_segment() {
        let sizes = [25u64, 0, 3];
        let done = vec![vec![false, false, true], vec![], vec![true]];
        assert_eq!(done_bytes(&sizes, 10, &done), 5 + 3);
    }
}

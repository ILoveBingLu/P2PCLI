//! 抓到的画面怎么变成编码好的帧：单独一个编码线程，边排画面边送编码器，不等上一帧编完。
//!
//! 硬件编码一帧的延迟（整屏在动时 8~27 毫秒）比 60 帧的间隔还长，一帧编完再拿下一帧就到不了 60 帧，
//! 间隔也忽长忽短。编码器本身是异步的，同时塞两帧进去，它的吞吐够 60 帧，结果按送进去的顺序回来。
//!
//! 流量控制：观看方那边能收时才会来要帧（next_frame）。有人在等时编码器里最多两帧；没人要时
//! 最多先编好一帧（在编的也算），不再往里送：P 帧编出来就不能丢，编了没人要只会越积越多。
//! 编好还没被取走的帧最多一个。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use objc2_core_foundation::CFRetained;
use objc2_core_media::CMTime;
use objc2_core_video::{CVImageBuffer, CVPixelBufferGetHeight, CVPixelBufferGetWidth};
use p2pcli_core::host::BoxFuture;
use p2pcli_core::screen::{CaptureStats, ColorFormat, EncodedFrame, Layout, VideoCodec};
use tracing::{debug, warn};

use super::encode::{Encoder, Reply};
use super::pack::Packer;

/// 送进编码器的是什么画面：系统直接给的 NV12（4:2:0）
pub(super) const MODE_NV12: u8 = 0;
/// 系统给 BGRA，Metal 排成 4:4:4 叠图（W×3H）
pub(super) const MODE_STACKED: u8 = 1;
/// 系统给 BGRA，Metal 转成 NV12 全范围：4:4:4 中途开不了编码器时退到这里
pub(super) const MODE_PACKED_NV12: u8 = 2;

/// 要关键帧时最多等新画面这么久，等不到就拿上一帧重新编
const REUSE_AFTER: Duration = Duration::from_millis(100);

/// 编码器里同时最多几帧、没人要帧时最多先编好几帧
#[derive(Debug, Clone, Copy)]
pub(super) struct Limits {
    pub in_flight: usize,
    pub prefetch: usize,
}

/// 两帧就够让编码器一直有活干；先编好的只要一帧，延迟最多多一帧
pub(super) const PIPELINED: Limits = Limits { in_flight: 2, prefetch: 1 };

/// 抓到的一帧：系统给的像素缓冲（IOSurface）和它到的时间。CF 对象在线程间传递是安全的（只读）
#[derive(Clone)]
struct Sample {
    image: CFRetained<CVImageBuffer>,
    at: Instant,
}
unsafe impl Send for Sample {}

/// 编好的一帧，和它是按哪种画面编的
struct Done {
    result: Result<EncodedFrame>,
    mode: u8,
}

#[derive(Default)]
struct State {
    /// 最新的一帧：编码线程来取之前被新的覆盖，旧的直接丢掉
    latest: Option<Sample>,
    /// 上一次编码用的那帧：屏幕不动时要关键帧（恢复直连、观看方解码出错），就拿它重新编
    last: Option<Sample>,
    stopped: Option<String>,
    /// 抓屏已经停了（MacCapture 被丢掉），编码线程退出
    closed: bool,
    /// next_frame 正在等
    waiting: bool,
    /// 在编码器里还没回来的帧
    in_flight: usize,
    /// 编好还没取走的
    ready: VecDeque<Done>,
    /// 编码器换代：4:4:4 编坏了加一，旧编码器后面回来的结果都不要
    generation: u64,
    /// 要了关键帧：之前送进去的非关键帧回来了也不要，下一帧就是关键帧，解码用不着它们
    drop_until_key: bool,
    /// 4:4:4 编坏了，等编码线程退回 4:2:0
    stacked_failed: Option<anyhow::Error>,
    stats: CaptureStats,
}

/// 抓屏回调、编码线程、编码回调、next_frame 共用的
pub(super) struct Frames {
    state: Mutex<State>,
    /// 编码线程等的：新画面、有人来要、编码器腾出位置、退出
    wake: Condvar,
    /// next_frame 等的：编好了一帧、出错、抓屏停了
    ready: tokio::sync::Notify,
    want_key: AtomicBool,
    bitrate: AtomicU32,
    /// 最近交出去的那帧是哪种画面：color() 报这个，和刚交出去的关键帧对得上
    handed_mode: AtomicU8,
    color: ColorFormat,
    limits: Limits,
    /// 时间戳从这里算
    started: Instant,
}

/// 编码线程自己用的：编码器、GPU 排画面、现在出哪种画面
pub(super) struct Pipe {
    pub encoder: Option<Encoder>,
    /// 4:4:4 时在 GPU 上排画面
    pub packer: Option<Packer>,
    pub mode: u8,
    pub fps: u32,
    /// 想用的编码格式（编不了 H.265 时编码器自己退回 H.264）
    pub codec: VideoCodec,
    pub p3: bool,
}

impl Frames {
    /// 开编码线程。color 是 4:2:0 直接编系统画面时报的颜色格式
    pub(super) fn start(pipe: Pipe, color: ColorFormat, bitrate: u32, limits: Limits) -> Result<Arc<Self>> {
        let frames = Arc::new(Frames {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            ready: tokio::sync::Notify::new(),
            want_key: AtomicBool::new(true),
            bitrate: AtomicU32::new(bitrate),
            handed_mode: AtomicU8::new(pipe.mode),
            color,
            limits,
            started: Instant::now(),
        });
        let worker = frames.clone();
        std::thread::Builder::new().name("p2pcli-encode".into()).spawn(move || {
            set_interactive();
            worker.run(pipe)
        })?;
        Ok(frames)
    }

    /// 系统送来一帧新画面
    pub(super) fn deliver(&self, image: CFRetained<CVImageBuffer>) {
        let mut s = self.state.lock().unwrap();
        if s.latest.is_some() {
            s.stats.skipped_frames += 1;
        }
        s.latest = Some(Sample { image, at: Instant::now() });
        s.stats.source_frames += 1;
        drop(s);
        self.wake.notify_one();
    }

    /// 抓屏停了（系统报错）
    pub(super) fn stop(&self, message: String) {
        self.state.lock().unwrap().stopped = Some(message);
        self.wake.notify_one();
        self.ready.notify_one();
    }

    /// 不抓了：编码线程编完手上的就退出
    pub(super) fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.wake.notify_one();
    }

    pub(super) fn next_frame(self: &Arc<Self>) -> BoxFuture<Result<EncodedFrame>> {
        let this = self.clone();
        Box::pin(async move {
            // 不管是拿到了还是被取消了，出去时都不算在等了
            let _waiting = Waiting(&this);
            loop {
                {
                    let mut s = this.state.lock().unwrap();
                    if let Some(done) = s.ready.pop_front() {
                        if done.result.is_ok() {
                            s.stats.encoded_frames += 1;
                            this.handed_mode.store(done.mode, Ordering::Relaxed);
                        }
                        return done.result;
                    }
                    if let Some(message) = &s.stopped {
                        bail!("抓屏停止了：{message}");
                    }
                    if !s.waiting {
                        s.waiting = true;
                        drop(s);
                        this.wake.notify_one();
                    }
                }
                this.ready.notified().await;
            }
        })
    }

    pub(super) fn request_keyframe(&self) {
        self.want_key.store(true, Ordering::Relaxed);
        let mut s = self.state.lock().unwrap();
        // 先编好的 P 帧不要了：下一帧是关键帧，早一点出来
        s.ready.retain(|d| d.result.as_ref().map_or(true, |f| f.key));
        s.drop_until_key = s.in_flight > 0;
        drop(s);
        self.wake.notify_one();
    }

    pub(super) fn set_bitrate(&self, bps: u32) {
        self.bitrate.store(bps, Ordering::Relaxed);
    }

    pub(super) fn color(&self) -> ColorFormat {
        match self.handed_mode.load(Ordering::Relaxed) {
            MODE_NV12 => self.color,
            // Metal 出的都是全范围
            MODE_STACKED => ColorFormat { full_range: true, layout: Layout::Yuv444Stacked, ..self.color },
            _ => ColorFormat { full_range: true, layout: Layout::Yuv420, ..self.color },
        }
    }

    pub(super) fn stats(&self) -> CaptureStats {
        let mut s = self.state.lock().unwrap();
        let stats = s.stats;
        s.stats.max_us = 0;
        stats
    }

    /// 编码线程：有位置、有画面就送进编码器，不等它编完
    fn run(self: Arc<Self>, mut pipe: Pipe) {
        let mut reuse_at: Option<Instant> = None;
        loop {
            let job = {
                let mut s = self.state.lock().unwrap();
                loop {
                    if s.closed || s.stopped.is_some() {
                        debug!("编码线程退出");
                        return;
                    }
                    if let Some(e) = s.stacked_failed.take() {
                        break Err(e);
                    }
                    let allowed = self.limits.prefetch + usize::from(s.waiting);
                    let room = s.in_flight < self.limits.in_flight && s.in_flight + s.ready.len() < allowed;
                    if room {
                        if let Some(sample) = s.latest.take() {
                            s.last = Some(sample.clone());
                            // 位置在拿画面的同时占上
                            s.in_flight += 1;
                            break Ok((sample, s.generation));
                        }
                        // 要关键帧但屏幕没动：等一小会儿新画面，等不到就重编上一帧
                        if self.want_key.load(Ordering::Relaxed)
                            && let Some(last) = s.last.clone()
                        {
                            let now = Instant::now();
                            let deadline = *reuse_at.get_or_insert(now + REUSE_AFTER);
                            if now >= deadline {
                                s.in_flight += 1;
                                break Ok((Sample { image: last.image, at: now }, s.generation));
                            }
                            s = self.wake.wait_timeout(s, deadline - now).unwrap().0;
                            continue;
                        }
                    }
                    s = self.wake.wait(s).unwrap();
                }
            };
            reuse_at = None;
            match job {
                Ok((sample, generation)) => self.submit(&mut pipe, sample, generation),
                Err(e) => {
                    // 扔掉坏了的编码器：它会等在编的帧都回调完，那些结果按旧的一代丢掉
                    pipe.encoder = None;
                    self.fall_back(&mut pipe, &e);
                }
            }
        }
    }

    /// 排好画面送进编码器。编码器的位置已经占上了，出错时在这里还回去
    fn submit(self: &Arc<Self>, pipe: &mut Pipe, sample: Sample, generation: u64) {
        let taken = Instant::now();
        let mut key = self.want_key.swap(false, Ordering::Relaxed);
        let bitrate = self.bitrate.load(Ordering::Relaxed);
        let pts = unsafe { CMTime::new(self.started.elapsed().as_micros() as i64, 1_000_000) };
        if pipe.packer.is_some() {
            key |= self.prepare_stacked(pipe, &sample.image, bitrate);
        }
        let mode = pipe.mode;
        let packed = match &mut pipe.packer {
            Some(packer) => match packer.pack(&sample.image, mode == MODE_STACKED) {
                Ok(b) => Some(b),
                Err(e) => return self.failed(e, mode),
            },
            None => None,
        };
        let image = packed.as_deref().unwrap_or(&sample.image);
        let reply = self.reply(generation, mode, key, sample.at, taken);
        if let Err(e) = Encoder::submit(&mut pipe.encoder, image, pts, pipe.fps, bitrate, key, pipe.codec, pipe.p3, reply) {
            // 编码器开不起来：reply 没被调，位置在这里还
            self.failed(e, mode);
        }
    }

    /// 送不进编码器：还回位置，错误交给 next_frame（画面结束）
    fn failed(&self, e: anyhow::Error, mode: u8) {
        self.want_key.store(true, Ordering::Relaxed);
        let mut s = self.state.lock().unwrap();
        s.in_flight -= 1;
        s.ready.push_back(Done { result: Err(e), mode });
        drop(s);
        self.ready.notify_one();
    }

    /// 一帧编完的回调（在 VideoToolbox 的线程上，或者编码器扔掉时在编码线程上）
    fn reply(self: &Arc<Self>, generation: u64, mode: u8, key: bool, at: Instant, taken: Instant) -> Reply {
        let this = self.clone();
        Box::new(move |result| {
            let mut s = this.state.lock().unwrap();
            s.in_flight -= 1;
            if generation == s.generation {
                match result {
                    Ok(Some(mut frame)) => {
                        if s.drop_until_key && !frame.key {
                            debug!("要了关键帧，之前的帧不要了");
                        } else {
                            s.drop_until_key = false;
                            frame.ts_us = at.saturating_duration_since(this.started).as_micros() as u64;
                            let us = taken.elapsed().as_micros() as u64;
                            s.stats.busy_us += us;
                            s.stats.max_us = s.stats.max_us.max(us);
                            s.ready.push_back(Done { result: Ok(frame), mode });
                            this.ready.notify_one();
                        }
                    }
                    // 编码器丢了这帧：丢的是要求的关键帧就再要一次
                    Ok(None) => {
                        if key {
                            this.want_key.store(true, Ordering::Relaxed);
                        }
                    }
                    // 4:4:4 编坏了：这一代后面的结果都不要，编码线程退回 4:2:0 重来，画面不断
                    Err(e) if mode == MODE_STACKED => {
                        s.generation += 1;
                        s.stacked_failed = Some(e);
                    }
                    Err(e) => {
                        s.ready.push_back(Done { result: Err(e), mode });
                        this.ready.notify_one();
                    }
                }
            }
            drop(s);
            this.wake.notify_one();
        })
    }

    /// 4:4:4 时编码器要按 W×3H 开（只要硬件编码器）。没开或者尺寸不对就重开，开不了就退回 4:2:0。
    /// 返回是不是新开了编码器（要出关键帧）
    fn prepare_stacked(&self, pipe: &mut Pipe, image: &CVImageBuffer, bitrate: u32) -> bool {
        if pipe.mode != MODE_STACKED {
            return false;
        }
        let (w, h) = (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image));
        if pipe.encoder.as_ref().is_some_and(|e| e.size() == (w, h * 3)) {
            return false;
        }
        pipe.encoder = None;
        match Encoder::new(w, h * 3, pipe.fps, bitrate, pipe.codec, pipe.p3, true) {
            Ok(enc) => {
                pipe.encoder = Some(enc);
                true
            }
            Err(e) => {
                self.fall_back(pipe, &e);
                false
            }
        }
    }

    /// 4:4:4 编不了：之后一直用 Metal 转普通的 NV12，编码器随后按 W×H 重开（Encoder::submit 里看尺寸）。
    /// 每帧带着自己的画面种类，color() 按交出去的那帧报，和关键帧对得上。只退一次，警告也只记一次
    fn fall_back(&self, pipe: &mut Pipe, e: &anyhow::Error) {
        self.want_key.store(true, Ordering::Relaxed);
        if pipe.mode == MODE_STACKED {
            warn!("编码器编不了 4:4:4 的三倍高画面，退回 4:2:0：{e:#}");
        }
        pipe.mode = MODE_PACKED_NV12;
    }
}

unsafe extern "C" {
    fn pthread_set_qos_class_self_np(qos: u32, relative_priority: i32) -> i32;
}

/// 把当前线程标成“用户正在交互”：调度优先、定时器不合并。编码线程一慢，整条流水线都跟着晚
pub(super) fn set_interactive() {
    /// QOS_CLASS_USER_INTERACTIVE
    const USER_INTERACTIVE: u32 = 0x21;
    let r = unsafe { pthread_set_qos_class_self_np(USER_INTERACTIVE, 0) };
    if r != 0 {
        debug!("设不了线程优先级（{r}）");
    }
}

struct Waiting<'a>(&'a Frames);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().waiting = false;
        // 交出去一帧就腾出了位置
        self.0.wake.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::super::pack::tests::{Scene, bgra_buffer, pattern};
    use super::*;
    use p2pcli_core::screen::Primaries;

    const SERIAL: Limits = Limits { in_flight: 1, prefetch: 0 };

    fn start(stacked: bool, codec: VideoCodec, p3: bool, bitrate: u32, limits: Limits) -> Arc<Frames> {
        let pipe = Pipe {
            encoder: None,
            packer: stacked.then(|| Packer::new(p3).unwrap()),
            mode: if stacked { MODE_STACKED } else { MODE_NV12 },
            fps: 60,
            codec,
            p3,
        };
        let color = ColorFormat { full_range: true, primaries: if p3 { Primaries::P3 } else { Primaries::Bt709 }, ..Default::default() };
        Frames::start(pipe, color, bitrate, limits).unwrap()
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    fn next(rt: &tokio::runtime::Runtime, frames: &Arc<Frames>) -> EncodedFrame {
        rt.block_on(async { tokio::time::timeout(Duration::from_secs(5), frames.next_frame()).await }).expect("等不到帧").unwrap()
    }

    /// 能编 4:4:4 时出 W×3H 的关键帧、报叠图；编不了（H.264 的 3840×6480）时退回 W×H 的 4:2:0，报的也跟着变
    #[test]
    #[ignore = "要 GPU 和硬件编码器：cargo test -p p2pcli-screen -- --ignored frames_ --nocapture"]
    fn frames_report_layout() {
        let rt = runtime();
        let (w, h) = (1920, 1080);
        let frames = start(true, VideoCodec::H265, true, 20_000_000, PIPELINED);
        frames.deliver(bgra_buffer(&pattern(w, h), w, h));
        let frame = next(&rt, &frames);
        let config = frame.config.expect("第一帧要带解码配置");
        assert!(frame.key);
        assert_eq!((config.width, config.height), (1920, 3240));
        assert_eq!(frames.color(), ColorFormat { full_range: true, primaries: Primaries::P3, layout: Layout::Yuv444Stacked });
        frames.deliver(bgra_buffer(&pattern(w, h), w, h));
        assert!(!next(&rt, &frames).key, "接着编的帧不用关键帧");
        frames.close();

        let (w, h) = (3840, 2160);
        let frames = start(true, VideoCodec::H264, false, 20_000_000, PIPELINED);
        frames.deliver(bgra_buffer(&pattern(w, h), w, h));
        // 4:4:4 编坏了以后，要关键帧时没新画面就重编上一帧
        let frame = next(&rt, &frames);
        let config = frame.config.expect("退回以后第一帧要带解码配置");
        assert!(frame.key);
        println!("H.264 {w}×{h} 的 4:4:4：{} {}×{}，{:?}", config.codec, config.width, config.height, frames.color());
        assert_eq!((config.width, config.height), (3840, 2160));
        assert_eq!(frames.color(), ColorFormat { full_range: true, primaries: Primaries::Bt709, layout: Layout::Yuv420 });
        frames.close();
    }

    /// 没人要帧时最多先编好一帧；要了关键帧，先编好的 P 帧扔掉，下一帧就是关键帧；屏幕不动时要关键帧就重编上一帧
    #[test]
    #[ignore = "要 GPU 和硬件编码器：cargo test -p p2pcli-screen -- --ignored frames_ --nocapture"]
    fn frames_flow_control() {
        let rt = runtime();
        let (w, h) = (1280, 720);
        let frames = start(false, VideoCodec::H265, false, 8_000_000, PIPELINED);
        let mut scene = Scene::new();
        frames.deliver(scene.render_nv12(w, h));
        assert!(next(&rt, &frames).key);
        // 没人要：送 10 帧进去，只会编一帧
        for _ in 0..10 {
            frames.deliver(scene.render_nv12(w, h));
            std::thread::sleep(Duration::from_millis(17));
        }
        std::thread::sleep(Duration::from_millis(100));
        {
            let s = frames.state.lock().unwrap();
            assert_eq!(s.in_flight + s.ready.len(), 1, "没人要帧时只该先编好一帧");
        }
        let stats = frames.stats();
        assert_eq!(stats.source_frames, 11);
        assert_eq!(stats.encoded_frames, 1);
        // 10 帧里先编了一帧、最新的一帧还放着，中间 8 帧被盖掉
        assert_eq!(stats.skipped_frames, 8);
        // 先编好的是 P 帧；要了关键帧它就不要了，拿最新的那帧编成关键帧
        frames.request_keyframe();
        let frame = next(&rt, &frames);
        assert!(frame.key, "要了关键帧，下一帧该是关键帧");
        // 屏幕不动时要关键帧：等一小会儿没有新画面，就重编上一帧
        frames.request_keyframe();
        let t = Instant::now();
        assert!(next(&rt, &frames).key);
        println!("屏幕不动时要关键帧，{:?} 后拿到", t.elapsed());
        assert!(t.elapsed() >= REUSE_AFTER - Duration::from_millis(5));
        // 码率改了照样能编
        frames.set_bitrate(2_000_000);
        frames.deliver(scene.render_nv12(w, h));
        assert!(!next(&rt, &frames).key);
        assert!(frames.stats().max_us > 0);
        assert_eq!(frames.stats().max_us, 0, "读走以后清零");
        frames.close();
    }

    fn percentile(sorted: &[f64], p: f64) -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        sorted[((sorted.len() - 1) as f64 * p).round() as usize]
    }

    /// 按 60 Hz 送合成画面，消费端像 serve 那样拿到一帧马上要下一帧。返回一行结果
    fn bench(name: &str, w: usize, h: usize, stacked: bool, codec: VideoCodec, bitrate: u32, limits: Limits) -> String {
        const WARMUP: Duration = Duration::from_secs(1);
        const MEASURE: Duration = Duration::from_secs(6);
        let frames = start(stacked, codec, false, bitrate, limits);
        let done = Arc::new(AtomicBool::new(false));
        let producer = {
            let frames = frames.clone();
            let done = done.clone();
            std::thread::spawn(move || {
                set_interactive();
                let mut scene = Scene::new();
                // 先画一帧，把着色器编译这些一次性的开销放在计时外面
                scene.render(w, h);
                let begin = Instant::now();
                let mut n = 0u32;
                let mut sent = Vec::new();
                while begin.elapsed() < WARMUP + MEASURE {
                    let image = if stacked { scene.render(w, h) } else { scene.render_nv12(w, h) };
                    n += 1;
                    let due = begin + Duration::from_secs_f64(n as f64 / 60.0);
                    // 系统的 sleep 会晚好几毫秒（定时器合并），最后 6 毫秒空转，送帧的节奏才像屏幕刷新一样稳
                    if let Some(wait) = due.checked_duration_since(Instant::now() + Duration::from_millis(6)) {
                        std::thread::sleep(wait);
                    }
                    while Instant::now() < due {
                        std::hint::spin_loop();
                    }
                    frames.deliver(image);
                    sent.push(Instant::now());
                }
                done.store(true, Ordering::Relaxed);
                sent
            })
        };
        let rt = runtime();
        let mut begin = None;
        let mut times = Vec::new();
        let mut latency = Vec::new();
        let mut keys = 0;
        let (mut bytes, mut max_frame) = (0usize, 0usize);
        let mut base = None;
        rt.block_on(async {
            loop {
                let Ok(frame) = tokio::time::timeout(Duration::from_millis(300), frames.next_frame()).await else {
                    if done.load(Ordering::Relaxed) {
                        break;
                    }
                    continue;
                };
                let frame = frame.unwrap();
                let now = Instant::now();
                // 从拿到第一帧起算预热
                if now - *begin.get_or_insert(now) < WARMUP {
                    continue;
                }
                if base.is_none() {
                    base = Some(frames.stats());
                }
                keys += usize::from(frame.key);
                bytes += frame.data.len();
                max_frame = max_frame.max(frame.data.len());
                times.push(now);
                latency.push((now - (frames.started + Duration::from_micros(frame.ts_us))).as_secs_f64() * 1000.0);
            }
        });
        let sent = producer.join().unwrap();
        let mut source_gaps: Vec<f64> = sent.windows(2).skip(60).map(|p| (p[1] - p[0]).as_secs_f64() * 1000.0).collect();
        let source_over = source_gaps.iter().filter(|g| **g > 20.0).count();
        source_gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let stats = frames.stats();
        let base = base.unwrap_or_default();
        frames.close();
        let mut gaps: Vec<f64> = times.windows(2).map(|p| (p[1] - p[0]).as_secs_f64() * 1000.0).collect();
        let over = gaps.iter().filter(|g| **g > 20.0).count();
        gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        latency.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let span = (*times.last().unwrap() - times[0]).as_secs_f64();
        let fps = (times.len() - 1) as f64 / span;
        let source = stats.source_frames - base.source_frames;
        let skipped = stats.skipped_frames - base.skipped_frames;
        let encoded = (stats.encoded_frames - base.encoded_frames).max(1);
        let busy = (stats.busy_us - base.busy_us) as f64 / encoded as f64 / 1000.0;
        format!(
            "| {name} | {} | {fps:.1} | {:.1} / {:.1} / {:.1} | {over} | {:.1} / {:.1} / {:.1} | {skipped}/{source} | {busy:.1} | {keys} | {:.1} / {:.1} / {source_over} | {:.1} / {} |",
            if limits.in_flight == 1 { "串行" } else { "流水线" },
            percentile(&gaps, 0.5),
            percentile(&gaps, 0.95),
            gaps.last().copied().unwrap_or_default(),
            percentile(&latency, 0.5),
            percentile(&latency, 0.95),
            latency.last().copied().unwrap_or_default(),
            percentile(&source_gaps, 0.95),
            source_gaps.last().copied().unwrap_or_default(),
            bytes as f64 * 8.0 / span / 1e6,
            max_frame / 1024,
        )
    }

    /// 串行（编完一帧再拿下一帧，原来的做法）和流水线比帧率、间隔、延迟
    #[test]
    #[ignore = "要 GPU 和硬件编码器，跑一分多钟：cargo test -p p2pcli-screen -- --ignored frames_bench --nocapture --test-threads=1"]
    fn frames_bench() {
        let cases: [(&str, usize, usize, bool, VideoCodec, u32); 5] = [
            ("4:2:0 1920×1080 HEVC", 1920, 1080, false, VideoCodec::H265, 16_000_000),
            ("4:2:0 2560×1440 HEVC", 2560, 1440, false, VideoCodec::H265, 30_000_000),
            ("4:4:4 1920×3240 HEVC", 1920, 1080, true, VideoCodec::H265, 24_000_000),
            ("4:4:4 2560×4320 HEVC", 2560, 1440, true, VideoCodec::H265, 45_000_000),
            ("4:4:4 1920×3240 H.264", 1920, 1080, true, VideoCodec::H264, 24_000_000),
        ];
        let only = std::env::var("P2PCLI_BENCH_ONLY").ok();
        println!("| 画面 | 做法 | 帧率 | 间隔 ms 中位/p95/最大 | >20ms | 延迟 ms 中位/p95/最大 | 跳过/来源 | 平均处理 ms | 关键帧 | 来源间隔 p95/最大/>20ms | 码率 Mbps / 最大帧 KB |");
        println!("|---|---|---|---|---|---|---|---|---|---|---|");
        for (name, w, h, stacked, codec, bitrate) in cases {
            if only.as_deref().is_some_and(|o| !name.contains(o)) {
                continue;
            }
            for limits in [SERIAL, PIPELINED] {
                println!("{}", bench(name, w, h, stacked, codec, bitrate, limits));
            }
        }
    }
}

//! 抓屏到编码这一路能跑多少帧：不抓真屏幕，定时 60 Hz 往“最新一帧”里放会动的画面，
//! 走和抓屏一样的转换、编码和主循环，另一头像被连方那样一帧接一帧地要。
//! 老的一路（一帧一等）和流水线各跑一遍对比。要真显卡和硬件编码器，平时不跑：
//!
//! cargo test -p p2pcli-screen --release --lib -- --ignored --nocapture --test-threads=1 pipeline

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use p2pcli_core::screen::{CaptureOptions, EncodedFrame, VideoCodec};
use tokio::sync::oneshot;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

use super::capture::{Depth, Ev, PIPELINED, SERIAL, Shared, State, Synthetic};
use super::yuv444::tests::{Gpu, gpu, pattern, upload};

/// 每秒 60 帧
const TICK: Duration = Duration::from_nanos(16_666_667);
/// 轮着放几张不一样的画面
const FRAMES: usize = 16;

/// 会动的画面：整屏往右下滚（渐变加伪随机的“字”），再加一个白方块
fn moving_frames(gpu: &Gpu, w: usize, h: usize) -> Vec<ID3D11Texture2D> {
    let base = pattern(w, h, 3);
    (0..FRAMES)
        .map(|i| {
            let (dx, dy) = (i * 16 % w, i * 9 % h);
            let mut f = vec![255u8; w * h * 4];
            for y in 0..h {
                let src = &base[((y + dy) % h) * w * 4..][..w * 4];
                let dst = &mut f[y * w * 4..][..w * 4];
                dst[..(w - dx) * 4].copy_from_slice(&src[dx * 4..]);
                dst[(w - dx) * 4..].copy_from_slice(&src[..dx * 4]);
            }
            let (bx, by) = (i * 40 % (w - 200), i * 25 % (h - 200));
            for y in by..by + 200 {
                for x in bx..bx + 200 {
                    f[(y * w + x) * 4..][..3].copy_from_slice(&[255, 255, 255]);
                }
            }
            upload(gpu, &f, w as u32, h as u32)
        })
        .collect()
}

/// 一路跑起来的抓屏线程、定时放画面的线程，和要帧用的那头
struct Rig {
    tx: Sender<Ev>,
    shared: Arc<Shared>,
    started: Instant,
    /// 定时器在不在放画面
    ticking: Arc<AtomicBool>,
    quit: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    ticker: Option<JoinHandle<()>>,
}

impl Rig {
    fn start(w: u32, h: u32, yuv444: bool, codec: VideoCodec, depth: Depth) -> Result<Self> {
        let (tx, rx) = channel::<Ev>();
        let shared = Shared::new(20_000_000);
        let latest: Arc<Mutex<Option<(Instant, usize)>>> = Arc::default();
        let (ready_tx, ready_rx) = channel();
        let worker = {
            let (tx, shared, latest) = (tx.clone(), shared.clone(), latest.clone());
            std::thread::spawn(move || {
                let gpu = gpu();
                let frames = moving_frames(&gpu, w as usize, h as usize);
                let opts = CaptureOptions {
                    display: String::new(),
                    max_long_edge: w.max(h),
                    fps: 60,
                    bitrate: 20_000_000,
                    show_cursor: false,
                    codec,
                    full_range: true,
                    wide_gamut: false,
                    yuv444,
                };
                let source = Synthetic { frames, size: (w, h), latest };
                let mut state = match State::synthetic(gpu.device.clone(), gpu.ctx.clone(), source, &opts, shared, tx, depth) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(state.started));
                state.run(&rx, &opts);
            })
        };
        let started = ready_rx.recv().map_err(|_| anyhow!("抓屏线程没起来"))??;
        let ticking = Arc::new(AtomicBool::new(true));
        let quit = Arc::new(AtomicBool::new(false));
        let ticker = {
            let (tx, shared, ticking, quit) = (tx.clone(), shared.clone(), ticking.clone(), quit.clone());
            std::thread::spawn(move || {
                let mut next = Instant::now();
                let mut i = 0;
                while !quit.load(Ordering::Relaxed) {
                    next += TICK;
                    // 睡到差一毫秒，剩下的空转，间隔才稳
                    if let Some(d) = next.checked_duration_since(Instant::now()).and_then(|d| d.checked_sub(Duration::from_millis(1))) {
                        std::thread::sleep(d);
                    }
                    while Instant::now() < next {
                        std::hint::spin_loop();
                    }
                    if !ticking.load(Ordering::Relaxed) {
                        continue;
                    }
                    let old = latest.lock().unwrap().replace((Instant::now(), i));
                    shared.arrived(old.is_some());
                    let _ = tx.send(Ev::Wake);
                    i += 1;
                }
            })
        };
        Ok(Rig { tx, shared, started, ticking, quit, worker: Some(worker), ticker: Some(ticker) })
    }

    /// 像被连方那样要一帧，等它来
    fn next(&self) -> Result<EncodedFrame> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Ev::Next(reply)).map_err(|_| anyhow!("抓屏线程已经结束"))?;
        rx.blocking_recv().map_err(|_| anyhow!("抓屏线程已经结束"))?
    }

    fn request_keyframe(&self) {
        self.shared.want_key.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Ev::Wake);
    }

    /// 这帧从放进来到现在多久
    fn age(&self, f: &EncodedFrame) -> Duration {
        Instant::now().saturating_duration_since(self.started + Duration::from_micros(f.ts_us))
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Ev::Stop);
        if let Some(t) = self.ticker.take() {
            let _ = t.join();
        }
        if let Some(t) = self.worker.take() {
            let _ = t.join();
        }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// 排好序的毫秒数取百分位
fn pct(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() as f64 - 1.0) * p).round() as usize]
}

struct Row {
    fps: f64,
    gaps: Vec<f64>,
    ages: Vec<f64>,
    busy_avg: f64,
    busy_max: f64,
    skipped: u64,
    source: u64,
    config: String,
}

/// think：拿到一帧以后过这么久才要下一帧（模拟被连方发帧、调度的耽搁）
fn measure(w: u32, h: u32, yuv444: bool, codec: VideoCodec, depth: Depth, secs: u64, think: Duration) -> Result<Row> {
    let rig = Rig::start(w, h, yuv444, codec, depth)?;
    let mut config = String::new();
    // 热身：编码器建好、码率稳下来
    let warm = Instant::now();
    while warm.elapsed() < Duration::from_millis(1500) {
        let f = rig.next()?;
        if let Some(c) = f.config {
            config = format!("{} {}×{}", c.codec, c.width, c.height);
        }
    }
    let before = rig.shared.stats();
    let start = Instant::now();
    let mut last = Instant::now();
    let (mut gaps, mut ages) = (Vec::new(), Vec::new());
    while start.elapsed() < Duration::from_secs(secs) {
        let f = rig.next()?;
        let now = Instant::now();
        gaps.push(ms(now - last));
        ages.push(ms(rig.age(&f)));
        last = now;
        if !think.is_zero() {
            std::thread::sleep(think);
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    let after = rig.shared.stats();
    let encoded = (after.encoded_frames - before.encoded_frames).max(1);
    gaps.sort_by(f64::total_cmp);
    ages.sort_by(f64::total_cmp);
    Ok(Row {
        fps: gaps.len() as f64 / elapsed,
        gaps,
        ages,
        busy_avg: (after.busy_us - before.busy_us) as f64 / encoded as f64 / 1000.0,
        busy_max: after.max_us as f64 / 1000.0,
        skipped: after.skipped_frames - before.skipped_frames,
        source: after.source_frames - before.source_frames,
        config,
    })
}

/// 五种画面各跑一遍老的一路和流水线，打一张表
#[test]
#[ignore = "要真显卡和硬件编码器，跑两分钟"]
fn pipeline_fps() {
    let _ = tracing_subscriber::fmt().with_env_filter("warn").try_init();
    let none = Duration::ZERO;
    let cases = [
        ("4:2:0 1920×1200 H.265", 1920, 1200, false, VideoCodec::H265, none),
        ("4:2:0 2560×1600 H.265", 2560, 1600, false, VideoCodec::H265, none),
        ("4:4:4 1920×3600 H.265", 1920, 1200, true, VideoCodec::H265, none),
        ("4:4:4 2560×4800 H.265", 2560, 1600, true, VideoCodec::H265, none),
        ("4:4:4 1920×3600 H.264", 1920, 1200, true, VideoCodec::H264, none),
        // 一帧编得比 16.7 毫秒还久、或者要帧那头有耽搁时，才看得出流水线的差别
        ("4:2:0 2560×1600 H.265，拿到帧后 5 ms 才要下一帧", 2560, 1600, false, VideoCodec::H265, Duration::from_millis(5)),
        ("4:4:4 3840×6480 H.265", 3840, 2160, true, VideoCodec::H265, none),
    ];
    let mut table = vec![
        "| 画面 | 一路 | 编码 | 帧/秒 | 帧间隔 中位/95%/最长 ms | >20 ms | 放进来到拿到 中位/95%/最长 ms | 处理 平均/最长 ms | 屏幕出/跳过 |".to_string(),
        "|---|---|---|---|---|---|---|---|---|".to_string(),
    ];
    for (label, w, h, yuv444, codec, think) in cases {
        for (name, depth) in [("老的一帧一等", SERIAL), ("流水线", PIPELINED)] {
            let line = match measure(w, h, yuv444, codec, depth, 8, think) {
                Ok(r) => format!(
                    "| {label} | {name} | {} | {:.1} | {:.1} / {:.1} / {:.1} | {} | {:.1} / {:.1} / {:.1} | {:.1} / {:.1} | {} / {} |",
                    r.config,
                    r.fps,
                    pct(&r.gaps, 0.5),
                    pct(&r.gaps, 0.95),
                    r.gaps.last().copied().unwrap_or_default(),
                    r.gaps.iter().filter(|&&g| g > 20.0).count(),
                    pct(&r.ages, 0.5),
                    pct(&r.ages, 0.95),
                    r.ages.last().copied().unwrap_or_default(),
                    r.busy_avg,
                    r.busy_max,
                    r.source,
                    r.skipped,
                ),
                Err(e) => format!("| {label} | {name} | 不行：{e:#} |"),
            };
            println!("{line}");
            table.push(line);
        }
    }
    println!("\n{}", table.join("\n"));
}

/// 流控、关键帧、屏幕不动时重编关键帧
#[test]
#[ignore = "要真显卡和硬件编码器"]
fn pipeline_flow_control() {
    let _ = tracing_subscriber::fmt().with_env_filter("warn").try_init();
    let rig = Rig::start(1920, 1200, false, VideoCodec::H265, PIPELINED).unwrap();
    let first = rig.next().unwrap();
    assert!(first.key && first.config.is_some(), "第一帧要是带解码配置的关键帧");
    for _ in 0..60 {
        rig.next().unwrap();
    }

    // 不要帧：最多再编一帧备着，编码器里那帧编完也算，不能一直编下去
    let before = rig.shared.submitted.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(500));
    let during = rig.shared.submitted.load(Ordering::Relaxed) - before;
    println!("不要帧的 500 毫秒里送进编码器 {during} 帧");
    assert!(during <= 1, "没人要帧还编了 {during} 帧");
    let stale = rig.next().unwrap();
    let fresh = rig.next().unwrap();
    println!("停了 500 毫秒以后：第一帧是 {:.0} 毫秒前的画面（备着的那帧），下一帧 {:.0} 毫秒", ms(rig.age(&stale)), ms(rig.age(&fresh)));

    // 要关键帧：下一帧就是关键帧，备着、在编的 P 帧不交出来
    for _ in 0..10 {
        rig.next().unwrap();
    }
    let t = Instant::now();
    rig.request_keyframe();
    let k = rig.next().unwrap();
    println!("要关键帧以后 {:.1} 毫秒拿到", ms(t.elapsed()));
    assert!(k.key && k.config.is_some(), "要了关键帧，下一帧不是关键帧");
    let p = rig.next().unwrap();
    assert!(!p.key, "关键帧后面应该是 P 帧");

    // 屏幕不动时要关键帧：等一会儿拿上一帧重编
    rig.ticking.store(false, Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(100));
    // 停之前备着的那帧先拿走
    let (reply, mut rx) = oneshot::channel();
    rig.tx.send(Ev::Next(reply)).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let _ = rx.try_recv();
    let t = Instant::now();
    rig.request_keyframe();
    let k = rig.next().unwrap();
    println!("屏幕不动时要关键帧，{:.0} 毫秒拿到", ms(t.elapsed()));
    assert!(k.key && k.config.is_some(), "屏幕不动时没重编出关键帧");
    assert!(t.elapsed() < Duration::from_millis(500), "重编关键帧太慢");
}

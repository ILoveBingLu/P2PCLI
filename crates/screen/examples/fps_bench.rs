//! 测抓屏 + 编码一路能跑多少帧：屏幕要一直在动（放段动画），连着要帧，统计帧间隔。
//! cargo run --release -p p2pcli-screen --example fps_bench -- [420|444] [balanced|sharp] [h265|h264] [秒数]
//! 帧率按屏幕的刷新率（P2PCLI_BENCH_HZ 可以压低，假装观看方屏幕慢）
use std::time::{Duration, Instant};

use p2pcli_core::screen::{CaptureOptions, Quality, VideoCodec};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    let mut args = std::env::args().skip(1);
    let yuv444 = args.next().as_deref() == Some("444");
    let q = if args.next().as_deref() == Some("sharp") { Quality::Sharp } else { Quality::Balanced };
    let codec = if args.next().as_deref() == Some("h264") { VideoCodec::H264 } else { VideoCodec::H265 };
    let secs: u64 = args.next().and_then(|n| n.parse().ok()).unwrap_or(10);
    let backend = p2pcli_screen::backend().expect("这个系统不支持");
    backend.check_capture()?;
    let main = backend.displays()?.into_iter().find(|d| d.primary).unwrap();
    // 观看方当成和本机屏幕一样快；环境变量 P2PCLI_BENCH_HZ 可以假装观看方的刷新率
    let viewer_hz = std::env::var("P2PCLI_BENCH_HZ").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let fps = q.fps(main.refresh_hz, viewer_hz);
    println!("屏幕 {}×{} {} Hz，按 {fps} 帧抓", main.width, main.height, main.refresh_hz);
    let cap = backend.start(CaptureOptions {
        display: main.id.clone(),
        max_long_edge: q.max_long_edge(codec),
        fps,
        bitrate: p2pcli_core::screen::scaled_bitrate(q.max_bitrate(), fps),
        show_cursor: false,
        codec,
        full_range: true,
        wide_gamut: false,
        yuv444,
    })?;
    // 先热身一秒
    let warm = Instant::now();
    while warm.elapsed() < Duration::from_secs(1) {
        cap.next_frame().await?;
    }
    let start = Instant::now();
    let mut last = Instant::now();
    let mut gaps = Vec::new();
    let mut bytes = 0usize;
    let mut config = None;
    cap.request_keyframe();
    while start.elapsed() < Duration::from_secs(secs) {
        let f = cap.next_frame().await?;
        let now = Instant::now();
        gaps.push(now - last);
        last = now;
        bytes += f.data.len();
        if let Some(c) = f.config {
            config = Some(format!("{} {}x{}", c.codec, c.width, c.height));
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    let mut ms: Vec<f64> = gaps.iter().map(|g| g.as_secs_f64() * 1000.0).collect();
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| ms[((ms.len() as f64 - 1.0) * p) as usize];
    println!(
        "{} {:?} {:?} 编码 {} | {:.1} 帧/秒 | 帧间隔 中位 {:.1} ms、95% {:.1} ms、最长 {:.1} ms | 超过 20 ms 的 {} 次 | {:.1} Mbps | color={:?}",
        if yuv444 { "4:4:4" } else { "4:2:0" },
        q,
        codec,
        config.unwrap_or_default(),
        gaps.len() as f64 / elapsed,
        pct(0.5),
        pct(0.95),
        ms.last().copied().unwrap_or_default(),
        ms.iter().filter(|&&m| m > 20.0).count(),
        bytes as f64 * 8.0 / elapsed / 1e6,
        cap.color(),
    );
    let stats = cap.stats().unwrap_or_default();
    println!(
        "编码器 {:?} | 平均每帧从画面到手到编好 {:.1} ms",
        cap.encoder(),
        stats.busy_us as f64 / stats.encoded_frames.max(1) as f64 / 1000.0
    );
    Ok(())
}

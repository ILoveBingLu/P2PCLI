//! 测抓屏 + 编码一路能跑多少帧：屏幕要一直在动（放段动画），连着要帧，统计帧间隔。
//! cargo run --release -p p2pcli-screen --example fps_bench -- [420|444] [balanced|sharp] [h265|h264] [秒数]
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
    let max_long_edge = if yuv444 { q.max_long_edge().min(2560) } else { q.max_long_edge() };
    let cap = backend.start(CaptureOptions {
        display: main.id.clone(),
        max_long_edge,
        fps: q.fps(),
        bitrate: q.max_bitrate(),
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
    Ok(())
}

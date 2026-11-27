//! 真机上抓几秒本机正在放的声音：打印收到了多少、音量多大，再编成 Opus 看一包多大。
//! 测的时候让本机放点声音（音乐、提示音）。cargo run --release -p p2pcli-screen --example audio_check -- [秒数]
use std::time::{Duration, Instant};

use p2pcli_core::screen_audio::{CHANNELS, Encoder, SAMPLE_RATE};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    let secs: u64 = std::env::args().nth(1).and_then(|n| n.parse().ok()).unwrap_or(5);
    let backend = p2pcli_screen::backend().expect("这个系统不支持");
    anyhow::ensure!(backend.audio_supported(), "这个系统还不能抓声音");
    let (sink, mut rx) = tokio::sync::mpsc::channel(64);
    let guard = tokio::task::spawn_blocking(move || backend.start_audio(sink)).await??;
    let start = Instant::now();
    let (mut chunks, mut samples, mut sum, mut peak) = (0usize, 0usize, 0f64, 0f32);
    let mut encoder = Encoder::new()?;
    let (mut packets, mut bytes) = (0usize, 0usize);
    while start.elapsed() < Duration::from_secs(secs) {
        let Ok(Some(pcm)) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await else { continue };
        chunks += 1;
        samples += pcm.len();
        for v in &pcm {
            sum += (*v as f64) * (*v as f64);
            peak = peak.max(v.abs());
        }
        encoder.push(&pcm, |p| {
            packets += 1;
            bytes += p.len();
        })?;
    }
    drop(guard);
    let frames = samples / CHANNELS;
    let rms = if samples > 0 { (sum / samples as f64).sqrt() } else { 0.0 };
    println!(
        "{secs} 秒：收到 {chunks} 段、{frames} 帧（{:.2} 秒的声音），均方根 {rms:.4}，峰值 {peak:.3}；Opus {packets} 包，平均 {} 字节/包（{:.0} kbps）",
        frames as f64 / SAMPLE_RATE as f64,
        bytes / packets.max(1),
        bytes as f64 * 8.0 / secs as f64 / 1000.0,
    );
    Ok(())
}

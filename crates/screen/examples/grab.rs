//! 抓几帧主屏、硬件编码，写成 Annex B 的 .h264 文件，用来验证抓屏和编码：
//! cargo run -p p2pcli-screen --example grab -- out.h264 [帧数]
use std::io::Write;
use std::time::Instant;

use p2pcli_core::screen::{CaptureOptions, Quality};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "grab.h264".into());
    let frames: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(30);
    let backend = p2pcli_screen::backend().expect("这个系统不支持");
    let displays = backend.displays()?;
    println!("屏幕：{displays:?}");
    println!("权限：{:?}", p2pcli_screen::permissions());
    backend.check_capture()?;
    let main = displays.iter().find(|d| d.primary).unwrap();
    let q = Quality::Balanced;
    let cap = backend.start(CaptureOptions { display: main.id.clone(), max_long_edge: q.max_long_edge(), fps: q.fps(), bitrate: q.start_bitrate(), show_cursor: true })?;
    let mut file = std::fs::File::create(&out)?;
    let start = Instant::now();
    let mut total = 0;
    for i in 0..frames {
        let t = Instant::now();
        let f = tokio::time::timeout(std::time::Duration::from_secs(5), cap.next_frame()).await??;
        if let Some(c) = &f.config { println!("配置：{} {}x{}", c.codec, c.width, c.height); }
        println!("帧 {i}: key={} {} 字节 等了 {:?}", f.key, f.data.len(), t.elapsed());
        total += f.data.len();
        // AVCC → Annex B，关键帧前面补上 SPS/PPS
        if let Some(c) = &f.config {
            use base64::Engine as _;
            let rec = base64::engine::general_purpose::STANDARD.decode(&c.description)?;
            let sps_len = u16::from_be_bytes([rec[6], rec[7]]) as usize;
            let sps = &rec[8..8 + sps_len];
            let pps_len = u16::from_be_bytes([rec[9 + sps_len], rec[10 + sps_len]]) as usize;
            let pps = &rec[11 + sps_len..11 + sps_len + pps_len];
            for p in [sps, pps] { file.write_all(&[0, 0, 0, 1])?; file.write_all(p)?; }
        }
        let mut d = &f.data[..];
        while d.len() >= 4 {
            let n = u32::from_be_bytes(d[..4].try_into()?) as usize;
            file.write_all(&[0, 0, 0, 1])?;
            file.write_all(&d[4..4 + n])?;
            d = &d[4 + n..];
        }
        if i == frames / 2 { cap.request_keyframe(); cap.set_bitrate(2_000_000); }
    }
    println!("{frames} 帧 {total} 字节，用时 {:?}", start.elapsed());
    Ok(())
}

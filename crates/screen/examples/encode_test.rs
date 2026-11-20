//! 不抓屏，用合成的 NV12 画面验证硬件编码器：cargo run -p p2pcli-screen --example encode_test -- out.h264
#[cfg(target_os = "macos")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use std::io::Write;
    use std::ptr::{self, NonNull};
    use base64::Engine as _;
    use objc2_core_foundation::{CFDictionary, CFRetained, CFString, CFType};
    use objc2_core_media::CMTime;
    use objc2_core_video::*;
    use p2pcli_screen::mac_encode::Encoder;

    let out = std::env::args().nth(1).unwrap_or_else(|| "enc.h264".into());
    // 第二个参数写 h265 就测 H.265
    let codec = if std::env::args().nth(2).as_deref() == Some("h265") { p2pcli_core::screen::VideoCodec::H265 } else { p2pcli_core::screen::VideoCodec::H264 };
    let (w, h) = (1920usize, 1080usize);
    let empty = CFDictionary::<CFString, CFType>::from_slices(&[], &[]);
    let attrs = unsafe { CFDictionary::<CFString, CFType>::from_slices(&[kCVPixelBufferIOSurfacePropertiesKey], &[empty.as_ref()]) };
    let mut file = std::fs::File::create(&out)?;
    let mut enc = None;
    let mut total = 0;
    for i in 0..90u32 {
        let mut pb: *mut CVPixelBuffer = ptr::null_mut();
        let r = unsafe { CVPixelBufferCreate(None, w, h, 0x34323076, Some(attrs.as_opaque()), NonNull::from(&mut pb)) };
        assert_eq!(r, 0);
        let pb: CFRetained<CVPixelBuffer> = unsafe { CFRetained::from_raw(NonNull::new(pb).unwrap()) };
        unsafe {
            CVPixelBufferLockBaseAddress(&pb, CVPixelBufferLockFlags(0));
            let y = CVPixelBufferGetBaseAddressOfPlane(&pb, 0) as *mut u8;
            let ys = CVPixelBufferGetBytesPerRowOfPlane(&pb, 0);
            for row in 0..h { for col in 0..w {
                // 移动的斜条纹 + 一块文字样的细格子
                let v = if col < 400 && row < 200 { if (col / 2 + row / 2) % 2 == 0 { 235 } else { 16 } } else { (((col + row + i as usize * 8) / 32) % 2 * 200 + 16) as u8 };
                *y.add(row * ys + col) = v;
            }}
            let uv = CVPixelBufferGetBaseAddressOfPlane(&pb, 1) as *mut u8;
            let uvs = CVPixelBufferGetBytesPerRowOfPlane(&pb, 1);
            for row in 0..h / 2 { for col in 0..w { *uv.add(row * uvs + col) = if col % 2 == 0 { 90 } else { 200 }; } }
            CVPixelBufferUnlockBaseAddress(&pb, CVPixelBufferLockFlags(0));
        }
        let key = i == 0 || i == 45;
        let t = std::time::Instant::now();
        let rx = Encoder::encode(&mut enc, &pb, unsafe { CMTime::new(i as i64, 60) }, 60, if i < 45 { 8_000_000 } else { 1_000_000 }, key, codec)?;
        let Some(f) = rx.await?? else { println!("帧 {i}: 编码器丢帧"); continue };
        total += f.data.len();
        println!("帧 {i}: key={} {} 字节 编码 {:?}", f.key, f.data.len(), t.elapsed());
        if let Some(c) = &f.config {
            println!("配置：{} {}x{} avcC {}", c.codec, c.width, c.height, c.description);
            let rec = base64::engine::general_purpose::STANDARD.decode(&c.description)?;
            for p in parameter_sets(&rec, c.codec.starts_with("hvc1")) { file.write_all(&[0, 0, 0, 1])?; file.write_all(&p)?; }
        }
        let mut d = &f.data[..];
        while d.len() >= 4 { let n = u32::from_be_bytes(d[..4].try_into()?) as usize; file.write_all(&[0, 0, 0, 1])?; file.write_all(&d[4..4 + n])?; d = &d[4 + n..]; }
    }
    println!("总共 {total} 字节");
    Ok(())
}
/// 从 avcC / hvcC 里取出参数集，写成 Annex B 文件时放在关键帧前面
#[cfg(target_os = "macos")]
fn parameter_sets(rec: &[u8], hevc: bool) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if hevc {
        let mut i = 23;
        for _ in 0..rec[22] {
            let n = u16::from_be_bytes([rec[i + 1], rec[i + 2]]) as usize;
            i += 3;
            for _ in 0..n {
                let len = u16::from_be_bytes([rec[i], rec[i + 1]]) as usize;
                out.push(rec[i + 2..i + 2 + len].to_vec());
                i += 2 + len;
            }
        }
    } else {
        let sl = u16::from_be_bytes([rec[6], rec[7]]) as usize;
        let pl = u16::from_be_bytes([rec[9 + sl], rec[10 + sl]]) as usize;
        out.push(rec[8..8 + sl].to_vec());
        out.push(rec[11 + sl..11 + sl + pl].to_vec());
    }
    out
}

#[cfg(not(target_os = "macos"))]
fn main() {}

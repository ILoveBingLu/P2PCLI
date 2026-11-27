//! VideoToolbox 硬件 H.264 编码：低延迟码率控制、不重排帧（没有 B 帧）、只在要求时出关键帧。
//! 输入是 ScreenCaptureKit 给的 IOSurface 像素缓冲，硬件直接读，不拷贝。

use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, anyhow, bail};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFData, CFDictionary, CFNumber, CFRetained, CFString, CFType, kCFBooleanTrue,
};
use objc2_core_media::{
    CMFormatDescription, CMSampleBuffer, CMTime, CMVideoFormatDescriptionGetDimensions, kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms,
    CMVideoFormatDescriptionGetH264ParameterSetAtIndex, kCMSampleAttachmentKey_NotSync, kCMTimeInvalid,
};
use objc2_core_video::{
    CVImageBuffer, CVPixelBufferGetHeight, CVPixelBufferGetWidth, kCVImageBufferColorPrimaries_ITU_R_709_2,
    kCVImageBufferColorPrimaries_P3_D65,
    kCVImageBufferTransferFunction_sRGB, kCVImageBufferYCbCrMatrix_ITU_R_709_2,
};
use objc2_video_toolbox::{
    VTCompressionSession, VTEncodeInfoFlags, VTSessionSetProperty, kVTCompressionPropertyKey_AllowFrameReordering,
    kVTCompressionPropertyKey_AverageBitRate, kVTCompressionPropertyKey_ColorPrimaries,
    kVTCompressionPropertyKey_DataRateLimits, kVTCompressionPropertyKey_ExpectedFrameRate,
    kVTCompressionPropertyKey_MaxKeyFrameInterval, kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
    kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality, kVTCompressionPropertyKey_ProfileLevel,
    kVTCompressionPropertyKey_RealTime, kVTCompressionPropertyKey_TransferFunction,
    kVTCompressionPropertyKey_YCbCrMatrix, kVTEncodeFrameOptionKey_ForceKeyFrame,
    kVTProfileLevel_H264_ConstrainedHigh_AutoLevel, kVTProfileLevel_H264_High_AutoLevel, kVTProfileLevel_HEVC_Main_AutoLevel,
    kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder,
    kVTVideoEncoderSpecification_EnableLowLatencyRateControl,
    kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder,
};
use p2pcli_core::screen::{EncodedFrame, VideoCodec, avcc_config, hevc_config};
use tokio::sync::oneshot;
use tracing::{info, warn};


/// 'avc1'
const CODEC_H264: u32 = 0x6176_6331;
/// 'hvc1'
const CODEC_HEVC: u32 = 0x6876_6331;

/// 一帧的编码结果（编好、编码器丢了这帧、出错）交给它，正好一次，多半在 VideoToolbox 自己的线程上
pub type Reply = Box<dyn FnOnce(Result<Option<EncodedFrame>>) + Send>;

/// 编码回调要用的东西；回调在 VideoToolbox 自己的线程上跑
struct Context {
    /// 还在编码器里的帧：编号（当作 sourceFrameRefcon 传进去）和它的回调。可以同时有好几帧
    pending: Mutex<VecDeque<(usize, Reply)>>,
    next_id: AtomicUsize,
    /// 实际在用的格式：取解码配置时 H.264 和 H.265 不一样
    codec: VideoCodec,
}

pub struct Encoder {
    session: CFRetained<VTCompressionSession>,
    ctx: *mut Context,
    width: usize,
    height: usize,
    bitrate: u32,
}

unsafe impl Send for Encoder {}

fn set(session: &VTCompressionSession, key: &CFString, value: &CFType) -> i32 {
    unsafe { VTSessionSetProperty(session.as_ref(), key, Some(value)) }
}

fn num(v: i32) -> CFRetained<CFNumber> {
    CFNumber::new_i32(v)
}

impl Encoder {
    /// 要 H.265 时先开 HEVC 硬件编码器（Apple 芯片、带 T2 的 Mac 才有），开不了就退回 H.264。
    /// hw_only：H.264 也只要硬件编码器（4:4:4 的画面有三倍高，软件编不动）
    pub(super) fn new(width: usize, height: usize, fps: u32, bitrate: u32, codec: VideoCodec, p3: bool, hw_only: bool) -> Result<Self> {
        if codec == VideoCodec::H265 {
            match Self::open(width, height, fps, bitrate, VideoCodec::H265, p3, hw_only) {
                Ok(enc) => return Ok(enc),
                Err(e) => warn!("开不了 H.265 硬件编码器，改用 H.264：{e:#}"),
            }
        }
        Self::open(width, height, fps, bitrate, VideoCodec::H264, p3, hw_only)
    }

    /// 只开 H.265 硬件编码器，不退回 H.264：5K 以上的大画面只有它编得了
    pub(super) fn open_hevc(width: usize, height: usize, fps: u32, bitrate: u32, p3: bool) -> Result<Self> {
        Self::open(width, height, fps, bitrate, VideoCodec::H265, p3, true)
    }

    /// 编码的画面尺寸
    pub(super) fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// p3：画面是按 Display P3 抓的，码流里标 P3 原色
    fn open(width: usize, height: usize, fps: u32, bitrate: u32, codec: VideoCodec, p3: bool, hw_only: bool) -> Result<Self> {
        let hevc = codec == VideoCodec::H265;
        let ctx = Box::into_raw(Box::new(Context { pending: Mutex::new(VecDeque::new()), next_id: AtomicUsize::new(1), codec }));
        let t = CFBoolean::new(true);
        // 先要硬件编码器 + 低延迟码率控制；老机器不支持时退到“能用硬件就用”
        let specs: [Vec<(&CFString, &CFBoolean)>; 3] = unsafe {
            [
                vec![
                    (kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder, t),
                    (kVTVideoEncoderSpecification_EnableLowLatencyRateControl, t),
                ],
                vec![(kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder, t)],
                vec![(kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder, t)],
            ]
        };
        let mut created = None;
        let mut last = 0;
        // H.265 只用硬件编码器（前两种）：软件编 H.265 太慢，不如退回 H.264
        let tries = if hevc || hw_only { &specs[..2] } else { &specs[..] };
        // 测试里比较用：P2PCLI_TEST_VT=nolowlat 不开低延迟码率控制
        let skip = usize::from(cfg!(test) && std::env::var("P2PCLI_TEST_VT").is_ok_and(|v| v.contains("nolowlat")));
        for (n, spec) in tries.iter().enumerate().skip(skip) {
            let keys: Vec<&CFString> = spec.iter().map(|(k, _)| *k).collect();
            let values: Vec<&CFBoolean> = spec.iter().map(|(_, v)| *v).collect();
            let dict = CFDictionary::from_slices(&keys, &values);
            let mut out: *mut VTCompressionSession = ptr::null_mut();
            let status = unsafe {
                VTCompressionSession::create(
                    None,
                    width as i32,
                    height as i32,
                    if hevc { CODEC_HEVC } else { CODEC_H264 },
                    Some(dict.as_opaque()),
                    None,
                    None,
                    Some(on_encoded),
                    ctx.cast(),
                    NonNull::from(&mut out),
                )
            };
            if status == 0
                && let Some(p) = NonNull::new(out)
            {
                if n > 0 {
                    warn!(step = n, "硬件编码器不支持低延迟模式，退一步用");
                }
                created = Some((unsafe { CFRetained::from_raw(p) }, n == 0));
                break;
            }
            last = status;
        }
        let Some((session, low_latency)) = created else {
            drop(unsafe { Box::from_raw(ctx) });
            bail!("开不了 {} 硬件编码器（{last}）", if hevc { "H.265" } else { "H.264" });
        };


        unsafe {
            set(&session, kVTCompressionPropertyKey_RealTime, CFBoolean::new(true));
            set(&session, kVTCompressionPropertyKey_AllowFrameReordering, CFBoolean::new(false));
            if hevc {
                set(&session, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_HEVC_Main_AutoLevel);
            } else {
                let profile = if low_latency { kVTProfileLevel_H264_ConstrainedHigh_AutoLevel } else { kVTProfileLevel_H264_High_AutoLevel };
                if set(&session, kVTCompressionPropertyKey_ProfileLevel, profile) != 0 {
                    set(&session, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel);
                }
            }
            // 关键帧只在要的时候出（刚开始、解码出错、恢复直连）：可靠传输不会丢帧，定期关键帧只会白占带宽
            set(&session, kVTCompressionPropertyKey_MaxKeyFrameInterval, &num(i32::MAX / 2));
            set(&session, kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, &num(24 * 3600));
            set(&session, kVTCompressionPropertyKey_ExpectedFrameRate, &num(fps as i32));
            set(&session, kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality, CFBoolean::new(true));
            set(&session, kVTCompressionPropertyKey_ColorPrimaries, if p3 { kCVImageBufferColorPrimaries_P3_D65 } else { kCVImageBufferColorPrimaries_ITU_R_709_2 });
            // 电脑屏幕用的是 sRGB 曲线，不是电视的 BT.709 曲线：标错了解码端的明暗会不一样
            set(&session, kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_sRGB);
            set(&session, kVTCompressionPropertyKey_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_709_2);
        }
        let mut enc = Encoder { session, ctx, width, height, bitrate: 0 };
        enc.set_bitrate(bitrate);
        unsafe { enc.session.prepare_to_encode_frames() };
        info!(width, height, fps, bitrate, low_latency, codec = ?codec, "硬件编码器已就绪");
        Ok(enc)
    }

    fn set_bitrate(&mut self, bitrate: u32) {
        if bitrate == self.bitrate {
            return;
        }
        self.bitrate = bitrate;
        unsafe {
            set(&self.session, kVTCompressionPropertyKey_AverageBitRate, &num(bitrate as i32));
            // 1 秒内最多 1.5 倍平均码率，免得关键帧一下把链路塞满
            let bytes = num((bitrate as f64 * 1.5 / 8.0) as i32);
            let secs = num(1);
            let limits = CFArray::from_objects(&[&*bytes, &*secs]);
            set(&self.session, kVTCompressionPropertyKey_DataRateLimits, limits.as_ref());
        }
    }

    /// 把一帧送进编码器（尺寸变了就重建），返回等结果的接收端。只在一个线程上调
    #[allow(clippy::too_many_arguments)]
    pub fn encode(slot: &mut Option<Encoder>, image: &CVImageBuffer, pts: CMTime, fps: u32, bitrate: u32, key: bool, codec: VideoCodec, p3: bool) -> Result<oneshot::Receiver<Result<Option<EncodedFrame>>>> {
        let (tx, rx) = oneshot::channel();
        Self::submit(slot, image, pts, fps, bitrate, key, codec, p3, Box::new(move |r| {
            let _ = tx.send(r);
        }))?;
        Ok(rx)
    }

    /// 把一帧送进编码器（尺寸变了就重建），不等结果：上一帧还在编时就能送下一帧，结果按送进去的顺序回来。
    /// 返回错误（编码器开不起来）时 reply 不会被调；返回成功时 reply 正好被调一次，
    /// 送不进去也算（这时编码器坏了，已经扔掉，下次重建）。只在一个线程上调
    #[allow(clippy::too_many_arguments)]
    pub fn submit(slot: &mut Option<Encoder>, image: &CVImageBuffer, pts: CMTime, fps: u32, bitrate: u32, key: bool, codec: VideoCodec, p3: bool, reply: Reply) -> Result<()> {
        let (w, h) = (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image));
        let mut key = key;
        if slot.as_ref().is_none_or(|e| e.width != w || e.height != h) {
            *slot = None;
            *slot = Some(Encoder::new(w, h, fps, bitrate, codec, p3, false)?);
            key = true;
        }
        let enc = slot.as_mut().unwrap();
        enc.set_bitrate(bitrate);
        let ctx = unsafe { &*enc.ctx };
        let id = ctx.next_id.fetch_add(1, Ordering::Relaxed);
        ctx.pending.lock().unwrap().push_back((id, reply));
        let props = key.then(|| unsafe {
            CFDictionary::<CFString, CFBoolean>::from_slices(&[kVTEncodeFrameOptionKey_ForceKeyFrame], &[CFBoolean::new(true)])
        });
        let mut flags = VTEncodeInfoFlags(0);
        let status = unsafe {
            enc.session.encode_frame(image, pts, kCMTimeInvalid, props.as_deref().map(|d| d.as_opaque()), id as *mut c_void, &mut flags)
        };
        if status != 0 {
            // 回调可能已经带着错误来过了；没来过就在这里回
            let reply = ctx.take(id);
            // 编码器坏了就重建（扔掉时会等别的在编的帧都回调完）
            *slot = None;
            if let Some(reply) = reply {
                reply(Err(anyhow!("硬件编码失败（{status}）")));
            }
        }
        Ok(())
    }
}

impl Context {
    fn take(&self, id: usize) -> Option<Reply> {
        let mut pending = self.pending.lock().unwrap();
        let at = pending.iter().position(|(i, _)| *i == id)?;
        pending.remove(at).map(|(_, r)| r)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // 先等在编的帧都回调完，再作废、放掉回调用的上下文，免得回调还在别的线程上读它
        unsafe {
            self.session.complete_frames(kCMTimeInvalid);
            self.session.invalidate();
        }
        // 作废以后不会再有回调，可以放掉；万一还有没回的，按丢帧回掉
        let ctx = unsafe { Box::from_raw(self.ctx) };
        let left: Vec<_> = ctx.pending.lock().unwrap().drain(..).collect();
        for (_, reply) in left {
            reply(Ok(None));
        }
    }
}

unsafe extern "C-unwind" fn on_encoded(refcon: *mut c_void, frame: *mut c_void, status: i32, _flags: VTEncodeInfoFlags, sample: *mut CMSampleBuffer) {
    let ctx = unsafe { &*(refcon as *const Context) };
    let result = if status != 0 {
        Err(anyhow!("硬件编码失败（{status}）"))
    } else if let Some(sample) = unsafe { sample.as_ref() } {
        extract(sample, ctx.codec).map(Some)
    } else {
        Ok(None)
    };
    // 锁只在取回调时拿着：回调里可能还会碰别的锁
    if let Some(reply) = ctx.take(frame as usize) {
        reply(result);
    }
}

fn is_key(sample: &CMSampleBuffer) -> bool {
    let Some(array) = (unsafe { sample.sample_attachments_array(false) }) else { return true };
    let array: CFRetained<CFArray<CFDictionary<CFString, CFType>>> = unsafe { CFRetained::cast_unchecked(array) };
    let Some(dict) = array.get(0) else { return true };
    match dict.get(unsafe { kCMSampleAttachmentKey_NotSync }) {
        Some(v) => !unsafe { kCFBooleanTrue }.is_some_and(|t| ptr::eq(t as *const CFBoolean as *const c_void, &*v as *const CFType as *const c_void)),
        None => true,
    }
}

fn parameter_set(fd: &CMFormatDescription, index: usize) -> Result<Vec<u8>> {
    let mut p: *const u8 = ptr::null();
    let mut len = 0usize;
    let status = unsafe { CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fd, index, &mut p, &mut len, ptr::null_mut(), ptr::null_mut()) };
    if status != 0 || p.is_null() {
        bail!("读不到 H.264 参数集（{status}）");
    }
    Ok(unsafe { std::slice::from_raw_parts(p, len) }.to_vec())
}

/// H.265 的解码配置：系统的格式描述里直接带着 hvcC
fn hvcc(fd: &CMFormatDescription) -> Result<Vec<u8>> {
    let atoms = unsafe { fd.extension(kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms) }
        .ok_or_else(|| anyhow!("H.265 关键帧没有 hvcC"))?;
    let atoms: CFRetained<CFDictionary<CFString, CFData>> = unsafe { CFRetained::cast_unchecked(atoms) };
    let data = atoms.get(&CFString::from_static_str("hvcC")).ok_or_else(|| anyhow!("H.265 关键帧没有 hvcC"))?;
    Ok(data.to_vec())
}

fn extract(sample: &CMSampleBuffer, codec: VideoCodec) -> Result<EncodedFrame> {
    let block = unsafe { sample.data_buffer() }.ok_or_else(|| anyhow!("编码结果是空的"))?;
    let len = unsafe { block.data_length() };
    let mut data = vec![0u8; len];
    let status = unsafe { block.copy_data_bytes(0, len, NonNull::new(data.as_mut_ptr().cast()).unwrap()) };
    if status != 0 {
        bail!("读编码结果失败（{status}）");
    }
    let key = is_key(sample);
    let config = if key {
        let fd = unsafe { sample.format_description() }.ok_or_else(|| anyhow!("关键帧没有格式描述"))?;
        let dims = unsafe { CMVideoFormatDescriptionGetDimensions(&fd) };
        Some(match codec {
            VideoCodec::H265 => hevc_config(&hvcc(&fd)?, dims.width as u32, dims.height as u32)?,
            VideoCodec::H264 => {
                let sps = parameter_set(&fd, 0)?;
                let pps = parameter_set(&fd, 1)?;
                avcc_config(&sps, &pps, dims.width as u32, dims.height as u32)?
            }
        })
    } else {
        None
    };
    let pts: CMTime = unsafe { sample.presentation_time_stamp() };
    let ts_us = if pts.timescale > 0 { (pts.value as i128 * 1_000_000 / pts.timescale as i128) as u64 } else { 0 };
    Ok(EncodedFrame { key, ts_us, data, config })
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::{Duration, Instant};

    use base64::Engine as _;
    use p2pcli_core::screen::VideoCodec;

    use super::super::pack::{Packer, tests::{bgra_buffer, pattern}};
    use super::*;

    /// 从 avcC / hvcC 里取出参数集，写成 Annex B 文件时放在关键帧前面
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

    /// 开一个 W×3H 的硬件编码器，送几帧叠图进去。返回出来的第一个关键帧的解码配置、平均每帧（排画面 + 编码）的时间。
    /// dump 有值时把码流写成 Annex B 文件，好拿 ffmpeg 解出来对
    /// stacked 为假时是普通的 4:2:0（Metal 转 NV12），拿来比时间
    fn try_stacked(w: usize, h: usize, codec: VideoCodec, frames: usize, stacked: bool, dump: Option<&str>) -> Result<(p2pcli_core::screen::VideoConfig, Duration)> {
        let mut packer = Packer::new(false)?;
        let coded_h = if stacked { h * 3 } else { h };
        let mut slot = Some(Encoder::open(w, coded_h, 60, 40_000_000, codec, false, true)?);
        let rgba = pattern(w, h);
        // 第二帧整体挪一个像素，编码器要真的编点东西
        let mut moved = rgba.clone();
        moved.rotate_left(4);
        let sources = [bgra_buffer(&rgba, w, h), bgra_buffer(&moved, w, h)];
        let mut file = dump.map(std::fs::File::create).transpose()?;
        if let Some(dump) = dump {
            // 第一帧该是什么样：解码出来的亮度平面拿去和它比
            std::fs::write(format!("{dump}.ref"), p2pcli_core::screen::pack_yuv444_stacked(&rgba, w, h))?;
        }
        let mut config = None;
        let mut spent = Duration::ZERO;
        for i in 0..frames {
            let t = Instant::now();
            let packed = packer.pack(&sources[i % 2], stacked)?;
            let rx = Encoder::encode(&mut slot, &packed, unsafe { CMTime::new(i as i64, 60) }, 60, 40_000_000, i == 0, codec, false)?;
            assert_eq!(slot.as_ref().unwrap().size(), (w, coded_h), "编码器不该被重建");
            let Some(f) = rx.blocking_recv()?? else { continue };
            if i > 0 {
                spent += t.elapsed();
            }
            if let Some(c) = &f.config {
                config.get_or_insert(c.clone());
                if let Some(file) = &mut file {
                    let rec = base64::engine::general_purpose::STANDARD.decode(&c.description)?;
                    for p in parameter_sets(&rec, c.codec.starts_with("hvc1")) {
                        file.write_all(&[0, 0, 0, 1])?;
                        file.write_all(&p)?;
                    }
                }
            }
            if let Some(file) = &mut file {
                let mut d = &f.data[..];
                while d.len() >= 4 {
                    let n = u32::from_be_bytes(d[..4].try_into()?) as usize;
                    file.write_all(&[0, 0, 0, 1])?;
                    file.write_all(&d[4..4 + n])?;
                    d = &d[4 + n..];
                }
            }
        }
        let config = config.ok_or_else(|| anyhow!("没出关键帧"))?;
        Ok((config, spent / (frames.max(2) - 1) as u32))
    }

    /// HEVC 1920×3240 一定要能编；别的尺寸和 H.264 只打出来看
    #[test]
    #[ignore = "要 GPU 和硬件编码器：cargo test -p p2pcli-screen -- --ignored stacked_encode --nocapture"]
    fn stacked_encode() {
        let dump = std::env::var("P2PCLI_444_DUMP").ok();
        let (config, per_frame) = try_stacked(1920, 1080, VideoCodec::H265, 20, true, dump.as_deref()).expect("HEVC 1920×3240 编不了");
        println!("HEVC 1920×3240：{} {}×{}，每帧 {per_frame:?}", config.codec, config.width, config.height);
        assert!(config.codec.starts_with("hvc1"));
        assert_eq!((config.width, config.height), (1920, 3240));

        for (codec, w, h) in [
            (VideoCodec::H265, 1920, 1080),
            (VideoCodec::H265, 2560, 1440),
            (VideoCodec::H265, 3840, 2160),
            (VideoCodec::H264, 1280, 720),
            (VideoCodec::H264, 1920, 1080),
            (VideoCodec::H264, 3840, 2160),
        ] {
            match try_stacked(w, h, codec, 20, true, None) {
                Ok((c, per_frame)) => println!("{codec:?} {w}×{}：可以，{} {}×{}，每帧 {per_frame:?}", h * 3, c.codec, c.width, c.height),
                Err(e) => println!("{codec:?} {w}×{}：不行，{e:#}", h * 3),
            }
            // 同样的画面按 4:2:0 编，比一比时间
            if let Ok((_, per_frame)) = try_stacked(w, h, codec, 20, false, None) {
                println!("    对照 4:2:0 {w}×{h}：每帧 {per_frame:?}");
            }
        }
    }
}

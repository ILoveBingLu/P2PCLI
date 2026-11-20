//! VideoToolbox 硬件 H.264 编码：低延迟码率控制、不重排帧（没有 B 帧）、只在要求时出关键帧。
//! 输入是 ScreenCaptureKit 给的 IOSurface 像素缓冲，硬件直接读，不拷贝。

use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::sync::Mutex;

use anyhow::{Result, anyhow, bail};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, kCFBooleanTrue,
};
use objc2_core_media::{
    CMFormatDescription, CMSampleBuffer, CMTime, CMVideoFormatDescriptionGetDimensions,
    CMVideoFormatDescriptionGetH264ParameterSetAtIndex, kCMSampleAttachmentKey_NotSync, kCMTimeInvalid,
};
use objc2_core_video::{
    CVImageBuffer, CVPixelBufferGetHeight, CVPixelBufferGetWidth, kCVImageBufferColorPrimaries_ITU_R_709_2,
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
    kVTProfileLevel_H264_ConstrainedHigh_AutoLevel, kVTProfileLevel_H264_High_AutoLevel,
    kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder,
    kVTVideoEncoderSpecification_EnableLowLatencyRateControl,
    kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder,
};
use p2pcli_core::screen::{EncodedFrame, avcc_config};
use tokio::sync::oneshot;
use tracing::{info, warn};


/// 'avc1'
const CODEC_H264: u32 = 0x6176_6331;

type Reply = oneshot::Sender<Result<Option<EncodedFrame>>>;

/// 编码回调要用的东西；回调在 VideoToolbox 自己的线程上跑
struct Context {
    pending: Mutex<Option<Reply>>,
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
    fn new(width: usize, height: usize, fps: u32, bitrate: u32) -> Result<Self> {
        let ctx = Box::into_raw(Box::new(Context { pending: Mutex::new(None) }));
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
        for (n, spec) in specs.iter().enumerate() {
            let keys: Vec<&CFString> = spec.iter().map(|(k, _)| *k).collect();
            let values: Vec<&CFBoolean> = spec.iter().map(|(_, v)| *v).collect();
            let dict = CFDictionary::from_slices(&keys, &values);
            let mut out: *mut VTCompressionSession = ptr::null_mut();
            let status = unsafe {
                VTCompressionSession::create(
                    None,
                    width as i32,
                    height as i32,
                    CODEC_H264,
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
            bail!("开不了 H.264 硬件编码器（{last}）");
        };

        unsafe {
            set(&session, kVTCompressionPropertyKey_RealTime, CFBoolean::new(true));
            set(&session, kVTCompressionPropertyKey_AllowFrameReordering, CFBoolean::new(false));
            let profile = if low_latency { kVTProfileLevel_H264_ConstrainedHigh_AutoLevel } else { kVTProfileLevel_H264_High_AutoLevel };
            if set(&session, kVTCompressionPropertyKey_ProfileLevel, profile) != 0 {
                set(&session, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel);
            }
            // 关键帧只在要的时候出（刚开始、解码出错、恢复直连）：可靠传输不会丢帧，定期关键帧只会白占带宽
            set(&session, kVTCompressionPropertyKey_MaxKeyFrameInterval, &num(i32::MAX / 2));
            set(&session, kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, &num(24 * 3600));
            set(&session, kVTCompressionPropertyKey_ExpectedFrameRate, &num(fps as i32));
            set(&session, kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality, CFBoolean::new(true));
            set(&session, kVTCompressionPropertyKey_ColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_709_2);
            // 电脑屏幕用的是 sRGB 曲线，不是电视的 BT.709 曲线：标错了解码端的明暗会不一样
            set(&session, kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_sRGB);
            set(&session, kVTCompressionPropertyKey_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_709_2);
        }
        let mut enc = Encoder { session, ctx, width, height, bitrate: 0 };
        enc.set_bitrate(bitrate);
        unsafe { enc.session.prepare_to_encode_frames() };
        info!(width, height, fps, bitrate, low_latency, "H.264 硬件编码器已就绪");
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
    pub fn encode(slot: &mut Option<Encoder>, image: &CVImageBuffer, pts: CMTime, fps: u32, bitrate: u32, key: bool) -> Result<oneshot::Receiver<Result<Option<EncodedFrame>>>> {
        let (w, h) = (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image));
        let mut key = key;
        if slot.as_ref().is_none_or(|e| e.width != w || e.height != h) {
            *slot = None;
            *slot = Some(Encoder::new(w, h, fps, bitrate)?);
            key = true;
        }
        let enc = slot.as_mut().unwrap();
        enc.set_bitrate(bitrate);
        let (tx, rx) = oneshot::channel();
        let ctx = unsafe { &*enc.ctx };
        *ctx.pending.lock().unwrap() = Some(tx);
        let props = key.then(|| unsafe {
            CFDictionary::<CFString, CFBoolean>::from_slices(&[kVTEncodeFrameOptionKey_ForceKeyFrame], &[CFBoolean::new(true)])
        });
        let mut flags = VTEncodeInfoFlags(0);
        let status = unsafe {
            enc.session.encode_frame(image, pts, kCMTimeInvalid, props.as_deref().map(|d| d.as_opaque()), ptr::null_mut(), &mut flags)
        };
        if status != 0 {
            ctx.pending.lock().unwrap().take();
            // 编码器坏了就重建
            *slot = None;
            bail!("硬件编码失败（{status}）");
        }
        Ok(rx)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // 先等在编的帧都回调完，再作废、放掉回调用的上下文，免得回调还在别的线程上读它
        unsafe {
            self.session.complete_frames(kCMTimeInvalid);
            self.session.invalidate();
        }
        // 作废以后不会再有回调，可以放掉
        drop(unsafe { Box::from_raw(self.ctx) });
    }
}

unsafe extern "C-unwind" fn on_encoded(refcon: *mut c_void, _frame: *mut c_void, status: i32, _flags: VTEncodeInfoFlags, sample: *mut CMSampleBuffer) {
    let ctx = unsafe { &*(refcon as *const Context) };
    let result = if status != 0 {
        Err(anyhow!("硬件编码失败（{status}）"))
    } else if let Some(sample) = unsafe { sample.as_ref() } {
        extract(sample).map(Some)
    } else {
        Ok(None)
    };
    if let Some(tx) = ctx.pending.lock().unwrap().take() {
        let _ = tx.send(result);
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

fn extract(sample: &CMSampleBuffer) -> Result<EncodedFrame> {
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
        let sps = parameter_set(&fd, 0)?;
        let pps = parameter_set(&fd, 1)?;
        let dims = unsafe { CMVideoFormatDescriptionGetDimensions(&fd) };
        Some(avcc_config(&sps, &pps, dims.width as u32, dims.height as u32)?)
    } else {
        None
    };
    let pts: CMTime = unsafe { sample.presentation_time_stamp() };
    let ts_us = if pts.timescale > 0 { (pts.value as i128 * 1_000_000 / pts.timescale as i128) as u64 } else { 0 };
    Ok(EncodedFrame { key, ts_us, data, config })
}

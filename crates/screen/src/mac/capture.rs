//! ScreenCaptureKit 抓屏：在 GPU 里缩放、转成 NV12（视频范围、BT.709），画面一直在 IOSurface 里，
//! 编码器直接拿去用。屏幕没变化时系统不给新帧，不占带宽。
//!
//! 4:4:4：系统抓成 BGRA，再用 Metal 排成三倍高的灰度图（见 [`super::pack`]）交给编码器。
//! 编码器开不了这么高的画面时，同样用 Metal 转成普通的 NV12，退回 4:2:0。
//!
//! 抓到的画面交给 [`super::flow`]：单独的编码线程边排边编，不等上一帧编完。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayCopyDisplayMode, CGDisplayIsBuiltin, CGDisplayMode, CGGetActiveDisplayList, CGMainDisplayID,
    kCGColorSpaceDisplayP3, kCGColorSpaceSRGB, kCGDisplayStreamYCbCrMatrix_ITU_R_709_2,
};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamDelegate, SCStreamOutput,
    SCStreamOutputType,
};
use p2pcli_core::host::BoxFuture;
use p2pcli_core::screen::{CaptureOptions, CaptureStats, ColorFormat, DisplayInfo, EncodedFrame, Primaries, ScreenCapture, VideoCodec};
use tracing::{debug, info, warn};

use super::encode::Encoder;
use super::flow::{Frames, MODE_NV12, MODE_STACKED, PIPELINED, Pipe};
use super::pack::{PIXEL_FORMAT_BGRA, Packer};

/// kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange（'420v'）：硬件编码器最喜欢的格式
const PIXEL_FORMAT_NV12: u32 = 0x3432_3076;
/// kCVPixelFormatType_420YpCbCr8BiPlanarFullRange（'420f'）：同样的排法，用满 0~255
const PIXEL_FORMAT_NV12_FULL: u32 = 0x3432_3066;

define_class!(
    // SAFETY: 只继承 NSObject，没有重写 dealloc
    #[unsafe(super(NSObject))]
    #[name = "P2PCLIScreenSink"]
    #[ivars = Arc<Frames>]
    struct Sink;

    unsafe impl NSObjectProtocol for Sink {}

    unsafe impl SCStreamOutput for Sink {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind != SCStreamOutputType::Screen {
                return;
            }
            // 屏幕没变化（idle）、黑屏等状态帧不带画面
            if let Some(image) = unsafe { sample.image_buffer() } {
                self.ivars().deliver(image);
            }
        }
    }

    unsafe impl SCStreamDelegate for Sink {
        #[unsafe(method(stream:didStopWithError:))]
        unsafe fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            let message = error.localizedDescription().to_string();
            warn!("抓屏停止了：{message}");
            self.ivars().stop(message);
        }
    }
);

impl Sink {
    fn new(shared: Arc<Frames>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(shared);
        unsafe { msg_send![super(this), init] }
    }
}

/// 屏幕的刷新率（Hz）。ProMotion 报 120；老的内建屏报 0（读不到），那时按 60 算
pub(crate) fn refresh_hz(id: CGDirectDisplayID) -> u32 {
    let mode = CGDisplayCopyDisplayMode(id);
    let hz = CGDisplayMode::refresh_rate(mode.as_deref());
    if hz.is_finite() && hz >= 1.0 { hz.round() as u32 } else { 0 }
}

/// 一块屏幕的像素尺寸（Retina 屏是实际像素，不是点）
pub(crate) fn pixel_size(id: CGDirectDisplayID) -> (u32, u32) {
    let mode = CGDisplayCopyDisplayMode(id);
    let w = CGDisplayMode::pixel_width(mode.as_deref());
    let h = CGDisplayMode::pixel_height(mode.as_deref());
    (w as u32, h as u32)
}

pub fn displays() -> Result<Vec<DisplayInfo>> {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    let err = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    if err.0 != 0 {
        bail!("读不到屏幕列表（{}）", err.0);
    }
    let main = CGMainDisplayID();
    let mut external = 0;
    Ok(ids[..count as usize]
        .iter()
        .map(|&id| {
            let (width, height) = pixel_size(id);
            let name = if CGDisplayIsBuiltin(id) {
                "内建显示器".to_string()
            } else {
                external += 1;
                format!("显示器 {external}")
            };
            DisplayInfo { id: id.to_string(), name, width, height, primary: id == main, refresh_hz: refresh_hz(id) }
        })
        .collect())
}

/// 等一个带完成回调的系统调用，最多等 timeout
pub(crate) fn wait<T: Send + 'static>(timeout: Duration, start: impl FnOnce(std::sync::mpsc::Sender<T>)) -> Result<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    start(tx);
    rx.recv_timeout(timeout).map_err(|_| anyhow!("系统没有及时响应"))
}

pub(crate) struct SendRetained<T>(pub(crate) T);
unsafe impl<T> Send for SendRetained<T> {}

pub(crate) fn find_display(id: CGDirectDisplayID) -> Result<Retained<SCDisplay>> {
    let content = wait(Duration::from_secs(5), |tx| {
        let block = block2::RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
            let result = if content.is_null() {
                let message = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string()).unwrap_or_default();
                Err(message)
            } else {
                Ok(SendRetained(unsafe { Retained::retain(content) }.unwrap()))
            };
            let _ = tx.send(result);
        });
        unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    })?
    .map_err(|e| anyhow!("读不到可以抓的屏幕：{e}"))?
    .0;
    let displays = unsafe { content.displays() };
    displays
        .iter()
        .find(|d| unsafe { d.displayID() } == id)
        .ok_or_else(|| anyhow!("找不到这块屏幕，可能已经拔掉了"))
}

/// Mac 上 4:4:4 时画面长边最多这么大：M4 上 2560×4320 每帧要 27 ms，只有三十多帧；1920×3240 流水线能稳在 60 帧
const YUV444_MAX_LONG_EDGE: u32 = 1920;
/// 长边超过这么多（5K、6K 屏的原生分辨率）要先试开一下编码器：H.265 硬件编码器才编得了，开不了就缩到这么大
const LARGE_LONG_EDGE: u32 = 3840;

/// 按长边上限等比缩小，宽高取偶数（H.264 要求）
fn fit(w: u32, h: u32, max_long_edge: u32) -> (u32, u32) {
    let long = w.max(h).max(1);
    let scale = (max_long_edge as f64 / long as f64).min(1.0);
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(w as f64 * scale), even(h as f64 * scale))
}

pub struct MacCapture {
    stream: Retained<SCStream>,
    _sink: Retained<Sink>,
    _queue: DispatchRetained<DispatchQueue>,
    frames: Arc<Frames>,
}

// SCStream 等对象只在创建和停止时碰，系统允许跨线程调用
unsafe impl Send for MacCapture {}
unsafe impl Sync for MacCapture {}

impl MacCapture {
    pub fn start(opts: CaptureOptions) -> Result<Self> {
        let id: CGDirectDisplayID = opts.display.parse().context("屏幕编号不对")?;
        let display = find_display(id)?;
        let (pw, ph) = pixel_size(id);
        let (mut width, mut height) = fit(pw, ph, opts.max_long_edge);
        let p3 = opts.wide_gamut;
        // 4:4:4 要编三倍高的画面，只在不用缩小就放得下时用。高分屏不为了 4:4:4 缩小，用原生分辨率的 4:2:0
        let yuv444 = opts.yuv444 && width.max(height) <= YUV444_MAX_LONG_EDGE;

        // 4:4:4：先把 Metal 和三倍高的硬件编码器开起来，开不了就照旧抓 NV12
        let mut packer = None;
        let mut encoder = None;
        if yuv444 && opts.full_range {
            let ready = Packer::new(p3).and_then(|p| {
                let enc = Encoder::new(width as usize, height as usize * 3, opts.fps, opts.bitrate, opts.codec, p3, true)?;
                Ok((p, enc))
            });
            match ready {
                Ok((p, enc)) => {
                    packer = Some(p);
                    encoder = Some(enc);
                }
                Err(e) => warn!("4:4:4 开不起来，用 4:2:0：{e:#}"),
            }
        }
        let mode = if packer.is_some() { MODE_STACKED } else { MODE_NV12 };
        // 5K、6K 的原生分辨率：只有 H.265 硬件编码器编得了（H.264 最多 4096 宽，观看方也多半解不了）。
        // 先开好留给编码线程用，开不了就缩到 3840
        if mode == MODE_NV12 && width.max(height) > LARGE_LONG_EDGE {
            match Encoder::open_hevc(width as usize, height as usize, opts.fps, opts.bitrate, p3) {
                Ok(enc) if opts.codec == VideoCodec::H265 => encoder = Some(enc),
                result => {
                    if let Err(e) = result {
                        warn!(width, height, "编不了原生分辨率，缩到 {LARGE_LONG_EDGE}：{e:#}");
                    }
                    (width, height) = fit(pw, ph, LARGE_LONG_EDGE);
                }
            }
        }

        let config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            config.setWidth(width as usize);
            config.setHeight(height as usize);
            config.setPixelFormat(match mode {
                MODE_NV12 if opts.full_range => PIXEL_FORMAT_NV12_FULL,
                MODE_NV12 => PIXEL_FORMAT_NV12,
                _ => PIXEL_FORMAT_BGRA,
            });
            config.setColorMatrix(kCGDisplayStreamYCbCrMatrix_ITU_R_709_2);
            // Mac 屏幕多是广色域（Display P3）。观看方也是广色域屏幕就按 P3 抓，鲜艳的颜色不被压掉；
            // 不然让系统抓屏时就转成 sRGB，免得 P3 的数值被当成 sRGB 显示，颜色发灰发淡
            config.setColorSpaceName(if opts.wide_gamut { kCGColorSpaceDisplayP3 } else { kCGColorSpaceSRGB });
            // 最短间隔比一帧短一成：正好卡在 1/60 秒时，屏幕刷新稍微早一点的那帧会被系统扔掉（120 Hz 的屏幕
            // 上隔一帧取一帧，抖一下就变成隔两帧），帧率到不了 60。短一成以后最多比 fps 多一成
            config.setMinimumFrameInterval(CMTime::new(9, 10 * opts.fps.max(1) as i32));
            // 能单独读指针时不画进画面：观看方用本机的系统指针画，跟手
            config.setShowsCursor(opts.show_cursor);
            config.setScalesToFit(true);
            // 编码器里最多两帧，这里存着最新的和上一次编的，系统那边还要有空闲的缓冲
            config.setQueueDepth(6);
        }
        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &NSArray::new())
        };

        let color = ColorFormat { full_range: opts.full_range, primaries: if p3 { Primaries::P3 } else { Primaries::Bt709 }, ..Default::default() };
        let pipe = Pipe { encoder, packer, mode, fps: opts.fps, codec: opts.codec, p3 };
        let frames = Frames::start(pipe, color, opts.bitrate, PIPELINED)?;
        let sink = Sink::new(frames.clone());
        let queue = DispatchQueue::new("cn.p2pcli.screen", None);
        let delegate = ProtocolObject::from_ref(&*sink);
        let stream = unsafe { SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, Some(delegate)) };
        unsafe {
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(ProtocolObject::from_ref(&*sink), SCStreamOutputType::Screen, Some(&queue))
                .map_err(|e| anyhow!("抓屏开不起来：{}", e.localizedDescription()))?;
        }
        let started = wait(Duration::from_secs(5), |tx| {
            let block = block2::RcBlock::new(move |error: *mut NSError| {
                let result = match unsafe { error.as_ref() } {
                    Some(e) => Err(e.localizedDescription().to_string()),
                    None => Ok(()),
                };
                let _ = tx.send(result);
            });
            unsafe { stream.startCaptureWithCompletionHandler(Some(&block)) };
        })?;
        if let Err(e) = started {
            bail!("抓屏开不起来：{e}");
        }
        info!(display = id, width, height, fps = opts.fps, yuv444 = mode == MODE_STACKED, "开始抓屏");
        Ok(MacCapture { stream, _sink: sink, _queue: queue, frames })
    }
}

impl Drop for MacCapture {
    fn drop(&mut self) {
        unsafe { self.stream.stopCaptureWithCompletionHandler(None) };
        self.frames.close();
        debug!("停止抓屏");
    }
}

impl ScreenCapture for MacCapture {
    fn next_frame(&self) -> BoxFuture<Result<EncodedFrame>> {
        self.frames.next_frame()
    }

    fn request_keyframe(&self) {
        self.frames.request_keyframe();
    }

    fn set_bitrate(&self, bps: u32) {
        self.frames.set_bitrate(bps);
    }

    fn color(&self) -> ColorFormat {
        self.frames.color()
    }

    fn stats(&self) -> Option<CaptureStats> {
        Some(self.frames.stats())
    }

    fn encoder(&self) -> Option<p2pcli_core::screen::EncoderInfo> {
        self.frames.encoder()
    }
}

#[cfg(test)]
mod tests {
    use super::fit;

    #[test]
    fn fit_keeps_aspect_and_even() {
        assert_eq!(fit(2880, 1800, 1920), (1920, 1200));
        assert_eq!(fit(1280, 720, 1920), (1280, 720));
        assert_eq!(fit(3025, 1965, 1920), (1920, 1246));
        // 清晰档不缩小：5K 屏原样
        assert_eq!(fit(5120, 2880, 8192), (5120, 2880));
    }
}

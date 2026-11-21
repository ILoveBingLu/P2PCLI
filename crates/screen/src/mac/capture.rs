//! ScreenCaptureKit 抓屏：在 GPU 里缩放、转成 NV12（视频范围、BT.709），画面一直在 IOSurface 里，
//! 编码器直接拿去用。屏幕没变化时系统不给新帧，不占带宽。
//!
//! 4:4:4：系统抓成 BGRA，再用 Metal 排成三倍高的灰度图（见 [`super::pack`]）交给编码器。
//! 编码器开不了这么高的画面时，同样用 Metal 转成普通的 NV12，退回 4:2:0。

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_foundation::{CFRetained, Type};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayCopyDisplayMode, CGDisplayIsBuiltin, CGDisplayMode, CGGetActiveDisplayList, CGMainDisplayID,
    kCGColorSpaceDisplayP3, kCGColorSpaceSRGB, kCGDisplayStreamYCbCrMatrix_ITU_R_709_2,
};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{CVImageBuffer, CVPixelBufferGetHeight, CVPixelBufferGetWidth};

use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamDelegate, SCStreamOutput,
    SCStreamOutputType,
};
use p2pcli_core::host::BoxFuture;
use p2pcli_core::screen::{CaptureOptions, ColorFormat, DisplayInfo, EncodedFrame, Layout, Primaries, ScreenCapture, VideoCodec};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use super::encode::Encoder;
use super::pack::{PIXEL_FORMAT_BGRA, Packer};

/// kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange（'420v'）：硬件编码器最喜欢的格式
const PIXEL_FORMAT_NV12: u32 = 0x3432_3076;
/// kCVPixelFormatType_420YpCbCr8BiPlanarFullRange（'420f'）：同样的排法，用满 0~255
const PIXEL_FORMAT_NV12_FULL: u32 = 0x3432_3066;

/// 送进编码器的是什么画面：系统直接给的 NV12（4:2:0）
const MODE_NV12: u8 = 0;
/// 系统给 BGRA，Metal 排成 4:4:4 叠图（W×3H）
const MODE_STACKED: u8 = 1;
/// 系统给 BGRA，Metal 转成 NV12 全范围：4:4:4 中途开不了编码器时退到这里
const MODE_PACKED_NV12: u8 = 2;

/// 抓到的一帧，CF 对象在线程间传递是安全的（只读）
#[derive(Clone)]
pub(super) struct Sample(pub CFRetained<CMSampleBuffer>);
unsafe impl Send for Sample {}

#[derive(Default)]
struct Shared {
    /// 最新的一帧：编码器来取之前被新的覆盖，旧的直接丢掉
    latest: Mutex<Option<Sample>>,
    /// 上一次编码用的那帧：屏幕不动时要关键帧（恢复直连、观看方解码出错），就拿它重新编
    last: Mutex<Option<Sample>>,
    notify: tokio::sync::Notify,
    stopped: Mutex<Option<String>>,
}

define_class!(
    // SAFETY: 只继承 NSObject，没有重写 dealloc
    #[unsafe(super(NSObject))]
    #[name = "P2PCLIScreenSink"]
    #[ivars = Arc<Shared>]
    struct Sink;

    unsafe impl NSObjectProtocol for Sink {}

    unsafe impl SCStreamOutput for Sink {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind != SCStreamOutputType::Screen {
                return;
            }
            // 屏幕没变化（idle）、黑屏等状态帧不带画面
            if unsafe { sample.image_buffer() }.is_none() {
                return;
            }
            let shared = self.ivars();
            *shared.latest.lock().unwrap() = Some(Sample(sample.retain()));
            shared.notify.notify_one();
        }
    }

    unsafe impl SCStreamDelegate for Sink {
        #[unsafe(method(stream:didStopWithError:))]
        unsafe fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            let message = error.localizedDescription().to_string();
            warn!("抓屏停止了：{message}");
            let shared = self.ivars();
            *shared.stopped.lock().unwrap() = Some(message);
            shared.notify.notify_one();
        }
    }
);

impl Sink {
    fn new(shared: Arc<Shared>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(shared);
        unsafe { msg_send![super(this), init] }
    }
}

/// 一块屏幕的像素尺寸（Retina 屏是实际像素，不是点）
fn pixel_size(id: CGDirectDisplayID) -> (u32, u32) {
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
            DisplayInfo { id: id.to_string(), name, width, height, primary: id == main }
        })
        .collect())
}

/// 等一个带完成回调的系统调用，最多等 timeout
fn wait<T: Send + 'static>(timeout: Duration, start: impl FnOnce(std::sync::mpsc::Sender<T>)) -> Result<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    start(tx);
    rx.recv_timeout(timeout).map_err(|_| anyhow!("系统没有及时响应"))
}

struct SendRetained<T>(T);
unsafe impl<T> Send for SendRetained<T> {}

fn find_display(id: CGDirectDisplayID) -> Result<Retained<SCDisplay>> {
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
    shared: Arc<Shared>,
    pipe: Pipe,
}

/// 抓到的画面怎么送进编码器。next_frame 每次拿一份（里面都是共享的），同一时间只有一个在用
#[derive(Clone)]
struct Pipe {
    encoder: Arc<Mutex<Option<Encoder>>>,
    /// 4:4:4 时在 GPU 上排画面
    packer: Option<Arc<Mutex<Packer>>>,
    /// 现在送进编码器的是哪种画面（MODE_*）。从 4:4:4 退回 4:2:0 时，
    /// 在编出那个关键帧之前改好，取解码配置时 color() 和关键帧对得上
    mode: Arc<AtomicU8>,
    fps: u32,
    /// 想用的编码格式（编不了 H.265 时编码器自己退回 H.264）
    codec: VideoCodec,
    color: ColorFormat,
    bitrate: Arc<AtomicU32>,
    want_key: Arc<AtomicBool>,
    /// 时间戳从这里算：同一帧重新编码时时间戳也要往前走
    started: std::time::Instant,
}

/// 送进编码器的一帧，等它的结果
struct Submitted {
    rx: oneshot::Receiver<Result<Option<EncodedFrame>>>,
    key: bool,
    /// 按 4:4:4 编的：编坏了就退回 4:2:0 重来，不让画面断掉
    stacked: bool,
}

/// 要关键帧时最多等新画面这么久，等不到就拿上一帧重新编
const REUSE_AFTER: Duration = Duration::from_millis(100);

// SCStream 等对象只在创建和停止时碰，系统允许跨线程调用
unsafe impl Send for MacCapture {}
unsafe impl Sync for MacCapture {}

impl MacCapture {
    pub fn start(opts: CaptureOptions) -> Result<Self> {
        let id: CGDirectDisplayID = opts.display.parse().context("屏幕编号不对")?;
        let display = find_display(id)?;
        let (pw, ph) = pixel_size(id);
        let (width, height) = fit(pw, ph, opts.max_long_edge);
        let p3 = opts.wide_gamut;

        // 4:4:4：先把 Metal 和三倍高的硬件编码器开起来，开不了就照旧抓 NV12
        let mut packer = None;
        let mut encoder = None;
        if opts.yuv444 && opts.full_range {
            let ready = Packer::new(p3).and_then(|p| {
                let enc = Encoder::new(width as usize, height as usize * 3, opts.fps, opts.bitrate, opts.codec, p3, true)?;
                Ok((p, enc))
            });
            match ready {
                Ok((p, enc)) => {
                    packer = Some(Arc::new(Mutex::new(p)));
                    encoder = Some(enc);
                }
                Err(e) => warn!("4:4:4 开不起来，用 4:2:0：{e:#}"),
            }
        }
        let mode = if packer.is_some() { MODE_STACKED } else { MODE_NV12 };

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
            config.setMinimumFrameInterval(CMTime::new(1, opts.fps.max(1) as i32));
            // 能单独读指针时不画进画面：观看方用本机的系统指针画，跟手
            config.setShowsCursor(opts.show_cursor);
            config.setScalesToFit(true);
            // 编码器拿着一帧、这里存着一帧，系统那边还要有空闲的缓冲
            config.setQueueDepth(5);
        }
        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &NSArray::new())
        };

        let shared = Arc::new(Shared::default());
        let sink = Sink::new(shared.clone());
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
        Ok(MacCapture {
            stream,
            _sink: sink,
            _queue: queue,
            shared,
            pipe: Pipe {
                encoder: Arc::new(Mutex::new(encoder)),
                packer,
                mode: Arc::new(AtomicU8::new(mode)),
                fps: opts.fps,
                codec: opts.codec,
                color: ColorFormat { full_range: opts.full_range, primaries: if p3 { Primaries::P3 } else { Primaries::Bt709 }, ..Default::default() },
                bitrate: Arc::new(AtomicU32::new(opts.bitrate)),
                want_key: Arc::new(AtomicBool::new(true)),
                started: std::time::Instant::now(),
            },
        })
    }
}

impl Drop for MacCapture {
    fn drop(&mut self) {
        unsafe { self.stream.stopCaptureWithCompletionHandler(None) };
        debug!("停止抓屏");
    }
}

impl ScreenCapture for MacCapture {
    fn next_frame(&self) -> BoxFuture<Result<EncodedFrame>> {
        let shared = self.shared.clone();
        let pipe = self.pipe.clone();
        Box::pin(async move {
            loop {
                let sample = loop {
                    if let Some(message) = shared.stopped.lock().unwrap().clone() {
                        bail!("抓屏停止了：{message}");
                    }
                    if let Some(s) = shared.latest.lock().unwrap().take() {
                        *shared.last.lock().unwrap() = Some(s.clone());
                        break s;
                    }
                    let reuse = pipe.want_key.load(Ordering::Relaxed).then(|| shared.last.lock().unwrap().clone()).flatten();
                    match reuse {
                        // 要关键帧但屏幕没动：等一小会儿新画面，等不到就重编上一帧
                        Some(last) => {
                            if tokio::time::timeout(REUSE_AFTER, shared.notify.notified()).await.is_err() {
                                break last;
                            }
                        }
                        None => shared.notify.notified().await,
                    }
                };
                let submitted = {
                    let Some(image) = (unsafe { sample.0.image_buffer() }) else { continue };
                    pipe.submit(&image)?
                };
                // 刚退回 4:2:0：要关键帧，拿新画面（或者上一帧）重来
                let Some(submitted) = submitted else { continue };
                if let Some(frame) = pipe.finish(submitted).await? {
                    return Ok(frame);
                }
            }
        })
    }

    fn request_keyframe(&self) {
        self.pipe.want_key.store(true, Ordering::Relaxed);
    }

    fn set_bitrate(&self, bps: u32) {
        self.pipe.bitrate.store(bps, Ordering::Relaxed);
    }

    fn color(&self) -> ColorFormat {
        self.pipe.color()
    }
}

impl Pipe {
    fn p3(&self) -> bool {
        self.color.primaries == Primaries::P3
    }

    fn color(&self) -> ColorFormat {
        match self.mode.load(Ordering::Relaxed) {
            MODE_NV12 => self.color,
            // Metal 出的都是全范围
            MODE_STACKED => ColorFormat { full_range: true, layout: Layout::Yuv444Stacked, ..self.color },
            _ => ColorFormat { full_range: true, layout: Layout::Yuv420, ..self.color },
        }
    }

    /// 把一帧送进编码器（4:4:4 时先在 GPU 上排好）。返回空：4:4:4 刚编不了、退回了 4:2:0，要重来
    fn submit(&self, image: &CVImageBuffer) -> Result<Option<Submitted>> {
        let mut key = self.want_key.swap(false, Ordering::Relaxed);
        let bitrate = self.bitrate.load(Ordering::Relaxed);
        let mut guard = self.encoder.lock().unwrap();
        let pts = unsafe { CMTime::new(self.started.elapsed().as_micros() as i64, 1_000_000) };
        let mut stacked = false;
        let packed = match &self.packer {
            Some(packer) => {
                key |= self.prepare_stacked(&mut guard, image, bitrate);
                stacked = self.mode.load(Ordering::Relaxed) == MODE_STACKED;
                match packer.lock().unwrap().pack(image, stacked) {
                    Ok(b) => Some(b),
                    Err(e) => {
                        self.want_key.store(true, Ordering::Relaxed);
                        return Err(e);
                    }
                }
            }
            None => None,
        };
        let image = packed.as_deref().unwrap_or(image);
        match Encoder::encode(&mut guard, image, pts, self.fps, bitrate, key, self.codec, self.p3()) {
            Ok(rx) => Ok(Some(Submitted { rx, key, stacked })),
            Err(e) if stacked => {
                *guard = None;
                self.fall_back(&e);
                Ok(None)
            }
            Err(e) => {
                self.want_key.store(true, Ordering::Relaxed);
                Err(e)
            }
        }
    }

    /// 等编码结果。返回空：这帧没编出来（编码器丢了，或者 4:4:4 编坏了刚退回 4:2:0），接着要下一帧
    async fn finish(&self, s: Submitted) -> Result<Option<EncodedFrame>> {
        match tokio::time::timeout(Duration::from_secs(3), s.rx).await {
            Ok(Ok(Ok(Some(frame)))) => Ok(Some(frame)),
            // 编码器丢了这帧：接着等下一帧；丢的是要求的关键帧就再要一次
            Ok(Ok(Ok(None))) => {
                if s.key {
                    self.want_key.store(true, Ordering::Relaxed);
                }
                Ok(None)
            }
            // 有的尺寸编码器开得起来、一编就失败（比如 H.264 的 3840×6480）
            Ok(Ok(Err(e))) if s.stacked => {
                *self.encoder.lock().unwrap() = None;
                self.fall_back(&e);
                Ok(None)
            }
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(_)) => bail!("编码器没有回应"),
            Err(_) => bail!("硬件编码超时"),
        }
    }

    /// 4:4:4 时编码器要按 W×3H 开（只要硬件编码器）。没开或者尺寸不对就重开，开不了就退回 4:2:0。
    /// 返回是不是新开了编码器（要出关键帧）
    fn prepare_stacked(&self, slot: &mut Option<Encoder>, image: &CVImageBuffer, bitrate: u32) -> bool {
        if self.mode.load(Ordering::Relaxed) != MODE_STACKED {
            return false;
        }
        let (w, h) = (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image));
        if slot.as_ref().is_some_and(|e| e.size() == (w, h * 3)) {
            return false;
        }
        *slot = None;
        match Encoder::new(w, h * 3, self.fps, bitrate, self.codec, self.p3(), true) {
            Ok(enc) => {
                *slot = Some(enc);
                true
            }
            Err(e) => {
                self.fall_back(&e);
                false
            }
        }
    }

    /// 4:4:4 编不了：之后一直用 Metal 转普通的 NV12。编码器随后按 W×H 重开（Encoder::encode 里看尺寸），
    /// 它出第一个关键帧之前 mode 已经改好，color() 报的和这个关键帧对得上。只退一次，警告也只记一次
    fn fall_back(&self, e: &anyhow::Error) {
        self.want_key.store(true, Ordering::Relaxed);
        if self.mode.swap(MODE_PACKED_NV12, Ordering::Relaxed) == MODE_STACKED {
            warn!("编码器编不了 4:4:4 的三倍高画面，退回 4:2:0：{e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::pack::tests::{bgra_buffer, pattern};
    use super::*;

    /// 不抓屏，直接拿合成的 BGRA 画面走 4:4:4 的那条路
    fn stacked_pipe(codec: VideoCodec, p3: bool) -> Pipe {
        Pipe {
            encoder: Arc::new(Mutex::new(None)),
            packer: Some(Arc::new(Mutex::new(Packer::new(p3).unwrap()))),
            mode: Arc::new(AtomicU8::new(MODE_STACKED)),
            fps: 60,
            codec,
            color: ColorFormat { full_range: true, primaries: if p3 { Primaries::P3 } else { Primaries::Bt709 }, ..Default::default() },
            bitrate: Arc::new(AtomicU32::new(20_000_000)),
            want_key: Arc::new(AtomicBool::new(true)),
            started: std::time::Instant::now(),
        }
    }

    /// 一直送同一帧，直到编出一帧来（中途退回 4:2:0 也算）
    fn encode_one(pipe: &Pipe, image: &CVImageBuffer) -> EncodedFrame {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        for _ in 0..4 {
            let Some(s) = pipe.submit(image).unwrap() else { continue };
            if let Some(frame) = rt.block_on(pipe.finish(s)).unwrap() {
                return frame;
            }
        }
        panic!("编不出帧");
    }

    /// 能编 4:4:4 时出 W×3H 的关键帧、报叠图；编不了（H.264 的 3840×6480）时退回 W×H 的 4:2:0，报的也跟着变
    #[test]
    #[ignore = "要 GPU 和硬件编码器：cargo test -p p2pcli-screen -- --ignored stacked_pipe --nocapture"]
    fn stacked_pipe_reports_layout() {
        let (w, h) = (1920, 1080);
        let src = bgra_buffer(&pattern(w, h), w, h);
        let pipe = stacked_pipe(VideoCodec::H265, true);
        let frame = encode_one(&pipe, &src);
        let config = frame.config.expect("第一帧要带解码配置");
        assert!(frame.key);
        assert_eq!((config.width, config.height), (1920, 3240));
        assert_eq!(pipe.color(), ColorFormat { full_range: true, primaries: Primaries::P3, layout: Layout::Yuv444Stacked });
        // 接着编的帧不用关键帧
        let next = encode_one(&pipe, &src);
        assert!(!next.key);

        let (w, h) = (3840, 2160);
        let src = bgra_buffer(&pattern(w, h), w, h);
        let pipe = stacked_pipe(VideoCodec::H264, false);
        let frame = encode_one(&pipe, &src);
        let config = frame.config.expect("退回以后第一帧要带解码配置");
        assert!(frame.key);
        println!("H.264 {w}×{h} 的 4:4:4：{} {}×{}，{:?}", config.codec, config.width, config.height, pipe.color());
        assert_eq!((config.width, config.height), (3840, 2160));
        assert_eq!(pipe.color(), ColorFormat { full_range: true, primaries: Primaries::Bt709, layout: Layout::Yuv420 });
    }

    #[test]
    fn fit_keeps_aspect_and_even() {
        assert_eq!(fit(2880, 1800, 1920), (1920, 1200));
        assert_eq!(fit(1280, 720, 1920), (1280, 720));
        assert_eq!(fit(3025, 1965, 1920), (1920, 1246));
    }
}

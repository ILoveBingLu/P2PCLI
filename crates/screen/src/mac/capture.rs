//! ScreenCaptureKit 抓屏：在 GPU 里缩放、转成 NV12（视频范围、BT.709），画面一直在 IOSurface 里，
//! 编码器直接拿去用。屏幕没变化时系统不给新帧，不占带宽。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
    kCGDisplayStreamYCbCrMatrix_ITU_R_709_2,
};
use objc2_core_media::{CMSampleBuffer, CMTime};

use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamDelegate, SCStreamOutput,
    SCStreamOutputType,
};
use p2pcli_core::host::BoxFuture;
use p2pcli_core::screen::{CaptureOptions, DisplayInfo, EncodedFrame, ScreenCapture};
use tracing::{debug, info, warn};

use super::encode::Encoder;

/// kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange（'420v'）：硬件编码器最喜欢的格式
const PIXEL_FORMAT_NV12: u32 = 0x3432_3076;

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
    encoder: Arc<Mutex<Option<Encoder>>>,
    fps: u32,
    bitrate: Arc<AtomicU32>,
    want_key: Arc<AtomicBool>,
    /// 时间戳从这里算：同一帧重新编码时时间戳也要往前走
    started: std::time::Instant,
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

        let config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            config.setWidth(width as usize);
            config.setHeight(height as usize);
            config.setPixelFormat(PIXEL_FORMAT_NV12);
            config.setColorMatrix(kCGDisplayStreamYCbCrMatrix_ITU_R_709_2);
            config.setMinimumFrameInterval(CMTime::new(1, opts.fps.max(1) as i32));
            config.setShowsCursor(true);
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
        info!(display = id, width, height, fps = opts.fps, "开始抓屏");
        Ok(MacCapture {
            stream,
            _sink: sink,
            _queue: queue,
            shared,
            encoder: Arc::new(Mutex::new(None)),
            fps: opts.fps,
            bitrate: Arc::new(AtomicU32::new(opts.bitrate)),
            want_key: Arc::new(AtomicBool::new(true)),
            started: std::time::Instant::now(),
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
        let encoder = self.encoder.clone();
        let bitrate = self.bitrate.clone();
        let want_key = self.want_key.clone();
        let fps = self.fps;
        let started = self.started;
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
                    let reuse = want_key.load(Ordering::Relaxed).then(|| shared.last.lock().unwrap().clone()).flatten();
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
                let key = want_key.swap(false, Ordering::Relaxed);
                let rx = {
                    let mut guard = encoder.lock().unwrap();
                    let Some(image) = (unsafe { sample.0.image_buffer() }) else { continue };
                    let pts = unsafe { CMTime::new(started.elapsed().as_micros() as i64, 1_000_000) };
                    match Encoder::encode(&mut guard, &image, pts, fps, bitrate.load(Ordering::Relaxed), key) {
                        Ok(rx) => rx,
                        Err(e) => {
                            want_key.store(true, Ordering::Relaxed);
                            return Err(e);
                        }
                    }
                };
                match tokio::time::timeout(Duration::from_secs(3), rx).await {
                    Ok(Ok(Ok(Some(frame)))) => return Ok(frame),
                    // 编码器丢了这帧：接着等下一帧；丢的是要求的关键帧就再要一次
                    Ok(Ok(Ok(None))) => {
                        if key {
                            want_key.store(true, Ordering::Relaxed);
                        }
                        continue;
                    }
                    Ok(Ok(Err(e))) => return Err(e),
                    Ok(Err(_)) => bail!("编码器没有回应"),
                    Err(_) => bail!("硬件编码超时"),
                }
            }
        })
    }

    fn request_keyframe(&self) {
        self.want_key.store(true, Ordering::Relaxed);
    }

    fn set_bitrate(&self, bps: u32) {
        self.bitrate.store(bps, Ordering::Relaxed);
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
    }
}

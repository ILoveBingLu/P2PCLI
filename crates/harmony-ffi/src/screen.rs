//! 远程画面：鸿蒙上看对方电脑的屏幕，用触屏操作它。
//!
//! 和安卓、iOS 不一样的地方：画面帧不交给 ArkTS（ArkTS 没有能喂裸 H.264/H.265 帧的解码接口），
//! Rust 收到以后直接喂给系统的硬件解码器（AVCodec Kit 的 OH_VideoDecoder，纯 C 接口），
//! 解出来直接画到界面上 XComponent 的 Surface 里。ArkTS 只把 XComponent 的 surfaceId 交过来、收状态、发键鼠。
//!
//! 解码器只收 Annex-B（00 00 00 01 开头），对方发的是 AVCC（每个 NAL 前面几个字节的长度），喂之前换掉；
//! SPS、PPS（H.265 还有 VPS）从 avcC、hvcC 里拿出来，作为 CODEC_DATA 先送一次。
//! 最低 API 12（OH_NativeWindow_CreateNativeWindowFromSurfaceId、低延迟解码），和 App 的最低版本一样。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use base64::Engine;
use p2pcli_mobile as ffi;
use serde_json::json;

/// 状态变化交给 ArkTS（JSON）
pub(crate) type Emit = Box<dyn Fn(serde_json::Value) + Send + Sync>;

/// 一帧要喂的数据：已经是 Annex-B
struct Frame {
    data: Vec<u8>,
    key: bool,
    /// SPS、PPS 这些参数集
    codec_data: bool,
}

/// 喂解码器、换显示的地方。所有调用都可能来自不同的线程（ArkTS 主线程、Rust 收画面的线程、解码器的回调线程），
/// 状态都在一把锁里；建、拆解码器另有一把锁，同一时间只有一个在做
pub(crate) struct Decoder {
    state: Mutex<State>,
    rebuild: Mutex<()>,
    /// 要对方发一个关键帧
    need_keyframe: Box<dyn Fn() + Send + Sync>,
    /// 这台手机解不了这么大的画面（宽、高）
    unsupported: Box<dyn Fn(u32, u32) + Send + Sync>,
}

struct State {
    window: usize,
    codec: usize,
    config: Option<ffi::ScreenVideoConfig>,
    /// NAL 前面的长度占几个字节，从 avcC、hvcC 里读；一般是 4
    length_size: usize,
    /// 下一帧要从关键帧开始
    need_key: bool,
    /// 解码器出错了：下一帧来的时候重建（回调里不能拆解码器）
    broken: bool,
    /// 解码器给的空输入缓冲：编号和缓冲
    inputs: VecDeque<(u32, usize)>,
    frames: VecDeque<Frame>,
    pts: i64,
}

impl Decoder {
    pub(crate) fn new(need_keyframe: Box<dyn Fn() + Send + Sync>, unsupported: Box<dyn Fn(u32, u32) + Send + Sync>) -> Arc<Self> {
        Arc::new(Decoder {
            state: Mutex::new(State {
                window: 0,
                codec: 0,
                config: None,
                length_size: 4,
                need_key: true,
                broken: false,
                inputs: VecDeque::new(),
                frames: VecDeque::new(),
                pts: 0,
            }),
            rebuild: Mutex::new(()),
            need_keyframe,
            unsupported,
        })
    }

    /// 能硬件解的格式：有硬件 H.265 解码器就带上，对方会优先用它
    pub(crate) fn supported_codecs() -> Vec<ffi::ScreenCodec> {
        let mut list = vec![ffi::ScreenCodec::H264];
        if sys::hardware_hevc() {
            list.insert(0, ffi::ScreenCodec::H265);
        }
        list
    }

    /// XComponent 的 Surface 建好（surface_id）、销毁（None）。销毁时等解码器放开旧的再返回
    pub(crate) fn set_surface(self: &Arc<Self>, surface_id: Option<u64>) {
        let window = surface_id.map(sys::window_from_surface).unwrap_or(0);
        let old = std::mem::replace(&mut self.state.lock().unwrap().window, window);
        self.rebuild();
        sys::destroy_window(old);
    }

    pub(crate) fn configure(self: &Arc<Self>, config: ffi::ScreenVideoConfig) {
        self.state.lock().unwrap().config = Some(config);
        self.rebuild();
    }

    pub(crate) fn decode(self: &Arc<Self>, key: bool, data: &[u8]) {
        let broken = self.state.lock().unwrap().broken;
        if broken {
            self.rebuild();
        }
        let mut s = self.state.lock().unwrap();
        if s.codec == 0 || (s.need_key && !key) {
            return;
        }
        let Some(data) = avcc_to_annexb(data, s.length_size) else {
            s.need_key = true;
            drop(s);
            (self.need_keyframe)();
            return;
        };
        if key {
            s.frames.retain(|f| f.codec_data);
        }
        s.frames.push_back(Frame { data, key, codec_data: false });
        // 解码跟不上：丢掉攒着的，从下一个关键帧接着来
        if s.frames.len() > 6 {
            s.frames.retain(|f| f.codec_data);
            s.need_key = true;
            drop(s);
            (self.need_keyframe)();
            return;
        }
        s.need_key = false;
        self.feed(&mut s);
    }

    /// 切回前台：清掉重来，要一个关键帧
    pub(crate) fn reset(self: &Arc<Self>) {
        self.rebuild();
    }

    /// 把攒着的帧喂进空的输入缓冲（拿着锁调）
    fn feed(&self, s: &mut State) {
        while !s.inputs.is_empty() && !s.frames.is_empty() {
            let (index, buffer) = s.inputs.pop_front().unwrap();
            let frame = s.frames.pop_front().unwrap();
            s.pts += 16_667;
            if !sys::push(s.codec, index, buffer, &frame.data, s.pts, frame.key, frame.codec_data) {
                // 放不下：缓冲留着下次用，从关键帧重来
                s.inputs.push_front((index, buffer));
                s.frames.retain(|f| f.codec_data);
                s.need_key = true;
                (self.need_keyframe)();
                return;
            }
        }
    }

    /// 拆掉旧的解码器，有显示的地方和配置时建新的
    fn rebuild(self: &Arc<Self>) {
        let _guard = self.rebuild.lock().unwrap();
        let (old, window, config) = {
            let mut s = self.state.lock().unwrap();
            let old = std::mem::replace(&mut s.codec, 0);
            s.inputs.clear();
            s.frames.clear();
            s.need_key = true;
            s.broken = false;
            (old, s.window, s.config.clone())
        };
        // 不拿着状态锁拆：拆的时候要等回调跑完，回调里要拿锁
        sys::destroy_codec(old);
        let (Some(config), true) = (config, window != 0) else { return };
        let sets = match parameter_sets(&config) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("解码器配置记录读不了：{e}");
                return (self.unsupported)(config.width, config.height);
            }
        };
        let user = Arc::as_ptr(self) as usize;
        let Some(codec) = sys::create_codec(config.codec, config.width, config.height, window, user, || {
            // 回调还没开始来之前登记好，免得丢掉最早的几个空缓冲
            let mut s = self.state.lock().unwrap();
            s.length_size = sets.length_size;
            s.frames.push_back(Frame { data: sets.annexb.clone(), key: false, codec_data: true });
        }) else {
            return (self.unsupported)(config.width, config.height);
        };
        self.state.lock().unwrap().codec = codec;
        sys::start(codec);
        (self.need_keyframe)();
    }

    // ---- 解码器的回调（解码器自己的线程）----

    #[cfg_attr(not(target_env = "ohos"), allow(dead_code))]
    fn on_need_input(&self, codec: usize, index: u32, buffer: usize) {
        let mut s = self.state.lock().unwrap();
        if s.codec != codec {
            return;
        }
        s.inputs.push_back((index, buffer));
        self.feed(&mut s);
    }

    #[cfg_attr(not(target_env = "ohos"), allow(dead_code))]
    fn on_error(&self, codec: usize) {
        let mut s = self.state.lock().unwrap();
        if s.codec == codec {
            s.broken = true;
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let s = self.state.get_mut().unwrap();
        sys::destroy_codec(std::mem::replace(&mut s.codec, 0));
        sys::destroy_window(std::mem::replace(&mut s.window, 0));
    }
}

/// 每个 NAL 的长度前缀换成 00 00 00 01；长度不对返回空
fn avcc_to_annexb(data: &[u8], n: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() + 16);
    let mut pos = 0;
    while pos + n <= data.len() {
        let len = data[pos..pos + n].iter().fold(0usize, |a, b| (a << 8) | *b as usize);
        pos += n;
        if pos + len > data.len() {
            return None;
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&data[pos..pos + len]);
        pos += len;
    }
    Some(out)
}

struct ParameterSets {
    length_size: usize,
    /// 全部参数集，Annex-B
    annexb: Vec<u8>,
}

fn parameter_sets(c: &ffi::ScreenVideoConfig) -> Result<ParameterSets, String> {
    let b = &c.description;
    let u8_at = |i: usize| b.get(i).copied().map(usize::from).ok_or("太短");
    let u16_at = |i: usize| Ok::<usize, &str>((u8_at(i)? << 8) | u8_at(i + 1)?);
    let mut out = Vec::new();
    let mut nal = |pos: usize| -> Result<usize, &str> {
        let len = u16_at(pos)?;
        let data = b.get(pos + 2..pos + 2 + len).ok_or("太短")?;
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(data);
        Ok(pos + 2 + len)
    };
    let length_size = match c.codec {
        ffi::ScreenCodec::H264 => {
            // avcC：版本、profile、兼容性、level、长度字节数、SPS 个数和内容、PPS 个数和内容
            let mut pos = 6;
            for _ in 0..(u8_at(5)? & 0x1f) {
                pos = nal(pos)?;
            }
            let pps = u8_at(pos)?;
            pos += 1;
            for _ in 0..pps {
                pos = nal(pos)?;
            }
            (u8_at(4)? & 3) + 1
        }
        ffi::ScreenCodec::H265 => {
            // hvcC：前 22 字节是各种参数（第 21 字节低两位是长度字节数），然后是一组一组的 NAL（VPS、SPS、PPS）
            let mut pos = 23;
            for _ in 0..u8_at(22)? {
                let count = u16_at(pos + 1)?;
                pos += 3;
                for _ in 0..count {
                    pos = nal(pos)?;
                }
            }
            (u8_at(21)? & 3) + 1
        }
    };
    if out.is_empty() {
        return Err("没有参数集".into());
    }
    Ok(ParameterSets { length_size, annexb: out })
}

/// 画面这一路的回调：画面帧直接喂解码器，其他的转成 JSON 交给 ArkTS
pub(crate) struct Events {
    pub decoder: Arc<Decoder>,
    pub emit: Emit,
    pub saw_frame: std::sync::atomic::AtomicBool,
}

impl ffi::ScreenListener for Events {
    fn on_config(&self, config: ffi::ScreenVideoConfig) {
        (self.emit)(json!({ "type": "config", "width": config.width, "height": config.height }));
        self.decoder.configure(config);
    }
    fn on_frame(&self, key: bool, data: Vec<u8>) {
        self.decoder.decode(key, &data);
        if !self.saw_frame.swap(true, std::sync::atomic::Ordering::Relaxed) {
            (self.emit)(json!({ "type": "first_frame" }));
        }
    }
    fn on_cursor(&self, x: f64, y: f64, visible: bool) {
        (self.emit)(json!({ "type": "cursor", "x": x, "y": y, "visible": visible }));
    }
    fn on_cursor_shape(&self, s: ffi::ScreenCursorShape) {
        let rgba = base64::engine::general_purpose::STANDARD.encode(&s.rgba);
        (self.emit)(json!({
            "type": "cursor_shape", "id": s.id, "width": s.width, "height": s.height,
            "hot_x": s.hot_x, "hot_y": s.hot_y, "scale": s.scale, "rgba": rgba,
        }));
    }
    fn on_paused(&self, relay: bool) {
        (self.emit)(json!({ "type": "paused", "relay": relay }));
    }
    fn on_resumed(&self) {
        (self.emit)(json!({ "type": "resumed" }));
    }
    fn on_switched(&self, display: String) {
        (self.emit)(json!({ "type": "switched", "display": display }));
    }
    fn on_stats(&self, bitrate: u32, fps: u32) {
        (self.emit)(json!({ "type": "stats", "bitrate": bitrate, "fps": fps }));
    }
    fn on_closed(&self, reason: Option<String>) {
        (self.emit)(json!({ "type": "closed", "reason": reason }));
    }
}

/// 系统接口。鸿蒙以外（在电脑上 cargo check、跑测试）是空的
#[cfg(target_env = "ohos")]
mod sys {
    use std::ffi::{c_char, c_void};

    use super::Decoder;
    use p2pcli_mobile::ScreenCodec;

    #[repr(C)]
    struct OhAvCodecBufferAttr {
        pts: i64,
        size: i32,
        offset: i32,
        flags: u32,
    }

    type OnError = unsafe extern "C" fn(*mut c_void, i32, *mut c_void);
    type OnStreamChanged = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void);
    type OnBuffer = unsafe extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void);

    #[repr(C)]
    struct OhAvCodecCallback {
        on_error: OnError,
        on_stream_changed: OnStreamChanged,
        on_need_input_buffer: OnBuffer,
        on_new_output_buffer: OnBuffer,
    }

    const AV_ERR_OK: i32 = 0;
    const FLAGS_SYNC_FRAME: u32 = 1 << 1;
    const FLAGS_CODEC_DATA: u32 = 1 << 3;
    const CATEGORY_HARDWARE: i32 = 0;
    const SCALING_MODE_SCALE_TO_WINDOW_V2: i32 = 1;

    #[link(name = "native_media_codecbase")]
    unsafe extern "C" {
        static OH_AVCODEC_MIMETYPE_VIDEO_AVC: *const c_char;
        static OH_AVCODEC_MIMETYPE_VIDEO_HEVC: *const c_char;
        static OH_MD_KEY_WIDTH: *const c_char;
        static OH_MD_KEY_HEIGHT: *const c_char;
        static OH_MD_KEY_FRAME_RATE: *const c_char;
        static OH_MD_KEY_VIDEO_ENABLE_LOW_LATENCY: *const c_char;
        fn OH_AVCodec_GetCapabilityByCategory(mime: *const c_char, is_encoder: bool, category: i32) -> *mut c_void;
        fn OH_AVCapability_GetName(cap: *mut c_void) -> *const c_char;
    }

    #[link(name = "native_media_core")]
    unsafe extern "C" {
        fn OH_AVFormat_Create() -> *mut c_void;
        fn OH_AVFormat_Destroy(format: *mut c_void);
        fn OH_AVFormat_SetIntValue(format: *mut c_void, key: *const c_char, value: i32) -> bool;
        fn OH_AVFormat_SetDoubleValue(format: *mut c_void, key: *const c_char, value: f64) -> bool;
        fn OH_AVBuffer_GetAddr(buffer: *mut c_void) -> *mut u8;
        fn OH_AVBuffer_GetCapacity(buffer: *mut c_void) -> i32;
        fn OH_AVBuffer_SetBufferAttr(buffer: *mut c_void, attr: *const OhAvCodecBufferAttr) -> i32;
    }

    #[link(name = "native_media_vdec")]
    unsafe extern "C" {
        fn OH_VideoDecoder_CreateByMime(mime: *const c_char) -> *mut c_void;
        fn OH_VideoDecoder_CreateByName(name: *const c_char) -> *mut c_void;
        fn OH_VideoDecoder_Destroy(codec: *mut c_void) -> i32;
        fn OH_VideoDecoder_RegisterCallback(codec: *mut c_void, cb: OhAvCodecCallback, user: *mut c_void) -> i32;
        fn OH_VideoDecoder_Configure(codec: *mut c_void, format: *mut c_void) -> i32;
        fn OH_VideoDecoder_SetSurface(codec: *mut c_void, window: *mut c_void) -> i32;
        fn OH_VideoDecoder_Prepare(codec: *mut c_void) -> i32;
        fn OH_VideoDecoder_Start(codec: *mut c_void) -> i32;
        fn OH_VideoDecoder_Stop(codec: *mut c_void) -> i32;
        fn OH_VideoDecoder_PushInputBuffer(codec: *mut c_void, index: u32) -> i32;
        fn OH_VideoDecoder_RenderOutputBuffer(codec: *mut c_void, index: u32) -> i32;
    }

    #[link(name = "native_window")]
    unsafe extern "C" {
        fn OH_NativeWindow_CreateNativeWindowFromSurfaceId(surface_id: u64, window: *mut *mut c_void) -> i32;
        fn OH_NativeWindow_DestroyNativeWindow(window: *mut c_void);
        fn OH_NativeWindow_NativeWindowSetScalingModeV2(window: *mut c_void, mode: i32) -> i32;
    }

    pub fn hardware_hevc() -> bool {
        unsafe { !OH_AVCodec_GetCapabilityByCategory(OH_AVCODEC_MIMETYPE_VIDEO_HEVC, false, CATEGORY_HARDWARE).is_null() }
    }

    pub fn window_from_surface(id: u64) -> usize {
        let mut w: *mut c_void = std::ptr::null_mut();
        unsafe {
            if OH_NativeWindow_CreateNativeWindowFromSurfaceId(id, &mut w) != 0 || w.is_null() {
                tracing::warn!("拿不到 XComponent 的窗口：{id}");
                return 0;
            }
            // 解出来的画面铺满 XComponent（XComponent 自己按比例摆好）
            OH_NativeWindow_NativeWindowSetScalingModeV2(w, SCALING_MODE_SCALE_TO_WINDOW_V2);
        }
        w as usize
    }

    pub fn destroy_window(w: usize) {
        if w != 0 {
            unsafe { OH_NativeWindow_DestroyNativeWindow(w as *mut c_void) }
        }
    }

    pub fn destroy_codec(c: usize) {
        if c != 0 {
            unsafe {
                OH_VideoDecoder_Stop(c as *mut c_void);
                OH_VideoDecoder_Destroy(c as *mut c_void);
            }
        }
    }

    /// 建、配置好解码器（还没开始）。registered 在开始收回调之前调
    pub fn create_codec(codec: ScreenCodec, width: u32, height: u32, window: usize, user: usize, registered: impl FnOnce()) -> Option<usize> {
        unsafe {
            let mime = match codec {
                ScreenCodec::H264 => OH_AVCODEC_MIMETYPE_VIDEO_AVC,
                ScreenCodec::H265 => OH_AVCODEC_MIMETYPE_VIDEO_HEVC,
            };
            // 优先硬件解码器；没有就让系统挑
            let cap = OH_AVCodec_GetCapabilityByCategory(mime, false, CATEGORY_HARDWARE);
            let mut c = if cap.is_null() { std::ptr::null_mut() } else { OH_VideoDecoder_CreateByName(OH_AVCapability_GetName(cap)) };
            if c.is_null() {
                c = OH_VideoDecoder_CreateByMime(mime);
            }
            if c.is_null() {
                tracing::warn!("建不了解码器");
                return None;
            }
            let cb = OhAvCodecCallback {
                on_error,
                on_stream_changed,
                on_need_input_buffer: on_need_input,
                on_new_output_buffer: on_new_output,
            };
            let fmt = OH_AVFormat_Create();
            OH_AVFormat_SetIntValue(fmt, OH_MD_KEY_WIDTH, width as i32);
            OH_AVFormat_SetIntValue(fmt, OH_MD_KEY_HEIGHT, height as i32);
            OH_AVFormat_SetDoubleValue(fmt, OH_MD_KEY_FRAME_RATE, 60.0);
            // 实时：解码器不多攒帧，解出来就送显
            OH_AVFormat_SetIntValue(fmt, OH_MD_KEY_VIDEO_ENABLE_LOW_LATENCY, 1);
            let ok = OH_VideoDecoder_RegisterCallback(c, cb, user as *mut c_void) == AV_ERR_OK
                && OH_VideoDecoder_Configure(c, fmt) == AV_ERR_OK
                && OH_VideoDecoder_SetSurface(c, window as *mut c_void) == AV_ERR_OK
                && OH_VideoDecoder_Prepare(c) == AV_ERR_OK;
            OH_AVFormat_Destroy(fmt);
            if !ok {
                tracing::warn!("解码器配置失败：{width}x{height}");
                OH_VideoDecoder_Destroy(c);
                return None;
            }
            registered();
            Some(c as usize)
        }
    }

    pub fn start(c: usize) {
        unsafe {
            if OH_VideoDecoder_Start(c as *mut c_void) != AV_ERR_OK {
                tracing::warn!("解码器启动失败");
            }
        }
    }

    /// 把一帧拷进输入缓冲、送去解码；放不下返回 false
    pub fn push(codec: usize, index: u32, buffer: usize, data: &[u8], pts: i64, key: bool, codec_data: bool) -> bool {
        unsafe {
            let buf = buffer as *mut c_void;
            let cap = OH_AVBuffer_GetCapacity(buf);
            let addr = OH_AVBuffer_GetAddr(buf);
            if addr.is_null() || cap < 0 || data.len() > cap as usize {
                return false;
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), addr, data.len());
            let flags = if codec_data { FLAGS_CODEC_DATA } else if key { FLAGS_SYNC_FRAME } else { 0 };
            let attr = OhAvCodecBufferAttr { pts, size: data.len() as i32, offset: 0, flags };
            OH_AVBuffer_SetBufferAttr(buf, &attr);
            OH_VideoDecoder_PushInputBuffer(codec as *mut c_void, index) == AV_ERR_OK
        }
    }

    unsafe extern "C" fn on_error(codec: *mut c_void, code: i32, user: *mut c_void) {
        tracing::warn!("解码出错：{code}");
        let d = unsafe { &*(user as *const Decoder) };
        d.on_error(codec as usize);
    }

    unsafe extern "C" fn on_stream_changed(_codec: *mut c_void, _format: *mut c_void, _user: *mut c_void) {}

    unsafe extern "C" fn on_need_input(codec: *mut c_void, index: u32, buffer: *mut c_void, user: *mut c_void) {
        let d = unsafe { &*(user as *const Decoder) };
        d.on_need_input(codec as usize, index, buffer as usize);
    }

    /// 解出来一帧：马上显示
    unsafe extern "C" fn on_new_output(codec: *mut c_void, index: u32, _buffer: *mut c_void, _user: *mut c_void) {
        unsafe { OH_VideoDecoder_RenderOutputBuffer(codec, index) };
    }
}

#[cfg(not(target_env = "ohos"))]
mod sys {
    use p2pcli_mobile::ScreenCodec;

    pub fn hardware_hevc() -> bool {
        false
    }
    pub fn window_from_surface(_id: u64) -> usize {
        0
    }
    pub fn destroy_window(_w: usize) {}
    pub fn destroy_codec(_c: usize) {}
    pub fn create_codec(_codec: ScreenCodec, _w: u32, _h: u32, _window: usize, _user: usize, _registered: impl FnOnce()) -> Option<usize> {
        None
    }
    pub fn start(_c: usize) {}
    pub fn push(_codec: usize, _index: u32, _buffer: usize, _data: &[u8], _pts: i64, _key: bool, _codec_data: bool) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avcc_becomes_annexb() {
        let avcc = [0, 0, 0, 2, 0x65, 0xaa, 0, 0, 0, 1, 0x41];
        assert_eq!(avcc_to_annexb(&avcc, 4).unwrap(), vec![0, 0, 0, 1, 0x65, 0xaa, 0, 0, 0, 1, 0x41]);
        assert!(avcc_to_annexb(&[0, 0, 0, 9, 1], 4).is_none());
    }

    #[test]
    fn reads_avcc_parameter_sets() {
        // 一个 SPS（67 42）、一个 PPS（68 ce）
        let desc = vec![1, 0x42, 0, 0x1f, 0xff, 0xe1, 0, 2, 0x67, 0x42, 1, 0, 2, 0x68, 0xce];
        let c = ffi::ScreenVideoConfig {
            codec: ffi::ScreenCodec::H264,
            codec_name: "avc1.42001f".into(),
            width: 1920,
            height: 1080,
            description: desc,
            full_range: false,
        };
        let s = parameter_sets(&c).unwrap();
        assert_eq!(s.length_size, 4);
        assert_eq!(s.annexb, vec![0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xce]);
    }

    #[test]
    fn reads_hvcc_parameter_sets() {
        let mut desc = vec![0u8; 22];
        desc[21] = 0x0f; // 长度 4 字节
        desc.push(3); // 三组
        for (ty, nal) in [(32u8, [0x40u8, 1]), (33, [0x42, 1]), (34, [0x44, 1])] {
            desc.extend_from_slice(&[0x80 | ty, 0, 1, 0, 2]);
            desc.extend_from_slice(&nal);
        }
        let c = ffi::ScreenVideoConfig {
            codec: ffi::ScreenCodec::H265,
            codec_name: "hvc1.1.6.L120.90".into(),
            width: 1920,
            height: 1080,
            description: desc,
            full_range: false,
        };
        let s = parameter_sets(&c).unwrap();
        assert_eq!(s.length_size, 4);
        assert_eq!(s.annexb, vec![0, 0, 0, 1, 0x40, 1, 0, 0, 0, 1, 0x42, 1, 0, 0, 0, 1, 0x44, 1]);
    }
}

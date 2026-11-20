//! Media Foundation 的 H.264 编码器。先找显卡的硬件编码器（NVENC、Quick Sync、AMF），
//! 直接吃 GPU 上的 NV12 纹理；找不到（虚拟机、远程桌面会话里）才退到系统自带的软件编码器，
//! 那时画面从显存拷回内存再编码。都开低延迟模式、不要 B 帧、只在要求时出关键帧。

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use tracing::{info, warn};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{Interface, GUID};

const ME_NEED_INPUT: u32 = 601; // METransformNeedInput
const ME_HAVE_OUTPUT: u32 = 602; // METransformHaveOutput
/// 等编码结果最多多久；超时这帧就算丢了，下一帧接着来
const OUTPUT_WAIT: Duration = Duration::from_millis(400);

pub struct Encoder {
    transform: IMFTransform,
    codec: Option<ICodecAPI>,
    /// 硬件编码器是异步的：它要输入、有输出都通过事件说
    events: Option<Receiver<u32>>,
    need_input: u32,
    in_id: u32,
    out_id: u32,
    provides_samples: bool,
    out_size: u32,
    pub hardware: bool,
    pub width: u32,
    pub height: u32,
    fps: u32,
    bitrate: u32,
    /// 软件编码器要内存里的画面：从显存拷回来用的暂存纹理
    staging: Option<ID3D11Texture2D>,
    /// 编码器有时候不把 SPS/PPS 放进关键帧里，就从输出类型里取
    seq_header: Option<Vec<u8>>,
    /// 编出来还没交出去的结果，按顺序
    outputs: VecDeque<Vec<u8>>,
}

unsafe impl Send for Encoder {}

struct SendBox<T>(T);
unsafe impl<T> Send for SendBox<T> {}

fn set_u32(codec: &ICodecAPI, api: &GUID, v: u32) {
    let value = VARIANT::from(v);
    let _ = unsafe { codec.SetValue(api, &value) };
}

fn set_bool(codec: &ICodecAPI, api: &GUID, v: bool) {
    let value = VARIANT::from(v);
    let _ = unsafe { codec.SetValue(api, &value) };
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

/// 能用的 H.264 编码器，硬件的在前
fn candidates(hardware: bool) -> Result<Vec<IMFActivate>> {
    let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
    let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
    let flags = if hardware { MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER } else { MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER };
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    unsafe { MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, flags, Some(&input), Some(&output), &mut list, &mut count)? };
    let mut out = Vec::new();
    if !list.is_null() {
        for i in 0..count as usize {
            if let Some(a) = unsafe { (*list.add(i)).take() } {
                out.push(a);
            }
        }
        unsafe { CoTaskMemFree(Some(list as *const _)) };
    }
    Ok(out)
}

impl Encoder {
    /// 先试硬件编码器，都不行再用软件的
    pub fn new(device: &ID3D11Device, manager: &IMFDXGIDeviceManager, width: u32, height: u32, fps: u32, bitrate: u32) -> Result<Self> {
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL)? };
        let mut last_err = None;
        for activate in candidates(true).unwrap_or_default() {
            let name = friendly_name(&activate);
            match Self::open(&activate, Some(manager), width, height, fps, bitrate) {
                Ok(enc) => {
                    info!(encoder = %name, width, height, fps, bitrate, "H.264 硬件编码器已就绪");
                    return Ok(enc);
                }
                Err(e) => {
                    warn!(encoder = %name, "这个硬件编码器用不了：{e:#}");
                    last_err = Some(e);
                }
            }
        }
        for activate in candidates(false)? {
            match Self::open(&activate, None, width, height, fps, bitrate) {
                Ok(mut enc) => {
                    warn!(width, height, "没有可用的硬件编码器，改用系统的软件编码器");
                    enc.staging = Some(staging_texture(device, width, height)?);
                    return Ok(enc);
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("这台电脑上没有 H.264 编码器")))
    }

    fn open(activate: &IMFActivate, manager: Option<&IMFDXGIDeviceManager>, width: u32, height: u32, fps: u32, bitrate: u32) -> Result<Self> {
        let transform: IMFTransform = unsafe { activate.ActivateObject()? };
        let hardware = manager.is_some();
        if hardware {
            // 硬件编码器是异步的，要先解锁
            let attrs = unsafe { transform.GetAttributes()? };
            unsafe { attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)? };
            let manager = manager.unwrap();
            unsafe { transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize) }.context("编码器不接受这块显卡")?;
        }
        // 多数编码器流编号固定是 0，不实现这个接口
        let (mut ins, mut outs) = ([0u32], [0u32]);
        let _ = unsafe { transform.GetStreamIDs(&mut ins, &mut outs) };
        let (in_id, out_id) = (ins[0], outs[0]);

        let codec: Option<ICodecAPI> = transform.cast().ok();
        if let Some(c) = &codec {
            set_bool(c, &CODECAPI_AVLowLatencyMode, true);
            set_u32(c, &CODECAPI_AVEncCommonRateControlMode, eAVEncCommonRateControlMode_CBR.0 as u32);
            set_u32(c, &CODECAPI_AVEncCommonMeanBitRate, bitrate);
            set_u32(c, &CODECAPI_AVEncMPVDefaultBPictureCount, 0);
            // 关键帧只在要的时候出：可靠传输不会丢帧，定期关键帧只会白占带宽
            set_u32(c, &CODECAPI_AVEncMPVGOPSize, 0xFFFF);
        }

        // 先设输出类型，再设输入类型
        let out = unsafe { MFCreateMediaType()? };
        unsafe {
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
            out.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
            out.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
            out.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
            out.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
            out.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
            transform.SetOutputType(out_id, &out, 0).context("编码器不支持这个输出格式")?;
        }
        let input = unsafe { MFCreateMediaType()? };
        unsafe {
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            input.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
            input.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            input.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
            input.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
            input.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
            transform.SetInputType(in_id, &input, 0).context("编码器不支持这个输入格式")?;
        }

        let info = unsafe { transform.GetOutputStreamInfo(out_id)? };
        let provides_samples = info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32) != 0;
        let out_size = info.cbSize.max(width * height * 3 / 2);

        // 异步编码器的事件单独一个线程等，阻塞着等不会占 CPU
        let events = if hardware {
            let generator: IMFMediaEventGenerator = transform.cast()?;
            let (tx, rx) = channel();
            let generator = SendBox(generator);
            std::thread::Builder::new().name("p2pcli-encoder-events".into()).spawn(move || {
                let generator = generator;
                let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                while let Ok(ev) = unsafe { generator.0.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0)) } {
                    let kind = unsafe { ev.GetType() }.unwrap_or(0);
                    if tx.send(kind).is_err() {
                        break;
                    }
                }
            })?;
            Some(rx)
        } else {
            None
        };

        unsafe {
            transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        }
        let seq_header = unsafe { transform.GetOutputCurrentType(out_id) }
            .ok()
            .and_then(|t| blob(&t, &MF_MT_MPEG_SEQUENCE_HEADER));
        Ok(Encoder {
            transform,
            codec,
            events,
            need_input: 0,
            in_id,
            out_id,
            provides_samples,
            out_size,
            hardware,
            width,
            height,
            fps,
            bitrate,
            staging: None,
            seq_header,
            outputs: VecDeque::new(),
        })
    }

    pub fn set_bitrate(&mut self, bitrate: u32) {
        if bitrate == self.bitrate {
            return;
        }
        self.bitrate = bitrate;
        if let Some(c) = &self.codec {
            set_u32(c, &CODECAPI_AVEncCommonMeanBitRate, bitrate);
        }
    }

    /// SPS/PPS（Annex B），编码器没放进关键帧时用
    pub fn sequence_header(&self) -> Option<&[u8]> {
        self.seq_header.as_deref()
    }

    /// 编一帧 NV12 纹理，返回最早一份还没交出去的 H.264（Annex B）；暂时没有就是空
    pub fn encode(&mut self, ctx: &ID3D11DeviceContext, texture: &ID3D11Texture2D, ts_100ns: i64, key: bool) -> Result<Option<Vec<u8>>> {
        if key && let Some(c) = &self.codec {
            set_u32(c, &CODECAPI_AVEncVideoForceKeyFrame, 1);
        }
        let sample = unsafe { MFCreateSample()? };
        let buffer = if self.staging.is_some() {
            self.memory_buffer(ctx, texture)?
        } else {
            unsafe { MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, texture, 0, false)? }
        };
        unsafe {
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(ts_100ns)?;
            sample.SetSampleDuration(10_000_000 / self.fps.max(1) as i64)?;
        }
        if self.events.is_some() { self.encode_async(&sample) } else { self.encode_sync(&sample) }
    }

    /// 送进一帧，等它吐出结果。编码器的输出一份都不扔：先到的先返回，没赶上的留到下次。
    /// 丢一份输出后面的帧就解不出来了，所以宁可晚一帧，也不丢
    fn encode_async(&mut self, sample: &IMFSample) -> Result<Option<Vec<u8>>> {
        let deadline = Instant::now() + OUTPUT_WAIT;
        // 等它要输入
        while self.need_input == 0 {
            match self.next_event(deadline)? {
                Some(ME_NEED_INPUT) => self.need_input += 1,
                Some(ME_HAVE_OUTPUT) => self.collect()?,
                Some(_) => {}
                // 它一直不要输入：这帧没送进去，不影响后面的帧
                None => return Ok(self.outputs.pop_front()),
            }
        }
        unsafe { self.transform.ProcessInput(self.in_id, sample, 0)? };
        self.need_input -= 1;
        while self.outputs.is_empty() {
            match self.next_event(deadline)? {
                Some(ME_NEED_INPUT) => self.need_input += 1,
                Some(ME_HAVE_OUTPUT) => self.collect()?,
                Some(_) => {}
                // 结果还没出来，下次再取
                None => break,
            }
        }
        Ok(self.outputs.pop_front())
    }

    fn collect(&mut self) -> Result<()> {
        if let Some(data) = self.pull()? {
            self.outputs.push_back(data);
        }
        Ok(())
    }

    fn next_event(&mut self, deadline: Instant) -> Result<Option<u32>> {
        let rx = self.events.as_ref().unwrap();
        let wait = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(kind) => Ok(Some(kind)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => bail!("硬件编码器停止了"),
        }
    }

    fn encode_sync(&mut self, sample: &IMFSample) -> Result<Option<Vec<u8>>> {
        unsafe { self.transform.ProcessInput(self.in_id, sample, 0)? };
        while let Some(data) = self.pull()? {
            self.outputs.push_back(data);
        }
        Ok(self.outputs.pop_front())
    }

    /// 取一份输出；没有就是空
    fn pull(&mut self) -> Result<Option<Vec<u8>>> {
        let own = if self.provides_samples {
            None
        } else {
            let s = unsafe { MFCreateSample()? };
            let b = unsafe { MFCreateMemoryBuffer(self.out_size)? };
            unsafe { s.AddBuffer(&b)? };
            Some(s)
        };
        let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: self.out_id,
            pSample: std::mem::ManuallyDrop::new(own),
            dwStatus: 0,
            pEvents: std::mem::ManuallyDrop::new(None),
        }];
        let mut status = 0u32;
        let r = unsafe { self.transform.ProcessOutput(0, &mut buffers, &mut status) };
        let sample = unsafe { std::mem::ManuallyDrop::take(&mut buffers[0].pSample) };
        drop(unsafe { std::mem::ManuallyDrop::take(&mut buffers[0].pEvents) });
        match r {
            Ok(()) => {}
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
            Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                // 编码器换了输出格式（比如换了 SPS）：重新设一下再取
                let t = unsafe { self.transform.GetOutputAvailableType(self.out_id, 0)? };
                unsafe { self.transform.SetOutputType(self.out_id, &t, 0)? };
                self.seq_header = blob(&t, &MF_MT_MPEG_SEQUENCE_HEADER).or(self.seq_header.take());
                return Ok(None);
            }
            Err(e) => return Err(e).context("取编码结果失败"),
        }
        let Some(sample) = sample else { return Ok(None) };
        let buffer = unsafe { sample.ConvertToContiguousBuffer()? };
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        unsafe { buffer.Lock(&mut ptr, None, Some(&mut len))? };
        let data = unsafe { std::slice::from_raw_parts(ptr, len as usize) }.to_vec();
        unsafe { buffer.Unlock()? };
        Ok(Some(data))
    }

    /// 软件编码器：把显存里的 NV12 拷到内存
    fn memory_buffer(&self, ctx: &ID3D11DeviceContext, texture: &ID3D11Texture2D) -> Result<IMFMediaBuffer> {
        let staging = self.staging.as_ref().unwrap();
        let (w, h) = (self.width as usize, self.height as usize);
        let size = w * h * 3 / 2;
        let buffer = unsafe { MFCreateMemoryBuffer(size as u32)? };
        unsafe {
            ctx.CopyResource(staging, texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let mut dst = std::ptr::null_mut();
            buffer.Lock(&mut dst, None, None)?;
            let src = mapped.pData as *const u8;
            let pitch = mapped.RowPitch as usize;
            // Y 平面 h 行，后面紧跟 UV 平面 h/2 行
            for row in 0..h + h / 2 {
                std::ptr::copy_nonoverlapping(src.add(row * pitch), dst.add(row * w), w);
            }
            buffer.Unlock()?;
            buffer.SetCurrentLength(size as u32)?;
            ctx.Unmap(staging, 0);
        }
        Ok(buffer)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            // 异步编码器关掉以后，等事件的线程就会退出
            if let Ok(shutdown) = self.transform.cast::<IMFShutdown>() {
                let _ = shutdown.Shutdown();
            }
        }
    }
}

/// 编码器的名字，日志里用
fn friendly_name(activate: &IMFActivate) -> String {
    let mut p = windows::core::PWSTR::null();
    let mut len = 0u32;
    if unsafe { activate.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut p, &mut len) }.is_err() || p.is_null() {
        return String::new();
    }
    let name = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    name
}

fn blob(t: &IMFMediaType, key: &GUID) -> Option<Vec<u8>> {
    let size = unsafe { t.GetBlobSize(key) }.ok()?;
    let mut buf = vec![0u8; size as usize];
    unsafe { t.GetBlob(key, &mut buf, None) }.ok()?;
    Some(buf)
}

fn staging_texture(device: &ID3D11Device, width: u32, height: u32) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut tex = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut tex))? };
    tex.ok_or_else(|| anyhow!("建不了暂存纹理"))
}

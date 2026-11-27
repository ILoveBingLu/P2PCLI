//! 抓本机正在放的声音：WASAPI 回环（loopback）录默认的播放设备，不用装驱动。
//! 请系统直接转成 48 kHz、立体声的 f32（AUTOCONVERTPCM），不用自己重采样、混声道。
//! 单独一个线程，每 10 毫秒取一次，攒成一段送出去。默认播放设备换了（插上耳机）就重开。
//! 什么都没在放时系统不给数据，也就什么都不发。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use p2pcli_core::screen_audio::{AudioSink, CHANNELS, SAMPLE_RATE};
use tracing::{debug, info, warn};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, eConsole,
    eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};

/// WAVE_FORMAT_IEEE_FLOAT
const FLOAT: u16 = 3;
/// 系统那边的缓冲：200 毫秒（单位 100 纳秒），我们 10 毫秒取一次，绰绰有余
const BUFFER_HNS: i64 = 2_000_000;
const POLL: Duration = Duration::from_millis(10);

/// 一路正在抓的声音。丢掉就停
pub struct WinAudio {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

struct Loopback {
    client: IAudioClient,
    capture: IAudioCaptureClient,
}

impl Loopback {
    fn open() -> Result<Self> {
        let enumerator: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.context("开不了声音设备列表")?;
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }.context("没有默认的播放设备")?;
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }.context("开不了播放设备")?;
        let block = (CHANNELS * 4) as u16;
        let format = WAVEFORMATEX {
            wFormatTag: FLOAT,
            nChannels: CHANNELS as u16,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * block as u32,
            nBlockAlign: block,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, BUFFER_HNS, 0, &format, None) }.context("录不了播放设备的声音")?;
        let capture: IAudioCaptureClient = unsafe { client.GetService() }.context("录不了播放设备的声音")?;
        unsafe { client.Start() }.context("录不了播放设备的声音")?;
        Ok(Loopback { client, capture })
    }

    /// 把系统攒着的声音都取出来，接在 out 后面
    fn drain(&self, out: &mut Vec<f32>) -> Result<()> {
        loop {
            let n = unsafe { self.capture.GetNextPacketSize() }?;
            if n == 0 {
                return Ok(());
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            unsafe { self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }?;
            let count = frames as usize * CHANNELS;
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                out.resize(out.len() + count, 0.0);
            } else {
                out.extend_from_slice(unsafe { std::slice::from_raw_parts(data as *const f32, count) });
            }
            unsafe { self.capture.ReleaseBuffer(frames) }?;
        }
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        let _ = unsafe { self.client.Stop() };
    }
}

impl WinAudio {
    pub fn start(sink: AudioSink) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let (init_tx, init_rx) = channel::<Result<()>>();
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("p2pcli-audio".into())
            .spawn(move || {
                let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                let mut lb = match Loopback::open() {
                    Ok(l) => {
                        let _ = init_tx.send(Ok(()));
                        l
                    }
                    Err(e) => {
                        let _ = init_tx.send(Err(e));
                        return;
                    }
                };
                info!("开始抓本机的声音");
                let mut chunk = Vec::with_capacity(SAMPLE_RATE as usize / 50 * CHANNELS);
                while !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(POLL);
                    chunk.clear();
                    if let Err(e) = lb.drain(&mut chunk) {
                        // 默认播放设备换了、被拔了：过一会儿重开
                        debug!("播放设备变了，重开：{e:#}");
                        std::thread::sleep(Duration::from_millis(500));
                        match Loopback::open() {
                            Ok(l) => lb = l,
                            Err(e) => warn!("重开播放设备失败：{e:#}"),
                        }
                        continue;
                    }
                    if !chunk.is_empty() {
                        let _ = sink.try_send(std::mem::take(&mut chunk));
                    }
                }
                debug!("停止抓本机的声音");
            })
            .context("开不了抓声音的线程")?;
        init_rx.recv_timeout(Duration::from_secs(5)).map_err(|_| anyhow!("抓声音的线程没有响应"))??;
        Ok(WinAudio { stop, thread: Some(thread) })
    }
}

impl Drop for WinAudio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

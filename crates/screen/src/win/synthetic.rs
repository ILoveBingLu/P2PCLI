//! 合成屏幕：不抓真屏幕，按帧率定时出会动的画面（整屏往右下滚的渐变和伪随机的“字”，加一个移动的白方块），
//! 之后走和真抓屏完全一样的缩放、4:4:4、流水线编码、流控和计数。给端到端的网络测试用，bench 也用它。
//! 假装有一块 2560×1600 的主屏幕；键鼠事件不注入，只记日志。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use p2pcli_core::screen::{CaptureOptions, DisplayInfo, InputEvent, ScreenBackend, ScreenCapture};
use tracing::{debug, info};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

use super::capture::{Ev, PIPELINED, Shared, State, Synthetic, WinCapture, create_device};

/// 假屏幕的大小
const WIDTH: u32 = 2560;
const HEIGHT: u32 = 1600;
const DISPLAY_ID: &str = "synthetic";
/// 轮着放几张不一样的画面
const FRAMES: usize = 16;

pub struct SyntheticBackend;

impl ScreenBackend for SyntheticBackend {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Ok(vec![DisplayInfo { id: DISPLAY_ID.into(), name: "合成画面".into(), width: WIDTH, height: HEIGHT, primary: true, ..Default::default() }])
    }

    fn check_capture(&self) -> Result<()> {
        Ok(())
    }

    fn check_control(&self) -> Result<()> {
        Ok(())
    }

    fn start(&self, opts: CaptureOptions) -> Result<Arc<dyn ScreenCapture>> {
        Ok(Arc::new(WinCapture::start_with(opts, state)?))
    }

    fn inject(&self, screen: &str, ev: &InputEvent) -> Result<()> {
        debug!(screen, ?ev, "合成画面不注入键鼠");
        Ok(())
    }

    fn screenshot(&self, _display: &str, max_width: u32) -> Result<p2pcli_core::screen::Screenshot> {
        super::shot::synthetic(WIDTH, HEIGHT, max_width)
    }
}

/// 在抓屏线程上建合成屏幕：和真抓屏一样用默认的显卡，画面传上去，开始定时出帧
fn state(opts: &CaptureOptions, shared: Arc<Shared>, tx: Sender<Ev>) -> Result<State> {
    let (device, ctx) = create_device()?;
    let frames = frames(&device, WIDTH as usize, HEIGHT as usize)?;
    let latest: Arc<Mutex<Option<(Instant, usize)>>> = Arc::default();
    let fps = opts.fps.max(1);
    let ticker = Ticker::spawn(fps, latest.clone(), shared.clone(), tx.clone(), Arc::new(AtomicBool::new(true)))?;
    info!(width = WIDTH, height = HEIGHT, fps, "开始出合成画面");
    State::synthetic(device, ctx, Synthetic { frames, size: (WIDTH, HEIGHT), latest, ticker: Some(ticker) }, opts, shared, tx, PIPELINED)
}

/// 一张张会动的画面（BGRA），传到显卡上
pub(super) fn frames(device: &ID3D11Device, w: usize, h: usize) -> Result<Vec<ID3D11Texture2D>> {
    // 底图：横竖两个方向的渐变，每四行有一行伪随机的“字”
    let mut base = vec![255u8; w * h * 4];
    let mut rng = 0x9E37_79B9u32;
    for y in 0..h {
        for x in 0..w {
            let p = &mut base[(y * w + x) * 4..][..3];
            if y % 4 == 3 {
                for v in p.iter_mut() {
                    rng ^= rng << 13;
                    rng ^= rng >> 17;
                    rng ^= rng << 5;
                    *v = rng as u8;
                }
            } else {
                p.copy_from_slice(&[((x + y) % 256) as u8, (y * 255 / (h - 1)) as u8, (x * 255 / (w - 1)) as u8]);
            }
        }
    }
    let mut f = vec![255u8; w * h * 4];
    (0..FRAMES)
        .map(|i| {
            let (dx, dy) = (i * 16 % w, i * 9 % h);
            for y in 0..h {
                let src = &base[((y + dy) % h) * w * 4..][..w * 4];
                let dst = &mut f[y * w * 4..][..w * 4];
                dst[..(w - dx) * 4].copy_from_slice(&src[dx * 4..]);
                dst[(w - dx) * 4..].copy_from_slice(&src[..dx * 4]);
            }
            let side = 200.min(w / 2).min(h / 2);
            let (bx, by) = (i * 40 % (w - side), i * 25 % (h - side));
            for y in by..by + side {
                f[(y * w + bx) * 4..][..side * 4].fill(255);
            }
            upload(device, &f, w as u32, h as u32)
        })
        .collect()
}

fn upload(device: &ID3D11Device, bgra: &[u8], w: u32, h: u32) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let data = D3D11_SUBRESOURCE_DATA { pSysMem: bgra.as_ptr().cast(), SysMemPitch: w * 4, SysMemSlicePitch: 0 };
    let mut tex = None;
    unsafe { device.CreateTexture2D(&desc, Some(&data), Some(&mut tex)).context("建不了合成画面的纹理")? };
    tex.ok_or_else(|| anyhow!("建不了合成画面的纹理"))
}

/// 定时往“最新一帧”里放 (时刻, 第几帧) 的线程。丢掉就停
pub(super) struct Ticker {
    quit: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Ticker {
    /// ticking 为假时照样计时，只是不放画面（当作屏幕不动）
    pub fn spawn(fps: u32, latest: Arc<Mutex<Option<(Instant, usize)>>>, shared: Arc<Shared>, tx: Sender<Ev>, ticking: Arc<AtomicBool>) -> Result<Self> {
        let quit = Arc::new(AtomicBool::new(false));
        let stop = quit.clone();
        let tick = Duration::from_secs_f64(1.0 / fps.max(1) as f64);
        let thread = std::thread::Builder::new()
            .name("p2pcli-synthetic".into())
            .spawn(move || {
                let mut next = Instant::now();
                let mut i = 0;
                while !stop.load(Ordering::Relaxed) {
                    next += tick;
                    // 睡到差一毫秒，剩下的空转，间隔才稳
                    if let Some(d) = next.checked_duration_since(Instant::now()).and_then(|d| d.checked_sub(Duration::from_millis(1))) {
                        std::thread::sleep(d);
                    }
                    while Instant::now() < next {
                        std::hint::spin_loop();
                    }
                    if !ticking.load(Ordering::Relaxed) {
                        continue;
                    }
                    let old = latest.lock().unwrap().replace((Instant::now(), i));
                    shared.arrived(old.is_some());
                    if tx.send(Ev::Wake).is_err() {
                        break;
                    }
                    i += 1;
                }
            })
            .context("开不了合成画面的定时线程")?;
        Ok(Ticker { quit, thread: Some(thread) })
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

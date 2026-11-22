//! 4:4:4 叠成一张灰度图（[`Layout::Yuv444Stacked`]）：着色器把 BGRA 画面按 BT.709 全范围算出
//! Y、Cb、Cr，从上到下画进 W×3H 的 NV12 纹理的亮度平面，色度平面一直是 128。
//! 编码器照常编 4:2:0，观看方再把三段合成彩色，每个像素都有自己的颜色。
//!
//! 要缩放时先用视频处理器把画面缩成 BGRA（它的缩放比着色器里的双线性好），不缩放时着色器直接读输入纹理。
//! 着色器按整数坐标读，一个像素对一个像素，结果和 [`pack_yuv444_stacked`] 差不超过 1。
//!
//! [`Layout::Yuv444Stacked`]: p2pcli_core::screen::Layout::Yuv444Stacked
//! [`pack_yuv444_stacked`]: p2pcli_core::screen::pack_yuv444_stacked

use anyhow::{Context, Result, anyhow};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_ENABLE_STRICTNESS, D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
use windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob, ID3DInclude};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM};
use windows::core::{PCSTR, s};

use super::capture::{encoder_texture, texture_2d, video_processor, vp_blt, vp_input_view, vp_output_view};

/// 叠好的纹理轮着用几块：编码器里最多两帧在读，一块正在画，再留一块余量
const POOL: usize = super::capture::NV12_POOL;

/// 顶点着色器画一个盖住整个画面的三角形；像素着色器按像素在哪一段算 Y、Cb 或 Cr。
/// 自己先四舍五入到 0~255 的整数再交给渲染目标：显卡把小数转成 8 位时允许差一点，正好在 .5 附近的会差 1。
/// 系数和 p2pcli_core::screen::rgb_to_yuv444 一样：0.7152 = 1 - 0.2126 - 0.0722，
/// 1.8556 = 2 × (1 - 0.0722)，1.5748 = 2 × (1 - 0.2126)
const SHADER: &str = r#"
Texture2D<float4> src : register(t0);

float4 vs(uint id : SV_VertexID) : SV_Position {
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);
}

float ps(float4 pos : SV_Position) : SV_Target {
    uint w, h;
    src.GetDimensions(w, h);
    uint x = (uint)pos.x;
    uint y = (uint)pos.y;
    uint part = y / h;
    float3 c = src.Load(int3(x, y - part * h, 0)).rgb;
    float l = dot(c, float3(0.2126, 0.7152, 0.0722));
    float v = part == 0 ? l : part == 1 ? (c.b - l) / 1.8556 + 0.5 : (c.r - l) / 1.5748 + 0.5;
    return round(saturate(v) * 255.0) / 255.0;
}
"#;

/// 要缩放时用的视频处理器：输入纹理缩成 dst 大小的 BGRA
struct Scaler {
    processor: ID3D11VideoProcessor,
    input_view: ID3D11VideoProcessorInputView,
    output_view: ID3D11VideoProcessorOutputView,
}

/// 把 BGRA 画面叠成三倍高的 NV12，按输入输出尺寸建
pub struct Stacker {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    scaler: Option<Scaler>,
    /// 着色器读的画面：缩放过的或者输入纹理本身
    source: ID3D11ShaderResourceView,
    /// 叠好的 NV12 纹理和它亮度平面的渲染目标
    outputs: Vec<(ID3D11Texture2D, ID3D11RenderTargetView)>,
    next: usize,
    /// 原画面的宽高（编码的画面是 width × 3·height）
    width: u32,
    height: u32,
}

impl Stacker {
    /// input 是 src 大小的 BGRA 纹理（要能当着色器资源读），画面缩到 dst 再叠
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &ID3D11Device,
        ctx: &ID3D11DeviceContext,
        video_device: &ID3D11VideoDevice,
        video_ctx: &ID3D11VideoContext,
        input: &ID3D11Texture2D,
        src: (u32, u32),
        dst: (u32, u32),
        fps: u32,
    ) -> Result<Self> {
        let vs_code = compile("vs", s!("vs"), s!("vs_4_0"))?;
        let ps_code = compile("ps", s!("ps"), s!("ps_4_0"))?;
        let mut vs = None;
        let mut ps = None;
        unsafe {
            device.CreateVertexShader(blob_bytes(&vs_code), None::<&ID3D11ClassLinkage>, Some(&mut vs)).context("建不了顶点着色器")?;
            device.CreatePixelShader(blob_bytes(&ps_code), None::<&ID3D11ClassLinkage>, Some(&mut ps)).context("建不了像素着色器")?;
        }
        let vs = vs.ok_or_else(|| anyhow!("建不了顶点着色器"))?;
        let ps = ps.ok_or_else(|| anyhow!("建不了像素着色器"))?;

        let (scaler, source) = if src == dst {
            (None, shader_view(device, input)?)
        } else {
            let (processor, enumerator) = video_processor(video_device, video_ctx, src, dst, fps, None).context("视频处理器缩放不了 BGRA")?;
            let input_view = vp_input_view(video_device, &enumerator, input)?;
            let bind = D3D11_BIND_RENDER_TARGET.0 as u32 | D3D11_BIND_SHADER_RESOURCE.0 as u32;
            let scaled = texture_2d(device, dst.0, dst.1, DXGI_FORMAT_B8G8R8A8_UNORM, bind)?;
            let output_view = vp_output_view(video_device, &enumerator, &scaled).context("视频处理器输出不了 BGRA")?;
            (Some(Scaler { processor, input_view, output_view }), shader_view(device, &scaled)?)
        };

        let mut outputs = Vec::new();
        for _ in 0..POOL {
            let tex = encoder_texture(device, dst.0, dst.1 * 3).context("建不了三倍高的 NV12 纹理")?;
            let luma = render_target(device, &tex, DXGI_FORMAT_R8_UNORM).context("显卡不能往 NV12 的亮度平面上画")?;
            // 色度平面一直是 128，建好时涂一遍就行，以后只画亮度平面
            let chroma = render_target(device, &tex, DXGI_FORMAT_R8G8_UNORM).context("显卡不能往 NV12 的色度平面上画")?;
            let half = 128.0 / 255.0;
            unsafe { ctx.ClearRenderTargetView(&chroma, &[half, half, 0.0, 0.0]) };
            outputs.push((tex, luma));
        }
        tracing::debug!(?src, ?dst, "GPU 上叠 4:4:4 已就绪");
        Ok(Stacker { vs, ps, scaler, source, outputs, next: 0, width: dst.0, height: dst.1 })
    }

    /// 输入纹理里的画面叠成三倍高的 NV12，返回放结果的纹理
    pub fn pack(&mut self, ctx: &ID3D11DeviceContext, video_ctx: &ID3D11VideoContext) -> Result<ID3D11Texture2D> {
        if let Some(s) = &self.scaler {
            vp_blt(video_ctx, &s.processor, &s.input_view, &s.output_view).context("GPU 缩放失败")?;
        }
        let i = self.next;
        self.next = (i + 1) % self.outputs.len();
        let (tex, rtv) = &self.outputs[i];
        let viewport = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: self.width as f32,
            Height: (self.height * 3) as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        unsafe {
            // 编码器也用这块显卡，管线状态每次都设全，不指望上次留下的
            ctx.ClearState();
            ctx.IASetInputLayout(None::<&ID3D11InputLayout>);
            ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetShaderResources(0, Some(&[Some(self.source.clone())]));
            ctx.RSSetViewports(Some(&[viewport]));
            ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None::<&ID3D11DepthStencilView>);
            ctx.Draw(3, 0);
            // 解开绑定：下一帧要往输入纹理里拷画面，编码器要读输出纹理
            ctx.OMSetRenderTargets(None, None::<&ID3D11DepthStencilView>);
            ctx.PSSetShaderResources(0, Some(&[None]));
        }
        Ok(tex.clone())
    }
}

fn compile(name: &str, entry: PCSTR, target: PCSTR) -> Result<ID3DBlob> {
    let mut code = None;
    let mut errors = None;
    let flags = D3DCOMPILE_OPTIMIZATION_LEVEL3 | D3DCOMPILE_ENABLE_STRICTNESS;
    let result = unsafe { D3DCompile(SHADER.as_ptr().cast(), SHADER.len(), PCSTR::null(), None, None::<&ID3DInclude>, entry, target, flags, 0, &mut code, Some(&mut errors)) };
    if let Err(e) = result {
        let detail = errors.map(|b| String::from_utf8_lossy(blob_bytes(&b)).into_owned()).unwrap_or_default();
        return Err(anyhow!("编译着色器 {name} 失败：{e} {detail}"));
    }
    code.ok_or_else(|| anyhow!("编译着色器 {name} 没有结果"))
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()) }
}

fn shader_view(device: &ID3D11Device, tex: &ID3D11Texture2D) -> Result<ID3D11ShaderResourceView> {
    let mut view = None;
    unsafe { device.CreateShaderResourceView(tex, None, Some(&mut view)).context("建不了着色器读画面用的视图")? };
    view.ok_or_else(|| anyhow!("建不了着色器读画面用的视图"))
}

/// NV12 纹理一个平面的渲染目标：R8 是亮度平面，R8G8 是色度平面
fn render_target(device: &ID3D11Device, tex: &ID3D11Texture2D, format: DXGI_FORMAT) -> Result<ID3D11RenderTargetView> {
    let desc = D3D11_RENDER_TARGET_VIEW_DESC {
        Format: format,
        ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 } },
    };
    let mut view = None;
    unsafe { device.CreateRenderTargetView(tex, Some(&desc), Some(&mut view))? };
    view.ok_or_else(|| anyhow!("建不了渲染目标"))
}

#[cfg(test)]
pub(super) mod tests {
    //! 要真显卡，平时不跑：
    //! cargo test -p p2pcli-screen --release --lib -- --ignored --nocapture yuv444

    use super::*;
    use crate::win::capture::{ParamSets, package};
    use crate::win::encode::Encoder;
    use p2pcli_core::screen::{VideoCodec, VideoConfig, pack_yuv444_stacked, rgb_to_yuv444};
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC};
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter1, IDXGIFactory1};
    use windows::Win32::Media::MediaFoundation::{IMFDXGIDeviceManager, MFCreateDXGIDeviceManager};
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    use windows::core::Interface;

    pub(in crate::win) struct Gpu {
        pub device: ID3D11Device,
        pub ctx: ID3D11DeviceContext,
        pub video_device: ID3D11VideoDevice,
        pub video_ctx: ID3D11VideoContext,
    }

    /// 显存最大的那块硬件显卡（跳过远程桌面工具装的虚拟显卡、软件渲染器）
    pub(in crate::win) fn gpu() -> Gpu {
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.unwrap();
        let mut best: Option<(IDXGIAdapter1, usize, String)> = None;
        let mut i = 0;
        while let Ok(adapter) = unsafe { factory.EnumAdapters1(i) } {
            i += 1;
            let desc = unsafe { adapter.GetDesc1() }.unwrap();
            let name = String::from_utf16_lossy(&desc.Description).trim_end_matches('\0').to_string();
            println!("显卡 {i}：{name}，显存 {} MB", desc.DedicatedVideoMemory >> 20);
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            if best.as_ref().is_none_or(|b| desc.DedicatedVideoMemory > b.1) {
                best = Some((adapter, desc.DedicatedVideoMemory, name));
            }
        }
        let (adapter, _, name) = best.expect("没有硬件显卡");
        println!("用 {name}");
        let mut device = None;
        let mut ctx = None;
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )
            .unwrap();
        }
        let (device, ctx) = (device.unwrap(), ctx.unwrap());
        let mt: ID3D11Multithread = device.cast().unwrap();
        let _ = unsafe { mt.SetMultithreadProtected(true) };
        Gpu { video_device: device.cast().unwrap(), video_ctx: ctx.cast().unwrap(), device, ctx }
    }

    /// 测试图案（RGBA）：第一行开头是一个像素一个的纯色，其余是三个方向的渐变加上伪随机的像素
    pub(in crate::win) fn pattern(w: usize, h: usize, seed: u32) -> Vec<u8> {
        const SOLID: [[u8; 3]; 12] = [
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [0, 255, 255],
            [255, 0, 255],
            [255, 255, 0],
            [255, 255, 255],
            [0, 0, 0],
            [128, 128, 128],
            [255, 128, 0],
            [1, 254, 3],
            [127, 0, 128],
        ];
        let mut rng = seed.wrapping_mul(2654435761).max(1);
        let mut out = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let p = &mut out[(y * w + x) * 4..][..3];
                if y == 0 && x < SOLID.len() {
                    p.copy_from_slice(&SOLID[x]);
                } else if y % 4 == 3 {
                    for v in p.iter_mut() {
                        rng ^= rng << 13;
                        rng ^= rng >> 17;
                        rng ^= rng << 5;
                        *v = rng as u8;
                    }
                } else {
                    p.copy_from_slice(&[(x * 255 / (w - 1)) as u8, (y * 255 / (h - 1)) as u8, ((x + y + seed as usize) % 256) as u8]);
                }
            }
        }
        out
    }

    /// RGBA 上传成 BGRA 纹理，和抓屏的输入纹理一样能当渲染目标、着色器资源
    pub(in crate::win) fn upload(gpu: &Gpu, rgba: &[u8], w: u32, h: u32) -> ID3D11Texture2D {
        let bgra: Vec<u8> = rgba.chunks(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32 | D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let data = D3D11_SUBRESOURCE_DATA { pSysMem: bgra.as_ptr().cast(), SysMemPitch: w * 4, SysMemSlicePitch: 0 };
        let mut tex = None;
        unsafe { gpu.device.CreateTexture2D(&desc, Some(&data), Some(&mut tex)).unwrap() };
        tex.unwrap()
    }

    /// NV12 纹理读回内存：(亮度平面, 色度平面)，都去掉行尾的填充
    fn read_nv12(gpu: &Gpu, tex: &ID3D11Texture2D, w: usize, h: usize) -> (Vec<u8>, Vec<u8>) {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w as u32,
            Height: h as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging = None;
        unsafe { gpu.device.CreateTexture2D(&desc, None, Some(&mut staging)).unwrap() };
        let staging = staging.unwrap();
        let (mut luma, mut chroma) = (Vec::with_capacity(w * h), Vec::with_capacity(w * h / 2));
        unsafe {
            gpu.ctx.CopyResource(&staging, tex);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            gpu.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
            let base = mapped.pData as *const u8;
            let pitch = mapped.RowPitch as usize;
            for row in 0..h {
                luma.extend_from_slice(std::slice::from_raw_parts(base.add(row * pitch), w));
            }
            for row in 0..h / 2 {
                chroma.extend_from_slice(std::slice::from_raw_parts(base.add((h + row) * pitch), w));
            }
            gpu.ctx.Unmap(&staging, 0);
        }
        (luma, chroma)
    }

    /// 着色器叠出来的亮度平面和参考实现差不超过 1，色度平面全是 128
    #[test]
    #[ignore = "要真显卡"]
    fn yuv444_pack_matches_reference() {
        let gpu = gpu();
        for (w, h) in [(256usize, 144usize), (1920, 1080)] {
            let rgba = pattern(w, h, w as u32);
            let input = upload(&gpu, &rgba, w as u32, h as u32);
            let size = (w as u32, h as u32);
            let mut stacker = Stacker::new(&gpu.device, &gpu.ctx, &gpu.video_device, &gpu.video_ctx, &input, size, size, 60).unwrap();
            // 池子里每一块都叠一遍、都查
            for _ in 0..POOL {
                let out = stacker.pack(&gpu.ctx, &gpu.video_ctx).unwrap();
                let (luma, chroma) = read_nv12(&gpu, &out, w, h * 3);
                let want = pack_yuv444_stacked(&rgba, w, h);
                let mut worst = 0;
                let mut off = 0;
                for (i, (&got, &exp)) in luma.iter().zip(&want).enumerate() {
                    let d = (got as i32 - exp as i32).abs();
                    worst = worst.max(d);
                    if d > 0 {
                        off += 1;
                    }
                    assert!(d <= 1, "{w}×{h} 第 {} 段第 {} 个像素：算出 {got}，应该是 {exp}", i / (w * h), i % (w * h));
                }
                assert!(chroma.iter().all(|&c| c == 128), "色度平面不全是 128");
                println!("{w}×{h}：亮度平面 {} 个值里 {off} 个差 1，最大差 {worst}；色度平面全是 128", luma.len());
            }
        }
    }

    /// 缩放的那条路：大块纯色缩一半以后，块中间的颜色不变
    #[test]
    #[ignore = "要真显卡"]
    fn yuv444_pack_scaled() {
        let gpu = gpu();
        let (w, h) = (512usize, 256usize);
        let colors = [[255u8, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255], [0, 255, 255], [255, 0, 255], [255, 255, 0], [30, 60, 90]];
        let block = |x: usize, y: usize| colors[(y / 128) * 4 + x / 128];
        let mut rgba = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                rgba[(y * w + x) * 4..][..3].copy_from_slice(&block(x, y));
            }
        }
        let input = upload(&gpu, &rgba, w as u32, h as u32);
        let (dw, dh) = (w / 2, h / 2);
        let mut stacker = Stacker::new(&gpu.device, &gpu.ctx, &gpu.video_device, &gpu.video_ctx, &input, (w as u32, h as u32), (dw as u32, dh as u32), 60).unwrap();
        let out = stacker.pack(&gpu.ctx, &gpu.video_ctx).unwrap();
        let (luma, chroma) = read_nv12(&gpu, &out, dw, dh * 3);
        assert!(chroma.iter().all(|&c| c == 128));
        let mut worst = 0;
        for y in 0..dh {
            for x in 0..dw {
                // 块边上几个像素会被缩放滤波混到，不查
                if (x % 64) < 4 || (x % 64) >= 60 || (y % 64) < 4 || (y % 64) >= 60 {
                    continue;
                }
                let [r, g, b] = block(x * 2, y * 2);
                let want = rgb_to_yuv444(r, g, b);
                for (part, &exp) in want.iter().enumerate() {
                    let got = luma[part * dw * dh + y * dw + x];
                    let d = (got as i32 - exp as i32).abs();
                    worst = worst.max(d);
                    assert!(d <= 1, "缩放后 ({x},{y}) 第 {part} 段：算出 {got}，应该是 {exp}");
                }
            }
        }
        println!("缩放 {w}×{h} → {dw}×{dh}：块中间最大差 {worst}");
    }

    fn manager(gpu: &Gpu) -> IMFDXGIDeviceManager {
        let mut token = 0u32;
        let mut manager = None;
        unsafe { MFCreateDXGIDeviceManager(&mut token, &mut manager).unwrap() };
        let manager = manager.unwrap();
        unsafe { manager.ResetDevice(&gpu.device, token).unwrap() };
        manager
    }

    /// 用这种格式的硬件编码器开 w×3h，编几帧叠好的画面，等出带解码配置的关键帧；
    /// 再接着编 60 帧，量一帧从叠到编完平均多久。返回 (编码器名字, 解码配置, 每帧毫秒)
    fn try_encode(gpu: &Gpu, manager: &IMFDXGIDeviceManager, stacker: &mut Stacker, w: u32, h: u32, codec: VideoCodec) -> Result<(String, VideoConfig, f64)> {
        let mut enc = Encoder::hardware(manager, w, h * 3, 60, 30_000_000, codec, true)?;
        let mut sps_pps: Option<ParamSets> = None;
        let mut config = None;
        let mut ts = 0i64;
        for i in 0..30 {
            let tex = stacker.pack(&gpu.ctx, &gpu.video_ctx)?;
            ts += 166_667;
            let Some(data) = enc.encode(&gpu.ctx, &tex, ts, i == 0)? else { continue };
            let frame = package(&enc, &mut sps_pps, &data, 0)?;
            if let Some(c) = frame.config {
                anyhow::ensure!(frame.key, "带解码配置的不是关键帧");
                config = Some(c);
                break;
            }
        }
        let config = config.ok_or_else(|| anyhow!("编了 30 帧也没出关键帧"))?;
        let start = std::time::Instant::now();
        let mut out = 0;
        for _ in 0..60 {
            let tex = stacker.pack(&gpu.ctx, &gpu.video_ctx)?;
            ts += 166_667;
            out += usize::from(enc.encode(&gpu.ctx, &tex, ts, false)?.is_some());
        }
        anyhow::ensure!(out >= 50, "60 帧只编出 {out} 帧");
        Ok((enc.name.clone(), config, start.elapsed().as_secs_f64() * 1000.0 / 60.0))
    }

    /// 硬件编码器能不能开三倍高的画面：1920×3240 至少要有一种格式能编
    #[test]
    #[ignore = "要真显卡和硬件编码器"]
    fn yuv444_encoder_sizes() {
        let _ = tracing_subscriber::fmt().with_env_filter("warn").try_init();
        let gpu = gpu();
        let manager = manager(&gpu);
        let mut ok_1080 = false;
        let mut table = Vec::new();
        for (w, h) in [(1920u32, 1080u32), (2560, 1440), (3840, 2160)] {
            let rgba = pattern(w as usize, h as usize, 7);
            let input = upload(&gpu, &rgba, w, h);
            let mut stacker = Stacker::new(&gpu.device, &gpu.ctx, &gpu.video_device, &gpu.video_ctx, &input, (w, h), (w, h), 60).unwrap();
            for codec in [VideoCodec::H265, VideoCodec::H264] {
                let line = match try_encode(&gpu, &manager, &mut stacker, w, h, codec) {
                    Ok((name, c, ms)) => {
                        assert_eq!((c.width, c.height), (w, h * 3), "解码配置的宽高不对");
                        if h == 1080 {
                            ok_1080 = true;
                        }
                        format!("能编：{name}，{}，解码配置 {}×{}，一帧 {ms:.1} ms", c.codec, c.width, c.height)
                    }
                    Err(e) => format!("不行：{e:#}"),
                };
                table.push(format!("{w}×{} {codec:?}：{line}", h * 3));
            }
        }
        println!("\n{}", table.join("\n"));
        assert!(ok_1080, "1920×3240 哪种格式都编不了");
    }
}

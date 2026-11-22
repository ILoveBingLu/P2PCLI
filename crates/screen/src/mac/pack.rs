//! 4:4:4 叠图：在 GPU 上把抓到的 BGRA 画面（W×H）排成 W×3H 的 NV12（全范围），
//! 亮度平面从上到下是全分辨率的 Y、Cb、Cr，色度平面全是 128。编码器照常当 4:2:0 编，
//! 观看方解码后再合成彩色，每个像素都有自己的颜色。
//!
//! 4:4:4 开不了（编码器不接受这么高的画面）时，同一套东西也能把 BGRA 转成普通的 NV12 全范围。
//! 画面一直在 IOSurface 里，输入输出都通过 CVMetalTextureCache 直接给 GPU 读写，不拷贝。

use std::ptr::{self, NonNull};

use anyhow::{Result, anyhow, bail};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_video::{
    CVAttachmentMode, CVImageBuffer, CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBuffer,
    CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, CVPixelBufferPool,
    kCVImageBufferColorPrimariesKey, kCVImageBufferColorPrimaries_ITU_R_709_2, kCVImageBufferColorPrimaries_P3_D65,
    kCVImageBufferTransferFunctionKey, kCVImageBufferTransferFunction_sRGB, kCVImageBufferYCbCrMatrixKey,
    kCVImageBufferYCbCrMatrix_ITU_R_709_2, kCVMetalTextureUsage, kCVPixelBufferHeightKey,
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey, kCVPixelBufferPixelFormatTypeKey,
    kCVPixelBufferWidthKey,
};
use objc2_foundation::NSString;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandEncoder, MTLCommandQueue, MTLComputeCommandEncoder,
    MTLComputePipelineState, MTLCreateSystemDefaultDevice, MTLDevice, MTLLibrary, MTLPixelFormat, MTLSize, MTLTexture,
    MTLTextureUsage,
};

/// kCVPixelFormatType_32BGRA（'BGRA'）
pub(super) const PIXEL_FORMAT_BGRA: u32 = 0x4247_5241;
/// kCVPixelFormatType_420YpCbCr8BiPlanarFullRange（'420f'）
const PIXEL_FORMAT_NV12_FULL: u32 = 0x3432_3066;

// MTLCreateSystemDefaultDevice 要链接 CoreGraphics（objc2-core-graphics 已经链了，这里写明白）
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {}

/// 和 p2pcli_core::screen::rgb_to_yuv444 一模一样的公式：BT.709 矩阵、全范围，四舍五入
const SHADER: &str = r#"
#include <metal_stdlib>
using namespace metal;

constant float KR = 0.2126;
constant float KB = 0.0722;
constant float KG = 1.0 - KR - KB;

static float3 ycbcr(float3 c) {
    float y = KR * c.r + KG * c.g + KB * c.b;
    return float3(y, (c.b - y) / (2.0 * (1.0 - KB)) + 0.5, (c.r - y) / (2.0 * (1.0 - KR)) + 0.5);
}

static float q(float v) {
    return clamp(round(v * 255.0), 0.0, 255.0) / 255.0;
}

// 一个线程管一个像素：亮度平面写三行（Y、Cb、Cr），偶数列的线程顺手把色度平面填成 128
kernel void stacked(texture2d<float, access::read> src [[texture(0)]],
                    texture2d<float, access::write> luma [[texture(1)]],
                    texture2d<float, access::write> chroma [[texture(2)]],
                    uint2 p [[thread_position_in_grid]]) {
    uint w = src.get_width();
    uint h = src.get_height();
    if (p.x >= w || p.y >= h) return;
    float3 v = ycbcr(src.read(p).rgb);
    luma.write(float4(q(v.x)), p);
    luma.write(float4(q(v.y)), uint2(p.x, p.y + h));
    luma.write(float4(q(v.z)), uint2(p.x, p.y + 2 * h));
    if ((p.x & 1) == 0) {
        // 色度平面 W/2 × 3H/2：前 H 行按线程的行号，剩下的 H/2 行由上半的线程多写一行
        float4 mid = float4(128.0 / 255.0);
        uint cx = p.x >> 1;
        chroma.write(mid, uint2(cx, p.y));
        if (p.y < h / 2) chroma.write(mid, uint2(cx, p.y + h));
    }
}

// 退回 4:2:0 时用：一个线程管 2×2 个像素，色度取四个的平均
kernel void nv12(texture2d<float, access::read> src [[texture(0)]],
                 texture2d<float, access::write> luma [[texture(1)]],
                 texture2d<float, access::write> chroma [[texture(2)]],
                 uint2 p [[thread_position_in_grid]]) {
    uint2 o = p * 2;
    if (o.x >= src.get_width() || o.y >= src.get_height()) return;
    float2 c = float2(0.0);
    for (uint dy = 0; dy < 2; dy++) {
        for (uint dx = 0; dx < 2; dx++) {
            uint2 at = o + uint2(dx, dy);
            float3 v = ycbcr(src.read(at).rgb);
            luma.write(float4(q(v.x)), at);
            c += v.yz;
        }
    }
    c *= 0.25;
    chroma.write(float4(q(c.x), q(c.y), 0.0, 0.0), p);
}
"#;

type Pipeline = Retained<ProtocolObject<dyn MTLComputePipelineState>>;
/// 像素缓冲映射成的纹理：前一个要留到 GPU 用完，缓冲才不会被回收
type Texture = (CFRetained<CVMetalTexture>, Retained<ProtocolObject<dyn MTLTexture>>);

/// 输出缓冲的池子，尺寸变了就换一个
struct Pool {
    width: usize,
    height: usize,
    pool: CFRetained<CVPixelBufferPool>,
}

/// 从池子里拿一块缓冲（IOSurface，Metal 能直接读写），尺寸变了就换个池子
fn pool_buffer(slot: &mut Option<Pool>, format: u32, width: usize, height: usize) -> Result<CFRetained<CVPixelBuffer>> {
    if slot.as_ref().is_none_or(|p| p.width != width || p.height != height) {
        *slot = None;
        let format = CFNumber::new_i32(format as i32);
        let w = CFNumber::new_i32(width as i32);
        let h = CFNumber::new_i32(height as i32);
        let surface = CFDictionary::<CFString, CFType>::from_slices(&[], &[]);
        let attrs = unsafe {
            CFDictionary::<CFString, CFType>::from_slices(
                &[
                    kCVPixelBufferPixelFormatTypeKey,
                    kCVPixelBufferWidthKey,
                    kCVPixelBufferHeightKey,
                    kCVPixelBufferIOSurfacePropertiesKey,
                    kCVPixelBufferMetalCompatibilityKey,
                ],
                &[format.as_ref(), w.as_ref(), h.as_ref(), surface.as_ref(), CFBoolean::new(true).as_ref()],
            )
        };
        let mut pool: *mut CVPixelBufferPool = ptr::null_mut();
        let status = unsafe { CVPixelBufferPool::create(None, None, Some(attrs.as_opaque()), NonNull::from(&mut pool)) };
        let pool = NonNull::new(pool).filter(|_| status == 0).ok_or_else(|| anyhow!("建不了 {width}×{height} 的缓冲池（{status}）"))?;
        *slot = Some(Pool { width, height, pool: unsafe { CFRetained::from_raw(pool) } });
    }
    let pool = &slot.as_ref().unwrap().pool;
    let mut out: *mut CVPixelBuffer = ptr::null_mut();
    let status = unsafe { CVPixelBufferPool::create_pixel_buffer(None, pool, NonNull::from(&mut out)) };
    let out = NonNull::new(out).filter(|_| status == 0).ok_or_else(|| anyhow!("缓冲池给不出缓冲（{status}）"))?;
    Ok(unsafe { CFRetained::from_raw(out) })
}

pub(super) struct Packer {
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    stacked: Pipeline,
    nv12: Pipeline,
    cache: CFRetained<CVMetalTextureCache>,
    pool: Option<Pool>,
    /// 画面是 Display P3：输出缓冲标 P3 原色，和编码器的标记一致，免得编码器再转一遍颜色
    p3: bool,
}

// Metal 的设备、队列、管线和纹理缓存都可以跨线程用；同一时间只有一个线程在调
unsafe impl Send for Packer {}

fn ns_err(e: &objc2_foundation::NSError) -> String {
    e.localizedDescription().to_string()
}

impl Packer {
    pub(super) fn new(p3: bool) -> Result<Self> {
        let device = MTLCreateSystemDefaultDevice().ok_or_else(|| anyhow!("这台电脑没有能用的 Metal 显卡"))?;
        let library = device
            .newLibraryWithSource_options_error(&NSString::from_str(SHADER), None)
            .map_err(|e| anyhow!("着色器编译失败：{}", ns_err(&e)))?;
        let pipeline = |name: &str| -> Result<Pipeline> {
            let f = library.newFunctionWithName(&NSString::from_str(name)).ok_or_else(|| anyhow!("着色器里没有 {name}"))?;
            device.newComputePipelineStateWithFunction_error(&f).map_err(|e| anyhow!("建不了计算管线：{}", ns_err(&e)))
        };
        let stacked = pipeline("stacked")?;
        let nv12 = pipeline("nv12")?;
        let queue = device.newCommandQueue().ok_or_else(|| anyhow!("建不了 Metal 命令队列"))?;
        let mut cache: *mut CVMetalTextureCache = ptr::null_mut();
        let status = unsafe { CVMetalTextureCache::create(None, None, &device, None, NonNull::from(&mut cache)) };
        let cache = NonNull::new(cache).filter(|_| status == 0).ok_or_else(|| anyhow!("建不了纹理缓存（{status}）"))?;
        let cache = unsafe { CFRetained::from_raw(cache) };
        Ok(Packer { queue, stacked, nv12, cache, pool: None, p3 })
    }

    /// 从池子里拿一块 '420f' 的输出缓冲（IOSurface，Metal 能直接写）
    fn buffer(&mut self, width: usize, height: usize) -> Result<CFRetained<CVPixelBuffer>> {
        pool_buffer(&mut self.pool, PIXEL_FORMAT_NV12_FULL, width, height)
    }

    /// 把像素缓冲的一个平面当成 Metal 纹理
    fn texture(
        &self,
        image: &CVImageBuffer,
        format: MTLPixelFormat,
        width: usize,
        height: usize,
        plane: usize,
        write: bool,
    ) -> Result<Texture> {
        let usage = MTLTextureUsage::ShaderRead | if write { MTLTextureUsage::ShaderWrite } else { MTLTextureUsage::empty() };
        let usage = CFNumber::new_i64(usage.bits() as i64);
        let attrs = unsafe { CFDictionary::<CFString, CFType>::from_slices(&[kCVMetalTextureUsage], &[usage.as_ref()]) };
        let mut out: *mut CVMetalTexture = ptr::null_mut();
        let status = unsafe {
            CVMetalTextureCache::create_texture_from_image(
                None,
                &self.cache,
                image,
                Some(attrs.as_opaque()),
                format,
                width,
                height,
                plane,
                NonNull::from(&mut out),
            )
        };
        let out = NonNull::new(out).filter(|_| status == 0).ok_or_else(|| anyhow!("画面转不成 Metal 纹理（{status}）"))?;
        let cv = unsafe { CFRetained::from_raw(out) };
        let tex = CVMetalTextureGetTexture(&cv).ok_or_else(|| anyhow!("画面转不成 Metal 纹理"))?;
        Ok((cv, tex))
    }

    /// BGRA 画面（W×H）排成 NV12 全范围：stacked 为真时是 W×3H 的 4:4:4 叠图，假时是普通的 W×H 4:2:0。
    /// 等 GPU 做完才返回，拿到的缓冲可以直接交给编码器
    pub(super) fn pack(&mut self, src: &CVImageBuffer, stacked: bool) -> Result<CFRetained<CVPixelBuffer>> {
        let format = CVPixelBufferGetPixelFormatType(src);
        if format != PIXEL_FORMAT_BGRA {
            bail!("画面不是 BGRA（{format:#x}）");
        }
        let (w, h) = (CVPixelBufferGetWidth(src), CVPixelBufferGetHeight(src));
        if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 {
            bail!("画面尺寸要是偶数：{w}×{h}");
        }
        let (ow, oh) = if stacked { (w, h * 3) } else { (w, h) };
        let out = self.buffer(ow, oh)?;
        let (_src_cv, src_tex) = self.texture(src, MTLPixelFormat::BGRA8Unorm, w, h, 0, false)?;
        let (_luma_cv, luma) = self.texture(&out, MTLPixelFormat::R8Unorm, ow, oh, 0, true)?;
        let (_chroma_cv, chroma) = self.texture(&out, MTLPixelFormat::RG8Unorm, ow / 2, oh / 2, 1, true)?;

        let pipeline = if stacked { &self.stacked } else { &self.nv12 };
        let cmd = self.queue.commandBuffer().ok_or_else(|| anyhow!("建不了 Metal 命令"))?;
        let enc = cmd.computeCommandEncoder().ok_or_else(|| anyhow!("建不了 Metal 计算命令"))?;
        enc.setComputePipelineState(pipeline);
        unsafe {
            enc.setTexture_atIndex(Some(&src_tex), 0);
            enc.setTexture_atIndex(Some(&luma), 1);
            enc.setTexture_atIndex(Some(&chroma), 2);
        }
        let grid = if stacked { MTLSize { width: w, height: h, depth: 1 } } else { MTLSize { width: w / 2, height: h / 2, depth: 1 } };
        let tw = pipeline.threadExecutionWidth().max(1);
        let th = (pipeline.maxTotalThreadsPerThreadgroup() / tw).clamp(1, 16);
        enc.dispatchThreads_threadsPerThreadgroup(grid, MTLSize { width: tw, height: th, depth: 1 });
        enc.endEncoding();
        cmd.commit();
        // 一帧 4K 在 GPU 上不到一毫秒，直接等；编码器读的是同一块 IOSurface，必须等写完
        cmd.waitUntilCompleted();
        if cmd.status() != MTLCommandBufferStatus::Completed {
            let message = cmd.error().map(|e| ns_err(&e)).unwrap_or_default();
            bail!("GPU 排画面失败：{message}");
        }
        drop((_src_cv, _luma_cv, _chroma_cv));
        self.cache.flush(0);

        // 颜色标记和编码器的一致：BT.709 矩阵、sRGB 曲线、原色看抓屏用的色域
        unsafe {
            let primaries = if self.p3 { kCVImageBufferColorPrimaries_P3_D65 } else { kCVImageBufferColorPrimaries_ITU_R_709_2 };
            out.set_attachment(kCVImageBufferYCbCrMatrixKey, kCVImageBufferYCbCrMatrix_ITU_R_709_2, CVAttachmentMode::ShouldPropagate);
            out.set_attachment(kCVImageBufferColorPrimariesKey, primaries, CVAttachmentMode::ShouldPropagate);
            out.set_attachment(kCVImageBufferTransferFunctionKey, kCVImageBufferTransferFunction_sRGB, CVAttachmentMode::ShouldPropagate);
        }
        Ok(out)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use objc2_core_video::{
        CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferLockBaseAddress,
        CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    };
    use p2pcli_core::screen::{pack_yuv444_stacked, rgb_to_yuv444};

    /// 合成的“屏幕”：慢慢变的背景上一大块往上滚的彩色“文字”（像在滚动网页），几个来回跑的方块和圆。
    /// 每帧的内容都不一样，编码器得真的编
    const SCENE: &str = r#"
#include <metal_stdlib>
using namespace metal;

static uint hash2(uint x, uint y) {
    uint v = x * 374761393u + y * 668265263u;
    v = (v ^ (v >> 13)) * 1274126177u;
    return v ^ (v >> 16);
}

static float3 palette(uint i) {
    switch (i % 6) {
        case 0: return float3(0.95, 0.95, 0.95);
        case 1: return float3(1.0, 0.35, 0.3);
        case 2: return float3(0.3, 0.85, 0.4);
        case 3: return float3(0.35, 0.6, 1.0);
        case 4: return float3(1.0, 0.85, 0.2);
        default: return float3(0.85, 0.4, 1.0);
    }
}

kernel void scene(texture2d<float, access::write> out [[texture(0)]],
                  constant uint &frame [[buffer(0)]],
                  uint2 p [[thread_position_in_grid]]) {
    uint w = out.get_width();
    uint h = out.get_height();
    if (p.x >= w || p.y >= h) return;
    float t = float(frame);
    float3 c = float3(0.12 + 0.08 * sin((float(p.x) + t * 3.0) / 211.0), 0.14, 0.18 + 0.08 * cos((float(p.y) + t * 2.0) / 173.0));
    // 文字区：9×18 的字格，每帧往上滚 4 行像素
    if (p.x > w / 24 && p.x < w * 15 / 24) {
        uint sy = p.y + frame * 4u;
        uint cx = (p.x - w / 24) / 9u, cy = sy / 18u, gx = (p.x - w / 24) % 9u, gy = sy % 18u;
        uint line = hash2(7u, cy);
        bool text = (line & 7u) != 0u && cx < 30u + (line >> 8) % 120u;
        uint glyph = hash2(cx, cy);
        if (text && (glyph & 7u) != 0u && gx < 7u && gy >= 3u && gy < 15u) {
            if ((hash2(glyph + gx / 2u, gy / 3u) & 1u) != 0u) c = palette(line >> 4);
        }
    }
    // 来回跑的方块和圆
    for (uint k = 0; k < 4; k++) {
        float fk = float(k);
        float2 center = float2(float(w) * (0.5 + 0.4 * sin(t * 0.031 * (fk + 1.0) + fk)), float(h) * (0.5 + 0.4 * cos(t * 0.023 * (fk + 1.0) + fk * 2.0)));
        float2 d = abs(float2(p) - center);
        float r = float(h) * (0.05 + 0.02 * fk);
        bool hit = (k % 2 == 0) ? (d.x < r && d.y < r) : (length(d) < r);
        if (hit) c = palette(k + 1) * 0.8 + 0.2 * float3(float(p.x % 7) / 7.0);
    }
    out.write(float4(c, 1.0), p);
}
"#;

    /// 在 GPU 上画合成的画面，出 BGRA 像素缓冲
    pub(in super::super) struct Scene {
        gpu: Packer,
        pipeline: Pipeline,
        pool: Option<Pool>,
        frame: u32,
    }

    impl Scene {
        pub(in super::super) fn new() -> Self {
            let gpu = Packer::new(false).unwrap();
            let device = gpu.queue.device();
            let library = device.newLibraryWithSource_options_error(&NSString::from_str(SCENE), None).unwrap();
            let f = library.newFunctionWithName(&NSString::from_str("scene")).unwrap();
            let pipeline = device.newComputePipelineStateWithFunction_error(&f).unwrap();
            Scene { gpu, pipeline, pool: None, frame: 0 }
        }

        /// 下一帧画面（每次都往前动一点）
        pub(in super::super) fn render(&mut self, w: usize, h: usize) -> CFRetained<CVPixelBuffer> {
            let out = pool_buffer(&mut self.pool, PIXEL_FORMAT_BGRA, w, h).unwrap();
            let (_cv, tex) = self.gpu.texture(&out, MTLPixelFormat::BGRA8Unorm, w, h, 0, true).unwrap();
            let cmd = self.gpu.queue.commandBuffer().unwrap();
            let enc = cmd.computeCommandEncoder().unwrap();
            enc.setComputePipelineState(&self.pipeline);
            let frame = self.frame;
            self.frame += 1;
            unsafe {
                enc.setTexture_atIndex(Some(&tex), 0);
                enc.setBytes_length_atIndex(NonNull::from(&frame).cast(), 4, 0);
            }
            let tw = self.pipeline.threadExecutionWidth().max(1);
            let th = (self.pipeline.maxTotalThreadsPerThreadgroup() / tw).clamp(1, 16);
            enc.dispatchThreads_threadsPerThreadgroup(MTLSize { width: w, height: h, depth: 1 }, MTLSize { width: tw, height: th, depth: 1 });
            enc.endEncoding();
            cmd.commit();
            cmd.waitUntilCompleted();
            assert_eq!(cmd.status(), MTLCommandBufferStatus::Completed);
            drop(_cv);
            self.gpu.cache.flush(0);
            out
        }

        /// 同样的画面转成 NV12 全范围：当作系统直接给的 4:2:0 画面
        pub(in super::super) fn render_nv12(&mut self, w: usize, h: usize) -> CFRetained<CVPixelBuffer> {
            let bgra = self.render(w, h);
            self.gpu.pack(&bgra, false).unwrap()
        }
    }

    /// 测试用的 RGBA 图案：第一行每个像素一种饱和色，下面是横竖两个方向的渐变，再撒些伪随机像素
    pub(in super::super) fn pattern(w: usize, h: usize) -> Vec<u8> {
        let colors: [[u8; 3]; 12] = [
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
            [1, 2, 254],
            [254, 253, 1],
        ];
        let mut rgba = vec![255u8; w * h * 4];
        let mut seed = 0x1234_5678u32;
        for y in 0..h {
            for x in 0..w {
                let px = if y == 0 && x < colors.len() {
                    colors[x]
                } else if y % 7 == 3 {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let b = seed.to_le_bytes();
                    [b[1], b[2], b[3]]
                } else {
                    [(x * 255 / (w - 1)) as u8, (y * 255 / (h - 1)) as u8, ((x + 2 * y) % 256) as u8]
                };
                rgba[(y * w + x) * 4..][..3].copy_from_slice(&px);
            }
        }
        rgba
    }

    /// 建一块 IOSurface 的 BGRA 像素缓冲，填上 RGBA 图案
    pub(in super::super) fn bgra_buffer(rgba: &[u8], w: usize, h: usize) -> CFRetained<CVPixelBuffer> {
        let empty = CFDictionary::<CFString, CFType>::from_slices(&[], &[]);
        let attrs = unsafe {
            CFDictionary::<CFString, CFType>::from_slices(
                &[kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey],
                &[empty.as_ref(), CFBoolean::new(true).as_ref()],
            )
        };
        let mut pb: *mut CVPixelBuffer = ptr::null_mut();
        let r = unsafe { CVPixelBufferCreate(None, w, h, PIXEL_FORMAT_BGRA, Some(attrs.as_opaque()), NonNull::from(&mut pb)) };
        assert_eq!(r, 0);
        let pb = unsafe { CFRetained::from_raw(NonNull::new(pb).unwrap()) };
        unsafe {
            assert_eq!(CVPixelBufferLockBaseAddress(&pb, CVPixelBufferLockFlags(0)), 0);
            let base = CVPixelBufferGetBaseAddressOfPlane(&pb, 0) as *mut u8;
            let stride = CVPixelBufferGetBytesPerRowOfPlane(&pb, 0);
            for y in 0..h {
                for x in 0..w {
                    let s = &rgba[(y * w + x) * 4..][..4];
                    let d = std::slice::from_raw_parts_mut(base.add(y * stride + x * 4), 4);
                    d.copy_from_slice(&[s[2], s[1], s[0], 255]);
                }
            }
            CVPixelBufferUnlockBaseAddress(&pb, CVPixelBufferLockFlags(0));
        }
        pb
    }

    /// 读出 NV12 的两个平面（去掉行尾的填充）
    fn read_planes(pb: &CVPixelBuffer) -> (Vec<u8>, Vec<u8>, usize, usize) {
        let (w, h) = (CVPixelBufferGetWidth(pb), CVPixelBufferGetHeight(pb));
        let mut luma = Vec::with_capacity(w * h);
        let mut chroma = Vec::with_capacity(w * h / 2);
        unsafe {
            assert_eq!(CVPixelBufferLockBaseAddress(pb, CVPixelBufferLockFlags::ReadOnly), 0);
            let y = CVPixelBufferGetBaseAddressOfPlane(pb, 0) as *const u8;
            let ys = CVPixelBufferGetBytesPerRowOfPlane(pb, 0);
            for row in 0..h {
                luma.extend_from_slice(std::slice::from_raw_parts(y.add(row * ys), w));
            }
            let uv = CVPixelBufferGetBaseAddressOfPlane(pb, 1) as *const u8;
            let uvs = CVPixelBufferGetBytesPerRowOfPlane(pb, 1);
            for row in 0..h / 2 {
                chroma.extend_from_slice(std::slice::from_raw_parts(uv.add(row * uvs), w));
            }
            CVPixelBufferUnlockBaseAddress(pb, CVPixelBufferLockFlags::ReadOnly);
        }
        (luma, chroma, w, h)
    }

    /// GPU 排出来的叠图和 core 里的参考实现逐个像素比，差不超过 1；色度平面全是 128
    #[test]
    #[ignore = "要 GPU：cargo test -p p2pcli-screen -- --ignored pack"]
    fn pack_stacked_matches_reference() {
        let mut packer = Packer::new(false).unwrap();
        // 宽不是线程组的整数倍，边上的像素也要对
        for (w, h) in [(70usize, 40usize), (1920, 1080), (3840, 2160)] {
            let rgba = pattern(w, h);
            let src = bgra_buffer(&rgba, w, h);
            let out = packer.pack(&src, true).unwrap();
            assert_eq!(CVPixelBufferGetPixelFormatType(&out), PIXEL_FORMAT_NV12_FULL);
            let (luma, chroma, ow, oh) = read_planes(&out);
            assert_eq!((ow, oh), (w, h * 3));
            let want = pack_yuv444_stacked(&rgba, w, h);
            let mut max_diff = 0;
            for (i, (&a, &b)) in luma.iter().zip(&want).enumerate() {
                let d = (a as i32 - b as i32).abs();
                assert!(d <= 1, "{w}×{h} 第 {} 行第 {} 列：GPU {a}，参考 {b}", i / w, i % w);
                max_diff = max_diff.max(d);
            }
            assert!(chroma.iter().all(|&v| v == 128), "{w}×{h} 色度平面不全是 128");
            // 第一行的饱和色单独再核一遍，失败时好看
            for x in 0..12 {
                let px = &rgba[x * 4..x * 4 + 3];
                let [y, cb, cr] = rgb_to_yuv444(px[0], px[1], px[2]);
                let got = [luma[x], luma[w * h + x], luma[2 * w * h + x]];
                for k in 0..3 {
                    assert!((got[k] as i32 - [y, cb, cr][k] as i32).abs() <= 1, "颜色 {px:?}：GPU {got:?}，参考 {:?}", [y, cb, cr]);
                }
            }
            // 顺便看看一帧要多久（含等 GPU 做完）
            let t = std::time::Instant::now();
            for _ in 0..30 {
                packer.pack(&src, true).unwrap();
            }
            println!("{w}×{h}：亮度平面最大差 {max_diff}，每帧 {:?}", t.elapsed() / 30);
        }
    }

    /// 退回 4:2:0 时的转换：Y 和参考一样，色度是 2×2 的平均
    #[test]
    #[ignore = "要 GPU：cargo test -p p2pcli-screen -- --ignored pack"]
    fn pack_nv12_matches_reference() {
        let mut packer = Packer::new(true).unwrap();
        let (w, h) = (70usize, 40usize);
        let rgba = pattern(w, h);
        let src = bgra_buffer(&rgba, w, h);
        let out = packer.pack(&src, false).unwrap();
        let (luma, chroma, ow, oh) = read_planes(&out);
        assert_eq!((ow, oh), (w, h));
        let ref444 = pack_yuv444_stacked(&rgba, w, h);
        for i in 0..w * h {
            assert!((luma[i] as i32 - ref444[i] as i32).abs() <= 1, "Y 第 {i} 个");
        }
        for cy in 0..h / 2 {
            for cx in 0..w / 2 {
                for k in 0..2 {
                    let plane = &ref444[(k + 1) * w * h..];
                    let sum: u32 = [(0, 0), (1, 0), (0, 1), (1, 1)].iter().map(|(dx, dy)| plane[(cy * 2 + dy) * w + cx * 2 + dx] as u32).sum();
                    let want = sum as f64 / 4.0;
                    let got = chroma[cy * w + cx * 2 + k] as f64;
                    assert!((got - want).abs() <= 1.5, "色度 ({cx},{cy}) 分量 {k}：GPU {got}，参考 {want}");
                }
            }
        }
    }
}

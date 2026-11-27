//! 抓本机正在放的声音：ScreenCaptureKit 单开一路只要声音的流（画面设成 2×2、一秒一帧，几乎不占资源），
//! 系统给 48 kHz、立体声的 f32（一般左右分开放），这里交错成一段段送出去。用的是“屏幕录制”权限，看画面本来就要。
//! P2PCLI 自己放的声音不抓。

use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::CGMainDisplayID;
use objc2_core_media::{CMAudioFormatDescriptionGetStreamBasicDescription, CMBlockBuffer, CMSampleBuffer, CMTime};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{SCContentFilter, SCStream, SCStreamConfiguration, SCStreamOutput, SCStreamOutputType};
use p2pcli_core::screen_audio::{AudioSink, CHANNELS, SAMPLE_RATE};
use tracing::{debug, info, warn};

use super::capture::{find_display, wait};

/// kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment：每块数据 16 字节对齐
const ASSURE_16_BYTE_ALIGNMENT: u32 = 1;

pub(crate) struct Ivars {
    sink: AudioSink,
    /// 格式不对的警告只记一次
    warned: AtomicBool,
}

define_class!(
    // SAFETY: 只继承 NSObject，没有重写 dealloc
    #[unsafe(super(NSObject))]
    #[name = "P2PCLIAudioSink"]
    #[ivars = Ivars]
    struct Sink;

    unsafe impl NSObjectProtocol for Sink {}

    unsafe impl SCStreamOutput for Sink {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind != SCStreamOutputType::Audio {
                return;
            }
            match unsafe { interleaved(sample) } {
                Ok(Some(pcm)) => {
                    // 编码那头跟不上就丢这一段，不卡系统的线程
                    let _ = self.ivars().sink.try_send(pcm);
                }
                Ok(None) => {}
                Err(e) => {
                    if !self.ivars().warned.swap(true, Ordering::Relaxed) {
                        warn!("系统给的声音读不了：{e:#}");
                    }
                }
            }
        }
    }
);

impl Sink {
    fn new(sink: AudioSink) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { sink, warned: AtomicBool::new(false) });
        unsafe { msg_send![super(this), init] }
    }
}

/// 一段系统给的声音转成左右交错的 f32。不是 f32（不该发生）就报错；没有内容是 None
unsafe fn interleaved(sample: &CMSampleBuffer) -> Result<Option<Vec<f32>>> {
    let frames = unsafe { sample.num_samples() } as usize;
    if frames == 0 {
        return Ok(None);
    }
    let desc = unsafe { sample.format_description() }.ok_or_else(|| anyhow!("没有格式描述"))?;
    let asbd = unsafe { CMAudioFormatDescriptionGetStreamBasicDescription(&desc).as_ref() }.ok_or_else(|| anyhow!("不是声音"))?;
    if asbd.mFormatFlags & kAudioFormatFlagIsFloat == 0 || asbd.mBitsPerChannel != 32 {
        bail!("不是 32 位浮点（格式标志 {:#x}，{} 位）", asbd.mFormatFlags, asbd.mBitsPerChannel);
    }
    if asbd.mSampleRate as u32 != SAMPLE_RATE {
        bail!("采样率是 {}，不是 {SAMPLE_RATE}", asbd.mSampleRate);
    }
    let channels = asbd.mChannelsPerFrame.max(1) as usize;
    let planar = asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0;

    // 先问要多大的 AudioBufferList（块数随格式变，结构是变长的），再按这个大小取
    let mut needed = 0usize;
    let status = unsafe {
        sample.audio_buffer_list_with_retained_block_buffer(&mut needed, ptr::null_mut(), 0, None, None, ASSURE_16_BYTE_ALIGNMENT, ptr::null_mut())
    };
    if status != 0 || needed < std::mem::size_of::<AudioBufferList>() {
        bail!("问不出声音数据的大小（{status}，{needed}）");
    }
    // 按 8 字节对齐的内存放（里面有指针）
    let mut storage = vec![0u64; needed.div_ceil(8)];
    let list = storage.as_mut_ptr() as *mut AudioBufferList;
    let mut block: *mut CMBlockBuffer = ptr::null_mut();
    let status = unsafe {
        sample.audio_buffer_list_with_retained_block_buffer(ptr::null_mut(), list, needed, None, None, ASSURE_16_BYTE_ALIGNMENT, &mut block)
    };
    // 数据在 block 里：拷完再放
    let _block = NonNull::new(block).map(|b| unsafe { CFRetained::from_raw(b) });
    if status != 0 {
        bail!("取不出声音数据（{status}）");
    }
    let count = unsafe { (*list).mNumberBuffers } as usize;
    let buffers = unsafe { ptr::addr_of!((*list).mBuffers) as *const AudioBuffer };
    let plane = |i: usize| -> &[f32] {
        if i >= count {
            return &[];
        }
        let b = unsafe { &*buffers.add(i) };
        if b.mData.is_null() {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(b.mData as *const f32, b.mDataByteSize as usize / 4) }
    };
    let mut out = Vec::with_capacity(frames * CHANNELS);
    if planar {
        // 左右各一块；只有一块（单声道）就两边一样
        let left = plane(0);
        let right = if count > 1 { plane(1) } else { left };
        let n = frames.min(left.len()).min(right.len());
        for i in 0..n {
            out.push(left[i]);
            out.push(right[i]);
        }
    } else {
        // 已经交错：取前两个声道
        let data = plane(0);
        let n = frames.min(data.len() / channels);
        for i in 0..n {
            let l = data[i * channels];
            let r = if channels > 1 { data[i * channels + 1] } else { l };
            out.push(l);
            out.push(r);
        }
    }
    Ok(Some(out))
}

/// 一路正在抓的声音。丢掉就停
pub struct MacAudio {
    stream: Retained<SCStream>,
    _sink: Retained<Sink>,
    _queue: DispatchRetained<DispatchQueue>,
}

// SCStream 只在创建和停止时碰，系统允许跨线程调用
unsafe impl Send for MacAudio {}

impl MacAudio {
    pub fn start(sink: AudioSink) -> Result<Self> {
        let display = find_display(CGMainDisplayID())?;
        let config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            // 只要声音：画面最小、最慢
            config.setWidth(2);
            config.setHeight(2);
            config.setMinimumFrameInterval(CMTime::new(1, 1));
            config.setQueueDepth(3);
            config.setCapturesAudio(true);
            config.setSampleRate(SAMPLE_RATE as isize);
            config.setChannelCount(CHANNELS as isize);
            config.setExcludesCurrentProcessAudio(true);
        }
        let filter = unsafe { SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &NSArray::new()) };
        let sink = Sink::new(sink);
        let queue = DispatchQueue::new("cn.p2pcli.audio", None);
        let stream = unsafe { SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, None) };
        unsafe {
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(ProtocolObject::from_ref(&*sink), SCStreamOutputType::Audio, Some(&queue))
                .map_err(|e| anyhow!("抓声音开不起来：{}", e.localizedDescription()))?;
            // 画面也接上（直接扔掉），免得系统一直报“没人收画面”
            let _ = stream.addStreamOutput_type_sampleHandlerQueue_error(ProtocolObject::from_ref(&*sink), SCStreamOutputType::Screen, Some(&queue));
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
            bail!("抓声音开不起来：{e}");
        }
        info!("开始抓本机的声音");
        Ok(MacAudio { stream, _sink: sink, _queue: queue })
    }
}

impl Drop for MacAudio {
    fn drop(&mut self) {
        unsafe { self.stream.stopCaptureWithCompletionHandler(None) };
        debug!("停止抓本机的声音");
    }
}

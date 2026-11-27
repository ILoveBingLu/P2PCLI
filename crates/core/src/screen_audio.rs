//! 远程画面的声音：被连方抓本机正在放的声音（48 kHz 立体声），用 Opus 编成 20 毫秒一包，
//! 走单独的一条流发给观看方，不和画面挤在一条流上，优先级在终端和画面之间。
//!
//! 流照例由观看方开：被连方同意看画面、又能抓声音时，在 [`crate::screen::ScreenStarted::audio_token`] 里给一个号，
//! 观看方开一条 [`crate::proto::Msg::ScreenAudio`] 流带上这个号，被连方按号找到这一路画面，开始抓声音往里写。
//! 观看方静音时把流关掉，被连方就不抓了，不白占带宽；取消静音再开一条。
//!
//! 流上每一包：4 字节长度（大端，不含这 4 个字节）+ 8 字节时间戳（微秒，从开始抓声音算）+ Opus 数据。
//! Opus 用纯 Rust 的 opus-pure（不用编 C 代码、哪个平台都能编），码流就是标准的 Opus，别的解码器也能解。
//!
//! 画面暂停（连接走了中继）时声音也不发，和画面一致。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use opus_pure::{Application, MAX_PACKET_BYTES, MAX_PACKET_SAMPLES, OpusDecoder, OpusEncoder};
use tracing::{debug, info, warn};

use crate::screen::ScreenBackend;

/// 采样率：Opus 的标准
pub const SAMPLE_RATE: u32 = 48_000;
/// 立体声
pub const CHANNELS: usize = 2;
/// 一包 20 毫秒（每个声道 960 个采样）
pub const FRAME: usize = SAMPLE_RATE as usize / 50;
/// 码率：系统声音里音乐、视频多，96 kbps 的立体声听不出和原声的差别
const BITRATE: i32 = 96_000;
/// 比画面（-10）高、比终端（0）低：声音断断续续比画面卡更难受
pub const AUDIO_PRIORITY: i32 = -5;
/// 一包最大多少字节，超过就当流坏了
const MAX_PACKET: usize = 8 * 1024;
/// 平台送来的声音最多攒这么多段没编，再多就丢（编码线程卡住时不越积越多）
pub const SINK_DEPTH: usize = 64;

/// 平台抓到的声音往这里送：48 kHz、立体声、左右交错的 f32（-1.0 ~ 1.0），每段多长都行。
/// 满了就丢掉这一段（try_send），不阻塞平台的线程
pub type AudioSink = tokio::sync::mpsc::Sender<Vec<f32>>;

/// 把一段段的声音攒成 20 毫秒一包，编成 Opus
pub struct Encoder {
    enc: OpusEncoder,
    pending: Vec<f32>,
    out: Vec<u8>,
}

impl Encoder {
    pub fn new() -> Result<Self> {
        // 受限低延迟：只用 CELT，编码器自身的延迟最小（2.5 毫秒的预读），系统声音里有音乐也能编好
        let mut enc = OpusEncoder::new(SAMPLE_RATE as i32, CHANNELS, Application::RestrictedLowDelay).map_err(|e| anyhow!("开不了 Opus 编码器：{e}"))?;
        enc.bitrate_bps = BITRATE;
        Ok(Encoder { enc, pending: Vec::with_capacity(FRAME * CHANNELS * 2), out: vec![0; MAX_PACKET_BYTES] })
    }

    /// 加一段声音，凑够一包就编，每编好一包调一次 emit
    pub fn push(&mut self, pcm: &[f32], mut emit: impl FnMut(&[u8])) -> Result<()> {
        self.pending.extend_from_slice(pcm);
        let per = FRAME * CHANNELS;
        let mut start = 0;
        while self.pending.len() - start >= per {
            let n = self.enc.encode(&self.pending[start..start + per], FRAME, &mut self.out).map_err(|e| anyhow!("Opus 编码失败：{e}"))?;
            emit(&self.out[..n]);
            start += per;
        }
        self.pending.drain(..start);
        Ok(())
    }
}

/// Opus 包解成 48 kHz 立体声、左右交错的 f32
pub struct Decoder {
    dec: OpusDecoder,
    buf: Vec<f32>,
}

impl Decoder {
    pub fn new() -> Result<Self> {
        let dec = OpusDecoder::new(SAMPLE_RATE as i32, CHANNELS).map_err(|e| anyhow!("开不了 Opus 解码器：{e}"))?;
        Ok(Decoder { dec, buf: vec![0.0; MAX_PACKET_SAMPLES * CHANNELS] })
    }

    pub fn decode(&mut self, packet: &[u8]) -> Result<&[f32]> {
        let n = self.dec.decode(packet, MAX_PACKET_SAMPLES, &mut self.buf).map_err(|e| anyhow!("Opus 解码失败：{e}"))?;
        Ok(&self.buf[..n * CHANNELS])
    }
}

// ---------------------------------------------------------------- 被连方

/// 被连方这一路画面的声音：观看方开来的声音流按号找到它
pub struct HostAudio {
    remote: iroh::EndpointId,
    backend: Arc<dyn ScreenBackend>,
    token: u64,
    /// 画面暂停（走了中继）时声音也不发
    paused: AtomicBool,
    /// 同一路画面同时只抓一份声音：观看方重开声音流时，前一条的抓取停掉
    current: Mutex<u64>,
}

static HOST_AUDIO: Mutex<Option<HashMap<u64, Weak<HostAudio>>>> = Mutex::new(None);

impl HostAudio {
    pub fn register(remote: iroh::EndpointId, backend: Arc<dyn ScreenBackend>) -> Arc<Self> {
        let token = loop {
            let t: u64 = rand::random();
            if t != 0 {
                break t;
            }
        };
        let audio = Arc::new(Self { remote, backend, token, paused: AtomicBool::new(false), current: Mutex::new(0) });
        HOST_AUDIO.lock().unwrap().get_or_insert_with(HashMap::new).insert(token, Arc::downgrade(&audio));
        audio
    }

    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    fn find(token: u64, remote: iroh::EndpointId) -> Result<Arc<HostAudio>> {
        let audio = HOST_AUDIO.lock().unwrap().as_ref().and_then(|m| m.get(&token)).and_then(Weak::upgrade);
        match audio {
            Some(a) if a.remote == remote => Ok(a),
            _ => bail!("这一路画面已经结束了"),
        }
    }
}

impl Drop for HostAudio {
    fn drop(&mut self) {
        if let Some(m) = HOST_AUDIO.lock().unwrap().as_mut() {
            m.remove(&self.token);
        }
    }
}

/// 被连方：观看方开来的声音流。抓声音、编码、往流里写，直到观看方关流、画面结束或者又开了一条新的
pub async fn handle_stream(remote: iroh::EndpointId, token: u64, mut send: iroh::endpoint::SendStream) {
    let audio = match HostAudio::find(token, remote) {
        Ok(a) => a,
        Err(e) => {
            debug!("声音流找不到画面：{e:#}");
            let _ = send.finish();
            return;
        }
    };
    let _ = send.set_priority(AUDIO_PRIORITY);
    let me = {
        let mut cur = audio.current.lock().unwrap();
        *cur += 1;
        *cur
    };
    let weak = Arc::downgrade(&audio);
    let (sink, mut rx) = tokio::sync::mpsc::channel::<Vec<f32>>(SINK_DEPTH);
    let backend = audio.backend.clone();
    drop(audio);
    // 开抓可能要等系统回应，放到阻塞线程上
    let guard = match tokio::task::spawn_blocking(move || backend.start_audio(sink)).await {
        Ok(Ok(g)) => g,
        Ok(Err(e)) => {
            warn!("抓不了本机的声音：{e:#}");
            let _ = send.finish();
            return;
        }
        Err(e) => {
            warn!("抓声音的线程出错：{e}");
            return;
        }
    };
    info!("开始给观看方传声音");
    let mut encoder = match Encoder::new() {
        Ok(e) => e,
        Err(e) => {
            warn!("{e:#}");
            return;
        }
    };
    let started = Instant::now();
    let mut buf = Vec::with_capacity(MAX_PACKET + 12);
    let mut packets = 0u64;
    loop {
        let pcm = tokio::select! {
            pcm = rx.recv() => match pcm {
                Some(p) => p,
                None => break,
            },
            // 观看方关了流（静音、关窗口）
            _ = send.stopped() => break,
        };
        let Some(audio) = weak.upgrade() else { break };
        if *audio.current.lock().unwrap() != me {
            break;
        }
        if audio.paused.load(Ordering::Relaxed) {
            continue;
        }
        drop(audio);
        let ts = started.elapsed().as_micros() as u64;
        buf.clear();
        let encoded = encoder.push(&pcm, |packet| {
            buf.extend_from_slice(&((packet.len() + 8) as u32).to_be_bytes());
            buf.extend_from_slice(&ts.to_be_bytes());
            buf.extend_from_slice(packet);
            packets += 1;
        });
        if let Err(e) = encoded {
            warn!("{e:#}");
            break;
        }
        if !buf.is_empty() && send.write_all(&buf).await.is_err() {
            break;
        }
    }
    let _ = send.finish();
    // 停抓（系统接口可能要等一下）
    let _ = tokio::task::spawn_blocking(move || drop(guard)).await;
    info!(packets, "不再给观看方传声音");
}

// ---------------------------------------------------------------- 观看方

/// 观看方收声音：一包包读出来
pub struct AudioReceiver {
    recv: iroh::endpoint::RecvStream,
    /// 留着发送端：丢掉它对方会以为我们不要了
    _send: iroh::endpoint::SendStream,
}

impl AudioReceiver {
    pub(crate) fn new(send: iroh::endpoint::SendStream, recv: iroh::endpoint::RecvStream) -> Self {
        AudioReceiver { recv, _send: send }
    }

    /// 下一包：(时间戳微秒, Opus 数据)。流结束是 None
    pub async fn next(&mut self) -> Result<Option<(u64, Vec<u8>)>> {
        let mut len = [0u8; 4];
        match self.recv.read_exact(&mut len).await {
            Ok(_) => {}
            Err(_) => return Ok(None),
        }
        let len = u32::from_be_bytes(len) as usize;
        if !(8..=MAX_PACKET + 8).contains(&len) {
            bail!("声音包长度不对：{len}");
        }
        let mut data = vec![0u8; len];
        self.recv.read_exact(&mut data).await.map_err(|e| anyhow!("读声音失败：{e}"))?;
        let ts = u64::from_be_bytes(data[..8].try_into().unwrap());
        data.drain(..8);
        Ok(Some((ts, data)))
    }

    /// 不收了：告诉对方停
    pub async fn close(mut self) {
        let _ = self.recv.stop(0u32.into());
        let _ = self._send.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 正弦波编了再解，能量差不多，频率对（数过零点）
    #[test]
    fn opus_round_trip() {
        let mut enc = Encoder::new().unwrap();
        let mut dec = Decoder::new().unwrap();
        // 1 秒 440 Hz，左右一样；分成长短不一的段喂进去
        let tone: Vec<f32> = (0..SAMPLE_RATE as usize).flat_map(|i| {
            let v = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SAMPLE_RATE as f32).sin();
            [v, v]
        }).collect();
        let mut packets = Vec::new();
        for chunk in tone.chunks(733 * CHANNELS) {
            enc.push(chunk, |p| packets.push(p.to_vec())).unwrap();
        }
        assert_eq!(packets.len(), 50, "1 秒该是 50 包");
        let mut out = Vec::new();
        for p in &packets {
            out.extend_from_slice(dec.decode(p).unwrap());
        }
        assert_eq!(out.len(), tone.len());
        // 跳过开头编码器的延迟，看后半秒
        let tail = &out[out.len() / 2..];
        let rms = (tail.iter().map(|v| v * v).sum::<f32>() / tail.len() as f32).sqrt();
        assert!((rms - 0.5 / 2f32.sqrt()).abs() < 0.05, "音量不对：{rms}");
        let left: Vec<f32> = tail.iter().step_by(2).copied().collect();
        let crossings = left.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((215..=225).contains(&crossings), "半秒该有 220 个周期：{crossings}");
    }
}

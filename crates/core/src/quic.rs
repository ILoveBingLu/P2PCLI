//! QUIC 传输参数：除了拥塞控制，都沿用 iroh 的默认值。
//!
//! 拥塞控制默认用 Cubic。以前往返 20 ms 以上的链路自动换成 BBR3（模拟的“均匀随机丢 1.3%”链路上
//! BBR3 有约 40 Mbps，Cubic 只有约 1 Mbps），但真实的跨境链路（往返约 80 ms、能跑 80 Mbps）上
//! noq 的 BBR3 碰到几个零星丢包后，带宽估计从 130 Mbps 掉到 10 Mbps，之后不丢包、延迟不涨也一路往下掉，
//! 最后剩 0.5 Mbps 左右、恢复不了：传文件慢到几十 KB/s，远程画面卡成幻灯片。
//! 同一条链路上 Cubic 连续传 360 MB 一直 30~80 Mbps，突发丢 13% 也能马上恢复。
//! 环境变量 P2PCLI_CC=bbr3（一直用 BBR3）或 auto（以前按延迟选的做法）可以换回来对比。
//!
//! 流量窗口别放大：noq 的接收缓冲最多容纳 1024 个不连续的数据块，默认的 1.25 MB 窗口刚好在这条线以内；
//! 放大以后，丢包交错时会超过这个数，对方直接把连接掐断（实测复现过）。

use std::any::Any;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iroh::endpoint::{Controller, ControllerFactory, ControllerMetrics, QuicTransportConfig};
use noq_proto::congestion::{Bbr3Config, CubicConfig};
use noq_proto::RttEstimator;

/// auto 模式下最小往返延迟到这个值就当作广域网链路，改用 BBR3
const WAN_RTT: Duration = Duration::from_millis(20);
/// 攒够这么多次确认再判断：刚开始往返延迟的估计还没有真实采样
const DECIDE_AFTER_ACKS: u32 = 16;

pub fn transport_config() -> QuicTransportConfig {
    transport_config_with(Mode::from_env())
}

/// 指定拥塞控制的做法，测试对比用
pub fn transport_config_with(mode: Mode) -> QuicTransportConfig {
    let config = AdaptiveConfig { mode, ..Default::default() };
    QuicTransportConfig::builder()
        .congestion_controller_factory(Arc::new(config))
        .build()
}

/// 拥塞控制用哪种
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Cubic,
    Bbr3,
    /// 按链路延迟选：往返 20 ms 以上用 BBR3，否则 Cubic
    Auto,
}

impl Mode {
    /// 环境变量 P2PCLI_CC=cubic、bbr3 或 auto；没设或认不出就是 Cubic
    pub fn from_env() -> Mode {
        match std::env::var("P2PCLI_CC").as_deref() {
            Ok("bbr3") => Mode::Bbr3,
            Ok("auto") => Mode::Auto,
            _ => Mode::Cubic,
        }
    }
}

/// 这条路量到的最小往返；还没有确认时是空
pub fn min_rtt(controller: Box<dyn Controller>) -> Option<Duration> {
    let adaptive = controller.into_any().downcast::<Adaptive>().ok()?;
    adaptive.min_rtt
}

/// 这条连接现在用的拥塞控制算法，调试和测试用
pub fn algorithm(controller: Box<dyn Controller>) -> Option<&'static str> {
    let adaptive = controller.into_any().downcast::<Adaptive>().ok()?;
    Some(if adaptive.wan { "bbr3" } else { "cubic" })
}

#[derive(Debug, Default)]
struct AdaptiveConfig {
    cubic: Arc<CubicConfig>,
    bbr: Arc<Bbr3Config>,
    mode: Mode,
}

impl ControllerFactory for AdaptiveConfig {
    fn build(self: Arc<Self>, now: Instant, current_mtu: u16) -> Box<dyn Controller> {
        Box::new(Adaptive {
            cubic: self.cubic.clone().build(now, current_mtu),
            bbr: self.bbr.clone().build(now, current_mtu),
            // 只有 auto 要等确认攒够了再判断，固定的两种一开始就定了
            acks: if self.mode == Mode::Auto { 0 } else { DECIDE_AFTER_ACKS },
            wan: self.mode == Mode::Bbr3,
            both: self.mode == Mode::Auto,
            min_rtt: None,
        })
    }
}

/// 两个算法同时听所有事件，各自一直保持最新；窗口和发送节奏只取当前选中的那个。
/// 这样中途切换时，被选上的算法不是从零开始，也不会遇到没见过的包。
#[derive(Debug)]
struct Adaptive {
    cubic: Box<dyn Controller>,
    bbr: Box<dyn Controller>,
    acks: u32,
    /// 判断为广域网：用 BBR3，一旦选上就不再换回去
    wan: bool,
    /// auto 模式两个都喂；固定用一种时只喂那一种，省得每个包白算一遍
    both: bool,
    /// 最小往返，给远程画面的码率控制当底线
    min_rtt: Option<Duration>,
}

impl Adaptive {
    fn active(&self) -> &dyn Controller {
        if self.wan { self.bbr.as_ref() } else { self.cubic.as_ref() }
    }

    /// 把一个事件交给要听的算法
    fn each(&mut self, mut f: impl FnMut(&mut dyn Controller)) {
        if self.both || !self.wan {
            f(self.cubic.as_mut());
        }
        if self.both || self.wan {
            f(self.bbr.as_mut());
        }
    }
}

impl Controller for Adaptive {
    fn on_sent(&mut self, now: Instant, bytes: u64, largest_pn: u64) {
        self.each(|c| c.on_sent(now, bytes, largest_pn));
    }

    fn on_packet_sent(&mut self, now: Instant, bytes: u16, pn: u64) {
        self.each(|c| c.on_packet_sent(now, bytes, pn));
    }

    fn on_cwnd_limited(&mut self) {
        self.each(|c| c.on_cwnd_limited());
    }

    fn on_ack(&mut self, now: Instant, sent: Instant, bytes: u64, pn: u64, app_limited: bool, rtt: &RttEstimator) {
        self.each(|c| c.on_ack(now, sent, bytes, pn, app_limited, rtt));
        self.min_rtt = Some(rtt.min());
        if self.acks < DECIDE_AFTER_ACKS {
            self.acks += 1;
            if self.acks == DECIDE_AFTER_ACKS {
                self.wan = rtt.min() >= WAN_RTT;
            }
        }
    }

    fn on_end_acks(&mut self, now: Instant, in_flight: u64, app_limited: bool, largest_packet_num_acked: Option<u64>) {
        self.each(|c| c.on_end_acks(now, in_flight, app_limited, largest_packet_num_acked));
    }

    fn on_congestion_event(&mut self, now: Instant, sent: Instant, is_persistent_congestion: bool, is_ecn: bool, lost_bytes: u64, largest_lost_pn: u64) {
        self.each(|c| c.on_congestion_event(now, sent, is_persistent_congestion, is_ecn, lost_bytes, largest_lost_pn));
    }

    fn on_packet_lost(&mut self, lost_bytes: u16, pn: u64, now: Instant) {
        self.each(|c| c.on_packet_lost(lost_bytes, pn, now));
    }

    fn on_spurious_congestion_event(&mut self) {
        self.each(|c| c.on_spurious_congestion_event());
    }

    fn on_mtu_update(&mut self, new_mtu: u16) {
        self.each(|c| c.on_mtu_update(new_mtu));
    }

    fn on_ack_frequency_update(&mut self, ack_eliciting_threshold: u64, requested_max_ack_delay: Duration) {
        self.each(|c| c.on_ack_frequency_update(ack_eliciting_threshold, requested_max_ack_delay));
    }

    fn window(&self) -> u64 {
        self.active().window()
    }

    fn metrics(&self) -> ControllerMetrics {
        self.active().metrics()
    }

    fn clone_box(&self) -> Box<dyn Controller> {
        Box::new(Adaptive { cubic: self.cubic.clone_box(), bbr: self.bbr.clone_box(), acks: self.acks, wan: self.wan, both: self.both, min_rtt: self.min_rtt })
    }

    fn initial_window(&self) -> u64 {
        self.active().initial_window()
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

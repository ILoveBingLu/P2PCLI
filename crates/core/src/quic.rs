//! QUIC 传输参数：除了拥塞控制，都沿用 iroh 的默认值。
//!
//! 为什么要换拥塞控制：默认的 Cubic 靠丢包判断拥堵，丢包一多就把发送速度压得很低。
//! 实测（tests/lossy_link.rs，往返 106 ms、100 Mbps 带宽）：丢包 1.3% 时 Cubic 只有约 1 Mbps，BBR3 有约 40 Mbps。
//! 跨境、跨运营商这类链路常有 1% 上下的丢包，传文件就被这个卡死。
//! 但 BBR3 在低延迟的局域网里反而比 Cubic 慢（实测约六成），所以按链路延迟自动选：
//! 最小往返延迟到 [`WAN_RTT`] 以上就用 BBR3，局域网和近距离保持 Cubic。
//!
//! 流量窗口别放大：noq 的接收缓冲最多容纳 1024 个不连续的数据块，默认的 1.25 MB 窗口刚好在这条线以内；
//! 放大以后，丢包交错时会超过这个数，对方直接把连接掐断（实测复现过）。

use std::any::Any;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iroh::endpoint::{Controller, ControllerFactory, ControllerMetrics, QuicTransportConfig};
use noq_proto::congestion::{Bbr3Config, CubicConfig};
use noq_proto::RttEstimator;

/// 最小往返延迟到这个值就当作广域网链路，改用 BBR3
const WAN_RTT: Duration = Duration::from_millis(20);
/// 攒够这么多次确认再判断：刚开始往返延迟的估计还没有真实采样
const DECIDE_AFTER_ACKS: u32 = 16;

pub fn transport_config() -> QuicTransportConfig {
    QuicTransportConfig::builder()
        .congestion_controller_factory(Arc::new(AdaptiveConfig::default()))
        .build()
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
}

impl ControllerFactory for AdaptiveConfig {
    fn build(self: Arc<Self>, now: Instant, current_mtu: u16) -> Box<dyn Controller> {
        Box::new(Adaptive {
            cubic: self.cubic.clone().build(now, current_mtu),
            bbr: self.bbr.clone().build(now, current_mtu),
            acks: 0,
            wan: false,
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
}

impl Adaptive {
    fn active(&self) -> &dyn Controller {
        if self.wan { self.bbr.as_ref() } else { self.cubic.as_ref() }
    }
}

impl Controller for Adaptive {
    fn on_sent(&mut self, now: Instant, bytes: u64, largest_pn: u64) {
        self.cubic.on_sent(now, bytes, largest_pn);
        self.bbr.on_sent(now, bytes, largest_pn);
    }

    fn on_packet_sent(&mut self, now: Instant, bytes: u16, pn: u64) {
        self.cubic.on_packet_sent(now, bytes, pn);
        self.bbr.on_packet_sent(now, bytes, pn);
    }

    fn on_cwnd_limited(&mut self) {
        self.cubic.on_cwnd_limited();
        self.bbr.on_cwnd_limited();
    }

    fn on_ack(&mut self, now: Instant, sent: Instant, bytes: u64, pn: u64, app_limited: bool, rtt: &RttEstimator) {
        self.cubic.on_ack(now, sent, bytes, pn, app_limited, rtt);
        self.bbr.on_ack(now, sent, bytes, pn, app_limited, rtt);
        if self.acks < DECIDE_AFTER_ACKS {
            self.acks += 1;
            if self.acks == DECIDE_AFTER_ACKS {
                self.wan = rtt.min() >= WAN_RTT;
            }
        }
    }

    fn on_end_acks(&mut self, now: Instant, in_flight: u64, app_limited: bool, largest_packet_num_acked: Option<u64>) {
        self.cubic.on_end_acks(now, in_flight, app_limited, largest_packet_num_acked);
        self.bbr.on_end_acks(now, in_flight, app_limited, largest_packet_num_acked);
    }

    fn on_congestion_event(&mut self, now: Instant, sent: Instant, is_persistent_congestion: bool, is_ecn: bool, lost_bytes: u64, largest_lost_pn: u64) {
        self.cubic.on_congestion_event(now, sent, is_persistent_congestion, is_ecn, lost_bytes, largest_lost_pn);
        self.bbr.on_congestion_event(now, sent, is_persistent_congestion, is_ecn, lost_bytes, largest_lost_pn);
    }

    fn on_packet_lost(&mut self, lost_bytes: u16, pn: u64, now: Instant) {
        self.cubic.on_packet_lost(lost_bytes, pn, now);
        self.bbr.on_packet_lost(lost_bytes, pn, now);
    }

    fn on_spurious_congestion_event(&mut self) {
        self.cubic.on_spurious_congestion_event();
        self.bbr.on_spurious_congestion_event();
    }

    fn on_mtu_update(&mut self, new_mtu: u16) {
        self.cubic.on_mtu_update(new_mtu);
        self.bbr.on_mtu_update(new_mtu);
    }

    fn on_ack_frequency_update(&mut self, ack_eliciting_threshold: u64, requested_max_ack_delay: Duration) {
        self.cubic.on_ack_frequency_update(ack_eliciting_threshold, requested_max_ack_delay);
        self.bbr.on_ack_frequency_update(ack_eliciting_threshold, requested_max_ack_delay);
    }

    fn window(&self) -> u64 {
        self.active().window()
    }

    fn metrics(&self) -> ControllerMetrics {
        self.active().metrics()
    }

    fn clone_box(&self) -> Box<dyn Controller> {
        Box::new(Adaptive { cubic: self.cubic.clone_box(), bbr: self.bbr.clone_box(), acks: self.acks, wan: self.wan })
    }

    fn initial_window(&self) -> u64 {
        self.active().initial_window()
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

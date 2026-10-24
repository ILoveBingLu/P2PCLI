//! P2PCLI 核心：设备身份、点对点直连、配对码校验、远程终端。
//!
//! 连接层是 iroh：QUIC 加密直连，IPv6 优先、IPv4 打洞，打不通走中继。
//! 桌面端（Tauri）和开发用命令行都只调这里的接口。

pub mod client;
pub mod diag;
pub mod host;
pub mod identity;
pub mod node;
pub mod path;
pub mod proto;
pub mod proxy;
pub mod pty;
pub mod rdv_ws;
pub mod rendezvous;
pub mod store;
pub mod sessions;
pub mod v6;

/// 终端连接用的协议号
pub const ALPN_TERM: &[u8] = b"p2pcli/term/1";
/// 牵线服务器用的协议号
pub const ALPN_RDV: &[u8] = b"p2pcli/rdv/1";
/// IPv6 直连时被连方往外发包用的协议号，对面不接，只为打开本机防火墙
pub const ALPN_PUNCH: &[u8] = b"p2pcli/punch/1";
/// 终端协议版本，两边不一致直接拒
pub const PROTO_VERSION: u32 = 2;

pub use iroh::{EndpointAddr, EndpointId};

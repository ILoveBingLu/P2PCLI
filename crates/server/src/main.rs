//! 牵线服务器命令行入口，逻辑在 lib.rs。

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use p2pcli_core::identity::load_or_create_key;
use tracing::info;

#[derive(Parser)]
#[command(about = "P2PCLI 牵线服务器")]
struct Args {
    /// 数据目录，放服务器私钥和设备库
    #[arg(long, default_value = "./p2pcli-server-data")]
    data_dir: PathBuf,
    /// 监听的 UDP 端口
    #[arg(long, default_value_t = 7788)]
    port: u16,
    /// 同时开 WebSocket 入口的 TCP 端口（明文 ws，对外的 TLS 交给反向代理）
    #[arg(long)]
    ws_port: Option<u16>,
    /// 对外地址（公网 IP），只用来打印给客户端填的那串配置
    #[arg(long)]
    public_ip: Option<String>,
    /// 服务器自己挂在哪个中继上（和客户端用同一个自建中继）
    #[arg(long)]
    relay: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "p2pcli_server=info".into()),
        )
        .init();
    let args = Args::parse();
    std::fs::create_dir_all(&args.data_dir)?;
    let key = load_or_create_key(&args.data_dir)?;
    let state = p2pcli_server::State::open(&args.data_dir.join("devices.db"))?;
    let relays = args
        .relay
        .iter()
        .map(|u| u.parse::<iroh::RelayUrl>().with_context(|| format!("中继地址格式不对：{u}")))
        .collect::<Result<Vec<_>>>()?;
    let endpoint = p2pcli_server::bind(key, args.port, relays).await?;
    let id = endpoint.id();
    let ip = args.public_ip.as_deref().unwrap_or("<公网IP>");
    info!("牵线服务器已启动，客户端配置填：{id}@{ip}:{}", args.port);
    println!("{id}@{ip}:{}", args.port);
    if let Some(port) = args.ws_port {
        let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
            .await
            .with_context(|| format!("WebSocket 端口 {port} 绑不上"))?;
        info!("WebSocket 入口：ws://<地址>:{port}/v1/ws（对外用 wss:// 时在前面加反向代理）");
        tokio::spawn(p2pcli_server::serve_ws(listener, state.clone()));
    }
    p2pcli_server::serve(endpoint, state).await;
    Ok(())
}

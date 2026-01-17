use anyhow::Result;
use clap::Parser;
use tracing::{info, Level};
use tracing_subscriber;

mod config;
mod network;
mod protocol;
mod server;
mod transport;
mod utils;
mod handler;
mod xdp;
mod server_uring;

use crate::config::Config;
use crate::server::Server;

#[cfg(not(target_os = "windows"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[derive(Parser, Debug)]
#[command(author, version = env!("CARGO_PKG_VERSION"), about, long_about = None)]
struct Args {
    /// 配置文件路径
    #[arg(short, long, default_value = "config.json")]
    config: String,

    /// 日志级别
    #[arg(short, long, default_value = "info")]
    log_level: String,

    /// 启用 XDP 内核级 TLS 预过滤 (Need Root + Kernel 5.4+)
    #[arg(long, default_value_t = false)]
    enable_xdp: bool,

    /// XDP 绑定的网卡接口 (e.g., eth0)
    #[arg(long, default_value = "eth0")]
    xdp_iface: String,

    /// 启用 io_uring 高性能运行时 (Linux 5.10+)
    #[arg(long, default_value_t = false)]
    uring: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    
    if args.uring {
        info!("⚡ Using io_uring runtime (monoio)");
        use crate::utils::task::{set_runtime_mode, RuntimeMode};
        // Local-thread initialization for monoio
        // Local-thread initialization for monoio with timer enabled
        let mut rt = monoio::RuntimeBuilder::<monoio::IoUringDriver>::new()
            .enable_timer() // Important: Enable timer!
            .with_entries(1024)
            .build()
            .unwrap();

        rt.block_on(async move {
            set_runtime_mode(RuntimeMode::Monoio);
            async_main(args).await
        })
    } else {
        info!("🧵 Using standard tokio runtime");
        use crate::utils::task::{set_runtime_mode, RuntimeMode};
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async move {
            set_runtime_mode(RuntimeMode::Tokio);
            async_main(args).await
        })
    }
}

async fn async_main(args: Args) -> Result<()> {

    // 初始化日志
    // 优先使用环境变量 RUST_LOG，否则使用命令行参数
    let log_level_str = std::env::var("RUST_LOG")
        .unwrap_or_else(|_| args.log_level.clone());
    
    let log_level = match log_level_str.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "info" => Level::INFO,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        _ => Level::INFO,
    };

    tracing_subscriber::fmt()
        .with_max_level(log_level)
        .with_target(false)
        .with_thread_ids(true)
        .init();

    info!("🚀 Starting VLESS+Reality+XHTTP Server [V42-STABLE]");
    info!("📄 Loading config from: {}", args.config);

    // 1. Load config FIRST to get ports
    let config = Config::load(&args.config)?;
    info!("✅ Configuration loaded successfully");

    // 2. Extract ports for XDP protection
    let mut protected_ports = Vec::new();
    for inbound in &config.inbounds {
        protected_ports.push(inbound.port);
    }

    // 3. Attempt to start XDP
    let xdp_enabled = args.enable_xdp || std::env::var("XRAY_XDP_ENABLE").is_ok();
    
    if xdp_enabled {
        #[cfg(feature = "xdp")]
        {
            // 如果环境变量指定了接口，优先使用
            let iface = std::env::var("XRAY_XDP_IFACE").unwrap_or(args.xdp_iface);
            info!("🔥 Attempting to load XDP Firewall on interface: {}", iface);
            // Pass the extracted ports to the loader
            xdp::loader::start_xdp(&iface, protected_ports);
        }
        #[cfg(not(feature = "xdp"))]
        {
            tracing::warn!("⚠️  XDP was requested, but this binary was NOT compiled with XDP support.");
        }
    }

    // 运行服务器
    if args.uring {
        use crate::server_uring::UringServer;
        let server = UringServer::new(config)?;
        server.run().await?;
    } else {
        let server = Server::new(config)?;
        server.run().await?;
    }

    Ok(())
}

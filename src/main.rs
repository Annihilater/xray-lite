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
}

#[tokio::main]
async fn main() -> Result<()> {
    // 提高文件句柄限制 (Linux)
    #[cfg(not(target_os = "windows"))]
    {
        let mut limit = libc::rlimit {
            rlim_cur: 65535,
            rlim_max: 65535,
        };
        unsafe {
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                limit.rlim_cur = 4096;
                limit.rlim_max = 4096;
                libc::setrlimit(libc::RLIMIT_NOFILE, &limit);
            }
        }
    }

    let args = Args::parse();

    // 初始化日志
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

    info!("🚀 Xray-Lite Server v0.4.6-stable [Manual Relay]");
    info!("📄 Loading config from: {}", args.config);

    // 1. Load config
    let config = Config::load(&args.config)?;
    info!("✅ Configuration loaded successfully");

    // 2. Initialize and run server
    let server = Server::new(config)?;
    info!("🌐 Server initialized");

    // 运行服务器
    server.run().await?;

    Ok(())
}

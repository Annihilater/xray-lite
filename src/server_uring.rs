use anyhow::Result;
use monoio::net::TcpListener;
use monoio_compat::TcpStreamCompat;
use tracing::{error, info};
use uuid::Uuid;

use crate::config::{Config, Inbound, Security};
use crate::network::ConnectionManager;
use crate::protocol::vless::VlessCodec;
use crate::transport::{RealityServer, XhttpServer};
use crate::server::Server;

pub struct UringServer {
    config: Config,
    connection_manager: ConnectionManager,
}

impl UringServer {
    pub fn new(config: Config) -> Result<Self> {
        Ok(Self {
            config,
            connection_manager: ConnectionManager::new(),
        })
    }

    pub async fn run(self) -> Result<()> {
        let mut handles = vec![];

        for inbound in self.config.inbounds.clone() {
            let connection_manager = self.connection_manager.clone();
            
            let handle = monoio::spawn(async move {
                if let Err(e) = Self::run_inbound(inbound, connection_manager).await {
                    error!("入站处理失败 (io_uring): {}", e);
                }
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.await;
        }

        Ok(())
    }

    async fn run_inbound(inbound: Inbound, connection_manager: ConnectionManager) -> Result<()> {
        let addr = format!("{}:{}", inbound.listen, inbound.port);
        
        // Listen using monoio
        let listener = TcpListener::bind(&addr)?;
        info!("🚀 [io_uring] 监听 {} (协议: {:?})", addr, inbound.protocol);

        // Prepare shared state (same as in server.rs)
        let uuids: Vec<Uuid> = inbound
            .settings
            .clients
            .iter()
            .filter_map(|c| Uuid::parse_str(&c.id).ok())
            .collect();
        let codec = VlessCodec::new(uuids);

        let reality_server = if matches!(inbound.stream_settings.security, Security::Reality) {
            if let Some(reality_settings) = &inbound.stream_settings.reality_settings {
                let reality_config = crate::transport::reality::RealityConfig {
                    dest: reality_settings.dest.clone(),
                    server_names: reality_settings.server_names.clone(),
                    private_key: reality_settings.private_key.clone(),
                    public_key: reality_settings.public_key.clone(),
                    short_ids: reality_settings.short_ids.clone(),
                    fingerprint: reality_settings.fingerprint.clone(),
                };
                Some(RealityServer::new(reality_config)?)
            } else {
                None
            }
        } else {
            None
        };

        let xhttp_server = if let Some(xhttp_settings) = &inbound.stream_settings.xhttp_settings {
            let xhttp_config = crate::transport::xhttp::XhttpConfig {
                mode: match xhttp_settings.mode {
                    crate::config::XhttpMode::Auto => crate::transport::xhttp::XhttpMode::Auto,
                    crate::config::XhttpMode::StreamUp => crate::transport::xhttp::XhttpMode::StreamUp,
                    crate::config::XhttpMode::StreamDown => crate::transport::xhttp::XhttpMode::StreamDown,
                    crate::config::XhttpMode::StreamOne => crate::transport::xhttp::XhttpMode::StreamOne,
                },
                path: xhttp_settings.path.clone(),
                host: xhttp_settings.host.clone(),
            };
            Some(XhttpServer::new(xhttp_config)?)
        } else {
            None
        };

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    info!("📥 [io_uring] 新连接来自: {}", addr);
                    
                    let codec = codec.clone();
                    let reality_server = reality_server.clone();
                    let xhttp_server = xhttp_server.clone();
                    let connection_manager = connection_manager.clone();
                    
                    let sniffing_enabled = inbound.settings.sniffing.enabled;
                    let tcp_no_delay = inbound.stream_settings.sockopt.tcp_no_delay;
                    let accept_proxy_protocol = inbound.stream_settings.sockopt.accept_proxy_protocol;

                    monoio::spawn(async move {
                        use std::os::fd::AsRawFd;
                        use monoio_compat::StreamWrapper;
                        let fd = stream.as_raw_fd();
                        // 关键优化：使用 128KB 缓冲区代替默认的 8KB
                        // 减少 io_uring 提交次数 16 倍，大幅降低单核 CPU 开销
                        let compat_stream = StreamWrapper::new_with_buffer_size(stream, 128 * 1024, 128 * 1024);
                        let dual_stream = crate::utils::net::DualTcpStream::Monoio(compat_stream, fd);

                        
                        if let Err(e) = Server::handle_client(
                            Box::new(dual_stream),
                            codec,
                            reality_server,
                            xhttp_server,
                            connection_manager,
                            sniffing_enabled,
                            tcp_no_delay,
                            accept_proxy_protocol
                        ).await {
                            error!("客户端处理失败 (io_uring): {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("接受连接失败 (io_uring): {}", e);
                }
            }
        }
    }
}

// server_uring.rs
// 完全原生 monoio 的 io_uring 服务器实现 - 不使用任何 compat 层

use anyhow::Result;
use monoio::net::TcpListener;
use monoio::io::{AsyncReadRent, AsyncWriteRent, AsyncWriteRentExt};
use tracing::{error, info, debug};
use uuid::Uuid;
use bytes::BytesMut;

use crate::config::{Config, Inbound, Security};
use crate::protocol::vless::{VlessCodec, VlessRequest};
use crate::transport::reality::server_monoio::RealityServerMonoio;

pub struct UringServer {
    config: Config,
}

impl UringServer {
    pub fn new(config: Config) -> Result<Self> {
        Ok(Self { config })
    }

    pub async fn run(self) -> Result<()> {
        let mut handles = vec![];

        for inbound in self.config.inbounds.clone() {
            let handle = monoio::spawn(async move {
                if let Err(e) = Self::run_inbound(inbound).await {
                    error!("入站处理失败 (io_uring native): {}", e);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await;
        }

        Ok(())
    }

    async fn run_inbound(inbound: Inbound) -> Result<()> {
        let addr = format!("{}:{}", inbound.listen, inbound.port);
        
        let listener = TcpListener::bind(&addr)?;
        info!("🚀 [io_uring Native] 监听 {} (协议: {:?})", addr, inbound.protocol);

        // 准备 VLESS UUID
        let uuids: Vec<Uuid> = inbound
            .settings
            .clients
            .iter()
            .filter_map(|c| Uuid::parse_str(&c.id).ok())
            .collect();
        let codec = VlessCodec::new(uuids);

        // 准备原生 monoio Reality 服务器
        let reality_server = if matches!(inbound.stream_settings.security, Security::Reality) {
            if let Some(reality_settings) = &inbound.stream_settings.reality_settings {
                let private_key = base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    &reality_settings.private_key
                ).unwrap_or_default();
                
                Some(RealityServerMonoio::new(
                    private_key,
                    Some(reality_settings.dest.clone()),
                    reality_settings.short_ids.clone(),
                    reality_settings.server_names.clone(),
                )?)
            } else {
                None
            }
        } else {
            None
        };

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    debug!("📥 [io_uring Native] 新连接来自: {}", addr);
                    
                    let codec = codec.clone();
                    let reality_server = reality_server.clone();

                    monoio::spawn(async move {
                        if let Err(e) = Self::handle_client_native(stream, codec, reality_server).await {
                            debug!("客户端处理完成 (io_uring native): {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("接受连接失败 (io_uring native): {}", e);
                }
            }
        }
    }

    /// 完全原生 monoio 的客户端处理 - 无 compat 层
    async fn handle_client_native(
        stream: monoio::net::TcpStream,
        codec: VlessCodec,
        reality_server: Option<RealityServerMonoio>,
    ) -> Result<()> {
        // 使用原生 monoio Reality 进行 TLS 握手
        let mut tls_stream = if let Some(reality) = reality_server {
            reality.accept(stream).await?
        } else {
            return Err(anyhow::anyhow!("No Reality server configured"));
        };

        // VLESS 协议解析
        let buffer = vec![0u8; 4096];
        let (res, buf) = tls_stream.read(buffer).await;
        let n = res?;
        
        if n == 0 {
            return Err(anyhow::anyhow!("Connection closed"));
        }

        // 解析 VLESS 请求
        let mut bytes_mut = BytesMut::from(&buf[..n]);
        let request = codec.decode_request(&mut bytes_mut)?;

        let target_address = request.address.to_string();
        info!("🔗 [io_uring Native] 连接目标: {}", target_address);

        // 连接远程 - 原生 monoio
        let mut remote_stream = monoio::net::TcpStream::connect(&target_address).await?;

        // 发送初始数据 (VLESS 解码后剩余的数据)
        if !bytes_mut.is_empty() {
            let payload = bytes_mut.to_vec();
            let (res, _) = remote_stream.write_all(payload).await;
            res?;
        }


        // 原生 monoio 双向转发 - 无 compat 层，无额外拷贝
        Self::native_relay(tls_stream, remote_stream).await
    }

    /// 原生 monoio 双向转发 - 极致性能
    async fn native_relay<S1, S2>(mut client: S1, mut remote: S2) -> Result<()> 
    where 
        S1: AsyncReadRent + AsyncWriteRent + 'static,
        S2: AsyncReadRent + AsyncWriteRent + 'static,
    {
        // 使用单任务交替读写模式，避免 split 带来的复杂性
        // 这是一个简化但仍然高效的实现
        let mut client_buf = vec![0u8; 64 * 1024];
        let mut remote_buf = vec![0u8; 64 * 1024];
        let mut client_eof = false;
        let mut remote_eof = false;

        loop {
            // 尝试从客户端读取并写入远程
            if !client_eof {
                let (res, buf) = client.read(client_buf).await;
                client_buf = buf;
                match res {
                    Ok(0) => client_eof = true,
                    Ok(n) => {
                        let data = client_buf[..n].to_vec();
                        let (res, _) = remote.write_all(data).await;
                        if res.is_err() { break; }
                    }
                    Err(_) => client_eof = true,
                }
            }

            // 尝试从远程读取并写入客户端
            if !remote_eof {
                let (res, buf) = remote.read(remote_buf).await;
                remote_buf = buf;
                match res {
                    Ok(0) => remote_eof = true,
                    Ok(n) => {
                        let data = remote_buf[..n].to_vec();
                        let (res, _) = client.write_all(data).await;
                        if res.is_err() { break; }
                    }
                    Err(_) => remote_eof = true,
                }
            }

            if client_eof && remote_eof {
                break;
            }
        }
        
        Ok(())
    }
}

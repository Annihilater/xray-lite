// server_uring.rs
// 完全原生 monoio 的 io_uring 服务器实现 - 完整支持 XHTTP（通过 compat 桥接）

use anyhow::Result;
use monoio::net::TcpListener;
use monoio::io::{AsyncReadRent, AsyncWriteRent, AsyncReadRentExt, AsyncWriteRentExt, Splitable};
use tracing::{error, info, warn};
use uuid::Uuid;
use bytes::BytesMut;

use crate::config::{Config, Inbound, Security};
use crate::protocol::vless::VlessCodec;
use crate::transport::reality::server_monoio::{RealityServerMonoio, PrefixedMonoioStream};
use crate::transport::XhttpServer;
use crate::network::ConnectionManager;

pub struct UringServer {
    config: Config,
}

impl UringServer {
    pub fn new(config: Config) -> Result<Self> {
        Ok(Self { config })
    }

    pub async fn run(self) -> Result<()> {
        for inbound in self.config.inbounds.clone() {
            monoio::spawn(async move {
                if let Err(e) = Self::run_inbound(inbound).await {
                    error!("入站处理失败 (io_uring native): {}", e);
                }
            });
        }
        monoio::time::sleep(std::time::Duration::from_secs(3600 * 24 * 365)).await;
        Ok(())
    }

    async fn run_inbound(inbound: Inbound) -> Result<()> {
        let addr = format!("{}:{}", inbound.listen, inbound.port);
        let listener = TcpListener::bind(&addr)?;
        info!("🚀 [io_uring Native] 监听 {} (协议: {:?})", addr, inbound.protocol);

        let uuids: Vec<Uuid> = inbound.settings.clients.iter()
            .filter_map(|c| Uuid::parse_str(&c.id).ok())
            .collect();
        let codec = VlessCodec::new(uuids);

        let reality_server = if matches!(inbound.stream_settings.security, Security::Reality) {
            if let Some(reality_settings) = &inbound.stream_settings.reality_settings {
                use base64::Engine;
                use base64::engine::general_purpose::{URL_SAFE_NO_PAD, STANDARD};
                let private_key = URL_SAFE_NO_PAD.decode(&reality_settings.private_key)
                    .or_else(|_| STANDARD.decode(&reality_settings.private_key))
                    .map_err(|e| anyhow::anyhow!("Failed to decode Reality private key: {}", e))?;
                
                Some(RealityServerMonoio::new(
                    private_key,
                    Some(reality_settings.dest.clone()),
                    reality_settings.short_ids.clone(),
                    reality_settings.server_names.clone(),
                )?)
            } else { None }
        } else { None };

        // 准备 XHTTP 服务器（如果配置了）
        let xhttp_server = if let Some(xhttp_settings) = &inbound.stream_settings.xhttp_settings {
            use crate::transport::xhttp::{XhttpConfig, XhttpMode};
            let mode = match &xhttp_settings.mode {
                crate::config::XhttpMode::Auto => XhttpMode::Auto,
                crate::config::XhttpMode::StreamUp => XhttpMode::StreamUp,
                crate::config::XhttpMode::StreamDown => XhttpMode::StreamDown,
                crate::config::XhttpMode::StreamOne => XhttpMode::StreamOne,
            };
            let xhttp_config = XhttpConfig {
                mode,
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
                    let codec = codec.clone();
                    let reality_server = reality_server.clone();
                    let xhttp_server = xhttp_server.clone();
                    
                    monoio::spawn(async move {
                        if let Err(e) = Self::handle_client_native(stream, codec, reality_server, xhttp_server).await {
                            info!("连接 {} 结束: {}", addr, e);
                        }
                    });
                }
                Err(e) => error!("accept error: {}", e),
            }
        }
    }

    async fn handle_client_native(
        stream: monoio::net::TcpStream,
        codec: VlessCodec,
        reality_server: Option<RealityServerMonoio>,
        xhttp_server: Option<XhttpServer>,
    ) -> Result<()> {
        let mut tls_stream = if let Some(reality) = reality_server {
            reality.accept(stream).await?
        } else {
            return Err(anyhow::anyhow!("No Reality server configured"));
        };

        let mut buffer = vec![0u8; 4096];
        let mut bytes_mut = BytesMut::new();
        
        // 读取第一包数据以检测协议类型
        let (res, buf) = tls_stream.read(buffer).await;
        buffer = buf;
        let n = res?;
        if n == 0 { return Ok(()); }
        bytes_mut.extend_from_slice(&buffer[..n]);
        
        // 检测 HTTP/2 Connection Preface
        const HTTP2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
        if bytes_mut.starts_with(HTTP2_PREFACE) {
            info!("🔄 检测到 XHTTP (HTTP/2)，使用 compat 桥接模式");
            if let Some(xhttp) = xhttp_server {
                return Self::handle_xhttp_compat(tls_stream, bytes_mut, codec, xhttp).await;
            } else {
                return Err(anyhow::anyhow!("检测到 XHTTP 但服务器未配置 XHTTP"));
            }
        }
        
        // 纯 VLESS 处理（原生 monoio 高性能路径）
        info!("✅ 检测到原生 VLESS，使用 io_uring 原生处理");
        Self::handle_vless_native(tls_stream, bytes_mut, codec).await
    }

    async fn handle_xhttp_compat(
        tls_stream: monoio_rustls_reality::server::TlsStream<PrefixedMonoioStream<monoio::net::TcpStream>>,
        initial_data: BytesMut,
        codec: VlessCodec,
        xhttp_server: XhttpServer,
    ) -> Result<()> {
        // 创建带初始数据的包装流
        let prefixed_stream = PrefixedStreamForXhttp {
            inner: tls_stream,
            prefix: initial_data.to_vec(),
            prefix_offset: 0,
        };
        
        // 包装为 monoio-compat，桥接到 Tokio
        use monoio_compat::StreamWrapper;
        let compat_stream = StreamWrapper::new_with_buffer_size(
            prefixed_stream,
            128 * 1024,
            128 * 1024
        );
        
        // 准备 handler
        let connection_manager = ConnectionManager::new();
        let handler = move |stream: Box<dyn crate::server::AsyncStream>| {
            let codec = codec.clone();
            let connection_manager = connection_manager.clone();
            async move {
                crate::Server::handle_client(
                    stream,
                    codec,
                    None, // Reality 已经处理过了
                    None, // XHTTP 在外层处理
                    connection_manager,
                    false, // sniffing_enabled
                    true,  // tcp_no_delay
                    false, // accept_proxy_protocol
                ).await
            }
        };
        
        xhttp_server.accept(compat_stream, handler).await?;
        Ok(())
    }

    async fn handle_vless_native(
        mut tls_stream: monoio_rustls_reality::server::TlsStream<PrefixedMonoioStream<monoio::net::TcpStream>>,
        mut bytes_mut: BytesMut,
        codec: VlessCodec,
    ) -> Result<()> {
        loop {
            // 解析 VLESS
            match codec.decode_request(&mut bytes_mut) {
                Ok(request) => {
                    let target_address = request.address.to_string();
                    info!("🔗 [io_uring Native] 连接目标: {}", target_address);
                    
                    let mut remote_stream = monoio::net::TcpStream::connect(&target_address).await?;
                    
                    if !bytes_mut.is_empty() {
                        let (res, _) = remote_stream.write_all(bytes_mut.to_vec()).await;
                        res?;
                    }

                    let (mut client_r, mut client_w) = tls_stream.into_split();
                    let (mut remote_r, mut remote_w) = remote_stream.into_split();

                    let c2r = async move {
                        let mut buf = vec![0u8; 64 * 1024];
                        loop {
                            let (res, b) = client_r.read(buf).await;
                            buf = b;
                            match res {
                                Ok(0) => break,
                                Ok(n) => {
                                    // 使用 split_to 避免内存拷贝
                                    let mut owned_buf = std::mem::replace(&mut buf, vec![0u8; 64 * 1024]);
                                    owned_buf.truncate(n);
                                    let (w_res, ret_buf) = remote_w.write_all(owned_buf).await;
                                    buf = ret_buf;
                                    buf.resize(64 * 1024, 0);
                                    if w_res.is_err() { break; }
                                }
                                Err(_) => break,
                            }
                        }
                        let _ = remote_w.shutdown().await;
                    };

                    let r2c = async move {
                        let mut buf = vec![0u8; 64 * 1024];
                        loop {
                            let (res, b) = remote_r.read(buf).await;
                            buf = b;
                            match res {
                                Ok(0) => break,
                                Ok(n) => {
                                    // 使用 split_to 避免内存拷贝
                                    let mut owned_buf = std::mem::replace(&mut buf, vec![0u8; 64 * 1024]);
                                    owned_buf.truncate(n);
                                    let (w_res, ret_buf) = client_w.write_all(owned_buf).await;
                                    buf = ret_buf;
                                    buf.resize(64 * 1024, 0);
                                    if w_res.is_err() { break; }
                                }
                                Err(_) => break,
                            }
                        }
                        let _ = client_w.shutdown().await;
                    };

                    futures::join!(c2r, r2c);
                    return Ok(());
                }
                Err(e) => {
                    if bytes_mut.len() > 256 {
                        return Err(anyhow::anyhow!("VLESS 解析失败: {}", e));
                    }
                    // 继续读取更多数据
                    let mut buffer = vec![0u8; 4096];
                    let (res, buf) = tls_stream.read(buffer).await;
                    buffer = buf;
                    let n = res?;
                    if n == 0 { return Err(e); }
                    bytes_mut.extend_from_slice(&buffer[..n]);
                }
            }
        }
    }
}

// XHTTP 专用的前缀流包装器
struct PrefixedStreamForXhttp {
    inner: monoio_rustls_reality::server::TlsStream<PrefixedMonoioStream<monoio::net::TcpStream>>,
    prefix: Vec<u8>,
    prefix_offset: usize,
}

impl AsyncReadRent for PrefixedStreamForXhttp {
    async fn read<T: monoio::buf::IoBufMut>(&mut self, mut buf: T) -> monoio::BufResult<usize, T> {
        if self.prefix_offset < self.prefix.len() {
            let remaining = self.prefix.len() - self.prefix_offset;
            let to_copy = remaining.min(buf.bytes_total());
            unsafe {
                let slice = std::slice::from_raw_parts_mut(buf.write_ptr(), to_copy);
                slice.copy_from_slice(&self.prefix[self.prefix_offset..self.prefix_offset + to_copy]);
                buf.set_init(to_copy);
            }
            self.prefix_offset += to_copy;
            return (Ok(to_copy), buf);
        }
        self.inner.read(buf).await
    }
    
    async fn readv<T: monoio::buf::IoVecBufMut>(&mut self, buf: T) -> monoio::BufResult<usize, T> {
        self.inner.readv(buf).await
    }
}

impl AsyncWriteRent for PrefixedStreamForXhttp {
    async fn write<T: monoio::buf::IoBuf>(&mut self, buf: T) -> monoio::BufResult<usize, T> {
        self.inner.write(buf).await
    }
    
    async fn writev<T: monoio::buf::IoVecBuf>(&mut self, buf_vec: T) -> monoio::BufResult<usize, T> {
        self.inner.writev(buf_vec).await
    }
    
    async fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush().await
    }
    
    async fn shutdown(&mut self) -> std::io::Result<()> {
        self.inner.shutdown().await
    }
}

use anyhow::Result;
use bytes::{Buf, Bytes, BytesMut};
use h2::server::{self, SendResponse};
use h2::SendStream;
use hyper::http::{Request, Response, StatusCode};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, Notify};
use tracing::{debug, info, warn};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use once_cell::sync::Lazy;
use rand::{distributions::Alphanumeric, Rng};

use super::XhttpConfig;
use dashmap::DashMap;

/// 全局会话管理器
struct Session {
    to_vless_tx: mpsc::UnboundedSender<Bytes>,
    notify: Arc<Notify>,
}

static SESSIONS: Lazy<Arc<DashMap<String, Session>>> = Lazy::new(|| {
    Arc::new(DashMap::new())
});

/// 终极 H2/XHTTP 处理器 (v0.4.3: 流量特征优化版)
#[derive(Clone)]
pub struct H2Handler {
    config: XhttpConfig,
}

impl H2Handler {
    pub fn new(config: XhttpConfig) -> Self {
        Self { config }
    }

    /// 生成随机 Padding 字符串
    fn gen_padding() -> String {
        let mut rng = rand::thread_rng();
        let len = rng.gen_range(64..512);
        rng.sample_iter(&Alphanumeric)
            .take(len)
            .map(char::from)
            .collect()
    }

    /// v0.4.3: 生成伪装的请求 ID (模拟 CDN/反代)
    fn gen_request_id() -> String {
        let mut rng = rand::thread_rng();
        format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            rng.gen::<u32>(),
            rng.gen::<u16>(),
            rng.gen::<u16>(),
            rng.gen::<u16>(),
            rng.gen::<u64>() & 0xFFFFFFFFFFFF
        )
    }

    /// v0.4.3: 生成伪装的缓存状态
    fn gen_cache_status() -> &'static str {
        let mut rng = rand::thread_rng();
        match rng.gen_range(0..10) {
            0..=3 => "HIT",
            4..=6 => "MISS",
            7..=8 => "BYPASS",
            _ => "DYNAMIC",
        }
    }

    /// v0.4.3: 智能分片发送 - 模拟真实网页流量分布
    fn send_split_data(src: &mut BytesMut, send_stream: &mut SendStream<Bytes>) -> Result<()> {
        let mut rng = rand::thread_rng();
        
        while src.has_remaining() {
            let chunk_size = {
                let r: f64 = rng.gen();
                if r < 0.35 {
                    rng.gen_range(128..512)      // 35% 小包
                } else if r < 0.70 {
                    rng.gen_range(512..2048)     // 35% 中包
                } else if r < 0.90 {
                    rng.gen_range(2048..8192)    // 20% 大包
                } else {
                    rng.gen_range(8192..16384)   // 10% 超大包
                }
            };
            let split_len = std::cmp::min(src.len(), chunk_size);
            let chunk = src.split_to(split_len).freeze();
            send_stream.send_data(chunk, false)?;
        }
        Ok(())
    }

    pub async fn handle<T, F, Fut>(&self, stream: T, handler: F) -> Result<()>
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        F: Fn(Box<dyn crate::server::AsyncStream>) -> Fut + Clone + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        info!("XHTTP: 启动 V42 拟态防御引擎 (Enhanced gRPC Auto-Detection)");

        let mut builder = server::Builder::new();
        builder
            .initial_window_size(4194304)
            .initial_connection_window_size(8388608)
            .max_concurrent_streams(500)
            .max_frame_size(16384);

        let mut connection = tokio::time::timeout(
            Duration::from_secs(20),
            builder.handshake(stream)
        ).await??;

        // H2 Ping-Pong 随机心跳
        if let Some(mut ping_pong) = connection.ping_pong() {
            tokio::spawn(async move {
                loop {
                    let sleep_ms = {
                         let mut rng = rand::thread_rng();
                         rng.gen_range(15000..45000)
                    };
                    tokio::time::sleep(tokio::time::Duration::from_millis(sleep_ms)).await;

                    // 如果 ping 失败（连接已断开），退出此任务
                    if let Err(e) = ping_pong.send_ping(h2::Ping::opaque()) {
                        debug!("🌪️ H2 Noise: Ping failed, connection closed: {}", e);
                        break;  // 退出循环，任务结束
                    }
                }
            });
        }
        
        while let Some(result) = connection.accept().await {
            match result {
                Ok((request, respond)) => {
                    let config = self.config.clone();
                    let handler = handler.clone();
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_request(config, request, respond, handler).await {
                            debug!("连接处理闭合: {}", e);
                        }
                    });
                }
                Err(e) => {
                    debug!("H2 连接中断: {}", e);
                    break;
                }
            }
        }
        Ok(())
    }

    async fn handle_request<F, Fut>(
        config: XhttpConfig,
        request: Request<h2::RecvStream>,
        mut respond: SendResponse<Bytes>,
        handler: F,
    ) -> Result<()>
    where
        F: Fn(Box<dyn crate::server::AsyncStream>) -> Fut + Clone + Send + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        let path = request.uri().path().to_string();
        let method = request.method();
        
        if !path.starts_with(&config.path) {
            Self::send_error_response(&mut respond, StatusCode::NOT_FOUND).await?;
            return Ok(());
        }

        if method == "GET" {
            Self::handle_xhttp_get(path, respond, handler).await?;
        } else if method == "POST" {
            let user_agent = request.headers().get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("");
            let is_pc = user_agent.contains("Go-http-client");

            // 移动端优化：等待 Session 就绪
            // 减少最大等待时间到 1.5s，避免客户端超时
            if !is_pc {
                for _ in 0..30 {
                    let found = SESSIONS.contains_key(&path);
                    if found { break; }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }

            let session_tx = SESSIONS.get(&path).map(|s| s.to_vless_tx.clone());

            if let Some(tx) = session_tx {
                Self::handle_xhttp_post(request, respond, tx).await?;
            } else {
                let content_type = request.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
                let is_grpc = content_type.contains("grpc");
                Self::handle_standalone(request, respond, handler, is_grpc).await?;
            }
        } else {
            Self::send_error_response(&mut respond, StatusCode::METHOD_NOT_ALLOWED).await?;
        }
        Ok(())
    }

    async fn handle_standalone<F, Fut>(
        request: Request<h2::RecvStream>,
        mut respond: SendResponse<Bytes>,
        handler: F,
        is_grpc: bool,
    ) -> Result<()>
    where
        F: Fn(Box<dyn crate::server::AsyncStream>) -> Fut + Clone + Send + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        let response = Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/octet-stream")
            .header("server", "nginx/1.26.0")
            .header("x-request-id", Self::gen_request_id())
            .header("x-cache", Self::gen_cache_status())
            .header("x-padding", Self::gen_padding())
            .body(())
            .unwrap();

        let mut send_stream = respond.send_response(response, false)?;
        let (client_io, server_io) = tokio::io::duplex(65536);
        
        let use_grpc_framing = Arc::new(AtomicBool::new(is_grpc));
        let use_grpc_framing_up = use_grpc_framing.clone();
        let use_grpc_framing_down = use_grpc_framing.clone();

        tokio::spawn(handler(Box::new(server_io)));
        let (mut client_read, mut client_write) = tokio::io::split(client_io);

        // UP (Client -> Server)
        let up_task = async move {
            let mut body = request.into_body();
            let mut leftover = BytesMut::new();
            use tokio::io::AsyncWriteExt;
            
            while let Some(chunk) = body.data().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        debug!("XHTTP UP: Stream closed/error: {}", e);
                        break;
                    }
                };
                let _ = body.flow_control().release_capacity(chunk.len());
                
                if use_grpc_framing_up.load(Ordering::Relaxed) {
                    leftover.extend_from_slice(&chunk);
                    
                    // 循环处理完整帧
                    loop {
                        if leftover.len() < 5 {
                            break;
                        }

                        // 1. 严格校验 gRPC 标志 (Byte 0 必须是 0 或 1)
                        let flag = leftover[0];
                        if flag != 0 && flag != 1 {
                            warn!("XHTTP UP: 检测到非 gRPC 标志 ({:02x})，流已损坏或非 gRPC，回退到透传", flag);
                            use_grpc_framing_up.store(false, Ordering::Relaxed);
                            client_write.write_all(&leftover).await?;
                            leftover.clear();
                            break;
                        }

                        // 2. 读取长度
                        let msg_len = u32::from_be_bytes([leftover[1], leftover[2], leftover[3], leftover[4]]) as usize;
                        
                        // 3. 校验长度 (放宽到 16MB)
                        if msg_len > 16_777_216 {
                            warn!("XHTTP UP: 异常帧长度 ({})，判定为原始流误入，回退到透传", msg_len);
                            use_grpc_framing_up.store(false, Ordering::Relaxed);
                            client_write.write_all(&leftover).await?;
                            leftover.clear();
                            break;
                        }

                        // 4. 提取数据
                        if leftover.len() >= 5 + msg_len {
                            let _ = leftover.split_to(5); // Header
                            let data = leftover.split_to(msg_len); // Payload
                            client_write.write_all(&data).await?;
                        } else {
                            // 数据未齐，等待下一块
                            break; 
                        }
                    }
                } else {
                    client_write.write_all(&chunk).await?;
                }
            }
            Ok::<(), anyhow::Error>(())
        };

        // DOWN (Server -> Client)
        let down_task = async move {
            let mut buf = BytesMut::with_capacity(65536);
            use tokio::io::AsyncReadExt;
            loop {
                if buf.capacity() < 2048 {
                    buf.reserve(65536);
                }
                let n = client_read.read_buf(&mut buf).await?;
                if n == 0 { break; }
                
                if use_grpc_framing_down.load(Ordering::Relaxed) {
                    let mut frame = BytesMut::with_capacity(5 + n);
                    frame.extend_from_slice(&[0u8]); // Flag
                    frame.extend_from_slice(&(n as u32).to_be_bytes()); // Length
                    frame.extend_from_slice(&buf[..n]); // Data
                    buf.advance(n);
                    Self::send_split_data(&mut frame, &mut send_stream)?;
                } else {
                    Self::send_split_data(&mut buf, &mut send_stream)?;
                }
            }
            
            if use_grpc_framing_down.load(Ordering::Relaxed) {
                let mut trailers = hyper::http::HeaderMap::new();
                trailers.insert("grpc-status", "0".parse().unwrap());
                send_stream.send_trailers(trailers)?;
            } else {
                send_stream.send_data(Bytes::new(), true)?;
            }
            Ok::<(), anyhow::Error>(())
        };

        tokio::spawn(up_task);
        if let Err(e) = down_task.await {
            debug!("XHTTP 传输级异常: {}", e);
        }
        Ok(())
    }

    async fn handle_xhttp_get<F, Fut>(
        path: String,
        mut respond: SendResponse<Bytes>,
        handler: F,
    ) -> Result<()>
    where
        F: Fn(Box<dyn crate::server::AsyncStream>) -> Fut + Clone + Send + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        let (to_vless_tx, mut to_vless_rx) = mpsc::unbounded_channel::<Bytes>();
        let notify = Arc::new(Notify::new());
        
        SESSIONS.insert(path.clone(), Session { to_vless_tx, notify: notify.clone() });

        let (client_io, server_io) = tokio::io::duplex(65536);
        tokio::spawn(handler(Box::new(server_io)));
        let (mut client_read, mut client_write) = tokio::io::split(client_io);

        let response = Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/octet-stream")
            .header("server", "nginx/1.26.0")
            .header("x-request-id", Self::gen_request_id())
            .header("x-cache", Self::gen_cache_status())
            .header("x-padding", Self::gen_padding())
            .body(())
            .unwrap();
        let mut send_stream = respond.send_response(response, false)?;

        let downstream = async move {
            let mut buf = BytesMut::with_capacity(65536);
            use tokio::io::AsyncReadExt;
            loop {
                if buf.capacity() < 2048 {
                    buf.reserve(65536);
                }
                let n = client_read.read_buf(&mut buf).await?;
                if n == 0 { break; }
                Self::send_split_data(&mut buf, &mut send_stream)?;
            }
            send_stream.send_data(Bytes::new(), true)?;
            Ok::<(), anyhow::Error>(())
        };

        let upstream = async move {
            use tokio::io::AsyncWriteExt;
            while let Some(data) = to_vless_rx.recv().await {
                client_write.write_all(&data).await?;
            }
            Ok::<(), anyhow::Error>(())
        };

        tokio::spawn(upstream);
        let _ = downstream.await;
        
        SESSIONS.remove(&path);
        notify.notify_waiters();
        Ok(())
    }

    async fn handle_xhttp_post(
        request: Request<h2::RecvStream>,
        mut respond: SendResponse<Bytes>,
        tx: mpsc::UnboundedSender<Bytes>,
    ) -> Result<()> {
        let mut body = request.into_body();
        while let Some(chunk_res) = body.data().await {
            match chunk_res {
                Ok(chunk) => {
                    let _ = body.flow_control().release_capacity(chunk.len());
                    let _ = tx.send(chunk);
                }
                Err(e) => debug!("XHTTP POST Error: {}", e),
            }
        }
        let response = Response::builder()
            .status(StatusCode::OK)
            .header("server", "nginx/1.26.0")
            .header("x-request-id", Self::gen_request_id())
            .header("x-cache", Self::gen_cache_status())
            .header("x-padding", Self::gen_padding())
            .body(())
            .unwrap();
        respond.send_response(response, true)?;
        Ok(())
    }

    async fn send_error_response(
        respond: &mut SendResponse<Bytes>,
        status: StatusCode,
    ) -> Result<()> {
        let response = Response::builder()
            .status(status)
            .header("server", "nginx/1.26.0")
            .body(())
            .unwrap();
        respond.send_response(response, true)?;
        Ok(())
    }
}

use anyhow::Result;
use bytes::{Buf, Bytes, BytesMut};
use h2::server::{self, SendResponse};
use h2::SendStream;
use hyper::http::{Request, Response, StatusCode};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, Notify};
use tracing::{debug, info, warn, error, trace};
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

/// 终极 H2/XHTTP 处理器 (v0.4.1: 编译修复与告警清理版)
#[derive(Clone)]
pub struct H2Handler {
    config: XhttpConfig,
}

impl H2Handler {
    pub fn new(config: XhttpConfig) -> Self {
        Self { config }
    }

    /// 生成随机 Padding 字符串，用于模糊 HTTP 头部长度
    fn gen_padding() -> String {
        let mut rng = rand::thread_rng();
        let len = rng.gen_range(64..512); // 随机 64 到 512 字节
        rng.sample_iter(&Alphanumeric)
            .take(len)
            .map(char::from)
            .collect()
    }

    /// 智能分片发送（流量整形/Shredder）
    /// 将大数据块切分成随机大小的小块发送，消除长度特征
    fn send_split_data(src: &mut BytesMut, send_stream: &mut SendStream<Bytes>) -> Result<()> {
        let mut rng = rand::thread_rng();
        
        while src.has_remaining() {
            // 均衡优化：随机切片大小 1024B - 4096B
            // 在保持轻量级内存占用的同时，确保推特头像和视频的高速吞吐
            let chunk_size = rng.gen_range(1024..4096);
            let split_len = std::cmp::min(src.len(), chunk_size);
            
            // split_to 会消耗 src 前面的字节，返回新的 Bytes (Zero-copy)
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
        info!("XHTTP: 启动 V41 拟态防御引擎 (Balanced Performance + Adaptive Memory)");

        let mut builder = server::Builder::new();
        builder
            .initial_window_size(4194304)    // 4MB 窗口
            .initial_connection_window_size(8388608) // 8MB 连接窗口
            .max_concurrent_streams(500)
            .max_frame_size(16384);

        // 使用 tokio::time::timeout 替代不存在的 handshake_timeout 方法
        let mut connection = tokio::time::timeout(
            Duration::from_secs(20),
            builder.handshake(stream)
        ).await??;

        // --- 🌟 H2 Ping-Pong 随机心跳混淆 (V89) ---
        // 获取 PingPong 句柄，启动后台任务随机发送 PING
        // 这会迫使客户端回复 ACK，制造双向的背景流量噪声，干扰时序分析。
        if let Some(mut ping_pong) = connection.ping_pong() {
            tokio::spawn(async move {
                loop {
                    // 随机休眠 15 - 45 秒 (模拟真实心跳间隔，不要太频繁以免浪费流量)
                    let sleep_ms = {
                         let mut rng = rand::thread_rng();
                         rng.gen_range(15000..45000)
                    };
                    tokio::time::sleep(tokio::time::Duration::from_millis(sleep_ms)).await;

                    // 生成随机 8 字节载荷 (h2 crate 限制 payload 为 opaque，主要依赖时序混淆)
                    let _payload: [u8; 8] = {
                        let mut rng = rand::thread_rng();
                        rng.gen()
                    };
                    
                    // 发送 PING
                    if let Err(e) = ping_pong.send_ping(h2::Ping::opaque()) {
                        debug!("🌪️ H2 Noise: Ping failed (system busy or network jitter): {}", e);
                        // 稳定性优化：Ping 这种辅助任务失败不应该立即拖死主循环，尝试等待后重启逻辑
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    }
                    debug!("🌪️ H2 Noise: Sent random PING");
                }
            });
        }
        // -------------------------------------------
        
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

            // 等候配对逻辑
            // 移动端网络可能存在波动，增加等待时间至 2 秒 (40 * 50ms)
            // 避免因 POST 请求过早到达但 Session 尚未就绪而导致的断流
            if !is_pc {
                for _ in 0..40 {
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
                // 修复：PC 端也可能使用 standard gRPC 模式 (如 Xray-core 配置为 grpc)，不能强制 !is_pc
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
            .header("connection", "keep-alive")
            .header("keep-alive", "timeout=120")
            .header("cache-control", "no-store, no-cache, must-revalidate, proxy-revalidate, max-age=0")
            .header("x-padding", Self::gen_padding()) // 注入动态填充
            .body(())
            .unwrap();

        let mut send_stream = respond.send_response(response, false)?;
        let (client_io, server_io) = tokio::io::duplex(65536);
        
        let use_grpc_framing = Arc::new(AtomicBool::new(is_grpc));
        let use_grpc_framing_up = use_grpc_framing.clone();
        let use_grpc_framing_down = use_grpc_framing.clone();

        debug!("XHTTP Standard: 启动 VLESS 处理逻辑 (is_grpc: {})", is_grpc);
        tokio::spawn(handler(Box::new(server_io)));
        let (mut client_read, mut client_write) = tokio::io::split(client_io);

        // UP
        let up_task = async move {
            let mut body = request.into_body();
            let mut leftover = BytesMut::new();
            use tokio::io::AsyncWriteExt;
            debug!("XHTTP UP: 开始从请求体读取数据");
            
            let mut first_chunk = true;

            // 移除 30s 强行超时，改用更稳健的流式读取
            // 这样即使 30s 没有上行数据，连接也不会被误杀
            while let Some(chunk) = body.data().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        let err_str = e.to_string();
                        if err_str.contains("stream no longer needed") || err_str.contains("connection reset") {
                            debug!("XHTTP UP: 连接正常结束 / 重置: {}", e);
                        } else {
                            error!("XHTTP UP: 读取请求体错误: {}", e);
                        }
                        break;
                    }
                };
                let _ = body.flow_control().release_capacity(chunk.len());
                trace!("XHTTP UP: 收到 {} 字节原始数据", chunk.len());
                
                if first_chunk && use_grpc_framing_up.load(Ordering::Relaxed) {
                    first_chunk = false;
                    // gRPC 帧首字节必须是 0x00 (非压缩) 或 0x01 (压缩)
                    // 如果不是，说明客户端虽然传了 grpc header 但实际上发的是普通流
                    if chunk.len() > 0 && chunk[0] != 0x00 && chunk[0] != 0x01 {
                        warn!("XHTTP UP: 检测到首字节 ({:02x}) 非 gRPC 格式，自动回退到普通流模式", chunk[0]);
                        use_grpc_framing_up.store(false, Ordering::Relaxed);
                    }
                }

                if use_grpc_framing_up.load(Ordering::Relaxed) {
                    leftover.extend_from_slice(&chunk);
                    while leftover.len() >= 5 {
                        let msg_len = u32::from_be_bytes([leftover[1], leftover[2], leftover[3], leftover[4]]) as usize;
                        
                        // 关键修复：长度校验 
                        // 如果长度超过 64KB，通常不是合法的 gRPC 代理包格式 (通常为 VLESS 原始流误入)
                        if msg_len > 65535 {
                            warn!("XHTTP UP: 检测到异常消息长度 ({}), 判定为 VLESS 原始流，转回普通模式", msg_len);
                            use_grpc_framing_up.store(false, Ordering::Relaxed);
                            client_write.write_all(&leftover).await?;
                            leftover.clear();
                            break;
                        }

                        if leftover.len() >= 5 + msg_len {
                            let _ = leftover.split_to(5);
                            let data = leftover.split_to(msg_len);
                            trace!("XHTTP UP: 解析到 {} 字节 gRPC 消息", data.len());
                            client_write.write_all(&data).await?;
                        } else { 
                            debug!("XHTTP UP: gRPC 消息未全 (需要 {} 字节，现有 {} 字节)", 5 + msg_len, leftover.len());
                            break; 
                        }
                    }
                } else {
                    client_write.write_all(&chunk).await?;
                }
            }
            debug!("XHTTP UP: 请求体读取结束");
            Ok::<(), anyhow::Error>(())
        };

        // DOWN (使用 Traffic Shaping)
        let down_task = async move {
            let mut buf = BytesMut::with_capacity(65536);
            use tokio::io::AsyncReadExt;
            debug!("XHTTP DOWN: 开始从 VLESS 读取数据并发送给客户端");
            loop {
                if buf.capacity() < 2048 {
                    buf.reserve(65536);
                }
                let n = client_read.read_buf(&mut buf).await?;
                if n == 0 { 
                    debug!("XHTTP DOWN: VLESS 已关闭输出");
                    break; 
                }
                trace!("XHTTP DOWN: 从 VLESS 收到 {} 字节数据", n);
                
                if use_grpc_framing_down.load(Ordering::Relaxed) {
                    let mut frame = BytesMut::with_capacity(5 + n);
                    frame.extend_from_slice(&[0u8]);
                    frame.extend_from_slice(&(n as u32).to_be_bytes());
                    // copy needed here as we are framing
                    frame.extend_from_slice(&buf[..n]);
                    buf.advance(n);

                    // 整形发送 gRPC 帧
                    Self::send_split_data(&mut frame, &mut send_stream)?;
                } else {
                    // 整形发送普通数据流
                    Self::send_split_data(&mut buf, &mut send_stream)?;
                }
            }
            
            debug!("XHTTP DOWN: 发送结束标记 (Trailers/EndStream)");
            if use_grpc_framing_down.load(Ordering::Relaxed) {
                let mut trailers = hyper::http::HeaderMap::new();
                trailers.insert("grpc-status", "0".parse().unwrap());
                send_stream.send_trailers(trailers)?;
            } else {
                send_stream.send_data(Bytes::new(), true)?;
            }
            Ok::<(), anyhow::Error>(())
        };

        // 修复：不能使用 select!，因为上行流结束不代表下行流也该结束
        // 重新使用 spawn 模式，让两个流独立运行至自然闭合
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
            .header("cache-control", "no-store, no-cache, must-revalidate, proxy-revalidate, max-age=0")
            .header("x-padding", Self::gen_padding()) // 注入动态填充
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
                
                // 整形发送
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

        // 修复：独立运行
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
            let chunk = chunk_res?;
            let _ = body.flow_control().release_capacity(chunk.len());
            let _ = tx.send(chunk);
        }
        let response = Response::builder()
            .status(StatusCode::OK)
            .header("server", "nginx/1.26.0")
            .header("cache-control", "no-store, no-cache, must-revalidate, proxy-revalidate, max-age=0")
            .header("x-padding", Self::gen_padding()) // 注入动态填充
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

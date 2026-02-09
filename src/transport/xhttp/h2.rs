use anyhow::Result;
use bytes::{Buf, Bytes, BytesMut};
use h2::server::{self, SendResponse};
use h2::SendStream;
use hyper::http::{Request, Response, StatusCode};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, Notify};
use tracing::{debug, info, warn, error, trace};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, AtomicU64, Ordering};
use std::time::Duration;
use once_cell::sync::Lazy;
use rand::{distributions::Alphanumeric, Rng};

use super::XhttpConfig;
use dashmap::DashMap;

/// 全局会话管理器
struct Session {
    to_vless_tx: mpsc::UnboundedSender<Bytes>,
    notify: Arc<Notify>,
    transferred_bytes: Arc<AtomicUsize>,
}

static SESSIONS: Lazy<Arc<DashMap<String, Session>>> = Lazy::new(|| {
    Arc::new(DashMap::new())
});

/// 会话守卫 (RAII Guard)
/// 确保 Session 在离开作用域时必然被移除，防止内存泄漏
struct SessionGuard {
    path: String,
    notify: Arc<Notify>,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if SESSIONS.remove(&self.path).is_some() {
            debug!("Session clean up: {}", self.path);
        }
        self.notify.notify_waiters();
    }
}

/// 终极 H2/XHTTP 处理器 (v0.4.1: 编译修复与告警清理版)
#[derive(Clone)]
pub struct H2Handler {
    config: XhttpConfig,
    /// 流量计数器 (用于自适应优化)
    traffic_counter: Arc<std::sync::atomic::AtomicU64>,
}

impl H2Handler {
    pub fn new(config: XhttpConfig) -> Self {
        Self { 
            config,
            traffic_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// 自适应随机 Padding (V90: 流量敏感型)
    fn gen_adaptive_padding(traffic: u64) -> String {
        let mut rng = rand::thread_rng();
        
        let (min, max) = if traffic < 1048576 {
            (64, 512)   // 安全优先
        } else {
            (16, 32)    // 性能优先
        };

        let len = rng.gen_range(min..max);
        let mut bytes = vec![0u8; len];
        rng.fill(&mut bytes[..]);
        
        for b in &mut bytes {
            *b = (*b % 62) + 48; // 映射到 0-9, A-Z, a-z
            if *b > 57 { *b += 7; }
            if *b > 90 { *b += 6; }
        }
        unsafe { String::from_utf8_unchecked(bytes) }
    }

    /// 智能分片发送（流量整形/Shredder）
    /// 将大数据块切分成随机大小的小块发送，消除长度特征
    fn send_split_data(src: &mut BytesMut, send_stream: &mut SendStream<Bytes>, counter: &Arc<std::sync::atomic::AtomicU64>) -> Result<()> {
        let mut rng = rand::thread_rng();
        
        while src.has_remaining() {
            let chunk_size = rng.gen_range(8192..16384);
            let split_len = std::cmp::min(src.len(), chunk_size);
            
            // 累加流量计数
            counter.fetch_add(split_len as u64, Ordering::Relaxed);

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
                        // tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        break;
                    }
                    debug!("🌪️ H2 Noise: Sent random PING");
                }
            });
        }
        // -------------------------------------------
        
        let active_streams = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        loop {
            let is_idle = active_streams.load(Ordering::Relaxed) == 0;
            
            tokio::select! {
                result = connection.accept() => {
                    match result {
                        Some(Ok((request, respond))) => {
                            let config = self.config.clone();
                            let handler = handler.clone();
                            let counter = self.traffic_counter.clone();
                            let active_streams_inner = active_streams.clone();
                            
                            active_streams_inner.fetch_add(1, Ordering::Relaxed);
                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_request(config, request, respond, handler, counter).await {
                                    debug!("连接处理闭合: {}", e);
                                }
                                active_streams_inner.fetch_sub(1, Ordering::Relaxed);
                            });
                        }
                        Some(Err(e)) => {
                            debug!("H2 连接中断: {}", e);
                            break;
                        }
                        None => break,
                    }
                }
                // --- 🌟 H2 Zombie Watchdog (V92) ---
                // 如果当前没有任何活跃流 (Active Streams == 0)
                // 且持续 300 秒没有新请求进入，则认为此连接为僵尸连接，强制关闭。
                _ = tokio::time::sleep(Duration::from_secs(300)), if is_idle => {
                    debug!("H2 Connection: Zombie watchdog triggered (300s idle)");
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
        traffic_counter: Arc<std::sync::atomic::AtomicU64>,
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
            Self::handle_xhttp_get(path, respond, handler, traffic_counter).await?;
        } else if method == "POST" {
            let user_agent = request.headers().get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("");
            let is_pc = user_agent.contains("Go-http-client");

            // 等候配对逻辑
            if !is_pc {
                for _ in 0..40 {
                    let found = SESSIONS.contains_key(&path);
                    if found { break; }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }

            let session_tx = SESSIONS.get(&path).map(|s| s.to_vless_tx.clone());

            if let Some(tx) = session_tx {
                Self::handle_xhttp_post(request, respond, tx, traffic_counter).await?;
            } else {
                let content_type = request.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
                let is_grpc = content_type.contains("grpc");
                Self::handle_standalone(request, respond, handler, is_grpc, traffic_counter).await?;
            }
        }
 else {
            Self::send_error_response(&mut respond, StatusCode::METHOD_NOT_ALLOWED).await?;
        }
        Ok(())
    }

    async fn handle_standalone<F, Fut>(
        request: Request<h2::RecvStream>,
        mut respond: SendResponse<Bytes>,
        handler: F,
        is_grpc: bool,
        traffic_counter: Arc<std::sync::atomic::AtomicU64>,
    ) -> Result<()>
    where
        F: Fn(Box<dyn crate::server::AsyncStream>) -> Fut + Clone + Send + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        let response = Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/octet-stream")
            .header("server", "nginx/1.26.0")
            .header("cache-control", "no-store, no-cache, must-revalidate, proxy-revalidate, max-age=0")
            .header("x-padding", Self::gen_adaptive_padding(0)) // Standalone 通常为首包，使用全量填充
            .body(())
            .unwrap();

        let mut send_stream = respond.send_response(response, false)?;
        // 扩容核心：将内部管道从 64KB 扩大到 512KB (Zero-copy buffer)
        // 彻底消除高带宽下载时的反向压力 (Backpressure)
        let (client_io, server_io) = tokio::io::duplex(524288); // 512KB Buffer
        
        let use_grpc_framing = Arc::new(AtomicBool::new(is_grpc));
        let use_grpc_framing_up = use_grpc_framing.clone();
        let use_grpc_framing_down = use_grpc_framing.clone();

        debug!("XHTTP Standard: 启动 VLESS 处理逻辑 (is_grpc: {})", is_grpc);
        tokio::spawn(handler(Box::new(server_io)));
        let (mut client_read, mut client_write) = tokio::io::split(client_io);

        let traffic_counter_up = traffic_counter.clone();
        let traffic_counter_down = traffic_counter.clone();

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
                let chunk = chunk?;
                let len = chunk.len();
                traffic_counter_up.fetch_add(len as u64, Ordering::Relaxed);
                let _ = body.flow_control().release_capacity(len);
                trace!("XHTTP UP: 收到 {} 字节原始数据", len);
                
                if first_chunk && use_grpc_framing_up.load(Ordering::Relaxed) {
                    first_chunk = false;
                    if len > 0 && chunk[0] != 0x00 && chunk[0] != 0x01 {
                        warn!("XHTTP UP: 检测到首字节 ({:02x}) 非 gRPC 格式，自动回退到普通流模式", chunk[0]);
                        use_grpc_framing_up.store(false, Ordering::Relaxed);
                    }
                }

                if use_grpc_framing_up.load(Ordering::Relaxed) {
                    leftover.extend_from_slice(&chunk);
                    while leftover.len() >= 5 {
                        let msg_len = u32::from_be_bytes([leftover[1], leftover[2], leftover[3], leftover[4]]) as usize;
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
                    Self::send_split_data(&mut frame, &mut send_stream, &traffic_counter_down)?;
                } else {
                    // 整形发送普通数据流
                    Self::send_split_data(&mut buf, &mut send_stream, &traffic_counter_down)?;
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

        // Standalone 模式下，上行和下行在同一个 H2 Stream 中
        // 必须联动：一端彻底结束或出错，另一端也该停止，释放 H2 流
        debug!("XHTTP Standalone: 启动联动传输任务");
        
        let up_handle = tokio::spawn(up_task);
        let _ = down_task.await;
        let _ = up_handle.await;
        
        Ok(())
    }

    async fn handle_xhttp_get<F, Fut>(
        path: String,
        mut respond: SendResponse<Bytes>,
        handler: F,
        traffic_counter: Arc<std::sync::atomic::AtomicU64>,
    ) -> Result<()>
    where
        F: Fn(Box<dyn crate::server::AsyncStream>) -> Fut + Clone + Send + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        let (to_vless_tx, mut to_vless_rx) = mpsc::unbounded_channel::<Bytes>();
        let notify = Arc::new(Notify::new());
        let transferred_bytes = Arc::new(AtomicUsize::new(0));
        
        SESSIONS.insert(path.clone(), Session { 
            to_vless_tx, 
            notify: notify.clone(),
            transferred_bytes: transferred_bytes.clone(),
        });
        
        // 创建守卫，确保函数退出(无论成功/失败/Panic)都会清理 Session
        let _guard = SessionGuard { path: path.clone(), notify: notify.clone() };

        // 扩容核心：将内部管道从 64KB 扩大到 512KB (Zero-copy buffer)
        let (client_io, server_io) = tokio::io::duplex(524288);
        tokio::spawn(handler(Box::new(server_io)));
        let (mut client_read, mut client_write) = tokio::io::split(client_io);

        let response = Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/octet-stream")
            .header("server", "nginx/1.26.0")
            .header("cache-control", "no-store, no-cache, must-revalidate, proxy-revalidate, max-age=0")
            .header("x-padding", Self::gen_adaptive_padding(0)) // 初始响应使用 0 流量权重
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
                // 加入 300秒 闲置超时 (Idle Timeout)
                // 如果 5分钟 没有任何数据交换，主动断开回收资源
                let n = match tokio::time::timeout(std::time::Duration::from_secs(300), client_read.read_buf(&mut buf)).await {
                    Ok(Ok(n)) => n,
                    Ok(Err(e)) => return Err(e.into()),
                    Err(_) => {
                        debug!("XHTTP Split DOWN: Idle timeout (300s)");
                        break;
                    }
                };
                
                if n == 0 { break; }
                
                // 更新流量统计
                transferred_bytes.fetch_add(n, Ordering::Relaxed);
                
                // 整形发送
                Self::send_split_data(&mut buf, &mut send_stream, &traffic_counter)?;
            }
            send_stream.send_data(Bytes::new(), true)?;
            Ok::<(), anyhow::Error>(())
        };

        let upstream = async move {
            use tokio::io::AsyncWriteExt;
            // 上行同样加入闲置超时，防止 POST 端长时间挂死
            loop {
                match tokio::time::timeout(std::time::Duration::from_secs(300), to_vless_rx.recv()).await {
                    Ok(Some(data)) => {
                        client_write.write_all(&data).await?;
                    }
                    Ok(None) => break,
                    Err(_) => {
                        debug!("XHTTP Split UP: Idle timeout (300s)");
                        break;
                    }
                }
            }
            Ok::<(), anyhow::Error>(())
        };

        // 运行任务
        let up_handle = tokio::spawn(upstream);
        let _ = downstream.await;
        
        // 无论如何，确保从管理器移除 Session
        SESSIONS.remove(&path);
        notify.notify_waiters();
        
        // 等待上行任务结束
        let _ = up_handle.await;

        Ok(())
    }

    async fn handle_xhttp_post(
        request: Request<h2::RecvStream>,
        mut respond: SendResponse<Bytes>,
        tx: mpsc::UnboundedSender<Bytes>,
        traffic_counter: Arc<AtomicU64>,
    ) -> Result<()> {
        let mut body = request.into_body();
        while let Some(chunk_res) = body.data().await {
            let chunk = chunk_res?;
            let len = chunk.len();
            traffic_counter.fetch_add(len as u64, Ordering::Relaxed);
            let _ = body.flow_control().release_capacity(len);
            let _ = tx.send(chunk);
        }
        
        let total = traffic_counter.load(Ordering::Relaxed);

        let response = Response::builder()
            .status(StatusCode::OK)
            .header("server", "nginx/1.26.0")
            .header("cache-control", "no-store, no-cache, must-revalidate, proxy-revalidate, max-age=0")
            .header("x-padding", Self::gen_adaptive_padding(total)) // 注入动态填充 (自适应长度)
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

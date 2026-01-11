use anyhow::{anyhow, Result};
use bytes::{Bytes, BytesMut};
use h2::server::{self, SendResponse};
use hyper::http::{Request, Response, StatusCode};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use once_cell::sync::Lazy;

use super::XhttpConfig;

/// 会话管理器：用于将 XHTTP 的 GET 和 POST 绑定到同一个 VLESS 管道
struct Session {
    to_vless_tx: mpsc::UnboundedSender<Bytes>,
}

static SESSIONS: Lazy<Arc<Mutex<HashMap<String, Session>>>> = Lazy::new(|| {
    Arc::new(Mutex::new(HashMap::new()))
});

/// HTTP/2 处理器
#[derive(Clone)]
pub struct H2Handler {
    config: XhttpConfig,
}

impl H2Handler {
    pub fn new(config: XhttpConfig) -> Self {
        Self { config }
    }

    pub async fn handle<T, F, Fut>(&self, stream: T, handler: F) -> Result<()>
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        F: Fn(Box<dyn crate::server::AsyncStream>) -> Fut + Clone + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        debug!("XHTTP: 启动 V70 全兼容自适应引擎 (Pairing + Standalone)");

        let mut builder = server::Builder::new();
        builder
            .initial_window_size(4 * 1024 * 1024)
            .max_concurrent_streams(500)
            .max_frame_size(16384);

        let mut connection = builder.handshake(stream).await?;
        
        while let Some(result) = connection.accept().await {
            match result {
                Ok((request, respond)) => {
                    let config = self.config.clone();
                    let handler = handler.clone();
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_request(config, request, respond, handler).await {
                            debug!("XHTTP 处理流结束: {}", e);
                        }
                    });
                }
                Err(e) => {
                    debug!("H2 连接连接断开: {}", e);
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
            // --- XHTTP 模式: 初始化会话并处理下载流 ---
            Self::handle_xhttp_get(path, respond, handler).await?;
        } else if method == "POST" {
            // --- XHTTP 模式 vs Standalone 模式 判定 ---
            let session_found = {
                let sessions = SESSIONS.lock().unwrap();
                sessions.contains_key(&path)
            };

            if session_found {
                // 这是 XHTTP 分离流的上传部分
                Self::handle_xhttp_post(path, request, respond).await?;
            } else {
                // 这是传统的双向流 (PC 端 Xray 或 手机端直连 gRPC)
                Self::handle_standalone(request, respond, handler).await?;
            }
        } else {
            Self::send_error_response(&mut respond, StatusCode::METHOD_NOT_ALLOWED).await?;
        }
        Ok(())
    }

    /// 接管双向流 (Standalone 模式)
    async fn handle_standalone<F, Fut>(
        mut request: Request<h2::RecvStream>,
        mut respond: SendResponse<Bytes>,
        handler: F,
    ) -> Result<()>
    where
        F: Fn(Box<dyn crate::server::AsyncStream>) -> Fut + Clone + Send + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        let content_type = request.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
        let user_agent = request.headers().get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("");
        
        // 判定: 是否需要开启 gRPC 5字节分帧? (排除 PC 端)
        let is_grpc = content_type.contains("grpc") && !user_agent.contains("Go-http-client");
        debug!("STANDALONE: (GRPC={}) UA={}", is_grpc, user_agent);

        let response = Response::builder()
            .status(StatusCode::OK)
            .header("content-type", if is_grpc { "application/grpc" } else { "application/octet-stream" })
            .body(())
            .unwrap();

        let mut send_stream = respond.send_response(response, false)?;
        let (client_io, server_io) = tokio::io::duplex(65536);
        tokio::spawn(handler(Box::new(server_io)));
        let (mut client_read, mut client_write) = tokio::io::split(client_io);

        // UP: Client -> VLESS
        let up_task = async move {
            let mut body = request.into_body();
            let mut leftover = BytesMut::new();
            use tokio::io::AsyncWriteExt;
            while let Some(chunk_res) = body.data().await {
                let chunk = chunk_res?;
                let _ = body.flow_control().release_capacity(chunk.len());
                if is_grpc {
                    leftover.extend_from_slice(&chunk);
                    while leftover.len() >= 5 {
                        let msg_len = u32::from_be_bytes([leftover[1], leftover[2], leftover[3], leftover[4]]) as usize;
                        if leftover.len() >= 5 + msg_len {
                            let _ = leftover.split_to(5);
                            let data = leftover.split_to(msg_len);
                            client_write.write_all(&data).await?;
                        } else { break; }
                    }
                } else {
                    client_write.write_all(&chunk).await?;
                }
            }
            Ok::<(), anyhow::Error>(())
        };

        // DOWN: VLESS -> Client
        let down_task = async move {
            let mut buf = vec![0u8; 16384];
            use tokio::io::AsyncReadExt;
            loop {
                let n = client_read.read(&mut buf).await?;
                if n == 0 { break; }
                if is_grpc {
                    let mut frame = BytesMut::with_capacity(5 + n);
                    frame.extend_from_slice(&[0u8]);
                    frame.extend_from_slice(&(n as u32).to_be_bytes());
                    frame.extend_from_slice(&buf[..n]);
                    send_stream.send_data(frame.freeze(), false)?;
                } else {
                    send_stream.send_data(Bytes::copy_from_slice(&buf[..n]), false)?;
                }
            }
            if is_grpc {
                let mut trailers = hyper::http::HeaderMap::new();
                trailers.insert("grpc-status", "0".parse().unwrap());
                send_stream.send_trailers(trailers)?;
            } else {
                send_stream.send_data(Bytes::new(), true)?;
            }
            Ok::<(), anyhow::Error>(())
        };

        let _ = tokio::spawn(up_task);
        down_task.await?; 
        Ok(())
    }

    /// XHTTP GET: 下载通道实现
    async fn handle_xhttp_get<F, Fut>(
        path: String,
        mut respond: SendResponse<Bytes>,
        handler: F,
    ) -> Result<()>
    where
        F: Fn(Box<dyn crate::server::AsyncStream>) -> Fut + Clone + Send + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        debug!("XHTTP: 匹配到下载流 (GET)");
        let (to_vless_tx, mut to_vless_rx) = mpsc::unbounded_channel::<Bytes>();
        
        {
            let mut sessions = SESSIONS.lock().unwrap();
            sessions.insert(path.clone(), Session { to_vless_tx });
        }

        let (client_io, server_io) = tokio::io::duplex(65536);
        tokio::spawn(handler(Box::new(server_io)));
        let (mut client_read, mut client_write) = tokio::io::split(client_io);

        let response = Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/octet-stream")
            .body(())
            .unwrap();
        let mut send_stream = respond.send_response(response, false)?;

        // VLESS -> Client (通过 GET 响应发送)
        let downstream = async move {
            let mut buf = vec![0u8; 16384];
            use tokio::io::AsyncReadExt;
            loop {
                let n = client_read.read(&mut buf).await?;
                if n == 0 { break; }
                send_stream.send_data(Bytes::copy_from_slice(&buf[..n]), false)?;
            }
            send_stream.send_data(Bytes::new(), true)?;
            Ok::<(), anyhow::Error>(())
        };

        // Client -> VLESS (通过 POST 会话注入)
        let upstream = async move {
            use tokio::io::AsyncWriteExt;
            while let Some(data) = to_vless_rx.recv().await {
                client_write.write_all(&data).await?;
            }
            Ok::<(), anyhow::Error>(())
        };

        let _ = tokio::spawn(upstream);
        downstream.await?;
        
        {
            let mut sessions = SESSIONS.lock().unwrap();
            sessions.remove(&path);
        }
        Ok(())
    }

    /// XHTTP POST: 上传通道实现
    async fn handle_xhttp_post(
        path: String,
        request: Request<h2::RecvStream>,
        mut respond: SendResponse<Bytes>,
    ) -> Result<()> {
        debug!("XHTTP: 匹配到上传流 (POST)");
        let tx = {
            let sessions = SESSIONS.lock().unwrap();
            sessions.get(&path).map(|s| s.to_vless_tx.clone())
        };

        if let Some(tx) = tx {
            let mut body = request.into_body();
            while let Some(chunk_res) = body.data().await {
                let chunk = chunk_res?;
                let _ = body.flow_control().release_capacity(chunk.len());
                let _ = tx.send(chunk);
            }
            let response = Response::builder().status(StatusCode::OK).body(()).unwrap();
            respond.send_response(response, true)?;
        } else {
             Self::send_error_response(&mut respond, StatusCode::NOT_FOUND).await?;
        }
        Ok(())
    }

    async fn send_error_response(
        respond: &mut SendResponse<Bytes>,
        status: StatusCode,
    ) -> Result<()> {
        let response = Response::builder().status(status).body(()).unwrap();
        respond.send_response(response, true)?;
        Ok(())
    }
}

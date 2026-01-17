use anyhow::Result;
use crate::utils::net::DualTcpStream;
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{debug, error};

pub struct ProxyConnection<C, R> {
    client_stream: C,
    remote_stream: R,
}

impl<C, R> ProxyConnection<C, R> 
where 
    C: AsyncRead + AsyncWrite + Unpin + 'static,
    R: AsyncRead + AsyncWrite + Unpin + 'static
{
    pub fn new(client_stream: C, remote_stream: R) -> Self {
        Self {
            client_stream,
            remote_stream,
        }
    }

    /// 双向数据转发 - 恢复经受过考验的 copy_bidirectional 逻辑
    pub async fn relay(mut self) -> Result<()> {
        match tokio::io::copy_bidirectional(&mut self.client_stream, &mut self.remote_stream).await {
            Ok((c_to_r, r_to_c)) => {
                debug!("连接关闭: C->R {} B, R->C {} B", c_to_r, r_to_c);
                Ok(())
            }
            Err(e) => {
                debug!("连接中断: {}", e);
                Ok(())
            }
        }
    }
}

#[derive(Clone)]
pub struct ConnectionManager {
    active_connections: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            active_connections: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    pub fn active_count(&self) -> usize {
        self.active_connections.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn handle_connection<T>(
        &self,
        client_stream: T,
        remote_stream: DualTcpStream,
    ) -> Result<()> 
    where
        T: AsyncRead + AsyncWrite + Unpin + 'static
    {
        self.active_connections.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let proxy_conn = ProxyConnection::new(client_stream, remote_stream);
        let result = proxy_conn.relay().await;
        self.active_connections.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        result
    }
}

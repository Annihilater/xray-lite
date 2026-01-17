use anyhow::Result;
use crate::utils::net::DualTcpStream;
use tokio::io::{AsyncRead, AsyncWrite, AsyncReadExt, AsyncWriteExt};
use tracing::{debug, error};

const RELAY_BUFFER_SIZE: usize = 128 * 1024; // 128KB 巨型缓冲区，专为单核高吞吐设计

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

    /// 双向数据转发 - 深度优化版本 (1 vCPU 特化版)
    pub async fn relay(self) -> Result<()> {
        let (mut c_read, mut c_write) = tokio::io::split(self.client_stream);
        let (mut r_read, mut r_write) = tokio::io::split(self.remote_stream);

        // 使用大型缓冲区和并行任务
        let client_to_remote = async move {
            let mut buf = vec![0u8; RELAY_BUFFER_SIZE];
            loop {
                let n = c_read.read(&mut buf).await?;
                if n == 0 { break; }
                r_write.write_all(&buf[..n]).await?;
            }
            r_write.shutdown().await?;
            Ok::<u64, std::io::Error>(0)
        };

        let remote_to_client = async move {
            let mut buf = vec![0u8; RELAY_BUFFER_SIZE];
            loop {
                let n = r_read.read(&mut buf).await?;
                if n == 0 { break; }
                c_write.write_all(&buf[..n]).await?;
            }
            c_write.shutdown().await?;
            Ok::<u64, std::io::Error>(0)
        };

        // 并行处理两个方向
        match tokio::try_join!(client_to_remote, remote_to_client) {
            Ok(_) => {
                debug!("连接已平滑关闭");
                Ok(())
            }
            Err(e) => {
                debug!("连接异常中断: {}", e);
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

use std::sync::Mutex;
use once_cell::sync::Lazy;
use anyhow::Result;
use crate::utils::net::DualTcpStream;
use tokio::net::TcpStream;
use tokio::io::{AsyncRead, AsyncWrite, AsyncReadExt, AsyncWriteExt};
use tracing::{debug, error};

const BUFFER_SIZE: usize = 16 * 1024;
static BUFFER_POOL: Lazy<Mutex<Vec<Vec<u8>>>> = Lazy::new(|| Mutex::new(Vec::with_capacity(256)));

struct PooledBuffer(Option<Vec<u8>>);

impl PooledBuffer {
    fn get() -> Self {
        if let Ok(mut pool) = BUFFER_POOL.lock() {
            if let Some(buf) = pool.pop() {
                return PooledBuffer(Some(buf));
            }
        }
        PooledBuffer(Some(vec![0u8; BUFFER_SIZE]))
    }
}

impl std::ops::Deref for PooledBuffer {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for PooledBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().unwrap()
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        if let Some(buf) = self.0.take() {
            if let Ok(mut pool) = BUFFER_POOL.lock() {
                if pool.len() < 512 {
                    pool.push(buf);
                }
            }
        }
    }
}

/// 代理连接
pub struct ProxyConnection<C, R> {
    client_stream: C,
    remote_stream: R,
}

impl<C, R> ProxyConnection<C, R> 
where 
    C: AsyncRead + AsyncWrite + Unpin + 'static,
    R: AsyncRead + AsyncWrite + Unpin + 'static
{
    /// 创建新的代理连接
    pub fn new(client_stream: C, remote_stream: R) -> Self {
        Self {
            client_stream,
            remote_stream,
        }
    }

    /// 双向数据转发
    pub async fn relay(mut self) -> Result<()> {
        debug!("开始双向数据转发 (使用 copy_bidirectional)");

        // Optimize: Use copy_bidirectional which internaly uses splice/sendfile when possible
        // and avoids manual user-space buffer management overhead.
        match tokio::io::copy_bidirectional(&mut self.client_stream, &mut self.remote_stream).await {
            Ok((c_to_r, r_to_c)) => {
                 debug!("连接结束: C->R {} bytes, R->C {} bytes", c_to_r, r_to_c);
                 Ok(())
            },
            Err(e) => {
                debug!("连接断开: {}", e);
                // Don't treat connection reset as error
                Ok(()) 
            }
        }
    }
}

/// 连接管理器
#[derive(Clone)]
pub struct ConnectionManager {
    /// 活跃连接数
    active_connections: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl ConnectionManager {
    /// 创建新的连接管理器
    pub fn new() -> Self {
        Self {
            active_connections: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// 获取活跃连接数
    pub fn active_count(&self) -> usize {
        self.active_connections
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 处理新连接
    pub async fn handle_connection<T>(
        &self,
        client_stream: T,
        remote_stream: DualTcpStream,
    ) -> Result<()> 
    where
        T: AsyncRead + AsyncWrite + Unpin + 'static
    {
        // 增加活跃连接计数
        self.active_connections
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let active_connections = self.active_connections.clone();

        // 在新任务中处理连接
        crate::utils::task::spawn(async move {
            let connection = ProxyConnection::new(client_stream, remote_stream);
            
            if let Err(e) = connection.relay().await {
                error!("连接处理失败: {}", e);
            }

            // 减少活跃连接计数
            active_connections.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        });

        Ok(())
    }
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_manager_creation() {
        let manager = ConnectionManager::new();
        assert_eq!(manager.active_count(), 0);
    }
}

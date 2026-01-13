use std::sync::Mutex;
use once_cell::sync::Lazy;
use anyhow::Result;
use tokio::net::TcpStream;
use tokio::io::{AsyncRead, AsyncWrite, AsyncReadExt, AsyncWriteExt};
use tracing::{debug, error, info};

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
    C: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    R: AsyncRead + AsyncWrite + Unpin + Send + 'static
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
        debug!("开始双向数据转发 (已启用内存池)");

        let (mut c_r, mut c_w) = tokio::io::split(self.client_stream);
        let (mut r_r, mut r_w) = tokio::io::split(self.remote_stream);

        let client_to_remote = async {
            let mut buf = PooledBuffer::get();
            loop {
                let n = c_r.read(&mut buf).await?;
                if n == 0 {
                    r_w.shutdown().await?;
                    break;
                }
                r_w.write_all(&buf[..n]).await?;
            }
            Ok::<u64, std::io::Error>(0)
        };

        let remote_to_client = async {
            let mut buf = PooledBuffer::get();
            loop {
                let n = r_r.read(&mut buf).await?;
                if n == 0 {
                    c_w.shutdown().await?;
                    break;
                }
                c_w.write_all(&buf[..n]).await?;
            }
            Ok::<u64, std::io::Error>(0)
        };

        // 使用 try_join! 并发执行两个拷贝任务
        // 任何一方出错或完成，都会结束
        match tokio::try_join!(client_to_remote, remote_to_client) {
            Ok(_) => {
                debug!("连接正常关闭");
                Ok(())
            }
            Err(e) => {
                // 如果是正常的连接重置或关闭，不记录为错误
                debug!("连接断开: {}", e);
                Err(e.into())
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
        remote_stream: TcpStream,
    ) -> Result<()> 
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static
    {
        // 增加活跃连接计数
        self.active_connections
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let active_connections = self.active_connections.clone();

        // 在新任务中处理连接
        tokio::spawn(async move {
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

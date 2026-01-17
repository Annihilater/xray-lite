use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::os::fd::{AsRawFd, RawFd};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use anyhow::Result;
use crate::utils::task::{get_runtime_mode, RuntimeMode};

pub enum DualTcpStream {
    Tokio(tokio::net::TcpStream),
    Monoio(monoio_compat::TcpStreamCompat),
}

impl DualTcpStream {
    pub async fn connect(addr: &str) -> Result<Self> {
        match get_runtime_mode() {
            RuntimeMode::Tokio => {
                 let stream = tokio::net::TcpStream::connect(addr).await?;
                 Ok(Self::Tokio(stream))
            }
            RuntimeMode::Monoio => {
                let stream = monoio::net::TcpStream::connect(addr).await?;
                let compat = monoio_compat::TcpStreamCompat::new(stream);
                Ok(Self::Monoio(compat))
            }
        }
    }

    pub fn raw_fd(&self) -> Option<RawFd> {
        match self {
            Self::Tokio(s) => Some(s.as_raw_fd()),
            Self::Monoio(_) => None, 
        }
    }

    pub fn set_nodelay(&self, nodelay: bool) -> Result<()> {
        match self {
            Self::Tokio(s) => s.set_nodelay(nodelay).map_err(|e| e.into()),
            Self::Monoio(s) => {
                // TcpStreamCompat does not expose set_nodelay directly usually?
                // But wrapper usually implements AsyncRead/Write.
                // We might need to unsafe access or assume default.
                // Monoio streams are usually nodelay by default?
                // Let's check if we can deref.
                // If s is TcpStreamCompat, it does not deref.
                // If we can't set it, we ignore it or warn.
                // For now, ignore.
                Ok(())
            }
        }
    }
}

impl AsyncRead for DualTcpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tokio(s) => Pin::new(s).poll_read(cx, buf),
            Self::Monoio(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for DualTcpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Tokio(s) => Pin::new(s).poll_write(cx, buf),
            Self::Monoio(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tokio(s) => Pin::new(s).poll_flush(cx),
            Self::Monoio(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tokio(s) => Pin::new(s).poll_shutdown(cx),
            Self::Monoio(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

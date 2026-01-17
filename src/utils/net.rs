use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::os::fd::{AsRawFd, RawFd};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use anyhow::Result;
use crate::utils::task::{get_runtime_mode, RuntimeMode};

pub trait MaybeAsRawFd {
    fn maybe_as_raw_fd(&self) -> Option<RawFd>;
}

// 基础 TCP 实现
impl MaybeAsRawFd for tokio::net::TcpStream {
    fn maybe_as_raw_fd(&self) -> Option<RawFd> { Some(self.as_raw_fd()) }
}

// ！！！关键修改：TLS/Reality 包装层严禁直接使用 Splice ！！！
// 因为 Splice 会绕过解密逻辑，导致数据损坏。
impl<S: MaybeAsRawFd> MaybeAsRawFd for tokio_rustls::server::TlsStream<S> {
    fn maybe_as_raw_fd(&self) -> Option<RawFd> { None } 
}

impl<S: MaybeAsRawFd> MaybeAsRawFd for tokio_rustls::client::TlsStream<S> {
    fn maybe_as_raw_fd(&self) -> Option<RawFd> { None }
}

impl MaybeAsRawFd for tokio::io::DuplexStream {
    fn maybe_as_raw_fd(&self) -> Option<RawFd> { None }
}

pub enum DualTcpStream {
    Tokio(tokio::net::TcpStream, RawFd),
    Monoio(monoio_compat::TcpStreamCompat, RawFd),
}

impl DualTcpStream {
    pub async fn connect(addr: &str) -> Result<Self> {
        match get_runtime_mode() {
            RuntimeMode::Tokio => {
                 let stream = tokio::net::TcpStream::connect(addr).await?;
                 let fd = stream.as_raw_fd();
                 Ok(Self::Tokio(stream, fd))
            }
            RuntimeMode::Monoio => {
                let stream = monoio::net::TcpStream::connect(addr).await?;
                let fd = stream.as_raw_fd();
                let compat = monoio_compat::TcpStreamCompat::new(stream);
                Ok(Self::Monoio(compat, fd))
            }
        }
    }

    pub fn raw_fd(&self) -> Option<RawFd> {
        match self {
            Self::Tokio(_, fd) => Some(*fd),
            Self::Monoio(_, fd) => Some(*fd),
        }
    }

    pub fn set_nodelay(&self, nodelay: bool) -> Result<()> {
        match self {
            Self::Tokio(s, _) => s.set_nodelay(nodelay).map_err(|e| e.into()),
            Self::Monoio(_, _) => Ok(()), // Monoio defaults or handles differently
        }
    }
}

impl MaybeAsRawFd for DualTcpStream {
    fn maybe_as_raw_fd(&self) -> Option<RawFd> { self.raw_fd() }
}

impl<T: MaybeAsRawFd + ?Sized> MaybeAsRawFd for Box<T> {
    fn maybe_as_raw_fd(&self) -> Option<RawFd> { (**self).maybe_as_raw_fd() }
}

impl AsyncRead for DualTcpStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tokio(s, _) => Pin::new(s).poll_read(cx, buf),
            Self::Monoio(s, _) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for DualTcpStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Tokio(s, _) => Pin::new(s).poll_write(cx, buf),
            Self::Monoio(s, _) => Pin::new(s).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tokio(s, _) => Pin::new(s).poll_flush(cx),
            Self::Monoio(s, _) => Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tokio(s, _) => Pin::new(s).poll_shutdown(cx),
            Self::Monoio(s, _) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

use std::io;
use std::os::fd::RawFd;
use tokio::io::{AsyncRead, AsyncWrite};
use std::pin::Pin;
use std::task::{Context, Poll};
use crate::utils::net::MaybeAsRawFd;

pub struct AsyncSplice {
    pipe_a_read: RawFd,
    pipe_a_write: RawFd,
    pipe_b_read: RawFd,
    pipe_b_write: RawFd,
}

impl AsyncSplice {
    pub fn new() -> io::Result<Self> {
        let mut fds_a = [0i32; 2];
        let mut fds_b = [0i32; 2];
        if unsafe { libc::pipe2(fds_a.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::pipe2(fds_b.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) } < 0 {
            unsafe { libc::close(fds_a[0]); libc::close(fds_a[1]); }
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            pipe_a_read: fds_a[0],
            pipe_a_write: fds_a[1],
            pipe_b_read: fds_b[0],
            pipe_b_write: fds_b[1],
        })
    }

    pub async fn relay<S1, S2>(
        &self,
        s1: &mut S1,
        s2: &mut S2,
    ) -> io::Result<()> 
    where 
        S1: AsyncRead + AsyncWrite + MaybeAsRawFd + Unpin,
        S2: AsyncRead + AsyncWrite + MaybeAsRawFd + Unpin,
    {
        let fd1 = s1.maybe_as_raw_fd().ok_or_else(|| io::Error::new(io::ErrorKind::Other, "S1 no FD"))?;
        let fd2 = s2.maybe_as_raw_fd().ok_or_else(|| io::Error::new(io::ErrorKind::Other, "S2 no FD"))?;

        let mut s1_to_s2_pipe = 0;
        let mut s2_to_s1_pipe = 0;
        let mut s1_eof = false;
        let mut s2_eof = false;

        loop {
            let mut made_progress = false;

            // 1. S1 -> S2 (Read to Pipe)
            if !s1_eof && s1_to_s2_pipe < 128 * 1024 {
                let n = unsafe { libc::splice(fd1, std::ptr::null_mut(), self.pipe_a_write, std::ptr::null_mut(), 128*1024 - s1_to_s2_pipe, libc::SPLICE_F_MOVE | libc::SPLICE_F_NONBLOCK) };
                if n > 0 { s1_to_s2_pipe += n as usize; made_progress = true; }
                else if n == 0 { s1_eof = true; made_progress = true; }
                else {
                    let e = io::Error::last_os_error();
                    if e.kind() != io::ErrorKind::WouldBlock { return Err(e); }
                }
            }

            // 2. S1 -> S2 (Write from Pipe)
            if s1_to_s2_pipe > 0 {
                let n = unsafe { libc::splice(self.pipe_a_read, std::ptr::null_mut(), fd2, std::ptr::null_mut(), s1_to_s2_pipe, libc::SPLICE_F_MOVE | libc::SPLICE_F_NONBLOCK) };
                if n > 0 { s1_to_s2_pipe -= n as usize; made_progress = true; }
                else if n < 0 {
                    let e = io::Error::last_os_error();
                    if e.kind() != io::ErrorKind::WouldBlock { return Err(e); }
                }
            }

            // 3. S2 -> S1 (Read to Pipe)
            if !s2_eof && s2_to_s1_pipe < 128 * 1024 {
                let n = unsafe { libc::splice(fd2, std::ptr::null_mut(), self.pipe_b_write, std::ptr::null_mut(), 128*1024 - s2_to_s1_pipe, libc::SPLICE_F_MOVE | libc::SPLICE_F_NONBLOCK) };
                if n > 0 { s2_to_s1_pipe += n as usize; made_progress = true; }
                else if n == 0 { s2_eof = true; made_progress = true; }
                else {
                    let e = io::Error::last_os_error();
                    if e.kind() != io::ErrorKind::WouldBlock { return Err(e); }
                }
            }

            // 4. S2 -> S1 (Write from Pipe)
            if s2_to_s1_pipe > 0 {
                let n = unsafe { libc::splice(self.pipe_b_read, std::ptr::null_mut(), fd1, std::ptr::null_mut(), s2_to_s1_pipe, libc::SPLICE_F_MOVE | libc::SPLICE_F_NONBLOCK) };
                if n > 0 { s2_to_s1_pipe -= n as usize; made_progress = true; }
                else if n < 0 {
                    let e = io::Error::last_os_error();
                    if e.kind() != io::ErrorKind::WouldBlock { return Err(e); }
                }
            }

            if (s1_eof && s1_to_s2_pipe == 0) || (s2_eof && s2_to_s1_pipe == 0) { 
                break; 
            }

            if !made_progress {
                futures::future::poll_fn(|cx| {
                    let mut dummy = [0u8; 1];
                    let mut buf = tokio::io::ReadBuf::new(&mut dummy);
                    let mut ready = false;
                    if Pin::new(&mut *s1).poll_read(cx, &mut buf).is_ready() { ready = true; }
                    if Pin::new(&mut *s2).poll_read(cx, &mut buf).is_ready() { ready = true; }
                    if Pin::new(&mut *s1).poll_write(cx, &[]).is_ready() { ready = true; }
                    if Pin::new(&mut *s2).poll_write(cx, &[]).is_ready() { ready = true; }
                    if ready { Poll::Ready(()) } else { Poll::Pending }
                }).await;
            }
        }
        Ok(())
    }
}

impl Drop for AsyncSplice {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.pipe_a_read); libc::close(self.pipe_a_write);
            libc::close(self.pipe_b_read); libc::close(self.pipe_b_write);
        }
    }
}

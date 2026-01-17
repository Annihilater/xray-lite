use std::io;
use std::os::fd::RawFd;
use crate::utils::net::MaybeAsRawFd;

pub struct AsyncSplice {
    pipe_read: RawFd,
    pipe_write: RawFd,
}

impl AsyncSplice {
    pub fn new() -> io::Result<Self> {
        let mut fds = [0i32; 2];
        let res = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK) };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            pipe_read: fds[0],
            pipe_write: fds[1],
        })
    }

    pub async fn transfer<S: MaybeAsRawFd, D: MaybeAsRawFd>(
        &self,
        src: &S,
        dst: &D,
    ) -> io::Result<u64> {
        let mut total = 0;
        let s_fd = src.maybe_as_raw_fd().ok_or_else(|| io::Error::new(io::ErrorKind::Other, "No Src FD"))?;
        let d_fd = dst.maybe_as_raw_fd().ok_or_else(|| io::Error::new(io::ErrorKind::Other, "No Dst FD"))?;

        loop {
            let n = unsafe {
                libc::splice(
                    s_fd,
                    std::ptr::null_mut(),
                    self.pipe_write,
                    std::ptr::null_mut(),
                    128 * 1024,
                    libc::SPLICE_F_MOVE | libc::SPLICE_F_NONBLOCK,
                )
            };

            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock {
                    tokio::task::yield_now().await;
                    continue;
                }
                return Err(err);
            }

            if n == 0 { break; } 

            let mut remaining = n as usize;
            while remaining > 0 {
                let m = unsafe {
                    libc::splice(
                        self.pipe_read,
                        std::ptr::null_mut(),
                        d_fd,
                        std::ptr::null_mut(),
                        remaining,
                        libc::SPLICE_F_MOVE | libc::SPLICE_F_NONBLOCK,
                    )
                };

                if m < 0 {
                    let err = io::Error::last_os_error();
                    if err.kind() == io::ErrorKind::WouldBlock {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    return Err(err);
                }
                
                remaining -= m as usize;
                total += m as u64;
            }
        }
        Ok(total)
    }
}

impl Drop for AsyncSplice {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.pipe_read);
            libc::close(self.pipe_write);
        }
    }
}

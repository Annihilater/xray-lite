use std::io;
use std::os::fd::RawFd;

#[allow(dead_code)]
pub struct SpliceGate {
    pipe_read: RawFd,
    pipe_write: RawFd,
}

#[allow(dead_code)]
impl SpliceGate {
    pub fn new() -> io::Result<Self> {
        let mut fds = [0i32; 2];
        let res = unsafe { libc::pipe(fds.as_mut_ptr()) };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            pipe_read: fds[0],
            pipe_write: fds[1],
        })
    }

    pub fn relay(&self, src: RawFd, dst: RawFd, len: usize) -> io::Result<usize> {
        let flags = libc::SPLICE_F_MOVE | libc::SPLICE_F_NONBLOCK;

        // Splice from src to pipe
        let n = unsafe {
            libc::splice(
                src,
                std::ptr::null_mut(),
                self.pipe_write,
                std::ptr::null_mut(),
                len,
                flags,
            )
        };

        if n < 0 {
            return Err(io::Error::last_os_error());
        }

        let n = n as usize;
        if n == 0 {
            return Ok(0);
        }

        // Splice from pipe to dst
        let m = unsafe {
            libc::splice(
                self.pipe_read,
                std::ptr::null_mut(),
                dst,
                std::ptr::null_mut(),
                n,
                flags,
            )
        };

        if m < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(m as usize)
    }
}

impl Drop for SpliceGate {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.pipe_read);
            libc::close(self.pipe_write);
        }
    }
}

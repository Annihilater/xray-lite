use std::time::Duration;
use std::future::Future;
use crate::utils::task::{get_runtime_mode, RuntimeMode};

pub async fn timeout<T, F>(duration: Duration, future: F) -> std::io::Result<T>
where F: Future<Output = T> {
    match get_runtime_mode() {
        RuntimeMode::Tokio => {
            match tokio::time::timeout(duration, future).await {
                Ok(v) => Ok(v),
                Err(_) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout")),
            }
        },
        RuntimeMode::Monoio => {
            match monoio::time::timeout(duration, future).await {
                Ok(v) => Ok(v),
                Err(_) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout")),
            }
        }
    }
}

pub async fn sleep(duration: Duration) {
    match get_runtime_mode() {
        RuntimeMode::Tokio => tokio::time::sleep(duration).await,
        RuntimeMode::Monoio => monoio::time::sleep(duration).await,
    }
}

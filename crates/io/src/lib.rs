//! Bounded native I/O shared by compiled-in device adapters.
//!
//! Executables, arguments and socket paths come from each adapter's local
//! configuration and typed operation, never from a remote command string.

mod local;
mod process;

pub use local::LocalChannel;
pub use process::{ProcessOutput, ProcessRequest};
pub use tokio_util::sync::CancellationToken;

use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IoError {
    #[error("native operation configuration is invalid")]
    InvalidConfiguration,
    #[error("native operation capacity is exhausted")]
    Busy,
    #[error("native provider is unavailable")]
    Unavailable,
    #[error("native operation timed out")]
    Timeout,
    #[error("native operation was cancelled")]
    Cancelled,
    #[error("native response exceeds its byte limit")]
    OutputLimit,
    #[error("native response framing is invalid")]
    InvalidFrame,
    #[error("native provider response is invalid")]
    InvalidResponse,
    #[error("native provider returned an unsuccessful exit status")]
    ProviderExit,
    #[error("native I/O failed")]
    Transport,
}

impl IoError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidConfiguration => "invalid_configuration",
            Self::Busy => "busy",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::OutputLimit => "output_limit",
            Self::InvalidFrame => "invalid_frame",
            Self::InvalidResponse => "invalid_response",
            Self::ProviderExit => "provider_exit",
            Self::Transport => "transport",
        }
    }
}

fn transport(error: std::io::Error) -> IoError {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::NotFound | ErrorKind::ConnectionRefused => IoError::Unavailable,
        ErrorKind::TimedOut | ErrorKind::WouldBlock => IoError::Timeout,
        _ => IoError::Transport,
    }
}

/// One admission budget per agent, shared by all its native adapters.
/// Admission rejects immediately; it never creates an unbounded wait queue.
#[derive(Clone)]
pub struct NativeIo {
    permits: Arc<Semaphore>,
}

impl NativeIo {
    pub fn new(concurrency: usize) -> Result<Self, IoError> {
        if !(1..=16).contains(&concurrency) {
            return Err(IoError::InvalidConfiguration);
        }
        Ok(Self {
            permits: Arc::new(Semaphore::new(concurrency)),
        })
    }

    pub async fn local<T, F>(
        &self,
        cancellation: CancellationToken,
        operation: F,
    ) -> Result<T, IoError>
    where
        T: Send + 'static,
        F: FnOnce(CancellationToken) -> Result<T, IoError> + Send + 'static,
    {
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| IoError::Busy)?;
        let cancellation = cancellation.child_token();
        let cancel_on_drop = cancellation.clone().drop_guard();
        // LocalChannel checks cancellation and a single absolute deadline.
        // Keep the permit inside the worker until its socket actually closes.
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            operation(cancellation)
        })
        .await
        .map_err(|_| IoError::Transport)?;
        cancel_on_drop.disarm();
        result
    }
}

fn valid_timeout(value: Duration) -> bool {
    (Duration::from_millis(1)..=Duration::from_secs(30)).contains(&value)
}

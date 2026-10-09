use crate::{CancellationToken, IoError, NativeIo, transport, valid_timeout};
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use std::{
    ffi::OsString,
    path::PathBuf,
    process::{ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, AsyncReadExt};

pub struct ProcessRequest {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    /// Explicit repository scope, or the executable's directory when absent.
    pub directory: Option<PathBuf>,
    pub timeout: Duration,
    /// Combined stdout + stderr ceiling, at most 1 MiB.
    pub max_output_bytes: usize,
}

// Deliberately no Debug: tool output can contain private paths and account data.
pub struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

struct OwnedChild {
    child: Box<dyn ChildWrapper>,
    completed: bool,
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.completed {
            // ProcessGroup/JobObject kills the owned group. Tokio kill-on-drop
            // also ensures the direct child is reaped if the future is aborted.
            let _ = self.child.start_kill();
        }
    }
}

impl NativeIo {
    /// Decode a complete native response only after its owning adapter's exit
    /// policy accepts it. Never treat a truncated or failed command as success.
    pub async fn json<T: serde::de::DeserializeOwned>(
        &self,
        request: ProcessRequest,
        accepted_exit_codes: &[i32],
        cancellation: CancellationToken,
    ) -> Result<(T, i32), IoError> {
        let output = self.process(request, cancellation).await?;
        let code = output.status.code().ok_or(IoError::ProviderExit)?;
        if !accepted_exit_codes.contains(&code) {
            return Err(IoError::ProviderExit);
        }
        let value = serde_json::from_slice(&output.stdout).map_err(|_| IoError::InvalidResponse)?;
        Ok((value, code))
    }

    pub async fn process(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
    ) -> Result<ProcessOutput, IoError> {
        let started = Instant::now();
        let result = self.run_process(request, cancellation).await;
        match &result {
            Ok(output) => tracing::info!(
                event.name = "native.process.completed",
                duration_ms = started.elapsed().as_millis() as u64,
                outcome = if output.status.success() {
                    "success"
                } else {
                    "provider_error"
                },
            ),
            Err(error) => tracing::warn!(
                event.name = "native.process.failed",
                duration_ms = started.elapsed().as_millis() as u64,
                error.type = error.code(),
                outcome = "failed",
            ),
        }
        result
    }

    async fn run_process(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
    ) -> Result<ProcessOutput, IoError> {
        if !request.executable.is_absolute()
            || request
                .directory
                .as_ref()
                .is_some_and(|path| !path.is_absolute())
            || !valid_timeout(request.timeout)
            || !(1..=1_048_576).contains(&request.max_output_bytes)
            || request.arguments.len() > 32
            || request.arguments.iter().any(|arg| arg.len() > 4096)
            || request.arguments.iter().map(|arg| arg.len()).sum::<usize>() > 16_384
        {
            return Err(IoError::InvalidConfiguration);
        }
        if cancellation.is_cancelled() {
            return Err(IoError::Cancelled);
        }
        let _permit = self.permits.try_acquire().map_err(|_| IoError::Busy)?;
        let directory = request
            .directory
            .as_deref()
            .or_else(|| request.executable.parent())
            .ok_or(IoError::InvalidConfiguration)?;
        let mut command = CommandWrap::with_new(&request.executable, |command| {
            command
                .args(&request.arguments)
                .env_clear()
                .envs(native_environment())
                .current_dir(directory)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
        });
        command.wrap(KillOnDrop);
        #[cfg(unix)]
        command.wrap(process_wrap::tokio::ProcessGroup::leader());
        #[cfg(windows)]
        command.wrap(process_wrap::tokio::JobObject);
        let mut owned = OwnedChild {
            child: command.spawn().map_err(transport)?,
            completed: false,
        };
        let stdout = owned.child.stdout().take().ok_or(IoError::Transport)?;
        let stderr = owned.child.stderr().take().ok_or(IoError::Transport)?;
        let bytes = Arc::new(AtomicUsize::new(0));
        let result = {
            let capture = async {
                tokio::try_join!(
                    async { owned.child.wait().await.map_err(transport) },
                    bounded_read(stdout, bytes.clone(), request.max_output_bytes),
                    bounded_read(stderr, bytes, request.max_output_bytes),
                )
            };
            tokio::select! {
                biased;
                () = cancellation.cancelled() => Err(IoError::Cancelled),
                result = tokio::time::timeout(request.timeout, capture) => {
                    result.unwrap_or(Err(IoError::Timeout))
                }
            }
        };
        match result {
            Ok((status, stdout, stderr)) => {
                owned.completed = true;
                Ok(ProcessOutput {
                    status,
                    stdout,
                    stderr,
                })
            }
            Err(error) => {
                let _ = owned.child.start_kill();
                // Do not hang indefinitely while reaping an unresponsive child.
                if matches!(
                    tokio::time::timeout(Duration::from_secs(1), owned.child.wait()).await,
                    Ok(Ok(_))
                ) {
                    owned.completed = true;
                }
                Err(error)
            }
        }
    }
}

/// Native user/session discovery only. Do not inherit provider API tokens,
/// loader overrides, shell startup hooks or Git directory/index redirections.
fn native_environment() -> impl Iterator<Item = (&'static str, OsString)> {
    [
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "LANG",
        "LANGUAGE",
        "LC_ALL",
        "LC_CTYPE",
        "TMPDIR",
        "TMP",
        "TEMP",
        "XDG_RUNTIME_DIR",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "DBUS_SESSION_BUS_ADDRESS",
        "SystemRoot",
        "SYSTEMROOT",
        "WINDIR",
        "USERPROFILE",
        "USERNAME",
        "LOCALAPPDATA",
        "APPDATA",
        "PROGRAMDATA",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        // These references select native owner state/preservation policy.
        "RLDYOUR_UPDATER_CONFIG",
        "RLDYOUR_UPDATER_STATE",
        "GDS_ESTATE_ROOT",
        "GDS_TRUST_POLICY_FILE",
        "UV_CACHE_DIR",
        "UV_LINK_MODE",
    ]
    .into_iter()
    .filter_map(|name| std::env::var_os(name).map(|value| (name, value)))
}

async fn bounded_read(
    mut stream: impl AsyncRead + Unpin,
    total: Arc<AtomicUsize>,
    limit: usize,
) -> Result<Vec<u8>, IoError> {
    let mut output = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let count = stream.read(&mut chunk).await.map_err(transport)?;
        if count == 0 {
            return Ok(output);
        }
        if total
            .fetch_add(count, Ordering::Relaxed)
            .saturating_add(count)
            > limit
        {
            return Err(IoError::OutputLimit);
        }
        output.extend_from_slice(&chunk[..count]);
    }
}

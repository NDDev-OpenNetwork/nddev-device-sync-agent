use crate::view::{AgentError, ModuleState, Reachability};
use nddev_device_sync_adapter_io::{CancellationToken, IoError, NativeIo};
use nddev_device_sync_clipboard::{Clipboard, ReadClipboard};
use nddev_device_sync_domain::{ModuleDescriptor, ModuleGraph};
use nddev_device_sync_sysinfo::{ReadSystemInfo, SystemInfo};
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tracing::Instrument;

use nddev_device_sync_cleaner::Cleaner;
use nddev_device_sync_gds::Gds;
use nddev_device_sync_rds::Rds;
use nddev_device_sync_updater::Updater;

pub struct NativeAgent {
    modules: Mutex<Vec<ModuleState>>,
    cancellation: CancellationToken,
    sysinfo: Option<SystemInfo>,
    clipboard: Option<Clipboard>,
    cleaner: Option<Cleaner>,
    updater: Option<Updater>,
    gds: Option<Gds>,
    rds: Option<Rds>,
}

impl Drop for NativeAgent {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl NativeAgent {
    pub fn discover() -> Result<Self, AgentError> {
        let io = NativeIo::new(8).map_err(AgentError::from)?;
        let sysinfo = SystemInfo::for_current_user(io.local_handle()).ok();
        let clipboard = Clipboard::for_current_user(io.local_handle()).ok();
        let cleaner = executable("rldyour-cleaner")
            .map(|path| Cleaner::new(io.process_handle(), path))
            .transpose()?;
        let updater = executable("rldyour-updater")
            .map(|path| Updater::new(io.process_handle(), path))
            .transpose()?;
        let gds = executable("gds")
            .map(|path| Gds::new(io.process_handle(), path))
            .transpose()?;
        let rds = executable("rds")
            .map(|path| Rds::new(io.process_handle(), path, None))
            .transpose()?;
        let mut graph = ModuleGraph::default();
        for manifest in [
            nddev_device_sync_sysinfo::MANIFEST,
            nddev_device_sync_clipboard::MANIFEST,
            nddev_device_sync_cleaner::MANIFEST,
            nddev_device_sync_updater::MANIFEST,
            nddev_device_sync_gds::MANIFEST,
            nddev_device_sync_rds::MANIFEST,
        ] {
            let descriptor: ModuleDescriptor =
                serde_json::from_str(manifest).map_err(|_| AgentError::InvalidContract)?;
            graph
                .register(descriptor)
                .map_err(|_| AgentError::InvalidContract)?;
        }
        graph.validate().map_err(|_| AgentError::InvalidContract)?;
        let modules = graph
            .iter()
            .map(|descriptor| ModuleState {
                id: descriptor.id.as_str().into(),
                version: descriptor.version.clone(),
                configured: match descriptor.id.as_str() {
                    "sysinfo" => sysinfo.is_some(),
                    "clipboard" => clipboard.is_some(),
                    "cleaner" => cleaner.is_some(),
                    "updater" => updater.is_some(),
                    "gds" => gds.is_some(),
                    "rds" => rds.is_some(),
                    _ => false,
                },
                reachability: Reachability::NotChecked,
                observed_at_ms: None,
                error: None,
            })
            .collect();
        Ok(Self {
            modules: Mutex::new(modules),
            cancellation: CancellationToken::new(),
            sysinfo,
            clipboard,
            cleaner,
            updater,
            gds,
            rds,
        })
    }

    pub fn modules(&self) -> Vec<ModuleState> {
        self.modules
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// Terminal shutdown; constructing a new host is an explicit new lifetime.
    pub fn close(&self) {
        self.cancellation.cancel();
    }

    async fn observe<T>(
        &self,
        module: &'static str,
        work: impl Future<Output = Result<T, AgentError>>,
    ) -> Result<T, AgentError> {
        async {
            if self.cancellation.is_cancelled() { return Err(AgentError::Cancelled); }
            let started = Instant::now();
            let result = work.await;
            // Caller/admission failures are not new evidence about the provider.
            let observed = !matches!(result, Err(AgentError::Busy | AgentError::InvalidInput | AgentError::Cancelled | AgentError::UnsupportedPlatform | AgentError::InvalidContract));
            if observed {
                let observed_at_ms = SystemTime::now().duration_since(UNIX_EPOCH).ok().and_then(|value| value.as_millis().try_into().ok());
                if let Some(state) = self.modules.lock().unwrap_or_else(|error| error.into_inner()).iter_mut().find(|state| state.id == module)
                    && state.configured
                {
                    state.observed_at_ms = observed_at_ms;
                    state.error = result.as_ref().err().copied();
                    state.reachability = match &result { Ok(_) => Reachability::Reachable, Err(AgentError::Unavailable) => Reachability::Unavailable, Err(_) => Reachability::Failed };
                }
            }
            match &result {
                Ok(_) => tracing::info!(event.name = "module.query.completed", module, duration_ms = started.elapsed().as_millis() as u64, outcome = "ok"),
                Err(error) => tracing::warn!(event.name = "module.query.failed", module, duration_ms = started.elapsed().as_millis() as u64, error.type = %error, outcome = "error"),
            }
            result
        }.instrument(nddev_device_sync_telemetry::operation_span(module, "io").map_err(|_| AgentError::InvalidContract)?).await
    }

    pub async fn system_info(
        &self,
    ) -> Result<nddev_device_sync_sysinfo::view::SystemSnapshot, AgentError> {
        self.observe("sysinfo", async {
            self.sysinfo
                .as_ref()
                .ok_or(AgentError::Unavailable)?
                .sample(self.cancellation.clone())
                .await
                .map_err(|error| match error {
                    nddev_device_sync_sysinfo::Error::Io(error) => error.into(),
                    nddev_device_sync_sysinfo::Error::UnsupportedPlatform => {
                        AgentError::UnsupportedPlatform
                    }
                    _ => AgentError::InvalidResponse,
                })
        })
        .await
    }

    pub async fn clipboard_entries(
        &self,
        before: Option<u64>,
    ) -> Result<nddev_device_sync_clipboard::view::Page, AgentError> {
        self.observe("clipboard", async {
            self.clipboard
                .as_ref()
                .ok_or(AgentError::Unavailable)?
                .list(before, self.cancellation.clone())
                .await
                .map_err(clipboard_error)
        })
        .await
    }

    pub async fn clipboard_content(&self, entry: u64, mime: String) -> Result<Vec<u8>, AgentError> {
        self.observe("clipboard", async {
            self.clipboard
                .as_ref()
                .ok_or(AgentError::Unavailable)?
                .fetch(entry, mime, self.cancellation.clone())
                .await
                .map_err(clipboard_error)
        })
        .await
    }

    pub async fn cleaner_diagnostics(
        &self,
    ) -> Result<nddev_device_sync_cleaner::Diagnostics, AgentError> {
        self.observe("cleaner", async {
            Ok(self
                .cleaner
                .as_ref()
                .ok_or(AgentError::Unavailable)?
                .diagnose(self.cancellation.clone())
                .await?)
        })
        .await
    }

    pub async fn cleaner_preview(
        &self,
    ) -> Result<nddev_device_sync_cleaner::CleanerRunSummary, AgentError> {
        self.observe("cleaner", async {
            Ok(self
                .cleaner
                .as_ref()
                .ok_or(AgentError::Unavailable)?
                .preview(self.cancellation.clone())
                .await?)
        })
        .await
    }

    pub async fn cleaner_last_run(
        &self,
    ) -> Result<nddev_device_sync_cleaner::CleanerRunSummary, AgentError> {
        self.observe("cleaner", async {
            Ok(self
                .cleaner
                .as_ref()
                .ok_or(AgentError::Unavailable)?
                .last_run(self.cancellation.clone())
                .await?)
        })
        .await
    }

    pub async fn update_plan(&self) -> Result<nddev_device_sync_updater::Plan, AgentError> {
        self.observe("updater", async {
            Ok(self
                .updater
                .as_ref()
                .ok_or(AgentError::Unavailable)?
                .plan(self.cancellation.clone())
                .await?)
        })
        .await
    }

    pub async fn update_last_run(
        &self,
    ) -> Result<nddev_device_sync_updater::UpdateRunSummary, AgentError> {
        self.observe("updater", async {
            Ok(self
                .updater
                .as_ref()
                .ok_or(AgentError::Unavailable)?
                .last_run(self.cancellation.clone())
                .await?)
        })
        .await
    }

    pub async fn repository_status(
        &self,
        path: &Path,
    ) -> Result<nddev_device_sync_gds::Status, AgentError> {
        self.observe("gds", async {
            Ok(self
                .gds
                .as_ref()
                .ok_or(AgentError::Unavailable)?
                .status(path, self.cancellation.clone())
                .await?)
        })
        .await
    }

    pub async fn remote_sessions(
        &self,
    ) -> Result<nddev_device_sync_rds::RemoteSnapshot, AgentError> {
        self.observe("rds", async {
            Ok(self
                .rds
                .as_ref()
                .ok_or(AgentError::Unavailable)?
                .sessions(self.cancellation.clone())
                .await?)
        })
        .await
    }
}

impl From<IoError> for AgentError {
    fn from(error: IoError) -> Self {
        match error {
            IoError::Busy => Self::Busy,
            IoError::Unavailable => Self::Unavailable,
            IoError::InvalidConfiguration => Self::InvalidInput,
            IoError::Timeout => Self::Timeout,
            IoError::Cancelled => Self::Cancelled,
            IoError::ProviderExit | IoError::Transport => Self::ProviderFailed,
            IoError::OutputLimit | IoError::InvalidFrame | IoError::InvalidResponse => {
                Self::InvalidResponse
            }
        }
    }
}

fn clipboard_error(error: nddev_device_sync_clipboard::Error) -> AgentError {
    match error {
        nddev_device_sync_clipboard::Error::Io(error) => error.into(),
        nddev_device_sync_clipboard::Error::Protocol => AgentError::InvalidResponse,
        nddev_device_sync_clipboard::Error::Provider => AgentError::ProviderFailed,
    }
}

fn executable(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path).filter(|path| path.is_absolute()) {
        let candidate = directory.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.into()
        });
        let Ok(metadata) = candidate.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                continue;
            }
        }
        return Some(candidate);
    }
    None
}

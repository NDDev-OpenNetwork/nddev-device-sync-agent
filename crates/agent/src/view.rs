/// Portable presentation metadata. Reachability describes the last native
/// query, not the success of historical maintenance or a remote connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reachability {
    NotChecked,
    Reachable,
    Unavailable,
    Failed,
}

#[derive(Clone, Debug)]
pub struct ModuleState {
    pub id: String,
    pub version: String,
    pub configured: bool,
    pub reachability: Reachability,
    pub observed_at_ms: Option<u64>,
    pub error: Option<AgentError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AgentError {
    #[error("unsupported_platform")]
    UnsupportedPlatform,
    #[error("provider_unavailable")]
    Unavailable,
    #[error("operation_busy")]
    Busy,
    #[error("invalid_input")]
    InvalidInput,
    #[error("operation_timeout")]
    Timeout,
    #[error("operation_cancelled")]
    Cancelled,
    #[error("provider_failed")]
    ProviderFailed,
    #[error("invalid_response")]
    InvalidResponse,
    #[error("invalid_module_contract")]
    InvalidContract,
}

pub use nddev_device_sync_cleaner::view as cleaner;
pub use nddev_device_sync_clipboard::view as clipboard;
pub use nddev_device_sync_gds::view as gds;
pub use nddev_device_sync_rds::view as rds;
pub use nddev_device_sync_sysinfo::view as sysinfo;
pub use nddev_device_sync_updater::view as updater;

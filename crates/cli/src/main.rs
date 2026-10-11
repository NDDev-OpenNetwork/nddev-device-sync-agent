//! Agent point-of-use interface. Session, vault keys and provider credentials stay
//! native; child output is discarded and never forwarded to chat or telemetry.
use nddev_device_sync_accounts::IdentityClient;
use nddev_device_sync_adapter_io::{
    CancellationToken, CredentialEnvironment, NativeIo, ProcessInput, ProcessRequest,
};
use nddev_device_sync_vault::VaultClient;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zeroize::Zeroizing;

fn data_directory() -> Result<PathBuf, &'static str> {
    #[cfg(target_os = "linux")]
    let root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")));
    #[cfg(target_os = "macos")]
    let root = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support"));
    #[cfg(target_os = "windows")]
    let root = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let root = root.ok_or("native_directory_unavailable")?;
    if !root.is_absolute() {
        return Err("invalid_native_directory");
    }
    Ok(root.join("nddev-device-sync"))
}
fn executable(name: &str) -> Result<PathBuf, &'static str> {
    let paths = std::env::var_os("PATH").ok_or("provider_unavailable")?;
    #[cfg(target_os = "windows")]
    let name = format!("{name}.exe");
    #[cfg(target_os = "windows")]
    let name = name.as_str();
    for directory in std::env::split_paths(&paths)
        .take(64)
        .filter(|directory| directory.is_absolute())
    {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return std::fs::canonicalize(candidate).map_err(|_| "provider_unavailable");
        }
    }
    Err("provider_unavailable")
}
async fn operation(args: &[String]) -> Result<Value, &'static str> {
    let [flag, endpoint, scope, command, remaining @ ..] = args else {
        return Err(
            "usage: nds-agent --server URL vault list [AFTER_REF] | use RECORD_REF github|cloudflare|digitalocean",
        );
    };
    if flag != "--server"
        || scope != "vault"
        || args.len() > 6
        || args.iter().any(|value| value.len() > 2048)
    {
        return Err("invalid_arguments");
    }
    if command != "list" && command != "use" {
        return Err("unsupported_operation");
    }
    let identity = Arc::new(IdentityClient::new(endpoint).map_err(|_| "invalid_server")?);
    let view = identity
        .refresh()
        .await
        .map_err(|_| "identity_unavailable")?;
    if !view.authenticated || !view.session_verified {
        return Err("sign_in_required");
    }
    identity
        .prepare_device("NDS agent".into())
        .await
        .map_err(|_| "device_unavailable")?;
    let vault = VaultClient::new(identity, data_directory()?)
        .await
        .map_err(|_| "vault_unavailable")?;
    let mut current = false;
    for _ in 0..16 {
        match vault.synchronize().await {
            Ok(view) if !view.catching_up => {
                current = true;
                break;
            }
            Ok(_) | Err(nddev_device_sync_vault::Error::Busy) => {
                tokio::time::sleep(Duration::from_millis(100)).await
            }
            Err(_) => return Err("vault_unavailable"),
        }
    }
    if !current {
        return Err("replica_catching_up");
    }
    match (command.as_str(), remaining) {
        ("list", [] | [_]) => {
            let after = remaining.first().map_or("", String::as_str);
            let records = vault.records().await.map_err(|_| "vault_unavailable")?;
            let mut records = records
                .into_iter()
                .filter(|record| record.id.as_str() > after)
                .take(51)
                .collect::<Vec<_>>();
            let has_more = records.len() > 50;
            records.truncate(50);
            let next = if has_more {
                records
                    .last()
                    .map(|record| record.id.as_str())
                    .unwrap_or("")
            } else {
                ""
            };
            Ok(
                json!({"operation":"vault.list","records":records.iter().map(|record|json!({"ref":record.id,"title":record.title,"revision":record.revision,"conflicted":record.conflicted})).collect::<Vec<_>>(),"next_ref":next}),
            )
        }
        ("use", [record, provider]) => {
            let (name, arguments, environment) = match provider.as_str() {
                "github" => (
                    "gh",
                    vec!["api", "user", "--hostname", "github.com"],
                    CredentialEnvironment::GitHub,
                ),
                "cloudflare" => (
                    "wrangler",
                    vec!["whoami", "--json"],
                    CredentialEnvironment::Cloudflare,
                ),
                "digitalocean" => (
                    "doctl",
                    vec!["account", "get", "--output", "json"],
                    CredentialEnvironment::DigitalOcean,
                ),
                _ => return Err("unsupported_provider"),
            };
            let token = vault
                .native_secret(record.clone())
                .await
                .map_err(|_| "secret_reference_unavailable")?;
            let output = NativeIo::new(1)
                .map_err(|_| "native_io_unavailable")?
                .process_handle()
                .process_input(
                    ProcessRequest {
                        executable: executable(name)?,
                        arguments: arguments.into_iter().map(Into::into).collect(),
                        directory: None,
                        timeout: Duration::from_secs(20),
                        max_output_bytes: 65_536,
                    },
                    ProcessInput {
                        stdin: Zeroizing::new(Vec::new()),
                        credential: Some((environment, token)),
                    },
                    CancellationToken::new(),
                )
                .await
                .map_err(|_| "provider_unavailable")?;
            let success = output.status.success();
            let _stdout = Zeroizing::new(output.stdout);
            let _stderr = Zeroizing::new(output.stderr);
            if !success {
                return Err("provider_credential_rejected");
            }
            Ok(json!({"operation":"vault.use","provider":provider,"verified":true}))
        }
        _ => Err("invalid_arguments"),
    }
}
#[tokio::main]
async fn main() {
    let mut logger = nddev_device_sync_telemetry::install(nddev_device_sync_telemetry::Config {
        service: "nddev-device-sync-agent".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        repository: "NDDev-OpenNetwork/nddev-device-sync-agent".into(),
        commit: option_env!("NDS_SOURCE_COMMIT").unwrap_or("unknown").into(),
        channel: "alpha".into(),
        standards: "unreleased".into(),
        environment: "client".into(),
        target_prefixes: vec![
            "nds_agent".into(),
            "nds_accounts".into(),
            "nddev_device_sync_adapter_io".into(),
        ],
        default_module: "agent".into(),
        debug: None,
    })
    .ok();
    let arguments = std::env::args().skip(1).take(8).collect::<Vec<_>>();
    let result = tokio::time::timeout(Duration::from_secs(120), operation(&arguments))
        .await
        .unwrap_or(Err("operation_timeout"));
    match result {
        Ok(value) => {
            tracing::info!(event.name = "agent.operation.completed", outcome = "ok");
            println!("{value}");
        }
        Err(error) => {
            tracing::warn!(event.name="agent.operation.failed",error.type="agent_operation",outcome="error");
            println!("{}", json!({"error":error}));
            if let Some(logger) = &mut logger {
                logger.shutdown();
            }
            std::process::exit(2);
        }
    }
}

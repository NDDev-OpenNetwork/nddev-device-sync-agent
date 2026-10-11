//! Real Secret Service/PostgreSQL/GitHub acceptance; no provider-success stub.
use nddev_device_sync_accounts::IdentityClient;
use nddev_device_sync_adapter_io::{CancellationToken, NativeIo, ProcessRequest};
use nddev_device_sync_vault::VaultClient;
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use zeroize::Zeroizing;
#[tokio::test]
#[ignore = "requires an owned fixture and already-authorized official GitHub CLI"]
async fn actual_native_vault_reference_drives_official_github_cli() {
    let data: Value = serde_json::from_slice(
        &std::fs::read(std::env::var("NDS_FIXTURE_RECEIPT").unwrap()).unwrap(),
    )
    .unwrap();
    let endpoint = data["base_url"].as_str().unwrap();
    assert!(endpoint.starts_with("http://localhost:"));
    let identity = Arc::new(IdentityClient::new(endpoint).unwrap());
    identity.refresh().await.unwrap();
    let mailbox = data["mailpit_url"].as_str().unwrap();
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let before: Value = http
        .get(format!("{mailbox}/api/v1/messages"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let before = before["messages"].as_array().unwrap().len();
    identity
        .request_email(data["owner_email"].as_str().unwrap().into(), "en")
        .await
        .unwrap();
    let mailbox = data["mailpit_url"].as_str().unwrap();
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let code = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let messages: Value = http
                .get(format!("{mailbox}/api/v1/messages"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if messages["messages"].as_array().unwrap().len() <= before {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            if let Some(id) = messages["messages"]
                .as_array()
                .unwrap()
                .first()
                .and_then(|message| message["ID"].as_str())
            {
                let mail: Value = http
                    .get(format!("{mailbox}/api/v1/message/{id}"))
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                if let Some((_, suffix)) = mail["Text"].as_str().unwrap().split_once("code: ") {
                    break suffix.chars().take(8).collect::<String>();
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    identity.verify_email(code).await.unwrap();
    identity
        .prepare_device("Owned native agent acceptance".into())
        .await
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let vault = VaultClient::new(identity.clone(), root.path().join("nddev-device-sync"))
        .await
        .unwrap();
    vault.synchronize().await.unwrap();
    let io = NativeIo::new(1).unwrap().process_handle();
    let gh = std::env::var_os("NDS_TEST_GH")
        .map(std::path::PathBuf::from)
        .expect("absolute official gh executable required");
    let output = io
        .process(
            ProcessRequest {
                executable: gh,
                arguments: ["auth", "token", "--hostname", "github.com"]
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                directory: None,
                timeout: Duration::from_secs(10),
                max_output_bytes: 16_384,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(output.status.success());
    let secret = Zeroizing::new(
        String::from_utf8(output.stdout)
            .unwrap()
            .trim_end()
            .to_owned(),
    );
    assert!(!secret.is_empty());
    vault
        .save_record(
            None,
            "Owned native credential consumer".into(),
            String::new(),
            secret.clone(),
        )
        .await
        .unwrap();
    vault.synchronize().await.unwrap();
    let reference = vault
        .records()
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.title == "Owned native credential consumer" && !record.conflicted)
        .unwrap()
        .id;
    let credentials = identity.vault_credentials().unwrap();
    let command = ProcessRequest {
        executable: std::path::PathBuf::from(env!("CARGO_BIN_EXE_nds-agent")),
        arguments: ["--server", endpoint, "vault", "use", &reference, "github"]
            .into_iter()
            .map(Into::into)
            .collect(),
        directory: None,
        timeout: Duration::from_secs(60),
        max_output_bytes: 65_536,
    };
    // NativeIo preserves this test-owned XDG_STATE_HOME; spawning directly lets
    // the acceptance supply a private sandbox without changing global env.
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new(command.executable)
            .args(command.arguments)
            .env("XDG_STATE_HOME", root.path())
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    let classes = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .filter_map(|value| value["error"].as_str().map(str::to_owned))
        .filter(|value| {
            value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        })
        .collect::<Vec<_>>();
    assert!(
        output.status.success(),
        "Native CLI error classes: {classes:?}"
    );
    assert!(
        !output
            .stdout
            .windows(secret.len())
            .any(|value| value == secret.as_bytes())
    );
    assert!(
        !output
            .stderr
            .windows(secret.len())
            .any(|value| value == secret.as_bytes())
    );
    assert!(
        output
            .stdout
            .split(|byte| *byte == b'\n')
            .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
            .any(|value| value["operation"] == "vault.use"
                && value["provider"] == "github"
                && value["verified"] == true)
    );
    credentials.delete().await.unwrap();
    identity.logout().await.unwrap();
}

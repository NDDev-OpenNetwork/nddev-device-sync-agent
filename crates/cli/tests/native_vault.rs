//! Real native vault and official GitHub rejection; no provider-success stub.
//! A positive check requires an explicitly supplied isolated test credential.
#![cfg(target_os = "linux")]
use nddev_device_sync_accounts::IdentityClient;
use nddev_device_sync_adapter_io::ProcessRequest;
use nddev_device_sync_vault::VaultClient;
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use zeroize::Zeroizing;
#[tokio::test]
#[ignore = "requires an owned fixture and official GitHub CLI; optional isolated test credential"]
async fn actual_native_vault_reference_drives_official_github_cli() {
    let data: Value = serde_json::from_slice(
        &std::fs::read(std::env::var("NDS_FIXTURE_RECEIPT").unwrap()).unwrap(),
    )
    .unwrap();
    let endpoint = data["base_url"].as_str().unwrap();
    assert!(endpoint.starts_with("http://localhost:") || endpoint.starts_with("http://127.0.0.1:"));
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
    let positive = std::env::var_os("NDS_TEST_ISOLATED_GITHUB_TOKEN_FILE");
    let secret = Zeroizing::new(if let Some(path) = &positive {
        let path = std::path::PathBuf::from(path);
        assert!(path.is_absolute());
        let file = std::fs::File::open(path).unwrap();
        use std::io::Read;
        let mut value = String::new();
        file.take(16_385).read_to_string(&mut value).unwrap();
        assert!(value.len() <= 16_384);
        value.trim_end().to_owned()
    } else {
        format!("nds-invalid-acceptance-{}", std::process::id())
    });
    assert!(!secret.is_empty());
    vault
        .save_record(
            None,
            0,
            "Owned native credential consumer".into(),
            String::new(),
            secret.clone(),
        )
        .await
        .unwrap();
    vault.synchronize().await.unwrap();
    let reference = vault
        .records(String::new())
        .await
        .unwrap()
        .items
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
    let result: Value = serde_json::from_slice(&output.stdout)
        .expect("CLI stdout must contain exactly one JSON result");
    if let Ok(policy) = std::env::var("NDS_TEST_EXPECT_EXPORT_MODE") {
        assert!(matches!(
            policy.as_str(),
            "enabled" | "disabled" | "unknown"
        ));
        assert_eq!(result["diagnostics"]["export"], policy);
        if policy == "disabled" {
            assert_eq!(result["diagnostics"]["acknowledged_events"], 0);
            assert_eq!(result["diagnostics"]["acknowledged_spans"], 0);
            assert_eq!(result["diagnostics"]["undelivered"], 0);
            assert!(result["diagnostics"]["dropped"].as_u64().unwrap() > 0);
        }
    }
    assert_eq!(output.status.success(), positive.is_some());
    if positive.is_some() {
        assert_eq!(result["verified"], true);
    } else {
        assert_eq!(result["error"], "provider_credential_rejected");
    }
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
    credentials.delete().await.unwrap();
    identity.logout().await.unwrap();
}

#![cfg(unix)]
use nddev_device_sync_adapter_io::{CancellationToken, NativeIo, ProcessRequest};
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

fn request(receipt: &Path, parent_waits: bool) -> ProcessRequest {
    let script = if parent_waits {
        r#"sleep 30 >/dev/null 2>&1 & printf '%s' "$!" > "$1"; wait"#
    } else {
        r#"sleep 30 >/dev/null 2>&1 & printf '%s' "$!" > "$1"; exit 0"#
    };
    ProcessRequest {
        executable: "/bin/sh".into(),
        arguments: vec![
            "-c".into(),
            script.into(),
            "native-scope-test".into(),
            receipt.as_os_str().into(),
        ],
        directory: None,
        timeout: Duration::from_secs(3),
        max_output_bytes: 1024,
    }
}

async fn terminated(pid: &str) {
    assert!(!pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit()));
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let output = Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .unwrap();
        let state = String::from_utf8(output.stdout).unwrap();
        // An orphan zombie awaits the OS reaper but cannot keep doing work.
        if !output.status.success() || state.trim().is_empty() || state.trim().starts_with('Z') {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "owned helper survived operation lifetime"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn successful_parent_exit_cannot_leave_a_closed_pipe_helper_running() {
    let fixture = tempfile::tempdir().unwrap();
    let receipt = fixture.path().join("helper.pid");
    let output = NativeIo::new(1)
        .unwrap()
        .process_handle()
        .process(request(&receipt, false), CancellationToken::new())
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "preserve the real parent exit status"
    );
    terminated(&std::fs::read_to_string(receipt).unwrap()).await;
}

#[tokio::test]
async fn dropping_the_caller_terminates_owned_parent_and_helper_scope() {
    let fixture = tempfile::tempdir().unwrap();
    let receipt = fixture.path().join("helper.pid");
    let request = request(&receipt, true);
    let io = NativeIo::new(1).unwrap().process_handle();
    let task = tokio::spawn(async move { io.process(request, CancellationToken::new()).await });
    let deadline = Instant::now() + Duration::from_secs(1);
    let pid = loop {
        if let Ok(pid) = std::fs::read_to_string(&receipt)
            && pid.parse::<u32>().is_ok_and(|value| value > 1)
        {
            break pid;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    terminated(&pid).await;
}

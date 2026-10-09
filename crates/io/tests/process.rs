use nddev_device_sync_adapter_io::{CancellationToken, IoError, NativeIo, ProcessRequest};
use std::{
    ffi::OsString,
    io::Write,
    time::{Duration, Instant},
};

fn request(mode: &str) -> ProcessRequest {
    ProcessRequest {
        executable: std::env::current_exe().unwrap(),
        arguments: ["--exact", mode, "--nocapture"]
            .into_iter()
            .map(OsString::from)
            .collect(),
        directory: None,
        timeout: Duration::from_secs(2),
        max_output_bytes: 4096,
    }
}

// These run as real OS child processes. They exercise transport failure paths,
// not a substitute for acceptance against installed native providers.
#[test]
fn child_unicode() {
    println!("Native UTF-8: Проверка Android");
    eprintln!("stderr is separate");
}

#[test]
#[ignore = "only launched by bounded process acceptance"]
fn child_flood() {
    loop {
        std::io::stdout().write_all(&[b'x'; 4096]).unwrap();
        std::io::stderr().write_all(&[b'y'; 4096]).unwrap();
    }
}

#[test]
#[ignore = "only launched by bounded process acceptance"]
fn child_wait() {
    std::thread::sleep(Duration::from_secs(30));
}

fn ignored_request(mode: &str) -> ProcessRequest {
    let mut request = request(mode);
    request.arguments.push("--ignored".into());
    request
}

#[tokio::test]
async fn preserves_exit_status_and_separate_unicode_streams() {
    let output = NativeIo::new(1)
        .unwrap()
        .process_handle()
        .process(request("child_unicode"), CancellationToken::new())
        .await
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Проверка Android")
    );
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("stderr is separate")
    );
}

#[tokio::test]
async fn bounds_aggregate_output_and_releases_admission() {
    let io = NativeIo::new(1).unwrap().process_handle();
    assert!(matches!(
        io.process(ignored_request("child_flood"), CancellationToken::new())
            .await,
        Err(IoError::OutputLimit)
    ));
    assert!(
        io.process(request("child_unicode"), CancellationToken::new())
            .await
            .unwrap()
            .status
            .success()
    );
}

#[tokio::test]
async fn rejects_overload_and_cancels_owned_process() {
    let io = NativeIo::new(1).unwrap().process_handle();
    let token = CancellationToken::new();
    let operation = io.process(ignored_request("child_wait"), token.clone());
    tokio::pin!(operation);
    tokio::select! {
        _ = &mut operation => panic!("sleeping child exited early"),
        () = tokio::time::sleep(Duration::from_millis(100)) => {}
    }
    assert!(matches!(
        io.process(request("child_unicode"), CancellationToken::new())
            .await,
        Err(IoError::Busy)
    ));
    token.cancel();
    assert!(matches!(operation.await, Err(IoError::Cancelled)));
    assert!(
        io.process(request("child_unicode"), CancellationToken::new())
            .await
            .unwrap()
            .status
            .success()
    );
}

#[tokio::test]
async fn deadline_covers_child_exit_and_pipe_drain() {
    let mut request = ignored_request("child_wait");
    request.timeout = Duration::from_millis(100);
    let start = Instant::now();
    assert!(matches!(
        NativeIo::new(1)
            .unwrap()
            .process_handle()
            .process(request, CancellationToken::new())
            .await,
        Err(IoError::Timeout)
    ));
    assert!(start.elapsed() < Duration::from_secs(2));
}

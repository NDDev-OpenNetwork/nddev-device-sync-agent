//! Cancellation owns the actual CLI process, including its pending HTTP request.
#![cfg(all(unix, feature = "development-http"))]
use nddev_device_sync_adapter_io::{CancellationToken, IoError, NativeIo, ProcessRequest};
use std::{
    io::{ErrorKind, Read},
    net::TcpListener,
    time::{Duration, Instant},
};

#[tokio::test]
async fn cancelling_real_cli_closes_its_unresponsive_transport() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let token = CancellationToken::new();
    let cancellation = token.clone();
    let task = tokio::spawn(async move {
        NativeIo::new(1)
            .unwrap()
            .process_handle()
            .process(
                ProcessRequest {
                    executable: env!("CARGO_BIN_EXE_nds-agent").into(),
                    arguments: ["--server", &endpoint, "vault", "list"]
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                    directory: None,
                    timeout: Duration::from_secs(10),
                    max_output_bytes: 65_536,
                },
                cancellation,
            )
            .await
    });
    let connection = tokio::task::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut connection = loop {
            match listener.accept() {
                Ok((connection, _)) => break connection,
                Err(error)
                    if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("owned CLI accept failed: {}", error.kind()),
            }
        };
        connection.set_nonblocking(false).unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            assert!(headers.len() < 8192);
            let mut byte = [0];
            connection.read_exact(&mut byte).unwrap();
            headers.push(byte[0]);
        }
        connection
    })
    .await
    .unwrap();
    let cancelled_at = Instant::now();
    token.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap(),
        Err(IoError::Cancelled)
    ));
    assert!(cancelled_at.elapsed() < Duration::from_secs(2));
    tokio::task::spawn_blocking(move || {
        let mut connection = connection;
        let mut byte = [0];
        assert_eq!(
            connection.read(&mut byte).unwrap(),
            0,
            "owned CLI retained a transport after cancellation"
        );
    })
    .await
    .unwrap();
}

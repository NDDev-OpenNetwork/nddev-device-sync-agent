#![cfg(unix)]

use nddev_device_sync_adapter_io::{CancellationToken, IoError, LocalChannel};
use std::{
    io::Write,
    os::unix::net::UnixListener,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct LocalPeer(PathBuf);

impl LocalPeer {
    fn new(send: impl FnOnce(std::os::unix::net::UnixStream) + Send + 'static) -> Self {
        let path = std::env::temp_dir().join(format!(
            "nds-io-{}-{}.sock",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        let listener = UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            send(stream);
        });
        Self(path)
    }
}

impl Drop for LocalPeer {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).unwrap();
    }
}

#[test]
fn preserves_adjacent_frames_and_rejects_unterminated_payload() {
    let peer = LocalPeer::new(|mut stream| {
        stream.write_all(b"one\ntwo\npartial").unwrap();
        // Explicit EOF keeps this a framing test. A full peer close can be a
        // reset on Darwin, whose correct outcome is a transport error instead.
        stream.shutdown(std::net::Shutdown::Write).unwrap();
    });
    let mut channel = LocalChannel::connect(
        &peer.0,
        Duration::from_secs(1),
        16,
        CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(channel.read_frame().unwrap(), b"one");
    assert_eq!(channel.read_frame().unwrap(), b"two");
    assert!(matches!(
        channel.read_frame(),
        Err(IoError::InvalidFrame | IoError::Transport)
    ));
}

#[test]
fn rejects_oversized_response_before_allocating_it() {
    let peer = LocalPeer::new(|mut stream| {
        let _ = stream.write_all(&[b'x'; 65_536]);
    });
    let mut channel = LocalChannel::connect(
        &peer.0,
        Duration::from_secs(1),
        32,
        CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(channel.read_frame().unwrap_err(), IoError::OutputLimit);
}

#[test]
fn slow_peer_cannot_refresh_the_absolute_deadline() {
    let peer = LocalPeer::new(|mut stream| {
        for _ in 0..10 {
            if stream.write_all(b"x").is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    });
    let start = Instant::now();
    let mut channel = LocalChannel::connect(
        &peer.0,
        Duration::from_millis(100),
        32,
        CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(channel.read_frame().unwrap_err(), IoError::Timeout);
    assert!(start.elapsed() < Duration::from_millis(500));
}

#[test]
fn cancellation_is_terminal_even_with_buffered_data() {
    let peer = LocalPeer::new(|mut stream| {
        let _ = stream.write_all(b"one\ntwo\n");
    });
    let cancellation = CancellationToken::new();
    let mut channel =
        LocalChannel::connect(&peer.0, Duration::from_secs(1), 32, cancellation.clone()).unwrap();
    assert_eq!(channel.read_frame().unwrap(), b"one");
    cancellation.cancel();
    assert_eq!(channel.read_frame().unwrap_err(), IoError::Cancelled);
}

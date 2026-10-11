//! The real machine-facing binary must reject invalid OS arguments with JSON.
#![cfg(unix)]
use std::{ffi::OsString, os::unix::ffi::OsStringExt, process::Command};

#[test]
fn non_utf8_arguments_return_one_json_error_instead_of_panicking() {
    let output = Command::new(env!("CARGO_BIN_EXE_nds-agent"))
        .arg(OsString::from_vec(vec![0xff]))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["error"], "invalid_arguments");
    assert!(!output.stderr.windows(11).any(|part| part == b"panicked at"));
}

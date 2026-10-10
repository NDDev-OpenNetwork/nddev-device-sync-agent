//! Compiled-in native module composition. Domain contracts come from NDS core;
//! concrete tools retain their own data, permissions and execution policies.
pub mod view;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod native;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub use native::NativeAgent;

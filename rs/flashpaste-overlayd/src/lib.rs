pub mod ipc;
pub mod protocol;

#[cfg(feature = "render")]
pub mod render;

pub mod store;

#[cfg(any(feature = "surface", feature = "wayland"))]
pub mod surface;

use std::path::PathBuf;

pub const SOCKET_NAME: &str = "flashpaste-overlay.sock";

pub fn socket_path() -> PathBuf {
    ipc::default_socket_path()
}

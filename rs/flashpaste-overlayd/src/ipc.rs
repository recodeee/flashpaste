use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug_span, error, info, warn, Instrument};

use crate::protocol::Message;
use crate::store::{Shape, SharedShapeStore};
use crate::SOCKET_NAME;

pub type RedrawNotifier = mpsc::Sender<()>;

pub struct IpcServer {
    socket_path: PathBuf,
    handle: JoinHandle<()>,
}

impl IpcServer {
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn abort(&self) {
        self.handle.abort();
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.handle.abort();
        if is_socket(&self.socket_path) {
            let _ = std::fs::remove_file(&self.socket_path);
        }
    }
}

pub fn default_socket_path() -> PathBuf {
    runtime_dir()
        .unwrap_or_else(|| {
            warn!("XDG_RUNTIME_DIR is unset; using /tmp for flashpaste-overlayd socket");
            PathBuf::from("/tmp")
        })
        .join(SOCKET_NAME)
}

pub async fn serve(store: SharedShapeStore) -> Result<()> {
    let path = default_socket_path();
    let listener = bind_listener(&path).await?;
    info!(path = %path.display(), "flashpaste-overlayd IPC listener up");
    accept_loop(listener, store, None).await;
    Ok(())
}

pub async fn spawn_listener(store: SharedShapeStore) -> Result<IpcServer> {
    spawn_listener_at_with_redraw(default_socket_path(), store, None).await
}

pub async fn spawn_listener_at(
    socket_path: impl Into<PathBuf>,
    store: SharedShapeStore,
) -> Result<IpcServer> {
    spawn_listener_at_with_redraw(socket_path, store, None).await
}

pub async fn spawn_listener_with_redraw(
    store: SharedShapeStore,
    redraw: RedrawNotifier,
) -> Result<IpcServer> {
    spawn_listener_at_with_redraw(default_socket_path(), store, Some(redraw)).await
}

pub async fn spawn_listener_at_with_redraw(
    socket_path: impl Into<PathBuf>,
    store: SharedShapeStore,
    redraw: Option<RedrawNotifier>,
) -> Result<IpcServer> {
    let socket_path = socket_path.into();
    let listener = bind_listener(&socket_path).await?;
    info!(path = %socket_path.display(), "flashpaste-overlayd IPC listener up");
    let handle = tokio::spawn(async move {
        accept_loop(listener, store, redraw).await;
    });

    Ok(IpcServer {
        socket_path,
        handle,
    })
}

async fn bind_listener(socket_path: &Path) -> Result<UnixListener> {
    prepare_socket_path(socket_path)?;
    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("bind {}", socket_path.display()))?;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod 0600 {}", socket_path.display()))?;
    Ok(listener)
}

fn prepare_socket_path(socket_path: &Path) -> Result<()> {
    if socket_path.exists() {
        if is_socket(socket_path) {
            std::fs::remove_file(socket_path)
                .with_context(|| format!("remove stale socket {}", socket_path.display()))?;
        } else {
            anyhow::bail!(
                "socket path {} exists but is not a socket",
                socket_path.display()
            );
        }
    }

    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create socket parent {}", parent.display()))?;
    }

    Ok(())
}

fn runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn is_socket(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_socket())
        .unwrap_or(false)
}

async fn accept_loop(
    listener: UnixListener,
    store: SharedShapeStore,
    redraw: Option<RedrawNotifier>,
) {
    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let store = store.clone();
                let redraw = redraw.clone();
                tokio::spawn(async move {
                    if let Err(err) = handle_connection(stream, store, redraw).await {
                        warn!(error = %err, "flashpaste-overlayd IPC connection failed");
                    }
                });
            }
            Err(err) => {
                error!(error = %err, "flashpaste-overlayd IPC accept failed");
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
}

async fn handle_connection(
    stream: UnixStream,
    store: SharedShapeStore,
    redraw: Option<RedrawNotifier>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = async { lines.next_line().await.context("read IPC line") }
        .instrument(debug_span!("socket_receive"))
        .await?
    {
        if let Some(response) = handle_test_command(&line, &store).await {
            write_json_line(&mut writer, &response).await?;
            continue;
        }

        match debug_span!("socket_parse").in_scope(|| serde_json::from_str::<Message>(&line)) {
            Ok(message) => {
                let response = handle_message(message, &store, redraw.as_ref()).await;
                write_json_line(&mut writer, &response).await?;
            }
            Err(err) => {
                warn!(error = %err, "flashpaste-overlayd rejected malformed IPC message");
                write_json_line(
                    &mut writer,
                    &json!({
                        "ok": false,
                        "error": err.to_string(),
                    }),
                )
                .await?;
            }
        }
    }

    Ok(())
}

async fn handle_test_command(line: &str, store: &SharedShapeStore) -> Option<Value> {
    std::env::var_os("FLASHPASTE_OVERLAYD_TEST_MODE")?;

    let value = serde_json::from_str::<Value>(line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("debug_store") {
        return None;
    }

    let store = store.lock().await;
    let ids: Vec<String> = store
        .shapes()
        .iter()
        .map(|shape| shape.shape.id().to_string())
        .collect();

    Some(json!({
        "ok": true,
        "count": store.len(),
        "ids": ids,
    }))
}

async fn handle_message(
    message: Message,
    store: &SharedShapeStore,
    redraw: Option<&RedrawNotifier>,
) -> Value {
    match message {
        Message::Clear(clear) => {
            async {
                let mut store = store.lock().await;
                if let Some(id) = clear.id {
                    let needs_redraw = store.remove(id);
                    notify_redraw(redraw, needs_redraw);
                    json!({ "ok": true, "id": id })
                } else {
                    let needs_redraw = store.clear();
                    notify_redraw(redraw, needs_redraw);
                    json!({ "ok": true })
                }
            }
            .instrument(debug_span!("store_update", message_type = "clear"))
            .await
        }
        draw_message => {
            let message_type = draw_message_type(&draw_message);
            let Some(shape) = Shape::from_message(draw_message) else {
                warn!("flashpaste-overlayd received a non-draw message in draw handler");
                return json!({
                    "ok": false,
                    "error": "internal message dispatch error",
                });
            };
            async {
                let mut store = store.lock().await;
                let id = store.add(shape);
                notify_redraw(redraw, true);
                json!({ "ok": true, "id": id })
            }
            .instrument(debug_span!("store_update", message_type))
            .await
        }
    }
}

fn notify_redraw(redraw: Option<&RedrawNotifier>, needs_redraw: bool) {
    if !needs_redraw {
        return;
    }

    if let Some(redraw) = redraw {
        let _ = redraw.try_send(());
    }
}

fn draw_message_type(message: &Message) -> &'static str {
    match message {
        Message::DrawRect(_) => "draw_rect",
        Message::DrawCircle(_) => "draw_circle",
        Message::DrawArrow(_) => "draw_arrow",
        Message::DrawLabel(_) => "draw_label",
        Message::Clear(_) => "clear",
    }
}

async fn write_json_line(writer: &mut OwnedWriteHalf, value: &Value) -> Result<()> {
    let mut body = serde_json::to_vec(value).context("serialize IPC response")?;
    body.push(b'\n');
    writer
        .write_all(&body)
        .await
        .context("write IPC response")?;
    writer.flush().await.context("flush IPC response")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UnixStream;

    #[tokio::test]
    async fn write_json_line_reports_closed_peer_without_panic() {
        let (stream, peer) = UnixStream::pair().unwrap();
        let (_reader, mut writer) = stream.into_split();
        drop(peer);

        let err = write_json_line(&mut writer, &json!({ "ok": true }))
            .await
            .unwrap_err();
        let message = err.to_string();

        assert!(
            message.contains("write IPC response") || message.contains("flush IPC response"),
            "unexpected error: {message}"
        );
    }
}

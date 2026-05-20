use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{error::Error, io::ErrorKind};

use flashpaste_overlayd::{
    ipc,
    protocol::{MAX_COORD_ABS, MAX_TTL_MS},
    store::{ShapeStore, MAX_SHAPES},
    SOCKET_NAME,
};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use uuid::Uuid;

#[tokio::test]
async fn draw_rect_round_trips_and_updates_store() {
    let runtime_dir = TestRuntimeDir::new();
    let socket_path = runtime_dir.path().join(SOCKET_NAME);
    let store = ShapeStore::shared();
    let Some(_server) = spawn_listener_or_skip(&socket_path, store.clone()).await else {
        return;
    };

    let mode = std::fs::metadata(&socket_path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);

    let mut stream = UnixStream::connect(&socket_path).await.unwrap();
    let id = Uuid::new_v4();
    let message = json!({
        "type": "draw_rect",
        "id": id,
        "x": 10.0,
        "y": 20.0,
        "w": 30.0,
        "h": 40.0
    });
    stream
        .write_all(format!("{message}\n").as_bytes())
        .await
        .unwrap();

    let mut response = String::new();
    let mut reader = BufReader::new(stream);
    reader.read_line(&mut response).await.unwrap();

    let value: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(value["id"], id.to_string());

    let store = store.lock().await;
    assert_eq!(store.len(), 1);
    assert_eq!(store.shapes()[0].shape.id(), id);
}

#[tokio::test]
async fn parse_error_keeps_connection_open_for_next_message() {
    let runtime_dir = TestRuntimeDir::new();
    let socket_path = runtime_dir.path().join(SOCKET_NAME);
    let store = ShapeStore::shared();
    let Some(_server) = spawn_listener_or_skip(&socket_path, store.clone()).await else {
        return;
    };

    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    writer.write_all(b"{not json}\n").await.unwrap();

    let mut response = String::new();
    reader.read_line(&mut response).await.unwrap();
    let value: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(value["ok"], false);
    assert!(!value["error"].as_str().unwrap().is_empty());

    let id = Uuid::new_v4();
    let message = json!({
        "type": "draw_rect",
        "id": id,
        "x": 1.0,
        "y": 2.0,
        "w": 3.0,
        "h": 4.0
    });
    writer
        .write_all(format!("{message}\n").as_bytes())
        .await
        .unwrap();

    response.clear();
    reader.read_line(&mut response).await.unwrap();
    let value: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(value["id"], id.to_string());

    let store = store.lock().await;
    assert_eq!(store.len(), 1);
}

#[tokio::test]
async fn ttl_above_max_returns_error_without_adding_shape() {
    let runtime_dir = TestRuntimeDir::new();
    let socket_path = runtime_dir.path().join(SOCKET_NAME);
    let store = ShapeStore::shared();
    let Some(_server) = spawn_listener_or_skip(&socket_path, store.clone()).await else {
        return;
    };

    let response = send_json(
        &socket_path,
        json!({
            "type": "draw_rect",
            "id": Uuid::new_v4(),
            "ttl_ms": MAX_TTL_MS + 1,
            "x": 10.0,
            "y": 20.0,
            "w": 30.0,
            "h": 40.0
        }),
    )
    .await;

    assert_eq!(response["ok"], false);
    assert!(response["error"].as_str().unwrap().contains("ttl_ms"));
    assert_eq!(store.lock().await.len(), 0);
}

#[tokio::test]
async fn label_with_control_character_returns_error_without_adding_shape() {
    let runtime_dir = TestRuntimeDir::new();
    let socket_path = runtime_dir.path().join(SOCKET_NAME);
    let store = ShapeStore::shared();
    let Some(_server) = spawn_listener_or_skip(&socket_path, store.clone()).await else {
        return;
    };

    let response = send_json(
        &socket_path,
        json!({
            "type": "draw_label",
            "id": Uuid::new_v4(),
            "x": 10.0,
            "y": 20.0,
            "text": "bad\nlabel"
        }),
    )
    .await;

    assert_eq!(response["ok"], false);
    assert!(response["error"].as_str().unwrap().contains("control"));
    assert_eq!(store.lock().await.len(), 0);
}

#[tokio::test]
async fn coordinate_overflow_returns_error_without_adding_shape() {
    let runtime_dir = TestRuntimeDir::new();
    let socket_path = runtime_dir.path().join(SOCKET_NAME);
    let store = ShapeStore::shared();
    let Some(_server) = spawn_listener_or_skip(&socket_path, store.clone()).await else {
        return;
    };

    let response = send_json(
        &socket_path,
        json!({
            "type": "draw_arrow",
            "id": Uuid::new_v4(),
            "x1": 0.0,
            "y1": 0.0,
            "x2": MAX_COORD_ABS + 1.0,
            "y2": 40.0
        }),
    )
    .await;

    assert_eq!(response["ok"], false);
    assert!(response["error"].as_str().unwrap().contains("coordinate"));
    assert_eq!(store.lock().await.len(), 0);
}

#[tokio::test]
async fn shape_limit_evicts_oldest_shape() {
    let runtime_dir = TestRuntimeDir::new();
    let socket_path = runtime_dir.path().join(SOCKET_NAME);
    let store = ShapeStore::shared();
    let Some(_server) = spawn_listener_or_skip(&socket_path, store.clone()).await else {
        return;
    };
    let first_id = Uuid::from_u128(1);
    let last_id = Uuid::from_u128((MAX_SHAPES + 1) as u128);

    for index in 0..=MAX_SHAPES {
        let id = Uuid::from_u128((index + 1) as u128);
        let response = send_json(
            &socket_path,
            json!({
                "type": "draw_rect",
                "id": id,
                "ttl_ms": MAX_TTL_MS,
                "x": 10.0,
                "y": 20.0,
                "w": 30.0,
                "h": 40.0
            }),
        )
        .await;
        assert_eq!(response["ok"], true);
    }

    let store = store.lock().await;
    assert_eq!(store.len(), MAX_SHAPES);
    assert!(!store
        .shapes()
        .iter()
        .any(|shape| shape.shape.id() == first_id));
    assert!(store
        .shapes()
        .iter()
        .any(|shape| shape.shape.id() == last_id));
}

struct TestRuntimeDir {
    path: PathBuf,
}

impl TestRuntimeDir {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "flashpaste-overlayd-ipc-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

async fn send_json(socket_path: &Path, message: Value) -> Value {
    let mut stream = UnixStream::connect(socket_path).await.unwrap();
    stream
        .write_all(format!("{message}\n").as_bytes())
        .await
        .unwrap();

    let mut response = String::new();
    let mut reader = BufReader::new(stream);
    reader.read_line(&mut response).await.unwrap();
    serde_json::from_str(&response).unwrap()
}

async fn spawn_listener_or_skip(
    socket_path: &Path,
    store: flashpaste_overlayd::store::SharedShapeStore,
) -> Option<ipc::IpcServer> {
    match ipc::spawn_listener_at(socket_path, store).await {
        Ok(server) => Some(server),
        Err(err) if is_permission_denied(err.as_ref()) => {
            eprintln!("skipping IPC socket test: Unix listener bind denied by environment");
            None
        }
        Err(err) => panic!("failed to spawn IPC listener: {err:#}"),
    }
}

fn is_permission_denied(err: &(dyn Error + 'static)) -> bool {
    let mut current = Some(err);
    while let Some(err) = current {
        if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
            if io_err.kind() == ErrorKind::PermissionDenied {
                return true;
            }
        }
        current = err.source();
    }
    false
}

impl Drop for TestRuntimeDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

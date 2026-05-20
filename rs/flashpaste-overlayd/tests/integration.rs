use std::{
    io::ErrorKind,
    os::unix::fs::FileTypeExt,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::Stdio,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use flashpaste_overlayd::{store::MAX_SHAPES, SOCKET_NAME};
use serde_json::Value;
use tokio::{
    process::Command,
    time::{sleep, timeout, Duration, Instant},
};

const DAEMON_BIN: &str = env!("CARGO_BIN_EXE_flashpaste-overlayd");
const CLIENT_BIN: &str = env!("CARGO_BIN_EXE_flashpaste-overlay");

#[tokio::test]
async fn draw_rect_then_status_reports_one_shape() -> Result<()> {
    let Some(daemon) = TestDaemon::spawn().await? else {
        return Ok(());
    };

    let response = daemon
        .client_ok(["rect", "--x", "10", "--y", "20", "--w", "30", "--h", "40"])
        .await?;
    assert_eq!(response["ok"], true);

    let status = daemon.status().await?;
    assert_eq!(status["count"], 1);
    assert_eq!(status["ids"].as_array().context("ids array")?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn clear_one_shape_leaves_four() -> Result<()> {
    let Some(daemon) = TestDaemon::spawn().await? else {
        return Ok(());
    };
    let mut ids = Vec::new();

    ids.push(
        daemon
            .client_ok(["rect", "--x", "1", "--y", "2", "--w", "3", "--h", "4"])
            .await?["id"]
            .as_str()
            .context("rect id")?
            .to_string(),
    );
    ids.push(
        daemon
            .client_ok(["circle", "--x", "5", "--y", "6", "--w", "7", "--h", "8"])
            .await?["id"]
            .as_str()
            .context("circle id")?
            .to_string(),
    );
    ids.push(
        daemon
            .client_ok([
                "arrow", "--x1", "1", "--y1", "2", "--x2", "30", "--y2", "40",
            ])
            .await?["id"]
            .as_str()
            .context("arrow id")?
            .to_string(),
    );
    ids.push(
        daemon
            .client_ok(["label", "--x", "9", "--y", "10", "--text", "note"])
            .await?["id"]
            .as_str()
            .context("label id")?
            .to_string(),
    );
    ids.push(
        daemon
            .client_ok(["rect", "--x", "11", "--y", "12", "--w", "13", "--h", "14"])
            .await?["id"]
            .as_str()
            .context("second rect id")?
            .to_string(),
    );

    daemon.client_ok(["clear", "--id", ids[2].as_str()]).await?;

    let status = daemon.status().await?;
    let remaining = status["ids"].as_array().context("ids array")?;
    assert_eq!(status["count"], 4);
    assert!(!remaining
        .iter()
        .any(|id| id.as_str() == Some(ids[2].as_str())));
    Ok(())
}

#[tokio::test]
async fn ttl_expiry_removes_shape() -> Result<()> {
    let Some(daemon) = TestDaemon::spawn().await? else {
        return Ok(());
    };
    daemon
        .client_ok([
            "rect", "--x", "10", "--y", "20", "--w", "30", "--h", "40", "--ttl-ms", "500",
        ])
        .await?;
    assert_eq!(daemon.status().await?["count"], 1);

    sleep(Duration::from_millis(700)).await;

    assert_eq!(daemon.status().await?["count"], 0);
    Ok(())
}

#[tokio::test]
async fn malformed_json_returns_error_and_socket_stays_alive() -> Result<()> {
    let Some(daemon) = TestDaemon::spawn().await? else {
        return Ok(());
    };

    let malformed = daemon
        .client_value(false, ["raw", "--payload", "{not json}"])
        .await?;
    assert_eq!(malformed["ok"], false);
    assert!(!malformed["error"].as_str().unwrap_or_default().is_empty());

    let status = daemon.status().await?;
    assert_eq!(status["ok"], true);
    Ok(())
}

#[tokio::test]
async fn flood_of_one_thousand_messages_is_processed() -> Result<()> {
    let Some(daemon) = TestDaemon::spawn().await? else {
        return Ok(());
    };

    let flood = daemon.client_ok(["flood", "--count", "1000"]).await?;
    assert_eq!(flood["count"], 1_000);

    let status = daemon.status().await?;
    assert_eq!(status["count"], MAX_SHAPES);
    Ok(())
}

#[cfg(feature = "visual-tests")]
#[tokio::test]
async fn visual_tests_require_virtual_wayland_compositor() -> Result<()> {
    let has_virtual_compositor = command_exists("weston") || command_exists("cage");
    if !has_virtual_compositor {
        eprintln!("skipping visual overlay smoke: weston/cage not found");
        return Ok(());
    }

    eprintln!(
        "visual overlay smoke is enabled; compositor-specific launch is covered by manual CI setup"
    );
    Ok(())
}

struct TestDaemon {
    runtime_dir: TestRuntimeDir,
    child: tokio::process::Child,
}

impl TestDaemon {
    async fn spawn() -> Result<Option<Self>> {
        let runtime_dir = TestRuntimeDir::new()?;
        if !unix_listener_bind_supported(runtime_dir.path())? {
            eprintln!("skipping daemon integration test: Unix listener bind denied by environment");
            return Ok(None);
        }

        let mut child = Command::new(DAEMON_BIN)
            .arg("--headless-test")
            .env("XDG_RUNTIME_DIR", runtime_dir.path())
            .env("FLASHPASTE_OVERLAYD_TEST_MODE", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn flashpaste-overlayd --headless-test")?;

        wait_for_socket(runtime_dir.path().join(SOCKET_NAME), &mut child).await?;
        Ok(Some(Self { runtime_dir, child }))
    }

    async fn status(&self) -> Result<Value> {
        self.client_ok(["status"]).await
    }

    async fn client_ok<I, S>(&self, args: I) -> Result<Value>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        self.client_value(true, args).await
    }

    async fn client_value<I, S>(&self, expect_success: bool, args: I) -> Result<Value>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let output = Command::new(CLIENT_BIN)
            .args(args)
            .env("XDG_RUNTIME_DIR", self.runtime_dir.path())
            .output()
            .await
            .context("run flashpaste-overlay client")?;

        if output.status.success() != expect_success {
            bail!(
                "flashpaste-overlay exit status mismatch: status={} stdout={} stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let stdout = String::from_utf8(output.stdout).context("client stdout was not UTF-8")?;
        serde_json::from_str(stdout.trim())
            .with_context(|| format!("parse client JSON: {stdout:?}"))
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn wait_for_socket(socket_path: PathBuf, child: &mut tokio::process::Child) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if is_socket(&socket_path) {
            return Ok(());
        }

        if let Some(status) = child.try_wait().context("poll daemon status")? {
            bail!("flashpaste-overlayd exited before socket was ready: {status}");
        }

        if Instant::now() >= deadline {
            bail!("timed out waiting for socket {}", socket_path.display());
        }

        timeout(Duration::from_millis(20), sleep(Duration::from_millis(20)))
            .await
            .ok();
    }
}

fn is_socket(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_socket())
        .unwrap_or(false)
}

fn unix_listener_bind_supported(runtime_dir: &Path) -> Result<bool> {
    let probe = runtime_dir.join("socket-probe.sock");
    match UnixListener::bind(&probe) {
        Ok(listener) => {
            drop(listener);
            let _ = std::fs::remove_file(probe);
            Ok(true)
        }
        Err(err) if err.kind() == ErrorKind::PermissionDenied => Ok(false),
        Err(err) => Err(err).with_context(|| format!("bind probe {}", probe.display())),
    }
}

#[cfg(feature = "visual-tests")]
fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|path| path.join(name).is_file()))
        .unwrap_or(false)
}

struct TestRuntimeDir {
    path: PathBuf,
}

impl TestRuntimeDir {
    fn new() -> Result<Self> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock before Unix epoch")?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "flashpaste-overlayd-integration-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).with_context(|| format!("create {}", path.display()))?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestRuntimeDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

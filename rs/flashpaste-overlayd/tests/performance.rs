use std::{
    io::{BufRead, BufReader, Write},
    os::unix::fs::FileTypeExt,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use flashpaste_overlayd::SOCKET_NAME;
use serde_json::{json, Value};

const DAEMON_BIN: &str = env!("CARGO_BIN_EXE_flashpaste-overlayd");

#[test]
#[ignore = "performance probe; run manually with --ignored --nocapture"]
fn mcp_highlight_region_headless_latency() -> Result<()> {
    let runtime_dir = TestRuntimeDir::new()?;
    if !unix_listener_bind_supported(runtime_dir.path())? {
        eprintln!("skipping performance probe: Unix listener bind denied by environment");
        return Ok(());
    }

    let socket_path = runtime_dir.path().join(SOCKET_NAME);
    let mut daemon = Command::new(DAEMON_BIN)
        .arg("--headless-test")
        .env("XDG_RUNTIME_DIR", runtime_dir.path())
        .env("FLASHPASTE_OVERLAYD_TEST_MODE", "1")
        .env("RUST_LOG", "flashpaste_overlayd=warn")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn flashpaste-overlayd --headless-test")?;
    wait_for_socket(&socket_path, &mut daemon)?;

    let mut mcp = Command::new(mcp_bin())
        .env("XDG_RUNTIME_DIR", runtime_dir.path())
        .env("RUST_LOG", "error")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn flashpaste-mcp")?;

    let mut stdin = mcp.stdin.take().context("mcp stdin")?;
    let mut stdout = BufReader::new(mcp.stdout.take().context("mcp stdout")?);
    let mut samples = Vec::new();
    let warmup = 20usize;
    let iterations = 520usize;

    for index in 0..iterations {
        let request = json!({
            "jsonrpc": "2.0",
            "id": index + 1,
            "method": "tools/call",
            "params": {
                "name": "highlight_region",
                "arguments": {
                    "shape": "rect",
                    "x": 200,
                    "y": 200,
                    "w": 300,
                    "h": 100,
                    "ttl_ms": 30000
                }
            }
        });
        let start = Instant::now();
        writeln!(stdin, "{request}").context("write MCP request")?;
        stdin.flush().context("flush MCP request")?;
        let mut response = String::new();
        stdout
            .read_line(&mut response)
            .context("read MCP response")?;
        let elapsed = start.elapsed();
        let response: Value = serde_json::from_str(&response).context("parse MCP response")?;
        if response.get("error").is_some() {
            bail!("MCP error response: {response}");
        }
        if response["result"]["isError"].as_bool() == Some(true) {
            bail!("MCP tool error response: {response}");
        }
        if index >= warmup {
            samples.push(elapsed);
        }
    }

    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "id": iterations + 1,
            "method": "tools/call",
            "params": {
                "name": "clear_annotations",
                "arguments": {}
            }
        })
    )
    .context("write clear request")?;
    stdin.flush().context("flush clear request")?;
    let mut response = String::new();
    stdout
        .read_line(&mut response)
        .context("read clear response")?;
    std::thread::sleep(Duration::from_millis(250));

    let idle_cpu_percent = idle_cpu_percent(daemon.id(), Duration::from_secs(3))?;
    drop(stdin);
    let _ = mcp.kill();
    let _ = mcp.wait();
    let _ = daemon.kill();
    let _ = daemon.wait();

    samples.sort();
    let metrics = json!({
        "samples": samples.len(),
        "p50_ms": millis(percentile(&samples, 50)),
        "p95_ms": millis(percentile(&samples, 95)),
        "p99_ms": millis(percentile(&samples, 99)),
        "max_ms": millis(*samples.last().context("latency sample")?),
        "mean_ms": millis(mean(&samples)),
        "idle_cpu_percent_3s": idle_cpu_percent,
    });
    println!("{metrics}");
    Ok(())
}

fn mcp_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/debug/flashpaste-mcp")
}

fn wait_for_socket(socket_path: &Path, daemon: &mut Child) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if is_socket(socket_path) {
            return Ok(());
        }
        if let Some(status) = daemon.try_wait().context("poll daemon")? {
            let stderr = daemon
                .stderr
                .take()
                .map(|mut stderr| {
                    let mut output = String::new();
                    let _ = std::io::Read::read_to_string(&mut stderr, &mut output);
                    output
                })
                .unwrap_or_default();
            bail!("flashpaste-overlayd exited before socket was ready: {status}\n{stderr}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    bail!("timed out waiting for socket {}", socket_path.display())
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
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => Ok(false),
        Err(err) => Err(err).with_context(|| format!("bind probe {}", probe.display())),
    }
}

fn percentile(samples: &[Duration], percentile: usize) -> Duration {
    let index = ((samples.len() - 1) * percentile) / 100;
    samples[index]
}

fn mean(samples: &[Duration]) -> Duration {
    let nanos = samples.iter().map(Duration::as_nanos).sum::<u128>() / samples.len() as u128;
    Duration::from_nanos(nanos as u64)
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn idle_cpu_percent(pid: u32, duration: Duration) -> Result<f64> {
    let ticks_per_second = 100.0;
    let start_ticks = process_ticks(pid)?;
    let start = Instant::now();
    std::thread::sleep(duration);
    let end_ticks = process_ticks(pid)?;
    let elapsed = start.elapsed().as_secs_f64();
    Ok(((end_ticks - start_ticks) as f64 / ticks_per_second) / elapsed * 100.0)
}

fn process_ticks(pid: u32) -> Result<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).context("read proc stat")?;
    let fields: Vec<_> = stat.split_whitespace().collect();
    let user_ticks = fields
        .get(13)
        .context("utime field")?
        .parse::<u64>()
        .context("parse utime")?;
    let system_ticks = fields
        .get(14)
        .context("stime field")?
        .parse::<u64>()
        .context("parse stime")?;
    Ok(user_ticks + system_ticks)
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
            "flashpaste-overlayd-perf-{}-{unique}",
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

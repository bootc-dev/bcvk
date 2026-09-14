//! Async QMP (QEMU Machine Protocol) client.
//!
//! Provides the subset of QMP commands needed for hot-plugging virtio-serial
//! ports at runtime. This enables multiple concurrent SSH sessions to a VM
//! by dynamically adding and removing virtio-serial channels.
//!
//! The protocol is JSON-over-Unix-socket. After connecting, the client must
//! negotiate capabilities before issuing commands.
//!
//! Reference: <https://www.qemu.org/docs/master/interop/qmp-spec.html>

use color_eyre::eyre::{eyre, Context};
use color_eyre::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::debug;

/// A connected QMP client.
#[derive(Debug)]
pub struct QmpClient {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl QmpClient {
    /// Connect to a QMP socket and negotiate capabilities.
    ///
    /// After connecting, reads the QMP greeting and sends
    /// `qmp_capabilities` to enter command mode.
    pub async fn connect(socket_path: &str) -> Result<Self> {
        let stream = UnixStream::connect(socket_path)
            .await
            .with_context(|| format!("connecting to QMP socket at {socket_path}"))?;

        let (read_half, write_half) = stream.into_split();
        let mut client = Self {
            reader: BufReader::new(read_half),
            writer: write_half,
        };

        // Read and discard the QMP greeting
        let greeting = client.read_line().await?;
        debug!("QMP greeting: {}", greeting.trim());

        // Negotiate capabilities
        client.execute("qmp_capabilities", json!({})).await?;
        debug!("QMP capabilities negotiated");

        Ok(client)
    }

    /// Hot-plug a virtio-serial port backed by a Unix socket chardev.
    ///
    /// Creates a `chardev-socket` listening at `socket_path` and attaches
    /// a `virtserialport` device named `port_name` to the VM's existing
    /// virtio-serial controller. The guest sees the port as
    /// `/dev/virtio-ports/{port_name}`.
    ///
    /// `device_id` is used as both the chardev ID and device ID prefix
    /// (chardev: `{device_id}_char`, device: `{device_id}_dev`).
    pub async fn hotplug_virtio_serial(
        &mut self,
        device_id: &str,
        port_name: &str,
        socket_path: &str,
    ) -> Result<()> {
        let char_id = format!("{device_id}_char");
        let dev_id = format!("{device_id}_dev");

        // Step 1: Add the chardev (Unix socket, server mode, non-blocking)
        self.execute(
            "chardev-add",
            json!({
                "id": char_id,
                "backend": {
                    "type": "socket",
                    "data": {
                        "addr": {
                            "type": "unix",
                            "data": { "path": socket_path }
                        },
                        "server": true,
                        "wait": false
                    }
                }
            }),
        )
        .await
        .with_context(|| format!("adding chardev {char_id} at {socket_path}"))?;

        // Step 2: Add the virtserialport device
        self.execute(
            "device_add",
            json!({
                "driver": "virtserialport",
                "chardev": char_id,
                "name": port_name,
                "id": dev_id,
            }),
        )
        .await
        .with_context(|| format!("adding virtserialport {dev_id}"))?;

        debug!(
            "Hot-plugged virtio-serial port '{}' (chardev at {})",
            port_name, socket_path
        );
        Ok(())
    }

    /// Hot-unplug a virtio-serial port previously added with
    /// [`hotplug_virtio_serial`](Self::hotplug_virtio_serial).
    ///
    /// Sends `device_del` for the port device, waits for the
    /// `DEVICE_DELETED` event (up to 5 seconds), then removes the chardev.
    pub async fn hot_unplug_virtio_serial(&mut self, device_id: &str) -> Result<()> {
        let char_id = format!("{device_id}_char");
        let dev_id = format!("{device_id}_dev");

        // Step 1: Request device removal
        self.execute("device_del", json!({ "id": dev_id }))
            .await
            .with_context(|| format!("removing device {dev_id}"))?;

        // Step 2: Wait for DEVICE_DELETED event (the guest must cooperate)
        self.wait_for_event("DEVICE_DELETED")
            .await
            .with_context(|| format!("waiting for {dev_id} to be removed"))?;

        // Step 3: Remove the chardev
        self.execute("chardev-remove", json!({ "id": char_id }))
            .await
            .with_context(|| format!("removing chardev {char_id}"))?;

        debug!("Hot-unplugged virtio-serial device '{}'", device_id);
        Ok(())
    }

    /// Execute a QMP command and return the result.
    pub async fn execute(&mut self, command: &str, arguments: Value) -> Result<Value> {
        let request = if arguments.is_null()
            || (arguments.is_object() && arguments.as_object().map_or(true, |m| m.is_empty()))
        {
            json!({ "execute": command })
        } else {
            json!({ "execute": command, "arguments": arguments })
        };

        let mut request_str = serde_json::to_string(&request)?;
        request_str.push('\n');
        self.writer
            .write_all(request_str.as_bytes())
            .await
            .with_context(|| format!("writing QMP command {command}"))?;

        // Read response, skipping any async events
        loop {
            let line = self.read_line().await?;
            let response: Value = serde_json::from_str(&line)
                .with_context(|| format!("parsing QMP response: {line}"))?;

            if response.get("return").is_some() {
                return Ok(response["return"].clone());
            }
            if let Some(error) = response.get("error") {
                return Err(eyre!("QMP command '{}' failed: {}", command, error));
            }
            // Otherwise it's an async event -- skip and read next line
            if let Some(event) = response.get("event") {
                debug!("QMP event (while waiting for response): {}", event);
            }
        }
    }

    /// Wait for a specific QMP event, discarding other events.
    async fn wait_for_event(&mut self, event_name: &str) -> Result<Value> {
        loop {
            let line = self
                .read_line()
                .await
                .with_context(|| format!("reading while waiting for {event_name} event"))?;
            let msg: Value = serde_json::from_str(&line)
                .with_context(|| format!("parsing QMP message: {line}"))?;

            if let Some(event) = msg.get("event").and_then(|v| v.as_str()) {
                if event == event_name {
                    return Ok(msg);
                }
                debug!("QMP event (not {event_name}): {event}");
            }
        }
    }

    /// Read a single line from the QMP socket.
    async fn read_line(&mut self) -> Result<String> {
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .await
            .with_context(|| "reading from QMP socket")?;
        if n == 0 {
            return Err(eyre!("QMP socket closed unexpectedly"));
        }
        Ok(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    /// A minimal mock QMP server for testing.
    ///
    /// Accepts one connection, sends a greeting, and responds to each
    /// command with `{"return": {}}`. Async events can be injected by
    /// the test before a command response.
    struct MockQmpServer {
        listener: UnixListener,
        socket_path: std::path::PathBuf,
    }

    impl MockQmpServer {
        /// Create a mock server on a temporary Unix socket.
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let socket_path = dir.path().join("qmp.sock");
            let listener = UnixListener::bind(&socket_path).expect("bind mock QMP socket");
            // Leak the tempdir so the socket stays alive for the test duration
            std::mem::forget(dir);
            Self {
                listener,
                socket_path,
            }
        }

        /// Run the mock server, handling one client connection.
        ///
        /// `handler` receives each command as a `Value` and returns a
        /// response `Value` (or multiple values to inject events before
        /// the response).
        async fn run<F>(self, handler: F)
        where
            F: Fn(Value) -> Vec<Value>,
        {
            let (stream, _) = self.listener.accept().await.expect("accept");
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);

            // Send QMP greeting
            let greeting = json!({
                "QMP": {
                    "version": {"qemu": {"micro": 0, "minor": 0, "major": 9}},
                    "capabilities": ["oob"]
                }
            });
            write_half
                .write_all(format!("{}\n", greeting).as_bytes())
                .await
                .unwrap();

            // Process commands
            let mut line = String::new();
            while reader.read_line(&mut line).await.unwrap() > 0 {
                let cmd: Value = serde_json::from_str(line.trim()).expect("parse command");
                let responses = handler(cmd);
                for resp in responses {
                    write_half
                        .write_all(format!("{}\n", resp).as_bytes())
                        .await
                        .unwrap();
                }
                line.clear();
            }
        }
    }

    #[tokio::test]
    async fn test_connect_and_negotiate() {
        let server = MockQmpServer::new();
        let path = server.socket_path.to_str().unwrap().to_string();

        let server_handle = tokio::spawn(server.run(|_cmd| vec![json!({"return": {}})]));

        let client = QmpClient::connect(&path).await;
        assert!(client.is_ok(), "connect failed: {:?}", client.err());
        drop(client);
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn test_execute_returns_result() {
        let server = MockQmpServer::new();
        let path = server.socket_path.to_str().unwrap().to_string();

        let server_handle = tokio::spawn(server.run(|cmd| {
            let command = cmd["execute"].as_str().unwrap_or("");
            match command {
                "qmp_capabilities" => vec![json!({"return": {}})],
                "query-version" => {
                    vec![json!({"return": {"qemu": {"major": 9}}})]
                }
                _ => vec![json!({"return": {}})],
            }
        }));

        let mut client = QmpClient::connect(&path).await.unwrap();
        let result = client.execute("query-version", json!({})).await.unwrap();
        assert_eq!(result["qemu"]["major"], 9);

        drop(client);
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn test_execute_handles_error() {
        let server = MockQmpServer::new();
        let path = server.socket_path.to_str().unwrap().to_string();

        let server_handle = tokio::spawn(server.run(|cmd| {
            let command = cmd["execute"].as_str().unwrap_or("");
            match command {
                "qmp_capabilities" => vec![json!({"return": {}})],
                _ => vec![json!({
                    "error": {"class": "GenericError", "desc": "device not found"}
                })],
            }
        }));

        let mut client = QmpClient::connect(&path).await.unwrap();
        let result = client.execute("device_del", json!({"id": "nope"})).await;
        assert!(result.is_err());
        assert!(format!("{:?}", result.unwrap_err()).contains("device not found"),);

        drop(client);
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn test_execute_skips_async_events() {
        let server = MockQmpServer::new();
        let path = server.socket_path.to_str().unwrap().to_string();

        let server_handle = tokio::spawn(server.run(|cmd| {
            let command = cmd["execute"].as_str().unwrap_or("");
            match command {
                "qmp_capabilities" => vec![json!({"return": {}})],
                "device_del" => {
                    // Inject an async event before the response
                    vec![
                        json!({"event": "SOME_OTHER_EVENT", "timestamp": {}}),
                        json!({"return": {}}),
                    ]
                }
                _ => vec![json!({"return": {}})],
            }
        }));

        let mut client = QmpClient::connect(&path).await.unwrap();
        // Should succeed despite the injected event
        let result = client.execute("device_del", json!({"id": "dev0"})).await;
        assert!(result.is_ok());

        drop(client);
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn test_hotplug_sends_chardev_and_device() {
        let server = MockQmpServer::new();
        let path = server.socket_path.to_str().unwrap().to_string();

        let commands_seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let commands_clone = commands_seen.clone();

        let server_handle = tokio::spawn(server.run(move |cmd| {
            let command = cmd["execute"].as_str().unwrap_or("").to_string();
            commands_clone.lock().unwrap().push(command);
            vec![json!({"return": {}})]
        }));

        let mut client = QmpClient::connect(&path).await.unwrap();
        client
            .hotplug_virtio_serial("test0", "org.bcvk.ssh.0", "/tmp/test.sock")
            .await
            .unwrap();

        drop(client);
        let _ = server_handle.await;

        let seen = commands_seen.lock().unwrap();
        assert_eq!(*seen, vec!["qmp_capabilities", "chardev-add", "device_add"]);
    }
}

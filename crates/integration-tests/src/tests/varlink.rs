//! Integration tests for the varlink IPC interface.
//!
//! Tests spawn bcvk as a child process with a connected socketpair,
//! simulating socket activation, then use zlink proxy traits to make
//! typed varlink calls. One test exercises `varlinkctl exec:` as a
//! sanity check.
//!
//! ⚠️  **CRITICAL INTEGRATION TEST POLICY** ⚠️
//!
//! INTEGRATION TESTS MUST NEVER "warn and continue" ON FAILURES!
//! If something is not working, use assert/unwrap to fail hard.

use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::{Arc, OnceLock};

use cap_std_ext::cmdext::CapStdExtCommandExt;
use color_eyre::Result;
use serde::Deserialize;

use crate::{get_bck_command, get_test_image, integration_test, shell};

// ---------------------------------------------------------------------------
// Client-side response types (redefined to keep integration tests
// independent of the bcvk library)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListReply {
    images: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PsReply {
    container_ids: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, Deserialize, Default)]
#[allow(dead_code)]
struct EphemeralRunOpts {
    ssh_keygen: Option<bool>,
    name: Option<String>,
    label: Option<Vec<String>>,
    network: Option<String>,
    env: Option<Vec<String>>,
    rm: Option<bool>,
    itype: Option<String>,
    memory: Option<String>,
    vcpus: Option<u32>,
    console: Option<bool>,
    bind: Option<Vec<String>>,
    ro_bind: Option<Vec<String>>,
    mount_disk_files: Option<Vec<String>>,
    kargs: Option<Vec<String>>,
    add_swap: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RunReply {
    container_id: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GetSshConnectionInfoReply {
    container_id: String,
    key_path: String,
    user: String,
    host: String,
    port: u16,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ToDiskReply {
    path: String,
    cached: bool,
}

// ---------------------------------------------------------------------------
// Error types (needed by proxy return types)
// ---------------------------------------------------------------------------

#[derive(Debug, zlink::ReplyError, zlink::introspect::ReplyError)]
#[zlink(interface = "io.bootc.vk.images")]
enum ImagesError {
    PodmanError {
        #[allow(dead_code)]
        message: String,
    },
}

impl std::fmt::Display for ImagesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PodmanError { message } => write!(f, "podman error: {message}"),
        }
    }
}

impl std::error::Error for ImagesError {}

#[derive(Debug, zlink::ReplyError, zlink::introspect::ReplyError)]
#[zlink(interface = "io.bootc.vk.ephemeral")]
enum EphemeralError {
    PodmanError {
        #[allow(dead_code)]
        message: String,
    },
}

impl std::fmt::Display for EphemeralError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PodmanError { message } => write!(f, "podman error: {message}"),
        }
    }
}

impl std::error::Error for EphemeralError {}

#[derive(Debug, zlink::ReplyError, zlink::introspect::ReplyError)]
#[zlink(interface = "io.bootc.vk.todisk")]
enum ToDiskError {
    Failed {
        #[allow(dead_code)]
        message: String,
    },
}

impl std::fmt::Display for ToDiskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed { message } => write!(f, "to-disk failed: {message}"),
        }
    }
}

impl std::error::Error for ToDiskError {}

// ---------------------------------------------------------------------------
// Proxy traits
// ---------------------------------------------------------------------------

#[zlink::proxy("io.bootc.vk.images")]
trait ImagesProxy {
    async fn list(&mut self) -> zlink::Result<Result<ListReply, ImagesError>>;
}

#[zlink::proxy("io.bootc.vk.ephemeral")]
trait EphemeralProxy {
    async fn ps(&mut self) -> zlink::Result<Result<PsReply, EphemeralError>>;

    async fn run(
        &mut self,
        image: String,
        opts: Option<EphemeralRunOpts>,
    ) -> zlink::Result<Result<RunReply, EphemeralError>>;

    async fn get_ssh_connection_info(
        &mut self,
        container_id: String,
    ) -> zlink::Result<Result<GetSshConnectionInfoReply, EphemeralError>>;
}

#[zlink::proxy("io.bootc.vk.todisk")]
trait ToDiskProxy {
    async fn to_disk(
        &mut self,
        source_image: String,
        target_disk: String,
        format: Option<String>,
        disk_size: Option<String>,
        filesystem: Option<String>,
        root_size: Option<String>,
        kargs: Option<Vec<String>>,
    ) -> zlink::Result<Result<ToDiskReply, ToDiskError>>;
}

// ---------------------------------------------------------------------------
// Helper: spawn bcvk with socket activation
// ---------------------------------------------------------------------------

struct ActivatedBcvk {
    conn: zlink::Connection<zlink::unix::Stream>,
    rt: tokio::runtime::Runtime,
}

/// Spawn bcvk with socket activation and return a zlink connection.
///
/// Creates a Unix socketpair and spawns bcvk directly with socket-activation
/// env vars (`LISTEN_FDS`, `LISTEN_PID`, `LISTEN_FDNAMES`) set via
/// `libc::setenv` in a `pre_exec` hook. We avoid `Command::env()` because
/// Rust's std overwrites the global `environ` pointer *after* `pre_exec`
/// callbacks run, which would discard our `LISTEN_PID` value (which must
/// equal `getpid()` for libsystemd's `receive_descriptors` to accept it).
/// See `library/std/src/sys/process/unix/unix.rs` in rust-lang/rust.
///
/// The child process is bound to the calling thread via
/// `lifecycle_bind_to_parent_thread`, so it is automatically killed when the
/// test thread exits. This must NOT be called inside `spawn_blocking`.
fn activated_connection() -> Result<ActivatedBcvk> {
    let bck = get_bck_command()?;
    let (ours, theirs) = UnixStream::pair()?;
    let theirs_fd: Arc<std::os::fd::OwnedFd> = Arc::new(theirs.into());

    let mut cmd = Command::new(&bck);
    // Do NOT use cmd.env() here -- it causes Rust's Command to build an
    // envp array that replaces environ after our pre_exec setenv calls.
    cmd.take_fd_n(theirs_fd, 3)
        .lifecycle_bind_to_parent_thread();
    #[allow(unsafe_code)]
    unsafe {
        cmd.pre_exec(|| {
            let pid = rustix::process::getpid();
            let pid_dec = rustix::path::DecInt::new(pid.as_raw_nonzero().get());
            libc::setenv(c"LISTEN_PID".as_ptr(), pid_dec.as_c_str().as_ptr(), 1);
            libc::setenv(c"LISTEN_FDS".as_ptr(), c"1".as_ptr(), 1);
            libc::setenv(c"LISTEN_FDNAMES".as_ptr(), c"varlink".as_ptr(), 1);
            Ok(())
        });
    }
    let _child = cmd.spawn()?;

    ours.set_nonblocking(true)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let tokio_stream = rt.block_on(async { tokio::net::UnixStream::from_std(ours) })?;
    let zlink_stream = zlink::unix::Stream::from(tokio_stream);
    let conn = zlink::Connection::from(zlink_stream);

    Ok(ActivatedBcvk { conn, rt })
}

/// Remove a container by ID, ignoring errors (for test cleanup).
fn cleanup_container(id: &str) {
    let _ = Command::new("podman").args(["rm", "-f", "--", id]).output();
}

// ===========================================================================
// Tests: io.bootc.vk.images
// ===========================================================================

/// Verify that the images `List` method returns a vec of image name strings.
fn test_varlink_images_list() -> Result<()> {
    let mut bcvk = activated_connection()?;
    let reply = bcvk.rt.block_on(async { bcvk.conn.list().await })??;
    // In CI there may be no bootc images; just verify deserialization succeeds.
    for name in &reply.images {
        assert!(!name.is_empty(), "image name must not be empty");
    }
    Ok(())
}
integration_test!(test_varlink_images_list);

/// Verify that the test image appears in the images List after pulling it.
///
/// This test pulls the primary test image (which has the `containers.bootc=1`
/// label) and then verifies it appears in the varlink List response.
fn test_varlink_images_list_contains_test_image() -> Result<()> {
    let image = get_test_image();

    // Ensure the image is pulled
    let sh = shell()?;
    xshell::cmd!(sh, "podman pull -q {image}").run()?;

    let mut bcvk = activated_connection()?;
    let reply = bcvk.rt.block_on(async { bcvk.conn.list().await })??;

    assert!(
        reply.images.iter().any(|name| name.contains(&image)),
        "expected test image {image} in varlink images list, got: {:?}",
        reply.images
    );
    Ok(())
}
integration_test!(test_varlink_images_list_contains_test_image);

// ===========================================================================
// Tests: io.bootc.vk.ephemeral
// ===========================================================================

/// Verify that the ephemeral `Ps` method returns container ID strings.
fn test_varlink_ephemeral_ps() -> Result<()> {
    let mut bcvk = activated_connection()?;
    let reply = bcvk.rt.block_on(async { bcvk.conn.ps().await })??;
    for id in &reply.container_ids {
        assert!(!id.is_empty(), "container ID must not be empty");
    }
    Ok(())
}
integration_test!(test_varlink_ephemeral_ps);

/// Test that `Run` with a nonexistent image returns an error.
fn test_varlink_ephemeral_run_bad_image() -> Result<()> {
    let mut bcvk = activated_connection()?;
    let result = bcvk.rt.block_on(async {
        bcvk.conn
            .run(
                "nonexistent-image-that-should-not-exist:latest".to_string(),
                None,
            )
            .await
    })?;
    match result {
        Err(EphemeralError::PodmanError { .. }) => Ok(()),
        Ok(reply) => Err(color_eyre::eyre::eyre!(
            "expected error for nonexistent image, got container_id: {}",
            reply.container_id
        )),
    }
}
integration_test!(test_varlink_ephemeral_run_bad_image);

/// End-to-end test: Run a VM, verify it in Ps, get SSH connection info,
/// and actually SSH into it using the returned values.
fn test_varlink_ephemeral_run_ps_and_ssh() -> Result<()> {
    let image = get_test_image();
    let mut bcvk = activated_connection()?;

    // Launch an ephemeral VM with SSH key injection
    let run_reply = bcvk.rt.block_on(async {
        bcvk.conn
            .run(
                image.clone(),
                Some(EphemeralRunOpts {
                    ssh_keygen: Some(true),
                    ..Default::default()
                }),
            )
            .await
    })??;
    assert!(
        !run_reply.container_id.is_empty(),
        "expected non-empty container_id from Run"
    );

    // Verify it shows up in Ps
    let ps_reply = bcvk.rt.block_on(async { bcvk.conn.ps().await })??;
    assert!(
        ps_reply.container_ids.contains(&run_reply.container_id),
        "expected container {} to appear in Ps, got: {:?}",
        run_reply.container_id,
        ps_reply.container_ids
    );

    // Get SSH connection info
    let ssh = bcvk.rt.block_on(async {
        bcvk.conn
            .get_ssh_connection_info(run_reply.container_id.clone())
            .await
    })??;

    // Use the returned info to actually SSH into the VM.
    // Retry with backoff since the VM needs time to boot.
    let port = ssh.port.to_string();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        let result = Command::new("podman")
            .args([
                "exec",
                "--",
                &ssh.container_id,
                "ssh",
                "-i",
                &ssh.key_path,
                "-p",
                &port,
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=2",
                "-o",
                "LogLevel=ERROR",
                &format!("{}@{}", ssh.user, ssh.host),
                "true",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        match result {
            Ok(status) if status.success() => break,
            _ if std::time::Instant::now() > deadline => {
                cleanup_container(&run_reply.container_id);
                return Err(color_eyre::eyre::eyre!(
                    "SSH did not become ready within 120s using info from GetSshConnectionInfo"
                ));
            }
            _ => std::thread::sleep(std::time::Duration::from_secs(2)),
        }
    }

    // Clean up
    cleanup_container(&run_reply.container_id);
    Ok(())
}
integration_test!(test_varlink_ephemeral_run_ps_and_ssh);

// ===========================================================================
// Tests: io.bootc.vk.todisk
// ===========================================================================

/// Test that `ToDisk` with a nonexistent image returns a `Failed` error.
fn test_varlink_todisk_bad_image() -> Result<()> {
    let mut bcvk = activated_connection()?;
    let target = tempfile::NamedTempFile::new()?;
    let target_path = target.path().to_str().unwrap().to_string();
    // Remove the temp file so to_disk sees a fresh path
    drop(target);

    let result = bcvk.rt.block_on(async {
        bcvk.conn
            .to_disk(
                "nonexistent-image-that-should-not-exist:latest".to_string(),
                target_path,
                None,
                None,
                None,
                None,
                None,
            )
            .await
    })?;
    match result {
        Err(ToDiskError::Failed { .. }) => Ok(()),
        Ok(reply) => Err(color_eyre::eyre::eyre!(
            "expected Failed error for nonexistent image, got path: {}",
            reply.path
        )),
    }
}
integration_test!(test_varlink_todisk_bad_image);

/// Test that `ToDisk` rejects invalid format strings.
fn test_varlink_todisk_bad_format() -> Result<()> {
    let mut bcvk = activated_connection()?;
    let td = tempfile::TempDir::new()?;
    let target_path = td.path().join("disk.img");
    let target = target_path.to_str().unwrap().to_string();

    let result = bcvk.rt.block_on(async {
        bcvk.conn
            .to_disk(
                // Image doesn't matter, format validation happens first
                "anything:latest".to_string(),
                target,
                Some("vdi".to_string()), // invalid format
                None,
                None,
                None,
                None,
            )
            .await
    })?;
    match result {
        Err(ToDiskError::Failed { message }) => {
            assert!(
                message.contains("unsupported disk format"),
                "expected 'unsupported disk format' in error, got: {message}"
            );
            Ok(())
        }
        Ok(reply) => Err(color_eyre::eyre::eyre!(
            "expected Failed error for invalid format, got path: {}",
            reply.path
        )),
    }
}
integration_test!(test_varlink_todisk_bad_format);

/// Test the ToDisk success path: create a disk image from the test image.
///
/// This is a heavyweight test that launches a VM internally. It verifies
/// the reply contains a valid path, that the file exists, and that it is
/// not marked as cached (first run).
fn test_varlink_todisk_creates_disk() -> Result<()> {
    let image = get_test_image();
    let td = tempfile::TempDir::new()?;
    let target_path = td.path().join("test-disk.raw");
    let target = target_path.to_str().unwrap().to_string();

    let mut bcvk = activated_connection()?;
    let reply = bcvk.rt.block_on(async {
        bcvk.conn
            .to_disk(
                image,
                target.clone(),
                Some("raw".to_string()),
                Some("10G".to_string()),
                None,
                None,
                None,
            )
            .await
    })??;

    assert_eq!(reply.path, target, "reply path should match target_disk");
    assert!(!reply.cached, "first run should not be cached");
    assert!(
        target_path.exists(),
        "disk image should exist at {}",
        target_path.display()
    );

    let metadata = std::fs::metadata(&target_path)?;
    assert!(metadata.len() > 0, "disk image should have nonzero size");

    Ok(())
}
integration_test!(test_varlink_todisk_creates_disk);

// ===========================================================================
// Tests: cross-interface / varlinkctl
// ===========================================================================

/// Check whether `varlinkctl` can successfully talk to a zlink-based server.
///
/// Older versions of systemd's `varlinkctl` (or versions affected by
/// <https://github.com/z-galaxy/zlink/issues/233>) send an introspection
/// request that zlink cannot deserialize, causing the connection to be
/// immediately dropped. Rather than failing hard on such systems, we
/// detect the incompatibility here and let callers skip or adapt.
fn varlinkctl_is_compatible() -> bool {
    static RESULT: OnceLock<bool> = OnceLock::new();
    *RESULT.get_or_init(|| {
        let bck = match get_bck_command() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("note: varlinkctl probe: get_bck_command() failed: {e}");
                return false;
            }
        };
        let sh = match shell() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("note: varlinkctl probe: shell() failed: {e}");
                return false;
            }
        };
        // Try a varlinkctl call; if it fails, the tool is either missing
        // or incompatible with our server.  The most common incompatibility
        // is that varlinkctl sends org.varlink.service.GetInfo during its
        // initial handshake, which zlink cannot deserialize (zlink#233).
        let ok = xshell::cmd!(sh, "varlinkctl call exec:{bck} io.bootc.vk.images.List")
            .ignore_status()
            .read()
            .map(|output| {
                serde_json::from_str::<serde_json::Value>(&output)
                    .ok()
                    .and_then(|v| v.get("images").cloned())
                    .is_some()
            })
            .unwrap_or(false);
        if !ok {
            eprintln!(
                "note: varlinkctl probe failed; varlinkctl-dependent tests will be skipped \
                 (see https://github.com/z-galaxy/zlink/issues/233)"
            );
        }
        ok
    })
}

/// Cross-check the images `List` API using the Rust varlink client, and
/// optionally verify that `varlinkctl` returns the same result when it is
/// available and compatible.
///
/// The Rust client path is the primary assertion — it always runs. The
/// `varlinkctl` cross-check is best-effort: if the installed systemd is
/// too old or suffers from the zlink introspection deserialization bug
/// (<https://github.com/z-galaxy/zlink/issues/233>), the cross-check is
/// skipped with a log message.
fn test_varlink_images_list_crosscheck() -> Result<()> {
    let image = get_test_image();

    // Ensure the test image is pulled so we have at least one image to compare
    let sh = shell()?;
    xshell::cmd!(sh, "podman pull -q {image}").run()?;

    // Primary path: Rust varlink client
    let mut bcvk = activated_connection()?;
    let reply = bcvk.rt.block_on(async { bcvk.conn.list().await })??;
    assert!(
        !reply.images.is_empty(),
        "Rust client: expected at least one image"
    );
    assert!(
        reply.images.iter().any(|name| name.contains(&image)),
        "Rust client: expected test image {image} in list, got: {:?}",
        reply.images
    );

    // Cross-check: varlinkctl (best-effort)
    if varlinkctl_is_compatible() {
        let bck = get_bck_command()?;
        let output =
            xshell::cmd!(sh, "varlinkctl call exec:{bck} io.bootc.vk.images.List").read()?;
        let parsed: serde_json::Value = serde_json::from_str(&output)?;
        let varlinkctl_images = parsed
            .get("images")
            .and_then(|v| v.as_array())
            .expect("varlinkctl response missing 'images' array");
        let varlinkctl_names: Vec<&str> = varlinkctl_images
            .iter()
            .filter_map(|v| v.as_str())
            .collect();

        // Both should see the same set of images
        assert_eq!(
            reply.images.len(),
            varlinkctl_names.len(),
            "image count mismatch: Rust client={:?}, varlinkctl={:?}",
            reply.images,
            varlinkctl_names
        );
        for img in &reply.images {
            assert!(
                varlinkctl_names.contains(&img.as_str()),
                "varlinkctl missing image {img} that Rust client returned"
            );
        }
        eprintln!(
            "varlinkctl cross-check passed ({} images)",
            reply.images.len()
        );
    } else {
        eprintln!(
            "note: skipping varlinkctl cross-check (varlinkctl missing or incompatible \
             with this zlink server, see https://github.com/z-galaxy/zlink/issues/233)"
        );
    }

    Ok(())
}
integration_test!(test_varlink_images_list_crosscheck);

/// Verify that `varlinkctl call` against the images List method works.
///
/// Skipped when `varlinkctl` is not compatible with the zlink server
/// (e.g. systemd < 258 due to <https://github.com/z-galaxy/zlink/issues/233>).
fn test_varlink_exec_varlinkctl() -> Result<()> {
    if !varlinkctl_is_compatible() {
        eprintln!(
            "note: skipping test_varlink_exec_varlinkctl (varlinkctl missing or incompatible, \
             see https://github.com/z-galaxy/zlink/issues/233)"
        );
        return Ok(());
    }
    let sh = shell()?;
    let bck = get_bck_command()?;
    let output = xshell::cmd!(sh, "varlinkctl call exec:{bck} io.bootc.vk.images.List").read()?;
    let parsed: serde_json::Value = serde_json::from_str(&output)?;
    assert!(
        parsed.get("images").is_some(),
        "response missing 'images' key"
    );
    Ok(())
}
integration_test!(test_varlink_exec_varlinkctl);

/// Test that `varlinkctl introspect` shows all three interface names.
///
/// Skipped when `varlinkctl` is not compatible with the zlink server.
fn test_varlink_introspect_varlinkctl() -> Result<()> {
    if !varlinkctl_is_compatible() {
        eprintln!(
            "note: skipping test_varlink_introspect_varlinkctl (varlinkctl missing or incompatible, \
             see https://github.com/z-galaxy/zlink/issues/233)"
        );
        return Ok(());
    }
    let sh = shell()?;
    let bck = get_bck_command()?;
    let output = xshell::cmd!(sh, "varlinkctl introspect exec:{bck} io.bootc.vk.images").read()?;
    assert!(
        output.contains("io.bootc.vk.images"),
        "introspect output missing 'io.bootc.vk.images'"
    );
    let output =
        xshell::cmd!(sh, "varlinkctl introspect exec:{bck} io.bootc.vk.ephemeral").read()?;
    assert!(
        output.contains("io.bootc.vk.ephemeral"),
        "introspect output missing 'io.bootc.vk.ephemeral'"
    );
    let output = xshell::cmd!(sh, "varlinkctl introspect exec:{bck} io.bootc.vk.todisk").read()?;
    assert!(
        output.contains("io.bootc.vk.todisk"),
        "introspect output missing 'io.bootc.vk.todisk'"
    );
    Ok(())
}
integration_test!(test_varlink_introspect_varlinkctl);

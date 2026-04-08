//! Host resource detection and VM concurrency control.
//!
//! Integration tests that dispatch to bcvk VMs must avoid
//! overcommitting host memory.  This module provides:
//!
//! 1. Memory detection (physical + cgroup v2/v1 limits)
//! 2. A pipe-based jobserver (like `make -j`) that limits total
//!    VM memory across all concurrent test processes
//!
//! The jobserver uses a Unix pipe filled with tokens, where each
//! token represents 128 MiB of VM memory budget.  A test that needs
//! a 512 MiB VM (u1.nano) reads 4 tokens; a 2 GiB VM reads 16.
//! Tokens are returned when the VM exits.
//!
//! ## Setup
//!
//! The Justfile (or CI script) creates the jobserver before running
//! tests.  The recommended pattern:
//!
//! ```bash
//! eval "$(my-test-binary --vm-jobserver)"
//! cargo nextest run ...
//! ```
//!
//! The `--vm-jobserver` flag creates a pipe, fills it with tokens
//! based on detected host memory, and prints shell commands to
//! export `ITEST_VM_FDS=<read_fd>,<write_fd>`.  All child
//! processes inherit these fds.
//!
//! If `ITEST_VM_FDS` is not set when `require_root()` runs, the
//! harness creates a process-local jobserver as fallback.  This
//! works for `cargo test` (single process) but not for nextest
//! (separate processes per test).

use std::cmp::min;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::io::{FromRawFd, RawFd};
use std::path::Path;
use std::sync::OnceLock;

/// Default VM memory in MiB (matches u1.nano).
pub(crate) const DEFAULT_VM_MEMORY_MIB: u32 = 512;

/// Default VM vCPU count.
pub(crate) const DEFAULT_VM_VCPUS: u32 = 1;

/// Fraction of host memory available for VMs (default 70%).
const DEFAULT_MEMORY_FRACTION: f64 = 0.70;

/// Each jobserver token represents this many MiB of VM memory.
pub(crate) const TOKEN_MIB: u32 = 128;

/// Resolve an instance type string (e.g. `"u1.nano"`) to its memory in MiB.
///
/// Returns `None` for unrecognised types — callers should fall back to
/// [`DEFAULT_VM_MEMORY_MIB`].
pub(crate) fn itype_memory_mib(itype: &str) -> Option<u32> {
    // Keep in sync with crates/kit/src/instancetypes.rs.
    // We duplicate a small table here so that itest doesn't depend on kit.
    match itype {
        "u1.nano" => Some(512),
        "u1.micro" => Some(1024),
        "u1.small" => Some(2048),
        "u1.medium" => Some(4096),
        "u1.2xmedium" => Some(4096),
        "u1.large" => Some(8192),
        "u1.xlarge" => Some(16384),
        "u1.2xlarge" => Some(32768),
        "u1.4xlarge" => Some(65536),
        "u1.8xlarge" => Some(131072),
        _ => None,
    }
}

// ── Pipe-based jobserver ────────────────────────────────────────────

/// A make-style pipe jobserver for VM memory budgeting.
///
/// The pipe contains N byte-tokens, where N = `available_memory / TOKEN_MIB`.
/// Acquiring K tokens blocks until K bytes can be read from the pipe.
/// Releasing writes them back.
pub(crate) struct VmJobserver {
    read_fd: RawFd,
    write_fd: RawFd,
}

/// Held jobserver tokens.  Dropping returns them to the pipe.
pub(crate) struct VmPermit {
    write_fd: RawFd,
    count: u32,
}

impl Drop for VmPermit {
    fn drop(&mut self) {
        let buf = vec![b'+'; self.count as usize];
        let mut f = unsafe { fs::File::from_raw_fd(self.write_fd) };
        let _ = f.write_all(&buf);
        // Don't close — fd is shared
        std::mem::forget(f);
    }
}

impl VmJobserver {
    /// Create a new jobserver pipe and fill it with `tokens` tokens.
    ///
    /// The pipe fds have `CLOEXEC` cleared so they are inherited by
    /// child processes (including nextest-launched test binaries).
    pub(crate) fn create(tokens: u32) -> io::Result<Self> {
        let (read_fd, write_fd) = pipe_fds()?;

        // Fill the pipe with tokens
        let buf = vec![b'+'; tokens as usize];
        let mut f = unsafe { fs::File::from_raw_fd(write_fd) };
        f.write_all(&buf)?;
        std::mem::forget(f);

        Ok(Self { read_fd, write_fd })
    }

    /// Adopt an existing jobserver from inherited file descriptors.
    fn from_fds(read_fd: RawFd, write_fd: RawFd) -> Self {
        Self { read_fd, write_fd }
    }

    /// The read and write file descriptors.
    pub(crate) fn fds(&self) -> (RawFd, RawFd) {
        (self.read_fd, self.write_fd)
    }

    /// Acquire `count` tokens (each = 128 MiB of VM memory).
    ///
    /// Blocks until enough tokens are available.
    pub fn acquire(&self, count: u32) -> io::Result<VmPermit> {
        let mut buf = vec![0u8; count as usize];
        let mut f = unsafe { fs::File::from_raw_fd(self.read_fd) };
        f.read_exact(&mut buf)?;
        std::mem::forget(f);

        Ok(VmPermit {
            write_fd: self.write_fd,
            count,
        })
    }
}

/// Create a pipe and return (read_fd, write_fd) with CLOEXEC cleared.
fn pipe_fds() -> io::Result<(RawFd, RawFd)> {
    use rustix::fd::IntoRawFd;
    use rustix::io::{fcntl_setfd, FdFlags};
    use rustix::pipe::pipe;

    let (reader, writer) = pipe().map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))?;

    // Clear CLOEXEC so children inherit the fds
    fcntl_setfd(&reader, FdFlags::empty())
        .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))?;
    fcntl_setfd(&writer, FdFlags::empty())
        .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()))?;

    Ok((reader.into_raw_fd(), writer.into_raw_fd()))
}

/// Get the global VM jobserver.
///
/// Checks `ITEST_VM_FDS` for inherited fds first (set by the Justfile
/// via `--vm-jobserver`).  Falls back to creating a process-local
/// jobserver — this works for `cargo test` (fork-exec capture in a
/// single process tree) but not for nextest (separate processes).
pub(crate) fn vm_jobserver() -> &'static VmJobserver {
    static JS: OnceLock<VmJobserver> = OnceLock::new();
    JS.get_or_init(|| {
        if let Some(js) = inherit_jobserver() {
            eprintln!("itest: inherited VM jobserver from ITEST_VM_FDS");
            return js;
        }

        let tokens = compute_token_count();
        let budget_mib = tokens * TOKEN_MIB;
        eprintln!(
            "itest: created VM jobserver: {tokens} token(s) \
             ({budget_mib} MiB budget, override with ITEST_VM_SLOTS)"
        );
        let js = VmJobserver::create(tokens).expect("failed to create VM jobserver pipe");

        // Export for fork-exec children (itest's own capture mode)
        let (r, w) = js.fds();
        // SAFETY: called from OnceLock init, effectively single-threaded
        unsafe {
            std::env::set_var("ITEST_VM_FDS", format!("{r},{w}"));
        }

        js
    })
}

/// Try to inherit a jobserver from `ITEST_VM_FDS=<read>,<write>`.
fn inherit_jobserver() -> Option<VmJobserver> {
    let val = std::env::var("ITEST_VM_FDS").ok()?;
    let (r, w) = val.split_once(',')?;
    let read_fd: RawFd = r.trim().parse().ok()?;
    let write_fd: RawFd = w.trim().parse().ok()?;

    // Verify the fds are valid
    if rustix::fs::fstat(unsafe { rustix::fd::BorrowedFd::borrow_raw(read_fd) }).is_err() {
        return None;
    }

    Some(VmJobserver::from_fds(read_fd, write_fd))
}

/// Compute how many tokens (128 MiB each) fit in the VM budget.
pub(crate) fn compute_token_count() -> u32 {
    if let Some(slots) = env_u32("ITEST_VM_SLOTS") {
        return slots.max(1);
    }

    let host_mem_mib = detect_memory_mib();
    let fraction = env_f64("ITEST_VM_MEMORY_FRACTION").unwrap_or(DEFAULT_MEMORY_FRACTION);
    let available_mib = (host_mem_mib as f64 * fraction) as u32;
    (available_mib / TOKEN_MIB).max(1)
}

// ── Memory detection ────────────────────────────────────────────────

/// Detect available memory in MiB, respecting cgroup limits.
fn detect_memory_mib() -> u32 {
    detect_memory_mib_from(
        Path::new("/proc/meminfo"),
        Path::new("/proc/self/cgroup"),
        Path::new("/sys/fs/cgroup"),
    )
}

fn detect_memory_mib_from(meminfo: &Path, self_cgroup: &Path, cgroup_root: &Path) -> u32 {
    let phys_kib = parse_meminfo_total(meminfo).unwrap_or(4 * 1024 * 1024);
    let phys_bytes = phys_kib * 1024;

    let cgroup_bytes = detect_cgroup_memory_limit(self_cgroup, cgroup_root);

    let effective = match cgroup_bytes {
        Some(limit) if limit < phys_bytes => limit,
        _ => phys_bytes,
    };

    (effective / (1024 * 1024)) as u32
}

fn parse_meminfo_total(path: &Path) -> Option<u64> {
    let content = fs::read_to_string(path).ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            return rest.trim().split_whitespace().next()?.parse().ok();
        }
    }
    None
}

fn detect_cgroup_memory_limit(self_cgroup: &Path, cgroup_root: &Path) -> Option<u64> {
    detect_cgroupv2_memory(self_cgroup, cgroup_root).or_else(|| detect_cgroupv1_memory(cgroup_root))
}

fn detect_cgroupv2_memory(self_cgroup: &Path, cgroup_root: &Path) -> Option<u64> {
    let content = fs::read_to_string(self_cgroup).ok()?;
    let cgroup_path = content.lines().find_map(|line| {
        let line = line.trim();
        line.starts_with("0::").then(|| line[3..].to_string())
    })?;

    let mut min_limit: Option<u64> = None;
    let mut path = cgroup_root.join(cgroup_path.trim_start_matches('/'));

    loop {
        if let Ok(content) = fs::read_to_string(path.join("memory.max")) {
            let content = content.trim();
            if content != "max" {
                if let Ok(limit) = content.parse::<u64>() {
                    min_limit = Some(min_limit.map_or(limit, |cur| min(cur, limit)));
                }
            }
        }

        if path == cgroup_root {
            break;
        }
        match path.parent() {
            Some(parent) if parent >= cgroup_root => path = parent.to_path_buf(),
            _ => break,
        }
    }

    min_limit
}

fn detect_cgroupv1_memory(cgroup_root: &Path) -> Option<u64> {
    let limit: u64 = fs::read_to_string(cgroup_root.join("memory/memory.limit_in_bytes"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    (limit <= (1u64 << 62)).then_some(limit)
}

fn env_u32(name: &str) -> Option<u32> {
    std::env::var(name).ok()?.parse().ok()
}

fn env_f64(name: &str) -> Option<f64> {
    std::env::var(name).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_meminfo(dir: &Path, total_kib: u64) {
        fs::write(
            dir.join("meminfo"),
            format!(
                "MemTotal:       {total_kib} kB\n\
                 MemFree:        1000000 kB\n\
                 MemAvailable:   2000000 kB\n"
            ),
        )
        .unwrap();
    }

    fn write_cgroupv2(dir: &Path, cgroup_path: &str, memory_max: &str) {
        fs::write(dir.join("self_cgroup"), format!("0::{cgroup_path}\n")).unwrap();
        let cg_dir = dir
            .join("cgroup_root")
            .join(cgroup_path.trim_start_matches('/'));
        fs::create_dir_all(&cg_dir).unwrap();
        fs::write(cg_dir.join("memory.max"), format!("{memory_max}\n")).unwrap();
    }

    // ── memory detection ────────────────────────────────────────────

    #[test]
    fn meminfo_parsing() {
        let dir = tempfile::tempdir().unwrap();
        write_meminfo(dir.path(), 8_000_000);
        assert_eq!(
            parse_meminfo_total(&dir.path().join("meminfo")).unwrap(),
            8_000_000
        );
    }

    #[test]
    fn meminfo_missing() {
        assert!(parse_meminfo_total(Path::new("/nonexistent")).is_none());
    }

    #[test]
    fn cgroupv2_limit() {
        let dir = tempfile::tempdir().unwrap();
        write_cgroupv2(dir.path(), "/user.slice/test", "7516192768");
        assert_eq!(
            detect_cgroupv2_memory(
                &dir.path().join("self_cgroup"),
                &dir.path().join("cgroup_root")
            ),
            Some(7_516_192_768)
        );
    }

    #[test]
    fn cgroupv2_unlimited() {
        let dir = tempfile::tempdir().unwrap();
        write_cgroupv2(dir.path(), "/test", "max");
        assert!(detect_cgroupv2_memory(
            &dir.path().join("self_cgroup"),
            &dir.path().join("cgroup_root")
        )
        .is_none());
    }

    #[test]
    fn cgroupv2_hierarchy_minimum() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("self_cgroup"), "0::/a/b/c\n").unwrap();
        let root = dir.path().join("cgroup_root");
        fs::create_dir_all(root.join("a/b/c")).unwrap();
        fs::write(root.join("a/memory.max"), "8589934592\n").unwrap();
        fs::write(root.join("a/b/c/memory.max"), "4294967296\n").unwrap();
        assert_eq!(
            detect_cgroupv2_memory(&dir.path().join("self_cgroup"), &root),
            Some(4_294_967_296)
        );
    }

    #[test]
    fn memory_cgroup_cap() {
        let dir = tempfile::tempdir().unwrap();
        write_meminfo(dir.path(), 16_777_216);
        write_cgroupv2(dir.path(), "/test", "7516192768");
        assert_eq!(
            detect_memory_mib_from(
                &dir.path().join("meminfo"),
                &dir.path().join("self_cgroup"),
                &dir.path().join("cgroup_root")
            ),
            7168
        );
    }

    #[test]
    fn memory_physical_smaller() {
        let dir = tempfile::tempdir().unwrap();
        write_meminfo(dir.path(), 4_194_304);
        write_cgroupv2(dir.path(), "/test", "17179869184");
        assert_eq!(
            detect_memory_mib_from(
                &dir.path().join("meminfo"),
                &dir.path().join("self_cgroup"),
                &dir.path().join("cgroup_root")
            ),
            4096
        );
    }

    #[test]
    fn memory_no_cgroup() {
        let dir = tempfile::tempdir().unwrap();
        write_meminfo(dir.path(), 8_388_608);
        assert_eq!(
            detect_memory_mib_from(
                &dir.path().join("meminfo"),
                &dir.path().join("x"),
                &dir.path().join("y")
            ),
            8192
        );
    }

    // ── token budget ────────────────────────────────────────────────

    #[test]
    fn tokens_gha_runner() {
        // 7 GiB host → 70% = 5017 MiB → 5017/128 = 39 tokens
        let dir = tempfile::tempdir().unwrap();
        write_meminfo(dir.path(), 7_340_032);
        let mib = detect_memory_mib_from(
            &dir.path().join("meminfo"),
            &dir.path().join("x"),
            &dir.path().join("y"),
        );
        let available = (mib as f64 * DEFAULT_MEMORY_FRACTION) as u32;
        assert_eq!(available / TOKEN_MIB, 39);
        // A 512 MiB VM (u1.nano) needs 512/128 = 4 tokens → 39/4 = 9 concurrent VMs
        assert_eq!(available / DEFAULT_VM_MEMORY_MIB, 9);
    }

    #[test]
    fn tokens_tiny_host() {
        // 1.5 GiB → 70% = 1075 MiB → 1075/128 = 8 tokens (two 512 MiB VMs)
        let dir = tempfile::tempdir().unwrap();
        write_meminfo(dir.path(), 1_572_864);
        let mib = detect_memory_mib_from(
            &dir.path().join("meminfo"),
            &dir.path().join("x"),
            &dir.path().join("y"),
        );
        let available = (mib as f64 * DEFAULT_MEMORY_FRACTION) as u32;
        assert_eq!((available / TOKEN_MIB).max(1), 8);
    }

    // ── itype lookup ──────────────────────────────────────────────

    #[test]
    fn itype_known() {
        assert_eq!(itype_memory_mib("u1.nano"), Some(512));
        assert_eq!(itype_memory_mib("u1.micro"), Some(1024));
        assert_eq!(itype_memory_mib("u1.large"), Some(8192));
    }

    #[test]
    fn itype_unknown() {
        assert_eq!(itype_memory_mib("custom.big"), None);
    }

    // ── jobserver ───────────────────────────────────────────────────

    #[test]
    fn jobserver_create_and_acquire() {
        let js = VmJobserver::create(3).unwrap();
        let p1 = js.acquire(1).unwrap();
        let p2 = js.acquire(2).unwrap();
        drop(p1);
        let _p3 = js.acquire(1).unwrap();
        drop(p2);
        drop(_p3);
    }

    #[test]
    fn jobserver_weighted() {
        let js = VmJobserver::create(4).unwrap();

        // 4 GiB VM takes all tokens
        let p1 = js.acquire(4).unwrap();
        drop(p1);

        // 2 × 2 GiB VMs
        let p2 = js.acquire(2).unwrap();
        let p3 = js.acquire(2).unwrap();
        drop(p2);
        drop(p3);
    }
}

mod dir_walk;
mod erofs;
mod fat32;
mod gpt;
mod initramfs;
mod regions;

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::RwLock;

use regions::Region;

static PLUGIN_STATE: RwLock<Option<PluginState>> = RwLock::new(None);

struct PluginState {
    dir: PathBuf,
    cmdline: Option<String>,
    ssh_pubkey: Option<String>,
    regions: Vec<Region>,
    total_size: u64,
}

// --- nbdkit C FFI ---

extern "C" {
    fn nbdkit_error(fmt: *const c_char, ...);
}

fn log_error(msg: &str) {
    let c = CString::new(msg).unwrap_or_default();
    unsafe { nbdkit_error(b"%s\0".as_ptr() as *const c_char, c.as_ptr()) };
}

// --- Plugin callbacks ---

#[no_mangle]
pub extern "C" fn plugin_config(key: *const c_char, value: *const c_char) -> c_int {
    let key = unsafe { CStr::from_ptr(key) }.to_str().unwrap_or("");
    let value = unsafe { CStr::from_ptr(value) }.to_str().unwrap_or("");

    let mut state = PLUGIN_STATE.write().unwrap();
    let state = state.get_or_insert_with(|| PluginState {
        dir: PathBuf::new(),
        cmdline: None,
        ssh_pubkey: None,
        regions: Vec::new(),
        total_size: 0,
    });

    match key {
        "dir" => state.dir = PathBuf::from(value),
        "cmdline" => state.cmdline = Some(value.to_string()),
        "ssh_pubkey" => state.ssh_pubkey = Some(value.to_string()),
        _ => {
            log_error(&format!("unknown parameter: {}", key));
            return -1;
        }
    }
    0
}

#[no_mangle]
pub extern "C" fn plugin_config_complete() -> c_int {
    let state = PLUGIN_STATE.read().unwrap();
    let state = match state.as_ref() {
        Some(s) => s,
        None => {
            log_error("dir parameter is required");
            return -1;
        }
    };

    if state.dir.as_os_str().is_empty() {
        log_error("dir parameter is required");
        return -1;
    }

    if state.cmdline.is_none() {
        log_error("cmdline parameter is required");
        return -1;
    }

    0
}

fn find_kernel_dir(dir: &std::path::Path) -> Option<(PathBuf, PathBuf)> {
    let modules = dir.join("usr/lib/modules");
    if let Ok(entries) = std::fs::read_dir(&modules) {
        for entry in entries.flatten() {
            let kdir = entry.path();
            let vmlinuz = kdir.join("vmlinuz");
            let initramfs = kdir.join("initramfs.img");
            if vmlinuz.exists() && initramfs.exists() {
                return Some((vmlinuz, initramfs));
            }
        }
    }
    None
}

fn find_grub(dir: &std::path::Path) -> Option<PathBuf> {
    fn walk(path: &std::path::Path, target: &str) -> Option<PathBuf> {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() && p.file_name().map(|n| n == target).unwrap_or(false) {
                    return Some(p);
                }
                if p.is_dir() {
                    if let Some(found) = walk(&p, target) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }
    walk(&dir.join("usr/lib"), "grubaa64.efi").or_else(|| walk(&dir.join("usr/lib"), "grubx64.efi"))
}

#[no_mangle]
pub extern "C" fn plugin_get_ready() -> c_int {
    let mut state_guard = PLUGIN_STATE.write().unwrap();
    let state = match state_guard.as_mut() {
        Some(s) => s,
        None => return -1,
    };

    // Walk directory for EROFS
    let root_dir =
        match cap_std::fs::Dir::open_ambient_dir(&state.dir, cap_std::ambient_authority()) {
            Ok(d) => d,
            Err(e) => {
                log_error(&format!("failed to open directory {:?}: {}", state.dir, e));
                return -1;
            }
        };
    let walk = match dir_walk::walk_directory(&root_dir, &state.dir) {
        Ok(w) => w,
        Err(e) => {
            log_error(&format!("failed to walk directory: {}", e));
            return -1;
        }
    };

    let erofs_layout = match erofs::build_erofs(&walk) {
        Ok(l) => l,
        Err(e) => {
            log_error(&format!("failed to build EROFS: {}", e));
            return -1;
        }
    };

    let erofs_regions =
        regions::consolidate_regions(erofs::build_erofs_regions(&erofs_layout, &walk));

    // Discover boot files from dir
    let (kernel_path, initrd_path) = match find_kernel_dir(&state.dir) {
        Some(paths) => paths,
        None => {
            log_error("kernel/initramfs not found in dir/usr/lib/modules/");
            return -1;
        }
    };

    let grub_path = match find_grub(&state.dir) {
        Some(p) => p,
        None => {
            log_error("grubaa64.efi not found in dir/usr/lib/");
            return -1;
        }
    };

    fn file_size(path: &std::path::Path) -> Option<u64> {
        match std::fs::metadata(path) {
            Ok(m) => Some(m.len()),
            Err(e) => {
                log_error(&format!("cannot stat {:?}: {}", path, e));
                None
            }
        }
    }

    let Some(kernel_size) = file_size(&kernel_path) else {
        return -1;
    };
    let Some(initrd_size) = file_size(&initrd_path) else {
        return -1;
    };
    let Some(grub_size) = file_size(&grub_path) else {
        return -1;
    };

    let cmdline = state.cmdline.as_deref().unwrap_or("");

    // Generate grub.cfg
    let grub_cfg = format!(
        "set timeout=0\nset default=0\nmenuentry \"bcvk\" {{\n  linux /boot/vmlinuz {}\n  initrd /boot/initrd.img\n}}\n",
        cmdline
    );

    // Generate CPIO archives
    let units_cpio = initramfs::build_units_cpio();
    let ssh_cpio = state.ssh_pubkey.as_deref().map(initramfs::build_ssh_cpio);

    // Build initrd regions (original file + padding + CPIO)
    let (initrd_parts, initrd_total) =
        fat32::build_initrd_regions(&initrd_path, initrd_size, &units_cpio, ssh_cpio.as_deref());

    // Build ESP regions
    let (esp_regions, esp_size) = fat32::build_esp_regions(
        &grub_path,
        grub_size,
        grub_cfg.as_bytes(),
        &kernel_path,
        kernel_size,
        initrd_parts,
        initrd_total,
    );

    // Build GPT disk with ESP + EROFS
    match gpt::build_gpt_disk(
        esp_regions,
        esp_size,
        erofs_regions,
        erofs_layout.total_size,
    ) {
        Ok(disk) => {
            state.regions = disk.regions;
            state.total_size = disk.total_size;
        }
        Err(e) => {
            log_error(&format!("failed to build GPT disk: {}", e));
            return -1;
        }
    }

    0
}

#[no_mangle]
pub extern "C" fn plugin_open(_readonly: c_int) -> *mut c_void {
    1 as *mut c_void
}

#[no_mangle]
pub extern "C" fn plugin_close(_handle: *mut c_void) {}

#[no_mangle]
pub extern "C" fn plugin_get_size(_handle: *mut c_void) -> i64 {
    let state = PLUGIN_STATE.read().unwrap();
    state.as_ref().map(|s| s.total_size as i64).unwrap_or(-1)
}

#[no_mangle]
pub extern "C" fn plugin_can_multi_conn(_handle: *mut c_void) -> c_int {
    1
}

#[no_mangle]
pub extern "C" fn plugin_pread(
    _handle: *mut c_void,
    buf: *mut c_void,
    count: u32,
    offset: u64,
    _flags: u32,
) -> c_int {
    let state = PLUGIN_STATE.read().unwrap();
    let state = match state.as_ref() {
        Some(s) => s,
        None => return -1,
    };

    let buf = unsafe { std::slice::from_raw_parts_mut(buf as *mut u8, count as usize) };

    match regions::pread(&state.regions, buf, offset) {
        Ok(()) => 0,
        Err(e) => {
            log_error(&format!("pread error at offset {}: {}", offset, e));
            -1
        }
    }
}

// --- Plugin registration ---

#[repr(C)]
pub struct NbdkitPlugin {
    _struct_size: u64,
    _api_version: c_int,
    _thread_model: c_int,
    name: *const c_char,
    longname: *const c_char,
    version: *const c_char,
    description: *const c_char,
    load: Option<extern "C" fn()>,
    unload: Option<extern "C" fn()>,
    config: Option<extern "C" fn(*const c_char, *const c_char) -> c_int>,
    config_complete: Option<extern "C" fn() -> c_int>,
    config_help: *const c_char,
    open: Option<extern "C" fn(c_int) -> *mut c_void>,
    close: Option<extern "C" fn(*mut c_void)>,
    get_size: Option<extern "C" fn(*mut c_void) -> i64>,
    can_write: Option<extern "C" fn(*mut c_void) -> c_int>,
    can_flush: Option<extern "C" fn(*mut c_void) -> c_int>,
    is_rotational: Option<extern "C" fn(*mut c_void) -> c_int>,
    can_trim: Option<extern "C" fn(*mut c_void) -> c_int>,
    _pread_v1: Option<extern "C" fn(*mut c_void, *mut c_void, u32, u64) -> c_int>,
    _pwrite_v1: Option<extern "C" fn(*mut c_void, *const c_void, u32, u64) -> c_int>,
    _flush_v1: Option<extern "C" fn(*mut c_void) -> c_int>,
    _trim_v1: Option<extern "C" fn(*mut c_void, u32, u64) -> c_int>,
    _zero_v1: Option<extern "C" fn(*mut c_void, u32, u64, c_int) -> c_int>,
    errno_is_preserved: c_int,
    dump_plugin: Option<extern "C" fn()>,
    can_zero: Option<extern "C" fn(*mut c_void) -> c_int>,
    can_fua: Option<extern "C" fn(*mut c_void) -> c_int>,
    pread: Option<extern "C" fn(*mut c_void, *mut c_void, u32, u64, u32) -> c_int>,
    pwrite: Option<extern "C" fn(*mut c_void, *const c_void, u32, u64, u32) -> c_int>,
    flush: Option<extern "C" fn(*mut c_void, u32) -> c_int>,
    trim: Option<extern "C" fn(*mut c_void, u32, u64, u32) -> c_int>,
    zero: Option<extern "C" fn(*mut c_void, u32, u64, u32) -> c_int>,
    magic_config_key: *const c_char,
    can_multi_conn: Option<extern "C" fn(*mut c_void) -> c_int>,
    can_extents: Option<extern "C" fn(*mut c_void) -> c_int>,
    extents: Option<extern "C" fn(*mut c_void, u32, u64, u32, *mut c_void) -> c_int>,
    can_cache: Option<extern "C" fn(*mut c_void) -> c_int>,
    cache: Option<extern "C" fn(*mut c_void, u32, u64, u32) -> c_int>,
    thread_model: Option<extern "C" fn() -> c_int>,
    can_fast_zero: Option<extern "C" fn(*mut c_void) -> c_int>,
    preconnect: Option<extern "C" fn(c_int) -> c_int>,
    get_ready: Option<extern "C" fn() -> c_int>,
    after_fork: Option<extern "C" fn() -> c_int>,
    // Fields after after_fork (list_exports, default_export, export_description,
    // cleanup, block_size) are omitted. nbdkit uses _struct_size to determine
    // which fields are present, so omitting trailing fields is safe.
}

unsafe impl Sync for NbdkitPlugin {}

static PLUGIN_NAME: &[u8] = b"erofs\0";
static PLUGIN_LONGNAME: &[u8] = b"nbdkit EROFS plugin\0";
static PLUGIN_VERSION: &[u8] = b"0.2.0\0";
static PLUGIN_DESCRIPTION: &[u8] = b"Create virtual EROFS+ESP disk from directory\0";
static PLUGIN_CONFIG_HELP: &[u8] = b"dir=<DIRECTORY>     (required) Container overlay merged directory\ncmdline=<STRING>    (required) Kernel command line for grub.cfg\nssh_pubkey=<STRING> SSH public key for root access\0";
static PLUGIN_MAGIC_KEY: &[u8] = b"dir\0";

static PLUGIN: NbdkitPlugin = NbdkitPlugin {
    _struct_size: std::mem::size_of::<NbdkitPlugin>() as u64,
    _api_version: 2,
    _thread_model: 3, // NBDKIT_THREAD_MODEL_PARALLEL
    name: PLUGIN_NAME.as_ptr() as *const c_char,
    longname: PLUGIN_LONGNAME.as_ptr() as *const c_char,
    version: PLUGIN_VERSION.as_ptr() as *const c_char,
    description: PLUGIN_DESCRIPTION.as_ptr() as *const c_char,
    load: None,
    unload: None,
    config: Some(plugin_config),
    config_complete: Some(plugin_config_complete),
    config_help: PLUGIN_CONFIG_HELP.as_ptr() as *const c_char,
    open: Some(plugin_open),
    close: Some(plugin_close),
    get_size: Some(plugin_get_size),
    can_write: None,
    can_flush: None,
    is_rotational: None,
    can_trim: None,
    _pread_v1: None,
    _pwrite_v1: None,
    _flush_v1: None,
    _trim_v1: None,
    _zero_v1: None,
    errno_is_preserved: 1,
    dump_plugin: None,
    can_zero: None,
    can_fua: None,
    pread: Some(plugin_pread),
    pwrite: None,
    flush: None,
    trim: None,
    zero: None,
    magic_config_key: PLUGIN_MAGIC_KEY.as_ptr() as *const c_char,
    can_multi_conn: Some(plugin_can_multi_conn),
    can_extents: None,
    extents: None,
    can_cache: None,
    cache: None,
    thread_model: None,
    can_fast_zero: None,
    preconnect: None,
    get_ready: Some(plugin_get_ready),
    after_fork: None,
};

#[no_mangle]
pub extern "C" fn plugin_init() -> *const NbdkitPlugin {
    &PLUGIN
}

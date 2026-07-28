mod dir_walk;
mod erofs;
mod fat32;
mod gpt;
mod initramfs;
pub mod nbd;
pub mod regions;

use std::net::TcpListener;
use std::path::PathBuf;

use regions::Region;

struct Args {
    dir: PathBuf,
    port: u16,
    cmdline: String,
    ssh_pubkey: Option<String>,
    vsock: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        dir: PathBuf::new(),
        port: 10809,
        cmdline: String::new(),
        ssh_pubkey: None,
        vsock: false,
    };

    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--dir" => {
                args.dir = PathBuf::from(argv.next().unwrap_or_else(|| {
                    eprintln!("--dir requires a value");
                    std::process::exit(1);
                }));
            }
            "--port" => {
                let val = argv.next().unwrap_or_else(|| {
                    eprintln!("--port requires a value");
                    std::process::exit(1);
                });
                args.port = val.parse().unwrap_or_else(|_| {
                    eprintln!("--port: invalid number: {val}");
                    std::process::exit(1);
                });
            }
            "--cmdline" => {
                args.cmdline = argv.next().unwrap_or_else(|| {
                    eprintln!("--cmdline requires a value");
                    std::process::exit(1);
                });
            }
            "--ssh-pubkey" => {
                args.ssh_pubkey = Some(argv.next().unwrap_or_else(|| {
                    eprintln!("--ssh-pubkey requires a value");
                    std::process::exit(1);
                }));
            }
            "--vsock" => {
                args.vsock = true;
            }
            other => {
                eprintln!("unknown argument: {other}");
                eprintln!(
                    "usage: bcvk-nbd --dir DIR --port PORT --cmdline CMDLINE [--ssh-pubkey KEY] [--vsock]"
                );
                std::process::exit(1);
            }
        }
    }

    if args.dir.as_os_str().is_empty() {
        eprintln!("--dir is required");
        std::process::exit(1);
    }
    if args.cmdline.is_empty() {
        eprintln!("--cmdline is required");
        std::process::exit(1);
    }

    args
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

fn file_size(path: &std::path::Path) -> u64 {
    match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) => {
            eprintln!("cannot stat {}: {}", path.display(), e);
            std::process::exit(1);
        }
    }
}

fn build_disk(args: &Args) -> (Vec<Region>, u64) {
    let root_dir = match cap_std::fs::Dir::open_ambient_dir(&args.dir, cap_std::ambient_authority())
    {
        Ok(d) => d,
        Err(e) => {
            eprintln!("failed to open directory {:?}: {}", args.dir, e);
            std::process::exit(1);
        }
    };

    let walk = match dir_walk::walk_directory(&root_dir, &args.dir) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("failed to walk directory: {e}");
            std::process::exit(1);
        }
    };

    let erofs_layout = match erofs::build_erofs(&walk) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("failed to build EROFS: {e}");
            std::process::exit(1);
        }
    };

    let erofs_regions =
        regions::consolidate_regions(erofs::build_erofs_regions(&erofs_layout, &walk));

    let (kernel_path, initrd_path) = match find_kernel_dir(&args.dir) {
        Some(paths) => paths,
        None => {
            eprintln!(
                "kernel/initramfs not found in {}/usr/lib/modules/",
                args.dir.display()
            );
            std::process::exit(1);
        }
    };

    let grub_path = match find_grub(&args.dir) {
        Some(p) => p,
        None => {
            eprintln!(
                "grubaa64.efi/grubx64.efi not found in {}/usr/lib/",
                args.dir.display()
            );
            std::process::exit(1);
        }
    };

    let kernel_size = file_size(&kernel_path);
    let initrd_size = file_size(&initrd_path);
    let grub_size = file_size(&grub_path);

    let grub_cfg = format!(
        "set timeout=0\nset default=0\nmenuentry \"bcvk\" {{\n  linux /boot/vmlinuz {}\n  initrd /boot/initrd.img\n}}\n",
        args.cmdline
    );

    let units_cpio = initramfs::build_units_cpio();
    let ssh_cpio = args.ssh_pubkey.as_deref().map(initramfs::build_ssh_cpio);

    let (initrd_parts, initrd_total) =
        fat32::build_initrd_regions(&initrd_path, initrd_size, &units_cpio, ssh_cpio.as_deref());

    let (esp_regions, esp_size) = fat32::build_esp_regions(
        &grub_path,
        grub_size,
        grub_cfg.as_bytes(),
        &kernel_path,
        kernel_size,
        initrd_parts,
        initrd_total,
    );

    match gpt::build_gpt_disk(
        esp_regions,
        esp_size,
        erofs_regions,
        erofs_layout.total_size,
    ) {
        Ok(disk) => (disk.regions, disk.total_size),
        Err(e) => {
            eprintln!("failed to build GPT disk: {e}");
            std::process::exit(1);
        }
    }
}

fn main() {
    let args = parse_args();
    let vsock = args.vsock;
    let port = args.port;

    eprintln!("bcvk-nbd: scanning {}...", args.dir.display());
    let (regions, total_size) = build_disk(&args);
    eprintln!(
        "bcvk-nbd: disk ready ({} regions, {} bytes)",
        regions.len(),
        total_size
    );

    let device = nbd::RegionBlockDevice::new(regions, total_size);

    if vsock {
        eprintln!("bcvk-nbd: listening on vsock port {port}");
        nbd::serve_vsock(port as u32, device);
    } else {
        let listener = TcpListener::bind(("0.0.0.0", port)).unwrap_or_else(|e| {
            eprintln!("bcvk-nbd: failed to bind TCP port {port}: {e}");
            std::process::exit(1);
        });
        eprintln!("bcvk-nbd: listening on TCP port {port}");
        nbd::serve_tcp(listener, device);
    }
}

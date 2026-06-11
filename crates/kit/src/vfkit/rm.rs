//! vm rm — Remove a persistent VM and its metadata.

use std::fs;

use clap::Parser;
use color_eyre::Result;
use tracing::info;

use super::VmMetadata;

/// Options for `vm rm`.
#[derive(Parser, Debug)]
pub struct VmRmOpts {
    /// VM name
    pub name: String,
    /// Force removal even if running
    #[clap(short, long)]
    pub force: bool,
    /// Stop domain if it's running (implied by --force)
    #[clap(long)]
    pub stop: bool,
}

/// Remove a persistent VM, optionally force-killing it.
pub fn run(opts: VmRmOpts) -> Result<()> {
    let meta = VmMetadata::load(&opts.name)?;

    if meta.is_alive() {
        if !(opts.force || opts.stop) {
            color_eyre::eyre::bail!(
                "VM '{}' is running. Stop it first or use --force/--stop",
                opts.name
            );
        }
        info!("force stopping VM '{}'...", opts.name);
        crate::vfkit::stop::run(crate::vfkit::stop::VmStopOpts {
            name: opts.name.clone(),
            force: true,
        })?;
    }

    // Remove disk image and SSH keys if they are inside the bcvk vms directory
    // (i.e., created by `bcvk run`). User-provided disks from `bcvk vm run` are left alone.
    let vms_dir = VmMetadata::vms_dir();
    if std::path::Path::new(&meta.disk_image).starts_with(&vms_dir) {
        for path in [
            meta.disk_image.clone(),
            format!("{}.key", meta.disk_image),
            format!("{}.key.pub", meta.disk_image),
        ] {
            if let Err(e) = fs::remove_file(&path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::debug!("failed to remove {}: {}", path, e);
                }
            }
        }
    }

    for path in [&meta.efi_store, &meta.serial_log] {
        if !path.is_empty() {
            if let Err(e) = fs::remove_file(path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::debug!("failed to remove {}: {}", path, e);
                }
            }
        }
    }

    for suffix in ["-gvproxy.sock", "-gvproxy-svc.sock"] {
        let p = vms_dir.join(format!("{}{}", meta.name, suffix));
        if let Err(e) = fs::remove_file(&p) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::debug!("failed to remove {}: {}", p.display(), e);
            }
        }
    }

    VmMetadata::remove(&opts.name);
    println!("Removed '{}'", opts.name);
    Ok(())
}

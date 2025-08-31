use std::fs::File;
use std::io::{BufWriter, Write};
use std::process::Command;

use clap::Parser;
use color_eyre::eyre::{eyre, Context};
use color_eyre::Result;
use itertools::Itertools;
use rustix::path::Arg;
use tracing::{info, instrument};

#[derive(Parser, Debug)]
pub struct RunEphemeralOpts {
    /// Container image to run as VM
    pub image: String,

    /// Memory in MiB
    #[clap(long, default_value_t = 2048)]
    pub memory: u32,

    /// Number of vCPUs
    #[clap(long, default_value_t = 2)]
    pub vcpus: u32,

    /// Additional kernel command line arguments
    #[clap(long = "karg")]
    pub kernel_args: Vec<String>,

    #[clap(long, default_value = "none")]
    pub net: String,

    /// Disable console output to terminal
    #[clap(long)]
    pub no_console: bool,

    /// Enable debug mode (drop into shell instead of running QEMU)
    #[clap(long)]
    pub debug: bool,

    /// Bind mount a host directory (read-write) into the VM at /mnt/<name>
    /// Format: <host-path>:<name> or <host-path> (uses basename as name)
    #[clap(long = "bind", value_name = "HOST_PATH[:NAME]")]
    pub bind_mounts: Vec<String>,

    /// Bind mount a host directory (read-only) into the VM at /mnt/<name>
    /// Format: <host-path>:<name> or <host-path> (uses basename as name)
    #[clap(long = "ro-bind", value_name = "HOST_PATH[:NAME]")]
    pub ro_bind_mounts: Vec<String>,

    /// Directory containing systemd units to inject into /etc/systemd/system
    /// The directory should contain 'system/' subdirectory with .service files
    #[clap(long = "systemd-units")]
    pub systemd_units_dir: Option<String>,
}

#[derive(Parser, Debug)]
pub struct RunEphemeralImplOpts {
    /// Memory in MiB
    #[clap(long)]
    pub memory: u32,

    /// Number of vCPUs
    #[clap(long)]
    pub vcpus: u32,

    /// Extra kernel arguments
    #[clap(long)]
    pub extra_args: Option<String>,

    /// Enable console output
    #[clap(long)]
    pub console: bool,
}

#[instrument]
fn run_qemu_in_container(opts: &RunEphemeralOpts) -> Result<std::process::ExitStatus> {
    info!("Running QEMU inside hybrid container for {}", opts.image);

    let script = include_str!("../scripts/entrypoint.sh");

    let td = tempfile::tempdir()?;
    let td = td.path().to_str().unwrap();

    let entrypoint_path = &format!("{td}/entrypoint");
    {
        let f = File::create(entrypoint_path)?;
        let mut f = BufWriter::new(f);
        f.write_all(script.as_bytes())?;
        use std::{fs::Permissions, os::unix::fs::PermissionsExt};
        let f = f.into_inner()?;
        let perms = Permissions::from_mode(0o755);
        f.set_permissions(perms)?;
    }

    let extra_args = opts.kernel_args.iter().map(|s| s.as_str()).join(" ");

    let self_exe = std::env::current_exe()?;
    let self_exe = self_exe.as_str()?;

    // Parse mount arguments (both bind and ro-bind)
    let mut host_mounts = Vec::new();
    
    // Parse writable bind mounts
    for mount_spec in &opts.bind_mounts {
        let (host_path, mount_name) = if let Some((path, name)) = mount_spec.split_once(':') {
            (path.to_string(), name.to_string())
        } else {
            let path = mount_spec.clone();
            let name = std::path::Path::new(&path)
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("mount"))
                .to_string_lossy()
                .to_string();
            (path, name)
        };
        host_mounts.push((host_path, mount_name, false)); // false = writable
    }
    
    // Parse read-only bind mounts
    for mount_spec in &opts.ro_bind_mounts {
        let (host_path, mount_name) = if let Some((path, name)) = mount_spec.split_once(':') {
            (path.to_string(), name.to_string())
        } else {
            let path = mount_spec.clone();
            let name = std::path::Path::new(&path)
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("mount"))
                .to_string_lossy()
                .to_string();
            (path, name)
        };
        host_mounts.push((host_path, mount_name, true)); // true = read-only
    }

    // Run the container with the setup script
    let mut cmd = Command::new("podman");
    cmd.arg("run");
    cmd.arg(format!("--net={}", opts.net.as_str()));
    cmd.args([
        "--rm",
        "-it", // Interactive for console passthrough
        // Needed to create nested containers (mountns, etc). Note when running
        // with userns (podman unpriv default) this is totally safe. TODO:
        // Default to enabling userns when running rootful.
        "--cap-add=all",
        // We mount the host /usr (though just *read-only*) but to do that we need to
        // disable default SELinux confinement
        "--security-opt=label=disable",
        // Also needed for nested containers
        "--security-opt=seccomp=unconfined",
        "--security-opt=unmask=/proc/*",
        // This is a general hardening thing to do when running privileged
        "-v",
        "/sys:/sys:ro",
        "--device=/dev/kvm",
        "-v",
        "/usr:/run/hostusr:ro", // Bind mount host /usr as read-only
        "-v",
        &format!("{entrypoint_path}:/run/entrypoint"),
        "-v",
        &format!("{self_exe}:/run/selfexe:ro"),
        // And bind mount in the pristine image (without any mounts on top)
        // that we'll use as a mount source for virtiofs. Mount as rw for testing.
        &format!(
            "--mount=type=image,source={},target=/run/source-image,rw=true",
            opts.image.as_str()
        ),
    ]);

    // Add host directory mounts to the container
    for (host_path, mount_name, is_readonly) in &host_mounts {
        let mount_spec = if *is_readonly {
            format!("{}:/run/host-mounts/{}:ro", host_path, mount_name)
        } else {
            format!("{}:/run/host-mounts/{}", host_path, mount_name)
        };
        cmd.args(["-v", &mount_spec]);
    }
    
    // Mount systemd units directory if specified
    if let Some(ref units_dir) = opts.systemd_units_dir {
        cmd.args(["-v", &format!("{}:/run/systemd-units:ro", units_dir)]);
    }

    // Set debug mode environment variable if requested
    if opts.debug {
        cmd.args(["-e", "DEBUG_MODE=true"]);
        info!("Debug mode enabled - will drop into shell instead of running QEMU");
    }

    // Pass configuration as environment variables for the entrypoint script
    cmd.args([
        "-e",
        &format!("BOOTC_MEMORY={}", opts.memory),
        "-e",
        &format!("BOOTC_VCPUS={}", opts.vcpus),
    ]);

    if !extra_args.is_empty() {
        cmd.args(["-e", &format!("BOOTC_EXTRA_ARGS={}", extra_args)]);
    }

    if !opts.no_console {
        cmd.args(["-e", "BOOTC_CONSOLE=1"]);
    }


    let status = cmd
        .args([&opts.image, "/run/entrypoint"])
        .status()
        .context("Failed to run QEMU in container")?;

    Ok(status)
}

#[instrument]
pub fn run(opts: RunEphemeralOpts) -> Result<()> {
    // Run QEMU inside the container with the hybrid rootfs approach
    let status = run_qemu_in_container(&opts)?;

    // QEMU may exit with non-zero status when VM powers off
    // For testing with poweroff.target, we accept exit code 1
    if !status.success() {
        if let Some(code) = status.code() {
            let kargs_str = opts.kernel_args.join(" ");
            if code == 1 && kargs_str.contains("poweroff.target") {
                info!("QEMU exited with code 1 (expected for poweroff.target)");
            } else {
                return Err(eyre!("QEMU exited with non-zero status: {}", code));
            }
        } else {
            return Err(eyre!("QEMU terminated by signal"));
        }
    }

    info!("VM terminated successfully");
    Ok(())
}

fn inject_systemd_units() -> Result<()> {
    use std::fs;
    
    info!("Injecting systemd units from /run/systemd-units");
    
    let source_units = "/run/systemd-units/system";
    let target_units = "/run/source-image/etc/systemd/system";
    
    if !std::path::Path::new(source_units).exists() {
        info!("No system/ directory found in systemd-units, skipping unit injection");
        return Ok(());
    }
    
    // Create target directories
    fs::create_dir_all(target_units)?;
    fs::create_dir_all(&format!("{}/default.target.wants", target_units))?;
    
    // Copy all .service files
    for entry in fs::read_dir(source_units)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map_or(false, |ext| ext == "service") {
            let filename = path.file_name().unwrap().to_string_lossy();
            let target_path = format!("{}/{}", target_units, filename);
            fs::copy(&path, &target_path)?;
            info!("Copied systemd unit: {}", filename);
        }
    }
    
    // Copy wants directory if it exists
    let source_wants = "/run/systemd-units/system/default.target.wants";
    let target_wants = &format!("{}/default.target.wants", target_units);
    
    if std::path::Path::new(source_wants).exists() {
        for entry in fs::read_dir(source_wants)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_symlink() || path.is_file() {
                let filename = path.file_name().unwrap().to_string_lossy();
                let target_path = format!("{}/{}", target_wants, filename);
                
                if path.is_symlink() {
                    let link_target = fs::read_link(&path)?;
                    let _ = fs::remove_file(&target_path); // Remove if exists
                    std::os::unix::fs::symlink(link_target, &target_path)?;
                } else {
                    fs::copy(&path, &target_path)?;
                }
                info!("Copied systemd wants link: {}", filename);
            }
        }
    }
    
    info!("Systemd unit injection complete");
    Ok(())
}

pub(crate) fn run_impl(opts: RunEphemeralImplOpts) -> Result<()> {
    use crate::qemu;
    use crate::virtiofsd;
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    info!("Running QEMU implementation inside container");

    // Check if we're in debug mode
    let debug_mode = std::env::var("DEBUG_MODE").unwrap_or_default() == "true";
    
    // Copy systemd units if provided
    if std::path::Path::new("/run/systemd-units").exists() {
        inject_systemd_units()?;
    }

    // Find kernel and initramfs from the container image (not the host)
    let modules_dir = Path::new("/run/source-image/usr/lib/modules");
    let mut vmlinuz_path = None;
    let mut initramfs_path = None;

    for entry in fs::read_dir(modules_dir)? {
        let entry = entry?;
        let kernel_dir = entry.path();
        if kernel_dir.is_dir() {
            let vmlinuz = kernel_dir.join("vmlinuz");
            let initramfs = kernel_dir.join("initramfs.img");
            if vmlinuz.exists() && initramfs.exists() {
                info!("Found kernel at: {:?}", vmlinuz);
                vmlinuz_path = Some(vmlinuz);
                initramfs_path = Some(initramfs);
                break;
            }
        }
    }

    let vmlinuz_path = vmlinuz_path
        .ok_or_else(|| eyre!("No kernel found in /run/source-image/usr/lib/modules"))?;
    let initramfs_path = initramfs_path
        .ok_or_else(|| eyre!("No initramfs found in /run/source-image/usr/lib/modules"))?;

    // Verify KVM access
    if !Path::new("/dev/kvm").exists() || !fs::File::open("/dev/kvm").is_ok() {
        return Err(eyre!("KVM device not accessible"));
    }

    // Create QEMU mount points
    fs::create_dir_all("/run/qemu")?;
    let kernel_mount = "/run/qemu/kernel";
    let initramfs_mount = "/run/qemu/initramfs";
    fs::File::create(&kernel_mount)?;
    fs::File::create(&initramfs_mount)?;

    // Bind mount kernel and initramfs
    let mut mount_cmd = Command::new("mount");
    mount_cmd.args([
        "--bind",
        "-o",
        "ro",
        vmlinuz_path.to_str().unwrap(),
        &kernel_mount,
    ]);
    let status = mount_cmd.status().context("Failed to bind mount kernel")?;
    if !status.success() {
        return Err(eyre!("Failed to bind mount kernel"));
    }

    let mut mount_cmd = Command::new("mount");
    mount_cmd.args([
        "--bind",
        "-o",
        "ro",
        initramfs_path.to_str().unwrap(),
        &initramfs_mount,
    ]);
    let status = mount_cmd
        .status()
        .context("Failed to bind mount initramfs")?;
    if !status.success() {
        return Err(eyre!("Failed to bind mount initramfs"));
    }

    // Create mount points in a writable location for host mounts
    if std::path::Path::new("/run/host-mounts").exists() {
        // Create writable mount directory
        let mnt_dir = "/run/host-mount-overlay";
        fs::create_dir_all(mnt_dir)?;
        
        for entry in fs::read_dir("/run/host-mounts")? {
            let entry = entry?;
            let mount_name = entry.file_name();
            let mount_target = format!("{}/{}", mnt_dir, mount_name.to_string_lossy());
            
            // Determine if this mount should be read-only by checking if the container mount is ro
            let source_path = entry.path();
            
            // Check if this directory is mounted as read-only using findmnt
            let mount_name = entry.file_name();
            let mount_name_str = mount_name.to_string_lossy();
            let mount_path = format!("/run/host-mounts/{}", mount_name_str);
            let is_readonly = Command::new("findmnt")
                .args(["-n", "-o", "OPTIONS", &mount_path])
                .output()
                .map(|output| {
                    let options = String::from_utf8_lossy(&output.stdout);
                    options.contains("ro")
                })
                .unwrap_or(false);
            
            let mode = if is_readonly { "read-only" } else { "read-write" };
            info!("Mounting host directory {} to {} ({})", source_path.display(), mount_target, mode);
            
            // Create mount point
            fs::create_dir_all(&mount_target)?;
            
            // Bind mount the host directory
            let mut mount_cmd = Command::new("mount");
            if is_readonly {
                mount_cmd.args([
                    "--bind",
                    "-o",
                    "ro",
                    &source_path.to_string_lossy(),
                    &mount_target,
                ]);
            } else {
                mount_cmd.args([
                    "--bind",
                    &source_path.to_string_lossy(),
                    &mount_target,
                ]);
            }
            
            let status = mount_cmd.status().context("Failed to bind mount host directory")?;
            if !status.success() {
                return Err(eyre!("Failed to bind mount host directory: {}", mount_target));
            }
        }
        
        // Mount the host directories to a location accessible by virtiofsd
        // We'll create the final mount points directly in the shared directory
        let shared_mnt = "/run/inner-shared/mnt";
        fs::create_dir_all(shared_mnt)?;
        
        let mut mount_cmd = Command::new("mount");
        mount_cmd.args([
            "--bind",
            mnt_dir,
            shared_mnt,
        ]);
        let status = mount_cmd.status().context("Failed to bind mount host mount overlay to shared")?;
        if !status.success() {
            return Err(eyre!("Failed to bind mount host mount overlay to {}", shared_mnt));
        }
        
        info!("Successfully mounted host directories to {}", shared_mnt);
    }

    // Start virtiofsd in background using the source image directly
    // If we have host mounts, we'll need QEMU to mount them separately
    let virtiofsd_config = virtiofsd::VirtiofsdConfig::default();
    let mut virtiofsd = virtiofsd::spawn_virtiofsd(&virtiofsd_config)?;

    // Wait for socket to be created
    std::thread::sleep(Duration::from_secs(2));

    if debug_mode {
        info!("=== DEBUG MODE: Dropping into bash shell ===");
        info!("Environment setup complete. You can:");
        info!("- Inspect /run/tmproot (the hybrid rootfs)");
        info!("- Check virtiofsd socket at /run/inner-shared/virtiofs.sock");
        info!("- Exit with 'exit' to terminate");

        let status = Command::new("bash")
            .status()
            .context("Failed to start debug shell")?;

        // Clean up virtiofsd
        virtiofsd.kill().ok();

        if !status.success() {
            return Err(eyre!("Debug shell exited with non-zero status"));
        }
    } else {
        // Build kernel command line
        let mut kernel_cmdline = vec![
            "rootfstype=virtiofs".to_string(),
            "root=rootfs".to_string(),
            "selinux=0".to_string(),
            "systemd.volatile=overlay".to_string(),
        ];

        if opts.console {
            kernel_cmdline.push("console=ttyS0".to_string());
        }

        if let Some(ref extra_args) = opts.extra_args {
            kernel_cmdline.push(extra_args.clone());
        }

        // Configure and start QEMU
        let qemu_config = qemu::QemuConfig {
            memory_mb: opts.memory,
            vcpus: opts.vcpus,
            kernel_path: "/run/qemu/kernel".to_string(),
            initramfs_path: "/run/qemu/initramfs".to_string(),
            virtiofs_socket: virtiofsd_config.socket_path.clone(),
            kernel_cmdline,
            enable_console: opts.console,
        };

        info!("Starting QEMU");
        let mut qemu = qemu::spawn_qemu(&qemu_config)?;

        // Wait for QEMU to finish
        let status = qemu.wait().context("Failed to wait for QEMU")?;

        // Clean up virtiofsd
        virtiofsd.kill().ok();

        // QEMU may exit with non-zero status when VM powers off
        // For testing with poweroff.target, we accept exit code 1
        if !status.success() {
            if let Some(code) = status.code() {
                if code == 1
                    && opts
                        .extra_args
                        .as_ref()
                        .map_or(false, |args| args.contains("poweroff.target"))
                {
                    info!("QEMU exited with code 1 (expected for poweroff.target)");
                } else {
                    return Err(eyre!("QEMU exited with non-zero status: {}", code));
                }
            } else {
                return Err(eyre!("QEMU terminated by signal"));
            }
        }
    }

    Ok(())
}

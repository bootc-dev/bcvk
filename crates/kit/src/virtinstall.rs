use std::collections::HashMap;
use std::fs::OpenOptions;
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use bootc_utils::CommandRunExt;
use clap::Parser;
use color_eyre::{
    eyre::{eyre, Context},
    Result,
};
use indicatif::{ProgressBar, ProgressStyle};

use tracing::instrument;

use crate::{hostexec, images, sshcred};

const VIRTIOFS_MOUNT: &str = "host-container-storage";
const USER_STORAGE: &str = ".local/share/containers/storage";

#[derive(Debug, Clone, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum LibvirtConnection {
    Session,
    System,
}

#[derive(Debug, Clone, Default, clap::Args)]
pub(crate) struct LibvirtGenericOpts {
    /// Connection to libvirt
    #[clap(long, default_value = "session")]
    connection: LibvirtConnection,
}

impl Default for LibvirtConnection {
    fn default() -> Self {
        Self::Session
    }
}

#[derive(Debug, Clone, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub(crate) enum OperatingSystem {
    Fedora,
    CentOSStream10,
}

impl OperatingSystem {
    fn cloud_url(&self) -> &'static str {
        match self {
            Self::Fedora => "https://download.fedoraproject.org/pub/fedora/linux/releases/42/Cloud/x86_64/images/Fedora-Cloud-Base-Generic-42-1.1.x86_64.qcow2",
            Self::CentOSStream10 => todo!(),
        }
    }

    fn libvirt_name(&self) -> &'static str {
        match self {
            Self::Fedora => "fedora-42-cloud.qcow2",
            Self::CentOSStream10 => "centos-stream-10-cloud.qcow2",
        }
    }

    fn osinfo_name(&self) -> &'static str {
        match self {
            OperatingSystem::Fedora => "fedora41",
            OperatingSystem::CentOSStream10 => "centos-stream10",
        }
    }

    fn from_osrelease(osrelease: &HashMap<String, String>) -> Option<Self> {
        let Some(id) = osrelease.get("ID") else {
            return None;
        };
        if id == "fedora" {
            return Some(Self::Fedora);
        }
        let id_like = osrelease
            .get("ID_LIKE")
            .into_iter()
            .flat_map(|v| v.split_ascii_whitespace())
            .collect::<Vec<&str>>();
        if id_like.contains(&"rhel") {
            return Some(Self::CentOSStream10);
        } else if id_like.contains(&"fedora") {
            return Some(Self::Fedora);
        } else {
            None
        }
    }
}

fn libvirt_storage_pool() -> &'static str {
    static POOL: OnceLock<String> = OnceLock::new();
    POOL.get_or_init(|| {
        std::env::var("LIBVIRT_STORAGE_POOL").unwrap_or_else(|_| "default".to_string())
    })
}

#[derive(clap::Subcommand, Debug)]
pub(crate) enum VirtInstallOpts {
    SyncCloudImage {
        #[clap(flatten)]
        libvirt_opts: LibvirtGenericOpts,
        os: OperatingSystem,
        #[clap(long)]
        force: bool,
    },
    FromSRB(FromSRBOpts),
}

#[derive(Parser, Debug)]
pub struct FromSRBOpts {
    #[clap(flatten)]
    pub libvirt_opts: LibvirtGenericOpts,

    /// Name of the image to install
    pub image: String,

    /// This virtual machine should not persist across host reboots
    #[clap(long)]
    pub transient: bool,

    /// Do not bind the container storage via virtiofs
    #[clap(long)]
    pub skip_bind_storage: bool,

    /// Instead of using a default cloud image associated
    /// with the container image OS, use this libvirt volume
    /// which should hold an image.
    #[clap(long)]
    pub base_volume: Option<String>,

    /// Name for the virtual machine
    #[clap(long)]
    pub name: Option<String>,

    /// Path to SSH key
    #[clap(long)]
    pub sshkey: Option<String>,

    /// Size of the root volume in GiB
    #[clap(long, default_value_t = 10)]
    pub size: u32,

    #[clap(long, default_value_t = 2)]
    pub vcpus: u32,

    #[clap(long, default_value = "4096")]
    pub memory: u32,

    /// Pass through this argument to virt-install
    #[clap(long, short = 'a')]
    pub vinstarg: Vec<String>,
}

impl VirtInstallOpts {
    pub fn run(self) -> Result<()> {
        match self {
            VirtInstallOpts::SyncCloudImage {
                libvirt_opts,
                os,
                force,
            } => sync(&libvirt_opts, &os, force),
            VirtInstallOpts::FromSRB(opts) => opts.run(),
        }
    }
}

fn virsh_command(libvirt_opts: &LibvirtGenericOpts) -> Command {
    let conn = match libvirt_opts.connection {
        LibvirtConnection::Session => "qemu:///session",
        LibvirtConnection::System => "qemu:///system",
    };
    let mut r = Command::new("virsh");
    r.args(["-c", conn]);
    r
}

#[instrument(skip(libvirt_opts))]
fn sync(libvirt_opts: &LibvirtGenericOpts, os: &OperatingSystem, force: bool) -> Result<()> {
    let vol = os.libvirt_name();
    let exists = virsh_command(&libvirt_opts)
        .args(["vol-info", "--pool", libvirt_storage_pool(), vol])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success();
    if exists {
        if !force {
            tracing::debug!("Volume already present: {vol}");
            return Ok(());
        } else {
            virsh_command(&libvirt_opts)
                .args(["vol-delete", "--pool", libvirt_storage_pool(), vol])
                .run()
                .map_err(|e| eyre!("Failed to delete volume: {e}"))?;
        }
    }

    let url = os.cloud_url();
    tracing::debug!("Fetching {url}");
    let r = reqwest::blocking::get(url)
        .and_then(|v| v.error_for_status())
        .wrap_err_with(|| format!("Fetching {url}"))?;
    let Some(size) = r.content_length() else {
        return Err(eyre!("No content length"));
    };
    tracing::debug!("size={size}");
    let size_str = format!("{size}");
    virsh_command(&libvirt_opts)
        .args([
            "vol-create-as",
            "--format",
            "qcow2",
            libvirt_storage_pool(),
            vol,
            size_str.as_str(),
        ])
        .run()
        .map_err(|e| eyre!("Failed to create volume: {e}"))?;
    let tempdir = tempfile::tempdir()?;
    let tempdir = tempdir.path().to_str().unwrap();
    // Indirect through a named pipe because libvirt uploads want a file,
    // but we don't want to download the whole thing and then upload to libvirt
    let fifopath = &format!("{tempdir}/libvirt-upload.fifo");
    Command::new("mkfifo")
        .arg(fifopath)
        .run()
        .map_err(|e| eyre!("Creating fifo: {e}"))?;
    let mut uploader = virsh_command(&libvirt_opts)
        .args(["vol-upload", vol, fifopath.as_str(), libvirt_storage_pool()])
        .stdout(Stdio::null())
        .spawn()?;
    let mut fifo = OpenOptions::new()
        .write(true)
        .open(&fifopath)
        .wrap_err("Opening fifo")?;
    let pb = ProgressBar::new(size);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes})",
        )
        .unwrap()
        .progress_chars("#>-"),
    );
    let mut r = pb.wrap_read(r);
    std::io::copy(&mut r, &mut fifo).wrap_err("Fetching and uploading to libvirt")?;
    drop(fifo);
    pb.finish_and_clear();
    let st = uploader.wait()?;
    if !st.success() {
        return Err(eyre!("Failed to upload to libvirt: {st:?}"));
    }
    Ok(())
}

fn vol_path(opts: &LibvirtGenericOpts, name: &str) -> Result<String> {
    let r = virsh_command(opts)
        .args(["vol-path", name, libvirt_storage_pool()])
        .run_get_string()
        .map_err(|e| eyre!("Failed to query volume path: {e}"))?;
    Ok(r.trim().to_owned())
}

impl FromSRBOpts {
    pub fn run(self) -> Result<()> {
        let image = self.image.as_str();
        let libvirt_opts = &self.libvirt_opts;

        // For session installs, it's a pain to deal with the TCP port allocation
        // across reboots, so just make the domain always transient.
        let transient =
            self.transient || self.libvirt_opts.connection == LibvirtConnection::Session;

        println!("Installing via system-reinstall-bootc: {image}");

        let _inspect = images::inspect(image)?;
        let osrelease = images::query_osrelease(image)?;
        let os = OperatingSystem::from_osrelease(&osrelease)
            .ok_or_else(|| eyre!("Failed to determine compatible cloud image from {image}"))?;

        let volname = if let Some(base) = self.base_volume.as_deref() {
            base
        } else {
            // Ensure we have a cloud image corresponding to this OS
            sync(&self.libvirt_opts, &os, false)?;
            os.libvirt_name()
        };
        let volpath = vol_path(libvirt_opts, volname)?;

        let mut qemu_commandline = Vec::new();
        let mut vinstall = hostexec::command("virt-install", None)?;
        vinstall.args([
            "--import",
            "--noautoconsole",
            "--memorybacking=source.type=memfd,access.mode=shared",
        ]);
        vinstall.args(transient.then_some("--transient"));
        vinstall.arg(format!("--os-variant={}", os.osinfo_name()));
        let home = std::env::var("HOME").context("Querying $HOME")?;
        vinstall.args(self.name.map(|name| format!("--name={name}")));
        vinstall.arg(format!(
            "--metadata=description=bootc-kit cloud installation of {image}"
        ));
        vinstall.arg(format!("--memory={}", self.memory));
        vinstall.arg(format!("--vcpus={}", self.vcpus));
        if transient {
            vinstall.arg(format!("--disk=size={},backing_store={volpath}", self.size));
        } else {
            vinstall.arg(format!(
                "--disk=transient,vol={}/{volname}",
                libvirt_storage_pool()
            ));
        }
        // Handle usermode port forwarding
        let port = if self.libvirt_opts.connection == LibvirtConnection::Session {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let port = listener.local_addr()?.port();
            qemu_commandline.push(format!("-netdev user,id=u0,hostfwd=tcp::{port}-:22"));
            Some(listener)
        } else {
            None
        };
        // We always pass through the user's container storage
        if !self.skip_bind_storage {
            vinstall.arg(format!(
                "--filesystem={home}/{USER_STORAGE},{VIRTIOFS_MOUNT},driver.type=virtiofs"
            ));
        }
        if let Some(key) = self.sshkey.as_deref() {
            let cred = sshcred::credential_for_root_ssh(key)?;
            qemu_commandline.push(format!("-smbios type=11,value={cred}"));
        }
        let qemu_commandline = qemu_commandline.join(" ");
        if !qemu_commandline.is_empty() {
            // Note that the way this is implemented through virt-install won't handle spaces in arguments,
            // but we really shouldn't have any of those.
            vinstall.arg(format!("--qemu-commandline={qemu_commandline}"));
        }
        // Pass through user-provided args
        vinstall.args(self.vinstarg);
        println!("+ {}", vinstall.to_string_pretty());
        // Drop listener at the last moment to reduce race window
        drop(port);
        vinstall
            .run()
            .map_err(|e| eyre!("Failed to run virt-install: {e}"))?;
        Ok(())
    }
}

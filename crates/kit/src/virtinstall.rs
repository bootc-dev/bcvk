use clap::Parser;
use color_eyre::{eyre::Context, Result};
use xshell::cmd;

use crate::images;

const FEDORA_CLOUD: &str = "https://download.fedoraproject.org/pub/fedora/linux/releases/42/Cloud/x86_64/images/Fedora-Cloud-Base-Generic-42-1.1.x86_64.qcow2";
const VIRTIOFS_MOUNT: &str = "host-container-storage";
const USER_STORAGE: &str = ".local/share/containers/storage";

#[derive(clap::Subcommand, Debug)]
pub enum VirtInstallOpts {
    FromSRB(FromSRBOpts),
}

#[derive(Parser, Debug)]
pub struct FromSRBOpts {
    /// Name of the image to install
    pub image: String,

    /// Name for the virtual machine
    pub name: Option<String>,
}

impl VirtInstallOpts {
    pub fn run(self) -> Result<()> {
        match self {
            VirtInstallOpts::FromSRB(opts) => opts.run(),
        }
    }
}

impl FromSRBOpts {
    pub fn run(self) -> Result<()> {
        println!("Installing via system-reinstall-bootc: {}", self.image);
        let inspect = images::inspect(&self.image)?;
        let osrelease = images::query_osrelease(&self.image)?;
        let sh = xshell::Shell::new()?;
        let home = std::env::var("HOME").context("Querying $HOME")?;
        let name = self.name.map(|name| format!("--name={name}"));
        let filesystem =
            format!("--filesystem={home}/{USER_STORAGE},{VIRTIOFS_MOUNT},driver.type=virtiofs");
        let location = "TODO";
        cmd!(
            sh,
            "virt-install --noautoconsole --location={location} {name...} {filesystem}"
        )
        .run()?;
        Ok(())
    }
}

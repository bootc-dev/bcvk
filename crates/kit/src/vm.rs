use std::io::Write;

use bootc_utils::CommandRunExt as _;
use color_eyre::eyre::eyre;
use color_eyre::Result;
use tracing::instrument;

use crate::libvirt::{virsh_command, LibvirtConnection};

#[derive(clap::Args, Debug)]
pub struct VmRunOpts {
    /// Name for the VM
    pub name: String,

    /// Path to disk image (on host)
    pub disk: String,

    /// Kernel path (optional)
    #[clap(long)]
    pub kernel: Option<String>,

    /// Initrd path (optional)
    #[clap(long)]
    pub initrd: Option<String>,

    /// Kernel cmdline
    #[clap(
        long,
        default_value = "console=ttyS0 rootfstype=virtiofs root=/dev/virtiofs rw init=/sbin/init panic=1"
    )]
    pub cmdline: String,

    /// Memory in MiB
    #[clap(long, default_value_t = 2048)]
    pub memory: u32,

    /// vcpus
    #[clap(long, default_value_t = 2)]
    pub vcpus: u32,

    /// virtiofs source directories to export, specified as source:target e.g. /var/lib/containers/storage:/root
    #[clap(long, value_delimiter = ',')]
    pub virtiofs: Vec<String>,

    /// Path to virtiofsd wrapper binary inside the host (default /usr/local/bin/virtiofsd-wrapper)
    #[clap(long, default_value = "/usr/local/bin/virtiofsd-wrapper")]
    pub vfsd_bin: String,

    /// libvirt connection: session or system
    #[clap(long, default_value = "session")]
    pub connection: String,
}

fn filesystem_entry(source: &str, target: &str, bin: &str) -> String {
    format!(
        r#"<filesystem type=\"mount\">\n      <source dir=\"{src}\"/>\n      <target dir=\"{tgt}\"/>\n      <driver name=\"qemu\" type=\"virtiofs\"/>\n      <address type=\"unix\" path=\"\"/>\n      <filesystemBinary>{bin}</filesystemBinary>\n    </filesystem>"#,
        src = xml_escape(source),
        tgt = xml_escape(target),
        bin = xml_escape(bin)
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[instrument]
pub fn run(opts: VmRunOpts) -> Result<()> {
    // parse connection
    let conn = match opts.connection.as_str() {
        "system" | "qemu:///system" => LibvirtConnection::System,
        _ => LibvirtConnection::Session,
    };

    // build domain XML
    let mut filesystems = String::new();
    for entry in opts.virtiofs.iter() {
        if let Some((s, t)) = entry.split_once(':') {
            filesystems.push_str(&filesystem_entry(s, t, &opts.vfsd_bin));
            filesystems.push('\n');
        }
    }

    let kernel_xml = if let Some(k) = opts.kernel.as_ref() {
        format!("<kernel>{}</kernel>", xml_escape(k))
    } else {
        String::new()
    };
    let initrd_xml = if let Some(i) = opts.initrd.as_ref() {
        format!("<initrd>{}</initrd>", xml_escape(i))
    } else {
        String::new()
    };
    let cmdline_xml = if !opts.cmdline.is_empty() {
        format!("<cmdline>{}</cmdline>", xml_escape(&opts.cmdline))
    } else {
        String::new()
    };

    let domain_xml = format!(
        r#"<domain type=\"kvm\" xmlns:qemu=\"http://libvirt.org/schemas/domain/qemu/1.0\">\n  <name>{name}</name>\n  <memory unit=\"MiB\">{memory}</memory>\n  <vcpu>{vcpus}</vcpu>\n  <os>\n    <type arch=\"x86_64\" machine=\"q35\">hvm</type>\n    {kernel}\n    {initrd}\n    {cmdline}\n  </os>\n  <devices>\n    <disk device=\"disk\" type=\"file\">\n      <driver name=\"qemu\" type=\"raw\"/>\n      <source file=\"{disk}\"/>\n      <target bus=\"virtio\" dev=\"vda\"/>\n    </disk>\n{filesystems}  </devices>\n</domain>"#,
        name = xml_escape(&opts.name),
        memory = opts.memory,
        vcpus = opts.vcpus,
        kernel = kernel_xml,
        initrd = initrd_xml,
        cmdline = cmdline_xml,
        disk = xml_escape(&opts.disk),
        filesystems = filesystems,
    );

    // write to temp file
    let mut tmp = tempfile::NamedTempFile::new()?;
    tmp.write_all(domain_xml.as_bytes())?;
    tmp.flush()?;
    let tmp_path = tmp
        .path()
        .to_str()
        .ok_or_else(|| eyre!("invalid tmp path"))?
        .to_owned();

    // define domain via virsh
    let mut def = virsh_command(conn)?;
    def.args(["define", tmp_path.as_str()]);
    def.run()
        .map_err(|e| eyre!(format!("virsh define failed: {:?}", e)))?;

    // start domain
    let mut start = virsh_command(conn)?;
    start.args(["start", opts.name.as_str()]);
    start
        .run()
        .map_err(|e| eyre!(format!("virsh start failed: {:?}", e)))?;

    println!("VM {} defined and started", opts.name);
    Ok(())
}

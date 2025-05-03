use std::ffi::OsString;
use std::process::Command;
use std::sync::atomic::AtomicUsize;

use anyhow::Result;
use cap_std_ext::cap_std;

use crate::containerenv::{get_cached_container_execution_info, global_rootfs};

static RUNID: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Default)]
pub struct SystemdConfig {
    inherit_fds: bool,
}

/// Generate a command instance which uses systemd-run to spawn the target
/// command in the host environment.
pub fn command(config: Option<SystemdConfig>) -> Result<Command> {
    let config = config.unwrap_or_default();

    let rootfs = global_rootfs(cap_std::ambient_authority())?;
    let info = get_cached_container_execution_info(&rootfs)?;
    let containerid = &info.id;
    let runid = RUNID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let unit = format!("hostcmd-{containerid}-{runid}.service");
    let scope = format!("libpod-{containerid}.scope");
    let properties = [format!("BindsTo={scope}"), format!("After={scope}")];

    let properties = properties.into_iter().flat_map(|p| ["-p".to_owned(), p]);
    let mut r = Command::new("systemd-run");
    r.args(["--quiet", "--collect", "-u", unit.as_str()]);
    if config.inherit_fds {
        r.arg("--pipe");
    }
    if info.rootless.is_some() {
        r.arg("--user");
    }
    r.args(properties);
    r.arg("--");
    Ok(r)
}

pub fn run<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let config = SystemdConfig {
        inherit_fds: true,
        ..Default::default()
    };
    let mut c = command(Some(config))?;
    c.args(args.into_iter().map(|c| c.into()));
    let st = c.status()?;
    if !st.success() {
        anyhow::bail!("{st:?}");
    }
    Ok(())
}

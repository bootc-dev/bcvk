use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::process::Command;
use std::{collections::HashMap, ffi::OsString};

use cap_std_ext::cap_std;
use color_eyre::eyre::eyre;
use color_eyre::Result;
use rand::distr::SampleString;

use crate::containerenv::{get_cached_container_execution_info, global_rootfs};

#[derive(Debug, Default)]
pub struct SystemdConfig {
    inherit_fds: bool,
}

/// Generate a command instance which uses systemd-run to spawn the target
/// command in the host environment. However, we use BindsTo= on our
/// unit to ensure the lifetime of the command is bounded by the container.
pub fn command(config: Option<SystemdConfig>) -> Result<Command> {
    let config = config.unwrap_or_default();

    let rootfs = global_rootfs(cap_std::ambient_authority())?;
    let hostenv = crate::envdetect::Environment::new()?;
    if !hostenv.container {
        return Err(eyre!("This command requires being executed in a container"));
    }
    if !hostenv.privileged {
        return Err(eyre!("This command requires running with --privileged"));
    }
    // This should be filled if run with --privileged and we're in a container
    let info = get_cached_container_execution_info(&rootfs)?.unwrap();
    if !hostenv.pidhost {
        return Err(eyre!("This command requires running with --pid=host"));
    }
    let containerid = &info.id;
    // A random suffix, 8 alphanumeric chars gives 62 ** 8 possibilities, so low chance of collision
    // And we only care about such collissions for *concurrent* processes bound to *the same*
    // podman container ID; after a unit has exited it's fine if we reuse an ID.
    let runid = rand::distr::Alphanumeric.sample_string(&mut rand::rng(), 8);
    let unit = format!("hostcmd-{containerid}-{runid}.service");
    let scope = format!("libpod-{containerid}.scope");
    let properties = [format!("BindsTo={scope}"), format!("After={scope}")];

    let properties = properties.into_iter().flat_map(|p| ["-p".to_owned(), p]);
    let mut r = Command::new("systemd-run");
    // Note that we need to specify this ExecSearchPath property to suppress heuristics
    // systemd-run has to search for the binary, which in the general case won't exist
    // in the container.
    r.args([
        "--quiet",
        "--collect",
        "-u",
        unit.as_str(),
        "--property=ExecSearchPath=/usr/bin",
    ]);
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

/// Synchronously execute the provided command arguments on the host via `systemd-run`.
/// File descriptors are inherited by default, and the command's result code is checked for errors.
/// The default output streams (stdout and stderr) are inherited.
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
        return Err(eyre!("{st:?}"));
    }
    Ok(())
}

/// Parse the output of the `env` command
fn parse_env(e: &[u8]) -> HashMap<&OsStr, &OsStr> {
    e.split(|&c| c == b'\n')
        .filter_map(|line| {
            let mut split = line.split(|&c| c == b'=');
            let Some(k) = split.next() else {
                return None;
            };
            let Some(v) = split.next() else {
                return None;
            };
            Some((OsStr::from_bytes(k), OsStr::from_bytes(v)))
        })
        .collect()
}

/// Initialize bind mounts and setup
#[allow(dead_code)]
pub fn prepare() -> Result<()> {
    let config = SystemdConfig {
        inherit_fds: true,
        ..Default::default()
    };
    let mut c = command(Some(config))?;
    c.args(["env"]);
    let o = c.output()?;
    let st = o.status;
    if !st.success() {
        return Err(eyre!("{st:?}"));
    }
    let env = parse_env(&o.stdout);
    let Some(&home) = env.get(OsStr::new("HOME")) else {
        return Err(eyre!("HOME is unset in host"));
    };

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_env() {
        let input = b"FOO=bar\nBAZ=quux\n";
        let expected: HashMap<&OsStr, &OsStr> = [
            (OsStr::new("FOO"), OsStr::new("bar")),
            (OsStr::new("BAZ"), OsStr::new("quux")),
        ]
        .into_iter()
        .collect();
        let actual = parse_env(input);
        assert_eq!(actual, expected);
    }
}

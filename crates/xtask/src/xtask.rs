//! See https://github.com/matklad/cargo-xtask
//! This is kind of like "Justfile but in Rust".

use std::process::Command;

use color_eyre::eyre::{eyre, Context, Report};
use color_eyre::Result;
use xshell::Shell;

mod man;

#[allow(clippy::type_complexity)]
const TASKS: &[(&str, fn(&Shell) -> Result<()>)] = &[
    ("manpages", manpages),
    ("update-manpages", update_manpages),
    ("sync-manpages", sync_manpages),
    ("package", package),
    ("package-srpm", package_srpm),
    ("spec", spec),
];

fn install_tracing() {
    use tracing_error::ErrorLayer;
    use tracing_subscriber::fmt;
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::EnvFilter;

    let fmt_layer = fmt::layer().with_target(false);
    let filter_layer = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap();

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt_layer)
        .with(ErrorLayer::default())
        .init();
}

fn main() -> Result<(), Report> {
    install_tracing();
    color_eyre::install()?;
    // Ensure our working directory is the toplevel
    {
        let toplevel_path = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .context("Invoking git rev-parse")?;
        if !toplevel_path.status.success() {
            return Err(eyre!("Failed to invoke git rev-parse"));
        }
        let path = String::from_utf8(toplevel_path.stdout)?;
        std::env::set_current_dir(path.trim()).context("Changing to toplevel")?;
    }

    let task = std::env::args().nth(1);

    let sh = xshell::Shell::new()?;
    if let Some(cmd) = task.as_deref() {
        let f = TASKS
            .iter()
            .find_map(|(k, f)| (*k == cmd).then_some(*f))
            .unwrap_or(print_help);
        f(&sh)?;
    } else {
        print_help(&sh)?;
    }
    Ok(())
}

fn print_help(_sh: &Shell) -> Result<()> {
    println!("Tasks:");
    for (name, _) in TASKS {
        println!("  - {name}");
    }
    Ok(())
}

fn manpages(sh: &Shell) -> Result<()> {
    man::generate_man_pages(sh)
}

fn update_manpages(sh: &Shell) -> Result<()> {
    man::update_manpages(sh)
}

fn sync_manpages(sh: &Shell) -> Result<()> {
    man::sync_all_man_pages(sh)
}

const NAME: &str = "bcvk";
const TAR_REPRODUCIBLE_OPTS: &[&str] = &[
    "--sort=name",
    "--owner=0",
    "--group=0",
    "--numeric-owner",
    "--pax-option=exthdr.name=%d/PaxHeaders/%f,delete=atime,delete=ctime",
];

fn gitrev_to_version(v: &str) -> String {
    let v = v.trim().trim_start_matches('v');
    v.replace('-', ".")
}

fn gitrev(sh: &Shell) -> Result<String> {
    use xshell::cmd;
    if let Ok(rev) = cmd!(sh, "git describe --tags --exact-match")
        .ignore_stderr()
        .read()
    {
        Ok(gitrev_to_version(&rev))
    } else {
        // Just use the version from Cargo.toml
        man::get_raw_package_version()
    }
}

/// Return the timestamp of the latest git commit in seconds since the Unix epoch.
fn git_source_date_epoch(dir: &str) -> Result<u64> {
    let o = Command::new("git")
        .args(["log", "-1", "--pretty=%ct"])
        .current_dir(dir)
        .output()?;
    if !o.status.success() {
        return Err(eyre!("git exited with an error: {:?}", o));
    }
    let buf = String::from_utf8(o.stdout).context("Failed to parse git log output")?;
    let r = buf.trim().parse()?;
    Ok(r)
}

/// When using cargo-vendor-filterer --format=tar, the config generated has a bogus source
/// directory. This edits it to refer to vendor/ as a stable relative reference.
fn edit_vendor_config(config: &str) -> Result<String> {
    let mut config: toml::Value = toml::from_str(config)?;
    let config = config
        .as_table_mut()
        .ok_or_else(|| eyre!("Vendor config is not a table"))?;
    let source_table = config
        .get_mut("source")
        .ok_or_else(|| eyre!("Missing 'source' key in vendor config"))?
        .as_table_mut()
        .ok_or_else(|| eyre!("'source' is not a table"))?;
    let vendored_sources = source_table
        .get_mut("vendored-sources")
        .ok_or_else(|| eyre!("Missing 'vendored-sources' key"))?
        .as_table_mut()
        .ok_or_else(|| eyre!("'vendored-sources' is not a table"))?;
    let previous =
        vendored_sources.insert("directory".into(), toml::Value::String("vendor".into()));
    if previous.is_none() {
        return Err(eyre!("Expected 'directory' key in vendored-sources"));
    }

    Ok(config.to_string())
}

struct Package {
    version: String,
    srcpath: camino::Utf8PathBuf,
    vendorpath: camino::Utf8PathBuf,
}

fn impl_package(sh: &Shell) -> Result<Package> {
    use camino::Utf8Path;
    use xshell::cmd;

    let source_date_epoch = git_source_date_epoch(".")?;
    let v = gitrev(sh)?;

    let namev = format!("{NAME}-{v}");
    let p = Utf8Path::new("target").join(format!("{namev}.tar"));
    let prefix = format!("{namev}/");
    cmd!(sh, "git archive --format=tar --prefix={prefix} -o {p} HEAD").run()?;
    // Generate the vendor directory now, as we want to embed the generated config to use
    // it in our source.
    let vendorpath = Utf8Path::new("target").join(format!("{namev}-vendor.tar.zstd"));
    let vendor_config = cmd!(
        sh,
        "cargo vendor-filterer --prefix=vendor --format=tar.zstd {vendorpath}"
    )
    .read()?;
    let vendor_config = edit_vendor_config(&vendor_config)?;
    // Append .cargo/vendor-config.toml (a made up filename) into the tar archive.
    {
        let tmpdir = tempfile::tempdir_in("target")?;
        let tmpdir_path = tmpdir.path();
        let path = tmpdir_path.join("vendor-config.toml");
        std::fs::write(&path, vendor_config)?;
        let source_date_epoch = format!("{source_date_epoch}");
        cmd!(
            sh,
            "tar -r -C {tmpdir_path} {TAR_REPRODUCIBLE_OPTS...} --mtime=@{source_date_epoch} --transform=s,^,{prefix}.cargo/, -f {p} vendor-config.toml"
        )
        .run()?;
    }
    // Compress with zstd
    let srcpath: camino::Utf8PathBuf = format!("{p}.zstd").into();
    cmd!(sh, "zstd --rm -f {p} -o {srcpath}").run()?;

    Ok(Package {
        version: v,
        srcpath,
        vendorpath,
    })
}

fn package(sh: &Shell) -> Result<()> {
    let p = impl_package(sh)?.srcpath;
    println!("Generated: {p}");
    Ok(())
}

fn update_spec(sh: &Shell) -> Result<camino::Utf8PathBuf> {
    use camino::Utf8Path;
    use std::fs::File;
    use std::io::{BufRead, BufReader, BufWriter, Write};

    let p = Utf8Path::new("target");
    let pkg = impl_package(sh)?;
    let srcpath = pkg.srcpath.file_name().unwrap();
    let v = pkg.version;
    let src_vendorpath = pkg.vendorpath.file_name().unwrap();
    {
        let specin = File::open(format!("contrib/packaging/{NAME}.spec"))
            .map(BufReader::new)
            .context("Opening spec")?;
        let mut o = File::create(p.join(format!("{NAME}.spec"))).map(BufWriter::new)?;
        for line in specin.lines() {
            let line = line?;
            if line.starts_with("Version:") {
                writeln!(o, "# Replaced by cargo xtask spec")?;
                writeln!(o, "Version: {v}")?;
            } else if line.starts_with("Source0") {
                writeln!(o, "Source0: {srcpath}")?;
            } else if line.starts_with("Source1") {
                writeln!(o, "Source1: {src_vendorpath}")?;
            } else {
                writeln!(o, "{line}")?;
            }
        }
    }
    let spec_path = p.join(format!("{NAME}.spec"));
    Ok(spec_path)
}

fn spec(sh: &Shell) -> Result<()> {
    let s = update_spec(sh)?;
    println!("Generated: {s}");
    Ok(())
}

fn impl_srpm(sh: &Shell) -> Result<camino::Utf8PathBuf> {
    use camino::Utf8Path;
    use std::fs::File;
    use std::io::{BufRead, BufReader, BufWriter, Write};
    use xshell::cmd;

    {
        let _g = sh.push_dir("target");
        for name in sh.read_dir(".")? {
            if let Some(name) = name.to_str() {
                if name.ends_with(".src.rpm") {
                    sh.remove_path(name)?;
                }
            }
        }
    }
    let pkg = impl_package(sh)?;
    let td = tempfile::tempdir_in("target").context("Allocating tmpdir")?;
    let td = td.keep();
    let td: &Utf8Path = td
        .as_path()
        .try_into()
        .context("Converting tmpdir path to UTF-8")?;
    let srcpath = &pkg.srcpath;
    cmd!(sh, "mv {srcpath} {td}").run()?;
    let v = pkg.version;
    let src_vendorpath = &pkg.vendorpath;
    cmd!(sh, "mv {src_vendorpath} {td}").run()?;
    let srcpath_filename = pkg.srcpath.file_name().unwrap();
    let vendorpath_filename = pkg.vendorpath.file_name().unwrap();
    {
        let specin = File::open(format!("contrib/packaging/{NAME}.spec"))
            .map(BufReader::new)
            .context("Opening spec")?;
        let mut o = File::create(td.join(format!("{NAME}.spec"))).map(BufWriter::new)?;
        for line in specin.lines() {
            let line = line?;
            if line.starts_with("Version:") {
                writeln!(o, "# Replaced by cargo xtask package-srpm")?;
                writeln!(o, "Version: {v}")?;
            } else if line.starts_with("Source0") {
                writeln!(o, "Source0: {srcpath_filename}")?;
            } else if line.starts_with("Source1") {
                writeln!(o, "Source1: {vendorpath_filename}")?;
            } else {
                writeln!(o, "{line}")?;
            }
        }
    }
    let d = sh.push_dir(td);
    let mut cmd = cmd!(sh, "rpmbuild");
    for k in [
        "_sourcedir",
        "_specdir",
        "_builddir",
        "_srcrpmdir",
        "_rpmdir",
    ] {
        cmd = cmd.arg("--define");
        cmd = cmd.arg(format!("{k} {td}"));
    }
    cmd.arg("--define")
        .arg(format!("_buildrootdir {td}/.build"))
        .args(["-bs", &format!("{NAME}.spec")])
        .run()?;
    drop(d);
    let mut srpm = None;
    for e in std::fs::read_dir(td)? {
        let e = e?;
        let n = e.file_name();
        let Some(n) = n.to_str() else {
            continue;
        };
        if n.ends_with(".src.rpm") {
            srpm = Some(Utf8Path::new(td).join(n));
            break;
        }
    }
    let srpm = srpm.ok_or_else(|| eyre!("Failed to find generated .src.rpm"))?;
    let dest = Utf8Path::new("target").join(srpm.file_name().unwrap());
    std::fs::rename(&srpm, &dest)?;
    Ok(dest)
}

fn package_srpm(sh: &Shell) -> Result<()> {
    let srpm = impl_srpm(sh)?;
    println!("Generated: {srpm}");
    Ok(())
}

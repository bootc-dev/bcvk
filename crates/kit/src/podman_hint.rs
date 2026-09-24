//! Explain podman failures caused by podman not seeing bcvk's filesystem.
//!
//! To launch a VM, bcvk asks podman to bind-mount paths from its own
//! filesystem, including its own binary. When podman runs somewhere else,
//! e.g. bcvk runs in a toolbox or distrobox with podman forwarded to the host,
//! or podman is a remote client, those paths are resolved on podman's side,
//! and one that only exists on bcvk's side fails with an obscure
//! `statfs /usr/bin/bcvk: no such file or directory`.
//!
//! See <https://github.com/bootc-dev/bcvk/issues/5> and bcvk-ephemeral-run(8)
//! for the full explanation.

use camino::Utf8Path;
use color_eyre::eyre::Report;
use color_eyre::Section;

/// How podman reports a bind mount source that doesn't exist, e.g.
/// `Error: statfs /usr/bin/bcvk: no such file or directory`.
const PODMAN_STATFS_PREFIX: &str = "statfs ";
const ENOENT_SUFFIX: &str = ": no such file or directory";

/// The bind mount source that podman reported missing, if any.
fn missing_bind_source(stderr: &str) -> Option<&Utf8Path> {
    stderr.lines().find_map(|line| {
        let (_, rest) = line.split_once(PODMAN_STATFS_PREFIX)?;
        let path = rest.trim_end().strip_suffix(ENOENT_SUFFIX)?;
        Some(Utf8Path::new(path))
    })
}

/// Wrap `err` with an explanation if podman's `stderr` says a bind mount
/// source is missing, but `exists` says bcvk can see it.
fn with_hint_if(err: Report, stderr: &str, exists: impl Fn(&Utf8Path) -> bool) -> Report {
    let Some(path) = missing_bind_source(stderr).filter(|p| exists(p)) else {
        return err;
    };
    err.wrap_err(format!(
        "podman could not find {path}, but bcvk can see it: podman is not seeing bcvk's filesystem"
    ))
    .note(
        "This happens when bcvk runs in a toolbox or distrobox container and podman is \
         forwarded to the host, or when podman is a remote client. bcvk needs to run \
         where podman runs.",
    )
    .suggestion(
        "Install and run bcvk on the host, alongside podman. \
         See bcvk-ephemeral-run(8) for details.",
    )
}

/// Add guidance to the error for a failed podman invocation if podman could
/// not find a bind mount source that bcvk can see.
pub(crate) fn with_podman_failure_hint(err: Report, stderr: &str) -> Report {
    with_hint_if(err, stderr, |p| p.try_exists().unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_missing_bind_source() {
        let cases = [
            (
                "Error: statfs /usr/bin/bcvk: no such file or directory\n",
                Some("/usr/bin/bcvk"),
            ),
            (
                "some warning\nError: statfs /a b/c: no such file or directory  \n",
                Some("/a b/c"),
            ),
            ("Error: statfs /x: permission denied\n", None),
            ("Error: short-name resolution enforced\n", None),
            ("", None),
        ];
        for (stderr, expected) in cases {
            assert_eq!(
                missing_bind_source(stderr),
                expected.map(Utf8Path::new),
                "{stderr:?}"
            );
        }
    }

    #[test]
    fn test_with_hint_if() {
        const STATFS: &str = "Error: statfs /usr/bin/bcvk: no such file or directory\n";
        const OTHER: &str = "Error: short-name resolution enforced\n";
        // (stderr, whether bcvk can see the path, whether a hint is added)
        let cases = [
            (STATFS, true, true),
            (STATFS, false, false),
            (OTHER, true, false),
        ];
        for (stderr, exists, hinted) in cases {
            let msg = format!("Podman command failed: {stderr}");
            let r = with_hint_if(color_eyre::eyre::eyre!(msg.clone()), stderr, |_| exists);
            let chain: Vec<String> = r.chain().map(|e| e.to_string()).collect();
            if hinted {
                assert_eq!(chain.len(), 2, "{chain:?}");
                assert!(chain[0].contains("/usr/bin/bcvk"), "{chain:?}");
                // The note and suggestion sections are only recorded with
                // color_eyre's handler, which unit tests can't reliably
                // install (eyre locks in its default hook on first use).
                // The original podman error is kept as the cause.
                assert_eq!(chain[1], msg);
            } else {
                assert_eq!(chain, [msg], "{stderr:?} exists={exists}");
            }
        }
    }
}

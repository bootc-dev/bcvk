use std::{collections::HashMap, os::unix::process::CommandExt};

use bootc_utils::CommandRunExt;
use color_eyre::{
    eyre::{self, eyre},
    Result,
};
use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::hostexec;

#[derive(clap::Subcommand, Debug)]
pub(crate) enum ImagesOpts {
    /// List available bootc images
    List {
        /// Output as JSON
        #[clap(long)]
        json: bool,
    },
}

impl ImagesOpts {
    pub(crate) fn run(self) -> Result<()> {
        match self {
            ImagesOpts::List { json } => {
                if json {
                    // Use the existing list function that returns JSON
                    let images = list()?;
                    let json_output = serde_json::to_string_pretty(&images)?;
                    println!("{}", json_output);
                    Ok(())
                } else {
                    // Use the standard podman images command for table output
                    let r = hostexec::command("podman", None)?
                        .args(["images", "--filter=label=containers.bootc=1"])
                        .exec();
                    Err(r.into())
                }
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Image {
    pub names: Option<Vec<String>>,
    pub id: String,
    pub size: u64,
    #[serde(default)]
    #[serde(with = "chrono::serde::ts_seconds_option")]
    pub created: Option<chrono::DateTime<chrono::Utc>>,
}

pub(crate) fn inspect(name: &str) -> Result<Image> {
    let mut r: Vec<Image> = hostexec::command("podman", None)?
        .args(["image", "inspect", name])
        .run_and_parse_json()
        .map_err(|e| eyre::eyre!("{e}"))?;
    r.pop().ok_or_else(|| eyre!("No such image"))
}

/// Parse an os-release string
fn parse_osrelease(s: &str) -> Result<HashMap<String, String>> {
    let r = s
        .lines()
        .filter_map(|line| {
            let Some((k, v)) = line.split_once('=') else {
                return None;
            };
            if k.starts_with('#') {
                return None;
            }
            let Some(v) = shlex::split(v) else {
                return None;
            };
            let Some(v) = v.into_iter().next() else {
                return None;
            };
            Some((k.to_string(), v.to_string()))
        })
        .collect();
    Ok(r)
}

/// Read and parse the /usr/lib/os-release from the provided image
#[instrument]
pub(crate) fn query_osrelease(name: &str) -> Result<HashMap<String, String>> {
    let r = hostexec::command("podman", None)?
        .args([
            "run",
            "--rm",
            "--entrypoint",
            "cat",
            name,
            "/usr/lib/os-release",
        ])
        .run_get_string()
        .map_err(|e| eyre::eyre!("{e}"))?;
    parse_osrelease(&r)
}

#[allow(dead_code)]
pub fn list() -> Result<Vec<Image>> {
    let images: Vec<Image> = hostexec::command("podman", None)?
        .args([
            "images",
            "--format",
            "json",
            "--filter=label=containers.bootc=1",
        ])
        .run_and_parse_json()
        .map_err(|e| eyre::eyre!("{e}"))?;
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_osrelease() {
        let input = r#"NAME="Fedora Linux"
VERSION="39 (Container Image)"
ID=fedora
VERSION_ID=39
PLATFORM_ID="platform:f39"
PRETTY_NAME="Fedora Linux 39 (Container Image)"
# Comment here then a blank line

LOGO="fedora-logo-icon"
# Trailing comment
"#;
        let expected = [
            ("NAME", "Fedora Linux"),
            ("VERSION", "39 (Container Image)"),
            ("ID", "fedora"),
            ("VERSION_ID", "39"),
            ("PLATFORM_ID", "platform:f39"),
            ("PRETTY_NAME", "Fedora Linux 39 (Container Image)"),
            ("LOGO", "fedora-logo-icon"),
        ];
        let actual = parse_osrelease(input).unwrap();
        assert_eq!(actual.len(), expected.len());
        for (k, v) in expected {
            assert_eq!(actual.get(k).unwrap(), v);
        }
    }
}

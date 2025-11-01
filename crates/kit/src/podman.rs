use std::process::Command;

use bootc_utils::CommandRunExt;
use color_eyre::{eyre::eyre, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    #[allow(dead_code)]
    pub graph_driver_name: String,
    pub graph_root: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PodmanSystemInfo {
    pub store: Store,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageInspect {
    pub size: u64,
}

pub fn get_system_info() -> Result<PodmanSystemInfo> {
    Command::new("podman")
        .arg("system")
        .arg("info")
        .arg("--format=json")
        .run_and_parse_json()
        .map_err(|e| eyre!("podman system info failed: {}", e))
}

/// Get the size of a container image in bytes
pub fn get_image_size(image: &str) -> Result<u64> {
    let inspect_result: Vec<ImageInspect> = Command::new("podman")
        .arg("inspect")
        .arg("--format=json")
        .arg("--type=image")
        .arg(image)
        .run_and_parse_json()
        .map_err(|e| eyre!("podman inspect failed for image {}: {}", image, e))?;

    if inspect_result.is_empty() {
        return Err(eyre!("No image found for: {}", image));
    }

    Ok(inspect_result[0].size)
}

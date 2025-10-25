//! Project-scoped VM management inspired by Vagrant
//!
//! The `bcvk project` commands provide a streamlined workflow for running bootc VMs
//! scoped to a project directory, with configuration via `.bcvk/config.toml`.

use camino::{Utf8Path, Utf8PathBuf};
use clap::Subcommand;
use color_eyre::{eyre::Context as _, Result};
use std::env;

pub mod config;
pub mod down;
pub mod init;
pub mod ls;
pub mod rm;
pub mod ssh;
pub mod up;

use config::ProjectConfig;

/// Project subcommands
#[derive(Debug, Subcommand)]
pub enum ProjectSubcommands {
    /// Initialize project configuration interactively
    Init(init::ProjectInitOpts),

    /// Create or start the project VM
    Up(up::ProjectUpOpts),

    /// SSH into the project VM
    Ssh(ssh::ProjectSshOpts),

    /// Shut down the project VM
    Down(down::ProjectDownOpts),

    /// Remove the project VM and its resources
    Rm(rm::ProjectRmOpts),

    /// List all project VMs
    Ls(ls::ProjectLsOpts),
}

/// Get the current project directory
///
/// Uses the current working directory.
pub fn current_project_dir() -> Result<Utf8PathBuf> {
    let cwd = env::current_dir().context("Failed to get current directory")?;
    Utf8PathBuf::from_path_buf(cwd)
        .map_err(|p| color_eyre::eyre::eyre!("Path is not valid UTF-8: {}", p.display()))
}

/// Generate a project name from the current directory
///
/// Priority:
/// 1. Config file `project.name` field
/// 2. Directory name (sanitized)
pub fn generate_project_name(
    project_dir: &Utf8Path,
    config: Option<&ProjectConfig>,
) -> Result<String> {
    if let Some(config) = config {
        if let Some(project) = &config.project {
            if let Some(name) = &project.name {
                return Ok(sanitize_name(name));
            }
        }
    }

    let dir_name = project_dir
        .file_name()
        .ok_or_else(|| color_eyre::eyre::eyre!("Could not determine directory name"))?;

    Ok(sanitize_name(dir_name))
}

/// Sanitize a name for use as a libvirt domain name
///
/// Replaces characters that are not alphanumeric, hyphen, or underscore with hyphens.
/// Ensures the name starts with an alphanumeric character.
fn sanitize_name(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    let mut chars = name.chars().peekable();

    // Skip leading non-alphanumeric characters
    while let Some(&c) = chars.peek() {
        if c.is_alphanumeric() {
            break;
        }
        chars.next();
    }

    // Process remaining characters
    for c in chars {
        if c.is_alphanumeric() || c == '-' || c == '_' {
            result.push(c);
        } else {
            result.push('-');
        }
    }

    // If empty after sanitization, use a default
    if result.is_empty() {
        result = "bcvk-project".to_string();
    }

    result
}

/// Generate the project VM name with "bcvk-project-" prefix
pub fn project_vm_name(project_dir: &Utf8Path, config: Option<&ProjectConfig>) -> Result<String> {
    let name = generate_project_name(project_dir, config)?;
    Ok(format!("bcvk-project-{}", name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_name() {
        assert_eq!(sanitize_name("my-project"), "my-project");
        assert_eq!(sanitize_name("my_project"), "my_project");
        assert_eq!(sanitize_name("my project"), "my-project");
        assert_eq!(sanitize_name("my/project"), "my-project");
        assert_eq!(sanitize_name("123project"), "123project");
        assert_eq!(sanitize_name("---project"), "project");
        assert_eq!(sanitize_name("project@123"), "project-123");
        assert_eq!(sanitize_name("!!!"), "bcvk-project");
    }

    #[test]
    fn test_git_origin_name_extraction() {
        // Test various git URL formats
        let test_cases = vec![
            "https://github.com/user/repo.git",
            "git@github.com:user/repo.git",
            "/path/to/repo.git",
            "https://github.com/user/repo",
        ];

        for url in test_cases {
            let name = url
                .rsplit('/')
                .next()
                .unwrap_or(url)
                .strip_suffix(".git")
                .unwrap_or(url.rsplit('/').next().unwrap_or(url));
            assert_eq!(name, "repo");
        }
    }
}

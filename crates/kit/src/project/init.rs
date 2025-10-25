//! Implementation of `bcvk project init` command

use camino::Utf8Path;
use clap::Parser;
use color_eyre::{eyre::Context as _, Result};
use dialoguer::{theme::ColorfulTheme, Confirm, FuzzySelect, Input};
use std::fs;
use toml_edit::{value, DocumentMut, Item, Table};

use super::{config::CONFIG_DIR, config::CONFIG_FILE, current_project_dir};

/// Initialize project configuration interactively
#[derive(Debug, Parser)]
pub struct ProjectInitOpts {
    /// Overwrite existing configuration if it exists
    #[clap(long, short = 'f')]
    pub force: bool,
}

/// Run the project init command
pub fn run(opts: ProjectInitOpts) -> Result<()> {
    let project_dir = current_project_dir()?;
    let config_path = project_dir.join(CONFIG_DIR).join(CONFIG_FILE);

    // Check if config already exists
    if config_path.exists() && !opts.force {
        color_eyre::eyre::bail!(
            "Configuration file already exists: {}\n\
             Use --force to overwrite it.",
            config_path
        );
    }

    generate_config_interactive(&project_dir)?;
    println!("\n✓ Configuration saved to .bcvk/config.toml");
    println!("Run 'bcvk project up' to start your VM");

    Ok(())
}

/// Generate project configuration interactively
fn generate_config_interactive(project_dir: &Utf8Path) -> Result<()> {
    println!("bcvk project configuration wizard\n");

    // Get list of bootc images for autocomplete
    let images = crate::images::list().unwrap_or_default();
    let image_names: Vec<String> = images
        .iter()
        .filter_map(|img| {
            img.names.as_ref().and_then(|names| {
                names.first().map(|name| {
                    // Remove :latest suffix for cleaner display
                    if name.ends_with(":latest") {
                        name.strip_suffix(":latest").unwrap_or(name).to_string()
                    } else {
                        name.clone()
                    }
                })
            })
        })
        .collect();

    // Prompt for container image
    let image = if !image_names.is_empty() {
        println!("Select a bootc container image (type to filter):");
        let selection = FuzzySelect::with_theme(&ColorfulTheme::default())
            .items(&image_names)
            .default(0)
            .interact()
            .context("Failed to select image")?;
        image_names[selection].clone()
    } else {
        println!("No bootc images found locally.");
        Input::<String>::with_theme(&ColorfulTheme::default())
            .with_prompt("Enter container image")
            .interact_text()
            .context("Failed to get image input")?
    };

    // Ask about custom project name
    let custom_name = if Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt("Do you want to set a custom project name?")
        .default(false)
        .interact()
        .context("Failed to get confirmation")?
    {
        Some(
            Input::<String>::with_theme(&ColorfulTheme::default())
                .with_prompt("Project name")
                .interact_text()
                .context("Failed to get project name")?,
        )
    } else {
        None
    };

    // Create .bcvk directory
    let config_dir = project_dir.join(CONFIG_DIR);
    fs::create_dir_all(&config_dir)
        .with_context(|| format!("Failed to create directory: {}", config_dir))?;

    // Build TOML document using toml_edit
    let mut doc = DocumentMut::new();

    // Add project name if set
    if let Some(name) = custom_name {
        let mut project_table = Table::new();
        project_table.insert("name", value(name));
        doc.insert("project", Item::Table(project_table));
    }

    // Add VM section with only the image
    let mut vm_table = Table::new();
    vm_table.insert("image", value(image));
    doc.insert("vm", Item::Table(vm_table));

    // Write configuration file
    let config_path = config_dir.join(CONFIG_FILE);
    fs::write(&config_path, doc.to_string())
        .with_context(|| format!("Failed to write config file: {}", config_path))?;

    Ok(())
}

//! Export CLI structure as JSON for documentation generation
#![cfg(feature = "docgen")]

use clap::Command;
use serde::{Deserialize, Serialize};

/// Representation of a CLI option for JSON export
#[derive(Debug, Serialize, Deserialize)]
pub struct CliOption {
    pub long: String,
    pub short: Option<String>,
    pub value_name: Option<String>,
    pub default: Option<String>,
    pub help: String,
    pub possible_values: Vec<String>,
    pub required: bool,
}

/// Representation of a CLI command for JSON export
#[derive(Debug, Serialize, Deserialize)]
pub struct CliCommand {
    pub name: String,
    pub about: Option<String>,
    pub options: Vec<CliOption>,
    pub positionals: Vec<CliPositional>,
    pub subcommands: Vec<CliCommand>,
}

/// Representation of a positional argument
#[derive(Debug, Serialize, Deserialize)]
pub struct CliPositional {
    pub name: String,
    pub help: Option<String>,
    pub required: bool,
    pub multiple: bool,
}

/// Convert a clap Command to our JSON representation
pub fn command_to_json(cmd: &Command) -> CliCommand {
    let mut options = Vec::new();
    let mut positionals = Vec::new();

    // Extract arguments
    for arg in cmd.get_arguments() {
        let id = arg.get_id().as_str();

        // Skip built-in help and version
        if id == "help" || id == "version" {
            continue;
        }

        if arg.is_positional() {
            positionals.push(CliPositional {
                name: id.to_string(),
                help: arg.get_help().map(|h| h.to_string()),
                required: arg.is_required_set(),
                multiple: arg.get_action().takes_values(),
            });
        } else {
            let long = arg.get_long().unwrap_or(id).to_string();

            let short = arg.get_short().map(|c| c.to_string());

            // Don't set value_name for boolean flags
            // Check if this is a boolean flag by looking at the action type
            let value_name = match arg.get_action() {
                clap::ArgAction::SetTrue | clap::ArgAction::SetFalse => None,
                // If it takes no values, it's likely a boolean
                _ if !arg.get_action().takes_values() => None,
                _ => arg
                    .get_value_names()
                    .and_then(|names| names.first())
                    .map(|name| name.as_str().to_string()),
            };

            let help = arg.get_help().map(|h| h.to_string()).unwrap_or_default();

            let all_possible_values: Vec<String> = arg
                .get_possible_values()
                .iter()
                .map(|v| v.get_name().to_string())
                .collect();

            // Filter out "true/false" for boolean flags - they're obvious
            let possible_values = if all_possible_values.len() == 2
                && all_possible_values.contains(&"true".to_string())
                && all_possible_values.contains(&"false".to_string())
            {
                // This is a boolean flag, don't show possible values
                Vec::new()
            } else {
                all_possible_values
            };

            let default = arg
                .get_default_values()
                .first()
                .and_then(|v| v.to_str())
                .map(|s| s.to_string());

            options.push(CliOption {
                long,
                short,
                value_name,
                default,
                help,
                possible_values,
                required: arg.is_required_set(),
            });
        }
    }

    // Extract subcommands
    let subcommands = cmd
        .get_subcommands()
        .filter(|subcmd| !subcmd.is_hide_set())
        .map(command_to_json)
        .collect();

    CliCommand {
        name: cmd.get_name().to_string(),
        about: cmd.get_about().map(|s| s.to_string()),
        options,
        positionals,
        subcommands,
    }
}

/// Dump the complete CLI structure as JSON
pub fn dump_cli_json() -> color_eyre::Result<String> {
    use clap::CommandFactory;

    let cmd = crate::Cli::command();
    let json_structure = command_to_json(&cmd);
    let json = serde_json::to_string_pretty(&json_structure)?;
    Ok(json)
}

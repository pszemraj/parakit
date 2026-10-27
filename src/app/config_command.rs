//! `parakit config` subcommands: show, path, init, and edit.

use super::*;
use crate::cli::{ConfigCli, ConfigCommand};
use std::io::Write as _;

/// Run a `config` subcommand, defaulting to `show` when none is given.
///
/// # Arguments
///
/// * `config_cli` - Parsed `config` subcommand.
/// * `quiet` - Suppress informational stdout output.
///
/// # Returns
///
/// Success after the selected subcommand completes.
///
/// # Errors
///
/// Fails on path resolution, template writing, config validation, or editor
/// launch errors.
pub(super) fn run_config_command(config_cli: &ConfigCli, quiet: bool) -> Result<()> {
    match config_cli.command.as_ref().unwrap_or(&ConfigCommand::Show) {
        ConfigCommand::Path => {
            let path = config::config_path()?;
            if !quiet {
                outln!("{}", path.display())?;
            }
        }
        ConfigCommand::Init { force } => init_config_file(*force, quiet)?,
        ConfigCommand::Show => print_config_show(quiet)?,
        ConfigCommand::Edit => {
            edit_config_file()?;
            if !quiet {
                outln!("Changes apply on the next start.")?;
            }
        }
    }
    Ok(())
}

/// Write the commented config template to the resolved config path.
///
/// # Errors
///
/// Returns an error if the config directory cannot be created, or if the
/// file already exists and `force` is `false`.
fn init_config_file(force: bool, quiet: bool) -> Result<()> {
    let path = config::config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let mut open_options = std::fs::OpenOptions::new();
    open_options.write(true);
    if force {
        open_options.create(true).truncate(true);
    } else {
        open_options.create_new(true);
    }
    let mut file = open_options.open(&path).with_context(|| {
        format!(
            "failed to create config file {} (use --force to overwrite an existing file)",
            path.display()
        )
    })?;
    file.write_all(config::TEMPLATE.as_bytes())
        .with_context(|| format!("failed to write config file {}", path.display()))?;

    if !quiet {
        outln!("wrote {}", path.display())?;
    }
    Ok(())
}

/// Print the resolved config path and effective merged values.
///
/// # Errors
///
/// Returns an error if the config path cannot be resolved or the config
/// file exists but fails to parse or validate.
fn print_config_show(quiet: bool) -> Result<()> {
    let path = config::config_path()?;
    let config = config::load()?;
    // Quiet suppresses the report, not path resolution or config validation.
    if quiet {
        return Ok(());
    }

    outln!("parakit config")?;
    outln!("  path: {}", path.display())?;
    outln!("  exists: {}", path.is_file())?;
    outln!("  daemon:")?;
    outln!(
        "    model: {}",
        config
            .daemon
            .model
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| config.daemon.model_preset.map_or_else(
                || match config.daemon.quantize.unwrap_or_default() {
                    ModelQuantization::Q8 => "(default: hosted Q8_0)".to_owned(),
                    ModelQuantization::Q6K => "(default: parakeet-v3 Q6_K)".to_owned(),
                },
                |preset| format!("(preset: {})", preset.id())
            ))
    )?;
    outln!(
        "    model_preset: {}",
        if config.daemon.model.is_some() {
            "(not applicable: local model)"
        } else {
            config
                .daemon
                .model_preset
                .map(|preset| preset.id())
                .unwrap_or("(default: parakeet-v3)")
        }
    )?;
    outln!(
        "    quantize: {}",
        if config.daemon.model.is_some() {
            "(from model file)"
        } else {
            config.daemon.quantize.unwrap_or_default().as_str()
        }
    )?;
    outln!(
        "    device: {}",
        if config.daemon.device.is_some() {
            config.daemon.effective_device().as_str().to_string()
        } else {
            format!("(default: {})", config.daemon.effective_device().as_str())
        }
    )?;
    outln!(
        "    threads: {}",
        config
            .daemon
            .threads
            .map(|t| t.to_string())
            .unwrap_or_else(|| "(default: auto-detected)".to_string())
    )?;
    outln!(
        "    paste_mode: {}",
        if config.daemon.paste_mode.is_some() {
            config.daemon.effective_paste_mode().label().to_string()
        } else {
            "(default: platform)".to_string()
        }
    )?;
    #[cfg(target_os = "linux")]
    outln!(
        "    insertion_backend: {}",
        config.daemon.insertion_backend.map_or_else(
            || "(default: auto)".to_owned(),
            |backend| backend.label().to_owned()
        )
    )?;
    outln!(
        "    keep_transcript_clipboard: {}",
        config.daemon.keep_transcript_clipboard()
    )?;
    outln!("    sounds: {}", config.daemon.sounds_enabled())?;
    outln!("    verbose: {}", config.daemon.verbose())?;
    outln!(
        "    model_idle_minutes: {}",
        config.daemon.effective_model_idle_minutes()
    )?;
    outln!(
        "    transcript_history: {}",
        config
            .daemon
            .transcript_history
            .map(|n| n.to_string())
            .unwrap_or_else(|| format!(
                "(default: {})",
                config.daemon.effective_transcript_history()
            ))
    )?;
    let qwen = config.qwen.options();
    outln!("  qwen:")?;
    outln!(
        "    language: {}",
        qwen.normalized_language()
            .map_err(anyhow::Error::msg)?
            .as_deref()
            .unwrap_or("auto")
    )?;
    outln!("    max_new_tokens: {}", qwen.max_new_tokens)?;
    outln!("  cleaning:")?;
    outln!("    enabled: {}", config.cleaning.enabled())?;
    outln!("    profile: {}", config.cleaning.effective_profile())?;
    outln!(
        "    keep_trailing_period: {}",
        config.cleaning.keep_trailing_period()
    )?;
    outln!(
        "    number_threshold: {}",
        config
            .cleaning
            .number_threshold
            .map(|value| value.to_string())
            .unwrap_or_else(|| format!("(default: {})", rules::DEFAULT_NUMBER_THRESHOLD))
    )?;
    outln!("    disabled_rules: {:?}", config.cleaning.disabled_rules)?;
    outln!("  logging:")?;
    outln!(
        "    dir: {}",
        config
            .logging
            .dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(disabled)".to_string())
    )?;
    #[cfg(target_os = "linux")]
    {
        outln!("  hotkey:")?;
        outln!(
            "    backend: {}",
            config
                .hotkey
                .backend
                .map(|b| b.label().to_string())
                .unwrap_or_else(|| "(default: auto)".to_string())
        )?;
    }
    outln!("  rules:")?;
    outln!("    user rules: {}", config.rules.user.len())?;
    for user_rule in &config.rules.user {
        outln!("      {} ({})", user_rule.name, user_rule.position.as_str())?;
    }
    Ok(())
}

/// Open the config file in `$VISUAL` or `$EDITOR`, creating it from the
/// template first if it does not exist yet.
///
/// # Errors
///
/// Returns an error if the config path cannot be resolved, the template
/// cannot be written when the file is missing, neither `$VISUAL` nor
/// `$EDITOR` is set, the editor cannot be launched, or the editor exits
/// with a non-zero status.
fn edit_config_file() -> Result<()> {
    let path = config::config_path()?;
    if !path.is_file() {
        init_config_file(false, true)?;
    }

    let visual = std::env::var("VISUAL").ok();
    let fallback = std::env::var("EDITOR").ok();
    let editor = configured_editor(visual.as_deref(), fallback.as_deref()).ok_or_else(|| {
        anyhow::anyhow!(
            "no editor configured: set $VISUAL or $EDITOR, or edit {} directly",
            path.display()
        )
    })?;
    let (program, arguments) = parse_editor_command(editor)?;

    let status = std::process::Command::new(&program)
        .args(arguments)
        .arg(&path)
        .status()
        .with_context(|| format!("failed to launch editor command '{editor}'"))?;
    if !status.success() {
        anyhow::bail!("editor command '{editor}' exited with {status}");
    }
    Ok(())
}

fn configured_editor<'a>(visual: Option<&'a str>, fallback: Option<&'a str>) -> Option<&'a str> {
    visual
        .filter(|value| !value.trim().is_empty())
        .or_else(|| fallback.filter(|value| !value.trim().is_empty()))
}

fn parse_editor_command(editor: &str) -> Result<(String, Vec<String>)> {
    let mut words = shlex::split(editor)
        .ok_or_else(|| anyhow::anyhow!("invalid editor command '{editor}': unmatched quote"))?;
    if words.first().is_none_or(String::is_empty) {
        anyhow::bail!("invalid editor command '{editor}': missing executable");
    }
    let program = words.remove(0);
    Ok((program, words))
}

#[cfg(test)]
#[path = "config_command_tests.rs"]
mod tests;

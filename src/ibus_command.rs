//! Install and inspect Parakit's Linux IBus component.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use parakit::outln;
use std::path::{Path, PathBuf};

const DEFAULT_COMPONENT_DIR: &str = "/usr/share/ibus/component";
const COMPONENT_FILE: &str = "parakit.xml";

/// Arguments for the Linux IBus component commands.
#[derive(Args, Debug)]
pub(crate) struct IbusCli {
    /// Component operation; defaults to reporting status.
    #[command(subcommand)]
    command: Option<IbusCommand>,
}

/// Registration and inspection operations for the IBus-managed engine.
#[derive(Subcommand, Debug)]
enum IbusCommand {
    /// Install Parakit's component XML; the system directory needs administrator privileges.
    Install {
        /// Directory scanned by ibus-daemon for component XML files.
        #[arg(long, default_value = DEFAULT_COMPONENT_DIR)]
        component_dir: PathBuf,
        /// XKB keyboard layout used while the Parakit input source is selected.
        #[arg(long, default_value = "us")]
        layout: String,
        /// Optional XKB variant of the selected keyboard layout.
        #[arg(long, default_value = "")]
        variant: String,
    },
    /// Remove only Parakit's component XML.
    Uninstall {
        /// Directory containing Parakit's component XML.
        #[arg(long, default_value = DEFAULT_COMPONENT_DIR)]
        component_dir: PathBuf,
    },
    /// Report component registration and the current IBus connection and input source.
    Status {
        /// Directory containing Parakit's component XML.
        #[arg(long, default_value = DEFAULT_COMPONENT_DIR)]
        component_dir: PathBuf,
    },
}

/// Run a component command without loading daemon configuration or models.
///
/// # Arguments
///
/// * `cli` - Parsed component command and options.
/// * `quiet` - Suppress informational stdout output.
///
/// # Returns
///
/// Success after the selected component operation completes.
///
/// # Errors
///
/// Returns filesystem, executable-path, engine-startup, or output errors.
pub(crate) fn run(cli: &IbusCli, quiet: bool) -> Result<()> {
    match &cli.command {
        Some(IbusCommand::Install {
            component_dir,
            layout,
            variant,
        }) => {
            if !cfg!(feature = "ibus") {
                anyhow::bail!(
                    "this build does not include IBus support; rebuild with default features or --features ibus"
                );
            }
            let executable =
                std::env::current_exe().context("could not locate parakit executable")?;
            let executable = executable.with_file_name("parakit-ibus");
            anyhow::ensure!(
                executable.is_file(),
                "IBus engine executable not found at {}; install both parakit and parakit-ibus before registering the component",
                executable.display()
            );
            let path = install(component_dir, &executable, layout, variant)?;
            if !quiet {
                outln!("wrote {}", path.display())?;
                outln!(
                    "Log out and back in, then add and select Parakit in your desktop's input-source settings."
                )?;
            }
            Ok(())
        }
        Some(IbusCommand::Uninstall { component_dir }) => {
            let removed = uninstall(component_dir)?;
            if !quiet {
                let path = component_dir.join(COMPONENT_FILE);
                if removed {
                    outln!("removed {}", path.display())?;
                    outln!("Remove Parakit from your desktop's input sources, then log out and back in.")?;
                } else {
                    outln!("not installed: {}", path.display())?;
                }
            }
            Ok(())
        }
        Some(IbusCommand::Status { component_dir }) => print_status(component_dir, quiet),
        None => print_status(Path::new(DEFAULT_COMPONENT_DIR), quiet),
    }
}

/// Write Parakit's component XML without changing input-source settings.
///
/// # Errors
///
/// Returns an error if the executable path cannot be represented in XML or the
/// component directory cannot be created or written.
fn install(
    component_dir: &Path,
    executable: &Path,
    layout: &str,
    variant: &str,
) -> Result<PathBuf> {
    let xml = component_xml(executable, layout, variant)?;
    std::fs::create_dir_all(component_dir).with_context(|| {
        format!(
            "could not create IBus component directory {}; the system directory requires administrator privileges",
            component_dir.display()
        )
    })?;
    let path = component_dir.join(COMPONENT_FILE);
    std::fs::write(&path, xml).with_context(|| {
        format!(
            "could not write {}; the system directory requires administrator privileges",
            path.display()
        )
    })?;
    Ok(path)
}

/// Remove Parakit's registration, returning whether it existed.
///
/// # Errors
///
/// Returns errors other than an already absent component file.
fn uninstall(component_dir: &Path) -> Result<bool> {
    let path = component_dir.join(COMPONENT_FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| {
            format!(
                "could not remove {}; the system directory requires administrator privileges",
                path.display()
            )
        }),
    }
}

/// Report the registration file and read-only IBus discovery state.
///
/// # Errors
///
/// Returns registration-file inspection or output errors.
fn print_status(component_dir: &Path, quiet: bool) -> Result<()> {
    let path = component_dir.join(COMPONENT_FILE);
    let installed = match std::fs::metadata(&path) {
        Ok(metadata) => metadata.is_file(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(error).with_context(|| format!("could not inspect {}", path.display()));
        }
    };
    let status = crate::daemon::desktop::ibus::probe_status();
    if !quiet {
        outln!("parakit IBus")?;
        outln!("  built: {}", status.compiled)?;
        outln!("  registration: {} ({})", installed, path.display())?;
        outln!("  connected: {}", status.connected)?;
        outln!("  engine listed: {}", status.engine_listed)?;
        outln!(
            "  active engine: {}",
            status.active_engine.as_deref().unwrap_or("none")
        )?;
    }
    Ok(())
}

/// Render an IBus component whose command is parsed by GLib, not a shell.
///
/// # Errors
///
/// Returns an error for relative or non-UTF-8 executable paths.
fn component_xml(executable: &Path, layout: &str, variant: &str) -> Result<String> {
    if !executable.is_absolute() {
        anyhow::bail!("IBus component executable must be an absolute path");
    }
    let executable = executable
        .to_str()
        .context("IBus component executable path is not UTF-8")?;
    let command = quote_argument(executable);
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<component>\n\
  <name>org.freedesktop.IBus.Parakit</name>\n\
  <description>Parakit desktop dictation</description>\n\
  <exec>{}</exec>\n\
  <version>{}</version>\n\
  <author>Parakit contributors</author>\n\
  <license>MIT</license>\n\
  <homepage>https://github.com/pszemraj/parakit</homepage>\n\
  <engines>\n\
    <engine>\n\
      <name>parakit</name>\n\
      <longname>Parakit</longname>\n\
      <description>Push-to-talk dictation with the selected keyboard layout</description>\n\
      <language>en</language>\n\
      <license>MIT</license>\n\
      <author>Parakit contributors</author>\n\
      <layout>{}</layout>\n\
      <layout_variant>{}</layout_variant>\n\
      <rank>0</rank>\n\
    </engine>\n\
  </engines>\n\
</component>\n",
        escape_xml(&command),
        env!("CARGO_PKG_VERSION"),
        escape_xml(layout),
        escape_xml(variant),
    ))
}

/// Quote one argument for GLib's shell-style command-line parser.
fn quote_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Escape XML text without interpreting its contents as markup.
fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
#[path = "ibus_command_tests.rs"]
mod tests;

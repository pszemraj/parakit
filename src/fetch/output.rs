//! Concise acquisition feedback, with native tool diagnostics behind verbose mode.

use super::{toml_path_literal, FetchOptions, FetchSource};
use crate::model::{ModelPreset, ModelQuantization};
use crate::output::ProgressStage;
use anyhow::{bail, Context, Result};
use std::io::Write as _;
use std::path::Path;
use std::process::Command;

impl FetchOptions {
    // Status writes are best-effort: a closed launching terminal must not
    // abort a model download or conversion.
    /// Print one acquisition status line unless quiet mode is enabled.
    pub(super) fn status(&self, message: std::fmt::Arguments<'_>) {
        if !self.quiet {
            let _ = writeln!(std::io::stdout(), "{message}");
        }
    }

    /// Print acquisition diagnostics only in verbose mode.
    pub(super) fn verbose_status(&self, message: std::fmt::Arguments<'_>) {
        if self.verbose {
            self.status(message);
        }
    }

    /// Announce the file being downloaded, with its URL reserved for diagnostics.
    pub(super) fn downloading(&self, url: &str) {
        let name = super::url_file_name(url).unwrap_or_else(|_| "model".to_owned());
        self.status(format_args!("parakit: downloading {name}"));
        self.verbose_status(format_args!("parakit: source: {url}"));
    }

    /// Confirm an explicit fetch and show a command that selects that artifact.
    pub(super) fn ready(&self, path: &Path) {
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        self.status(format_args!("parakit: model ready: {name}"));
        self.verbose_status(format_args!("parakit: saved to {}", path.display()));
        let preset = match &self.source {
            FetchSource::HostedQ8 | FetchSource::OfficialNemo { .. } => {
                Some(ModelPreset::ParakeetV3)
            }
            FetchSource::Preset(preset)
            | FetchSource::PresetQ6(preset)
            | FetchSource::PresetSource { preset, .. } => Some(*preset),
            _ => None,
        };
        if let Some(preset) = preset {
            let quant = if matches!(self.source, FetchSource::PresetQ6(_)) {
                ModelQuantization::Q6K
            } else {
                ModelQuantization::Q8
            };
            self.status(format_args!(
                "parakit: run `parakit start --model-preset {preset} --quantize {}`",
                quant.as_str()
            ));
        } else {
            #[cfg(target_os = "windows")]
            {
                self.status(format_args!(
                    "parakit: run in PowerShell: `parakit start -m {}`",
                    command_path(path)
                ));
                if let Some(path) = command_prompt_path(path) {
                    self.status(format_args!(
                        "parakit: run in Command Prompt: `parakit start -m {path}`"
                    ));
                }
            }
            #[cfg(not(target_os = "windows"))]
            self.status(format_args!(
                "parakit: run `parakit start -m {}`",
                command_path(path)
            ));
            self.verbose_status(format_args!(
                "parakit: config: daemon.model = {}",
                toml_path_literal(path)
            ));
        }
    }

    /// Run a conversion tool with activity feedback and bounded failure diagnostics.
    ///
    /// # Arguments
    /// * `command` - Native converter or quantizer to execute.
    /// * `label` - Short description shown while the command runs.
    ///
    /// # Returns
    /// Success when the tool exits successfully.
    ///
    /// # Errors
    /// Reports process startup failures or a failed tool with a concise reason.
    pub(super) fn run_command(&self, command: &mut Command, label: &str) -> Result<()> {
        if self.verbose && !self.quiet {
            self.status(format_args!("parakit: {label}..."));
            return super::run_command(command, label);
        }
        let _progress = ProgressStage::new(format!("parakit: {label}..."), self.quiet);
        let output = command
            .output()
            .with_context(|| format!("{label}: could not start tool"))?;
        if output.status.success() {
            return Ok(());
        }
        let diagnostics = String::from_utf8_lossy(&output.stderr);
        let reason = failure_summary(&diagnostics);
        bail!(
            "{label} failed{}{}. Rerun with --verbose for details",
            if reason.is_empty() { "" } else { ": " },
            reason
        );
    }
}

fn failure_summary(stderr: &str) -> String {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("invalid magic") {
        return "the model file is invalid; retry the fetch with --force".to_owned();
    }
    if lower.contains("no space left") || lower.contains("disk full") {
        return "not enough disk space; free some space and retry".to_owned();
    }
    if lower.contains("permission denied") || lower.contains("access is denied") {
        return "cannot access the model files; check permissions for the model cache".to_owned();
    }
    let last = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if last.to_ascii_lowercase().starts_with("failed to ") {
        return String::new();
    }
    last.chars().take(200).collect()
}

fn command_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    #[cfg(target_os = "windows")]
    {
        format!("'{}'", path.replace('\'', "''"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

#[cfg(target_os = "windows")]
fn command_prompt_path(path: &Path) -> Option<String> {
    let path = path.to_string_lossy();
    // cmd expands variables even inside quotes; the PowerShell hint remains literal.
    (!path.contains(['%', '!'])).then(|| format!("\"{path}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_model_start_command_preserves_shell_metacharacters() {
        #[cfg(not(target_os = "windows"))]
        for name in ["target/a model.gguf", "target/a'$model`name`.gguf"] {
            let command = format!("parakit start -m {}", command_path(Path::new(name)));
            assert_eq!(
                shlex::split(&command).unwrap(),
                ["parakit", "start", "-m", name]
            );
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt as _;
            let root = crate::test_support::fixture_root("parakit-fetch-tests", "shell quoting");
            for name in [
                "a model.gguf",
                "a'$model`name` & (one).gguf",
                "a%PATH%!model.gguf",
            ] {
                let path = root.join(name);
                std::fs::write(&path, b"model").unwrap();
                for (candidate, exists) in [(&path, true), (&path.with_extension("missing"), false)]
                {
                    let script = format!(
                        "if (Test-Path -LiteralPath {}) {{ exit 0 }} else {{ exit 1 }}",
                        command_path(candidate)
                    );
                    let output = Command::new("powershell.exe")
                        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                        .output()
                        .unwrap();
                    assert_eq!(output.status.success(), exists, "{script}: {output:?}");
                    if let Some(quoted) = command_prompt_path(candidate) {
                        let script = format!("if exist {quoted} (exit 0) else (exit 1)");
                        let output = Command::new("cmd.exe")
                            .args(["/D", "/V:OFF", "/C"])
                            .raw_arg(&script)
                            .output()
                            .unwrap();
                        assert_eq!(output.status.success(), exists, "{script}: {output:?}");
                    } else {
                        assert!(name.contains(['%', '!']));
                    }
                }
            }
        }
    }

    #[test]
    fn native_conversion_diagnostics_require_verbose_mode() {
        const ROLE: &str = "PARAKIT_FETCH_OUTPUT_TEST_ROLE";
        const MODE: &str = "PARAKIT_FETCH_OUTPUT_TEST_MODE";
        if std::env::var(ROLE).as_deref() == Ok("tool") {
            println!("native tensor listing");
            for index in 0..10 {
                eprintln!("native diagnostic frame {index}");
            }
            if std::env::var(MODE).as_deref() == Ok("failure") {
                eprintln!("model conversion error: invalid tensor");
                std::process::exit(1);
            }
            return;
        }
        let test = "fetch::output::tests::native_conversion_diagnostics_require_verbose_mode";
        if std::env::var(ROLE).as_deref() == Ok("wrapper") {
            let mode = std::env::var(MODE).unwrap();
            let options = FetchOptions {
                force: false,
                quiet: mode == "quiet",
                verbose: mode == "verbose",
                source: FetchSource::HostedQ8,
            };
            let mut command = crate::test_support::exact_test_command(test);
            command.env(ROLE, "tool");
            if let Err(error) = options.run_command(&mut command, "preparing model") {
                eprintln!("{error:#}");
            }
            return;
        }
        for mode in ["normal", "quiet", "verbose", "failure"] {
            let output = crate::test_support::exact_test_command(test)
                .env(ROLE, "wrapper")
                .env(MODE, mode)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                stdout.contains("native tensor listing"),
                mode == "verbose",
                "{mode}: {stdout}"
            );
            if mode == "failure" {
                assert!(
                    stderr.contains("invalid tensor") && stderr.contains("--verbose"),
                    "{stderr}"
                );
                assert!(!stderr.contains("native diagnostic frame 0"), "{stderr}");
            } else {
                assert_eq!(
                    stderr.contains("native diagnostic frame"),
                    mode == "verbose",
                    "{mode}: {stderr}"
                );
            }
        }
    }
}

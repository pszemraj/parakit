# parakit Docs

Everything under `docs/`, grouped by when you need it.

## Setup

- [build.md](build.md) - building from source and native dependencies per platform.
- [macos-desktop.md](macos-desktop.md) - macOS permissions, the hotkey, and the paste-acknowledgement step.
- [linux-desktop.md](linux-desktop.md) - Linux X11 requirements and the experimental evdev-proxy hotkey backend.
- [windows-desktop.md](windows-desktop.md) - Windows hotkey registration, insertion, and `doctor --deep`.
- [Windows bundle scripts](../scripts/windows/README.md) - packaging a runnable Windows bundle.

## Daily Use

- [running.md](running.md) - running the daemon, daemon control, the model cache, and paste modes.
- [configuration.md](configuration.md) - the config file, CLI/config precedence, and user-defined cleaning rules.
- [config_reference.toml](config_reference.toml) - the per-key configuration reference.
- [cleaning-rules.md](cleaning-rules.md) - the built-in transcript cleanup passes and profiles.
- [logging.md](logging.md) - the JSONL transcription and insertion log schema.
- [troubleshooting.md](troubleshooting.md) - symptom-first fixes for common problems.

## Development

- [dev/architecture.md](dev/architecture.md) - module map and platform work.
- [dev/quality.md](dev/quality.md) - validation and quality checks.
- [dev/README.md](dev/README.md) - maintainer notes.

# parakit Docs

## Setup

- [build.md](build.md) - building from source and native dependencies per platform.
- [macos-desktop.md](macos-desktop.md) - macOS permissions, the hotkey, and the paste-acknowledgement step.
- [linux-desktop.md](linux-desktop.md) - Linux X11 requirements and the experimental evdev-proxy hotkey backend.
- [windows-desktop.md](windows-desktop.md) - Windows hotkey registration, insertion, and `doctor --deep`.
- [Windows bundle scripts](../scripts/windows/README.md) - packaging a runnable Windows bundle.

## Daily Use

- [running.md](running.md) - running the daemon, daemon control, the model cache, and paste modes.
- [configuration.md](configuration.md) - the config file, CLI/config precedence, and user-defined cleaning rules.
- [example.toml](example.toml) - a minimal starter config with a comment on each choice.
- [config_reference.toml](config_reference.toml) - the per-key configuration reference, itself a loadable config that holds every default.
- [cleaning-rules.md](cleaning-rules.md) - the built-in transcript cleanup passes and profiles.
- [logging.md](logging.md) - the JSONL transcription and insertion log schema.
- [troubleshooting.md](troubleshooting.md) - symptom-first fixes for common problems.

## Development

- [dev/architecture.md](dev/architecture.md) - module map and platform work.
- [dev/quality.md](dev/quality.md) - validation and quality checks.
- [dev/model-validation.md](dev/model-validation.md) - native model, resampler, bundle, and physical desktop checks, with [focused Windows results](dev/model-validation.md#focused-windows-results-2026-10-19).
- [dev/memory.md](dev/memory.md) - historical Linux reload measurements and current loader accounting.
- [dev/crispasr-upgrade.md](dev/crispasr-upgrade.md) - CrispASR pin, dependency updates, measurements, and validation limits.
- [dev/macos-model-validation-2026-10-18.md](dev/macos-model-validation-2026-10-18.md) - CPU/Metal preset and precision coverage; [upgrade validation](dev/crispasr-macos-validation.md) and [earlier offload measurements](dev/macos-validation-2026-10-07.md) retain their dated results.
- [dev/README.md](dev/README.md) - maintainer notes.

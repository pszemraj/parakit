# parakit Docs

## Setup

- [build.md](build.md) - building from source and native dependencies per platform.
- [macos-desktop.md](macos-desktop.md) - macOS permissions, the hotkey, and the paste-acknowledgement step.
- [linux-desktop.md](linux-desktop.md) - IBus setup, focused-field GNOME Wayland dictation, and X11 fallback.
- [windows-desktop.md](windows-desktop.md) - Windows hotkey registration, insertion, and `doctor --deep`.
- [Windows bundle scripts](../scripts/windows/README.md) - packaging a runnable Windows bundle.

## Daily Use

- [running.md](running.md) - running the daemon, daemon control, the model cache, and paste modes.
- [models/qwen3-asr.md](models/qwen3-asr.md) - Qwen model setup, language controls, prior-dictation context, and long-audio behavior.
- [configuration.md](configuration.md) - the config file, CLI/config precedence, and user-defined cleaning rules.
- [example.toml](example.toml) - a minimal starter config with a comment on each choice.
- [config_reference.toml](config_reference.toml) - the per-key configuration reference, itself a loadable config that holds every default.
- [cleaning-rules.md](cleaning-rules.md) - the built-in transcript cleanup passes and profiles.
- [logging.md](logging.md) - the JSONL transcription and insertion log schema.
- [troubleshooting.md](troubleshooting.md) - symptom-first fixes for common problems.

## Development

- [dev/architecture.md](dev/architecture.md) - module map and platform work.
- [dev/quality.md](dev/quality.md) - validation and quality checks.
- [dev/model-validation.md](dev/model-validation.md) - native model, resampler, bundle, and physical desktop checks.
- [dev/memory.md](dev/memory.md) - session ownership, memory accounting, and reload diagnostics.
- [dev/README.md](dev/README.md) - maintainer notes.

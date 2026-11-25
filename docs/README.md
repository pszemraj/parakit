# parakit Docs

## Setup

- [Install and build](build.md) on Linux, macOS, or Windows.
- [Linux desktop input](linux-desktop.md) covers IBus on GNOME Wayland and hotkey capture on X11.
- [macOS desktop setup](macos-desktop.md) covers permissions, hotkeys, and paste confirmation.
- [Windows desktop setup](windows-desktop.md) covers microphone access, hotkeys, and insertion. Use the [bundle scripts](../scripts/windows/README.md) to package or install a Windows build.

## Daily Use

- [Run and control the daemon](running.md), choose a model, and configure insertion.
- [Use Qwen3-ASR](models/qwen3-asr.md) with a forced language or context from earlier dictations.
- [Configure parakit](configuration.md) with the [starter config](example.toml) or look up an option in the [TOML reference](config_reference.toml).
- [Clean transcripts](cleaning-rules.md) with profiles, word replacements, and regex rules.
- [Log dictations](logging.md) and interpret transcription and insertion records.
- [Troubleshoot](troubleshooting.md) hotkeys, the microphone, insertion, and model downloads.

## Development

- [Architecture](dev/architecture.md) and [model memory](dev/memory.md)
- [Validation and quality checks](dev/quality.md) and [native platform checks](dev/model-validation.md)
- [Model conversion and CrispASR maintenance](dev/README.md)

# Qwen3-ASR

Parakit's experimental Qwen3-ASR presets support the 0.6B and 1.7B models, language hints, and context from recent dictations. The [Qwen models](https://huggingface.co/Qwen/Qwen3-ASR-0.6B) use the Apache 2.0 license.

## Start

After following the [installation instructions](../README.md#install), select a Qwen model:

```bash
parakit start --model-preset qwen3-asr-1.7b --no-cleaning
```

Use `qwen3-asr-0.6b` for the smaller model. For advance downloading, run `parakit fetch --model-preset qwen3-asr-1.7b`. Both presets use the shared [model cache and precision options](running.md#model-presets).

To keep the selection, edit the file printed by `parakit config path`:

```toml
[daemon]
model_preset = "qwen3-asr-1.7b"

[qwen]
language = "auto"
max_new_tokens = 512
context_on_default_hotkey = false
context_dictations = 3
context_max_age_minutes = 10

[cleaning]
enabled = false
```

Apply model and configuration changes through the [restart workflow](configuration.md#validation-and-recovery). Disabling [cleaning](cleaning-rules.md#profiles) preserves raw model wording and avoids English cleanup on multilingual speech.

`--qwen-language en` (or `English`) forces English instead of automatic language identification. `--qwen-max-new-tokens N` overrides the per-chunk generation budget. Accepted languages, bounds, defaults, and CLI precedence are listed under `[qwen]` in the [configuration reference](config_reference.toml). Explicit Qwen flags require a Qwen model; persistent `[qwen]` settings are inactive for Parakeet after validation.

## Prior Dictation Context

Hold `Ctrl+Alt+Space` to request recent context. On macOS use `Left Control+Left Option+Space`. The original shortcut uses ordinary transcription by default; set `context_on_default_hotkey = true` to use context there too. Context intent is fixed when Space is pressed, so releasing Alt or Option while speaking does not change it or stop recording. Releasing Control or Space stops recording.

The worker selects prior dictations across all apps using the configured count and age limits, orders them oldest first, and keeps the newest 1,500 Unicode characters, including newline separators. It uses stored cleaned text, or raw text when cleaning is disabled. Earlier mistranscriptions can therefore bias later recognition.

Context reuses [daemon history](running.md#daemon-control); `daemon.transcript_history` limits available entries and never grows to satisfy a context request. History starts empty after a daemon restart and is not loaded from JSONL logs. Setting either `context_dictations` or `daemon.transcript_history` to zero disables context.

Empty, expired, disabled, or unusable context falls back to ordinary dictation. Captures shorter than one second also use ordinary transcription, retaining the existing silence padding, because Qwen can copy prior text on very short audio.

The extra shortcut is inactive for other models. If its registered chord is already owned, Parakit warns and keeps the original shortcut usable; the original-shortcut configuration flag remains available. Linux IBus upgrades need the matching `parakit-ibus` helper and an [engine reload](linux-desktop.md#ibus-setup).

## Long Audio And Inference

By default, Qwen splits long audio into native chunks around 30 seconds, resetting the decoder for each chunk. Every chunk receives the same prior-dictation context; a preceding chunk's new text is not passed to the next. The model's published 1,200-second input capability does not extend [Parakit's recording limit](running.md#microphone).

An incomplete decode fails the dictation instead of inserting partial text. Language hints and token budgets are restored after the [readiness and idle-reload flow](running.md#idle-model-offload). Windowed audio attention is enabled by default; its environment override and reference-comparison settings are covered by the [Qwen validation procedure](dev/quality.md#qwen-and-moondream-comparisons).

For inactive history or shortcut conflicts, follow [context troubleshooting](troubleshooting.md#qwen-context-has-no-effect). If recognition changes unexpectedly, compare ordinary dictation and the selected [cleaning profile](cleaning-rules.md#profiles).

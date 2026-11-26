# Qwen3-ASR

Parakit's experimental Qwen3-ASR presets support language hints and context from recent dictations. The 0.6B and 1.7B models use the Apache 2.0 license. [Qwen3-ASR model card](https://huggingface.co/Qwen/Qwen3-ASR-0.6B)

## Start

Select a Qwen preset after [installing parakit](../../README.md#install).

```bash
parakit start --model-preset qwen3-asr-1.7b --no-cleaning
```

Use `qwen3-asr-0.6b` for the smaller model. For advance downloads or another precision, see [model presets](../running.md#model-presets).

To keep the selection, add these settings to your [config file](../configuration.md#validation-and-recovery).

```toml
[daemon]
model_preset = "qwen3-asr-1.7b"

[cleaning]
enabled = false
```

These examples disable English transcript cleanup for multilingual speech.

`--qwen-language en` (or `English`) selects English as the source language. The default, `auto`, detects the language from audio. `--qwen-max-new-tokens N` sets the per-chunk generation budget. Both apply to Qwen presets and local GGUF files loaded by the Qwen backend. See `[qwen]` in the [configuration reference](../config_reference.toml) for other languages, bounds, and defaults.

## Prior Dictation Context

Use the context shortcut when continuing a topic or reusing names and terms from recent dictations.

| Intent | Linux and Windows | macOS |
| --- | --- | --- |
| Ordinary dictation, ignoring prior context | `Ctrl+Space` | `Left Control+Space` |
| Request prior-dictation context | `Ctrl+Left Alt+Space` | `Left Control+Left Option+Space` |

Set `qwen.context_on_default_hotkey = true` to request context with the original shortcut and leave the extra shortcut unregistered.

Context intent is fixed when Space is pressed, so releasing Alt or Option while speaking does not change it or stop recording. Releasing Control or Space stops recording.

On AltGr layouts, Windows reports Right Alt as Left Control plus Right Alt. Registering the context shortcut consumes `Ctrl+Right Alt+Space` and `AltGr+Space` without starting recording. To preserve `AltGr+Space`, set `context_on_default_hotkey = true` or disable context with `qwen.context_dictations = 0`, then restart.

Context comes from [daemon history](../running.md#daemon-control) across all apps, within the configured count and age limits. Parakit orders the selected dictations oldest first and keeps the newest 1,500 Unicode characters, including newline separators. Age includes time spent suspended. This uses the stored cleaned transcript, or raw text when cleaning is disabled. Setting `qwen.context_dictations` or `daemon.transcript_history` to zero disables context and its extra shortcut.

Captures shorter than one second and captures with empty, expired, disabled, or unusable context use ordinary transcription. Short captures keep their silence padding.

Qwen can copy earlier text or exhaust the generation budget when context is long. Parakit retries once without context if inference fails, returns empty output, or repeats at least five consecutive words from context. For results shorter than five words, all output words must match a phrase in the context. The check compares lowercase Unicode alphanumeric words and ignores punctuation. Repeated speech can also trigger it, and earlier mistranscriptions can still bias the output. If clearing the session context fails, parakit reports the error. If the retry stays empty, it reports no speech detected.

In an evaluation of the 1.7B model, same-conversation context left the word error rate against a model consensus roughly unchanged and improved some names and jargon, such as "cloud code" to "Claude Code" and "Flash Extension" to "Flash Attention". Context from an unrelated recording raised the measured error rate from 7.6% to 8.6%, with 52 captures worse and 29 better. In about 1% of those captures, the model skipped the start of the audio and transcribed only the end, which the retry check can miss. Generic encyclopedia text had no clear effect. About 1,000 characters of context added roughly 40 to 55% to inference time before any retry.

> [!NOTE]
> The evaluation used 228 English voice notes of 15 to 40 seconds from one user, about 100 minutes of audio, mostly from one two-speaker technical conversation. Context came from the model's own earlier transcripts. The reference was a word-level consensus from three Parakeet variants and Qwen3-ASR 0.6B running without context. The scores measure agreement with those models.

Inspect which context was used through [inference diagnostics](../logging.md#transcription-record-fields).

The extra shortcut is inactive for other models. If another application owns the optional shortcut, parakit warns and keeps the original shortcut usable. Follow the [context troubleshooting steps](../troubleshooting.md#qwen-context-has-no-effect) if the shortcut has no effect.

## Long Audio And Inference

By default, Qwen splits long audio at quiet points into chunks of at most 30 seconds. The decoder resets per chunk, and every chunk receives the same prior-dictation context. See [parakit's recording limit](../running.md#microphone) for the maximum combined duration.

If any chunk exhausts `--qwen-max-new-tokens` before reaching the end-of-sequence token, parakit discards the attempt's entire transcription, including completed chunks. A contextual attempt may retry without context. See [context retry rules](#prior-dictation-context), [readiness and idle reload](../running.md#idle-model-offload), and [native inference controls](../configuration.md#native-inference-controls).

For reference comparisons, use the [Qwen validation procedure](../dev/quality.md#qwen-and-moondream-comparisons).

If phrasing changes unexpectedly, compare with ordinary dictation using the same [cleaning profile](../cleaning-rules.md#profiles).

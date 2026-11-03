# Qwen3-ASR

Parakit's experimental Qwen3-ASR presets support the 0.6B and 1.7B models, language hints, and context from recent dictations. The [Qwen models](https://huggingface.co/Qwen/Qwen3-ASR-0.6B) use the Apache 2.0 license.

## Start

After following the [installation instructions](../../README.md#install), select a Qwen model:

```bash
parakit start --model-preset qwen3-asr-1.7b --no-cleaning
```

Use `qwen3-asr-0.6b` for the smaller model. For advance downloading, run `parakit fetch --model-preset qwen3-asr-1.7b`. Both presets use the shared [model cache and precision options](../running.md#model-presets).

To keep the selection, edit the file printed by `parakit config path`:

```toml
[daemon]
model_preset = "qwen3-asr-1.7b"

[cleaning]
enabled = false
```

Apply model and configuration changes through the [restart workflow](../configuration.md#validation-and-recovery). Disabling [cleaning](../cleaning-rules.md#profiles) preserves raw model wording and avoids English cleanup on multilingual speech.

`--qwen-language en` (or `English`) forces English instead of automatic language identification. `--qwen-max-new-tokens N` overrides the per-chunk generation budget. Accepted languages, bounds, defaults, and CLI precedence are listed under `[qwen]` in the [configuration reference](../config_reference.toml). Explicit Qwen flags require a Qwen model; persistent `[qwen]` settings are inactive for Parakeet after validation.

## Prior Dictation Context

The separate shortcut lets you decide for each dictation whether earlier speech is relevant. Request context when continuing a topic or reusing names and terms from recent dictations. Use the original shortcut for unrelated speech: by default, it ignores prior dictations, just like the other models.

Hold the shortcut while speaking:

| Intent | Linux and Windows | macOS |
| --- | --- | --- |
| Ordinary dictation, ignoring prior context | `Ctrl+Space` | `Left Control+Space` |
| Request prior-dictation context | `Ctrl+Left Alt+Space` | `Left Control+Left Option+Space` |

These are the defaults. Set `context_on_default_hotkey = true` to request context with the original shortcut instead; the extra shortcut then adds nothing, so Parakit leaves it unregistered and does not intercept it.

Context intent is fixed when Space is pressed, so releasing Alt or Option while speaking does not change it or stop recording. Releasing Control or Space stops recording.

Use the left Alt key on Linux and Windows. On Windows, `Ctrl+Right Alt+Space` is consumed without starting recording, because Windows reports AltGr as Left Control plus Right Alt; registering the context shortcut also consumes `AltGr+Space` on AltGr layouts. To preserve `AltGr+Space`, set `context_on_default_hotkey = true` to keep context on the original shortcut, or set `context_dictations = 0` to disable context; restart either way, and ordinary dictation remains available.

The worker selects prior dictations across all apps using the configured count and age limits, orders them oldest first, and keeps the newest 1,500 Unicode characters, including newline separators. Age includes time spent suspended. It uses stored cleaned text, or raw text when cleaning is disabled.

Context draws only from [daemon history](../running.md#daemon-control); it neither increases retention nor loads prior text from JSONL logs. Setting either `context_dictations` or `daemon.transcript_history` to zero disables context and its extra shortcut.

Empty, expired, disabled, or unusable context falls back to ordinary dictation, which is never rerun. Captures shorter than one second also use ordinary transcription, retaining the existing silence padding.

Qwen can copy earlier text instead of recognizing the audio, including on longer recordings, and a long context can push generation past its token budget. After contextual inference, Parakit reruns the capture once without context if inference fails, the output is empty, or the output is wholly contained in the prior text or shares at least five consecutive words with it. Matching compares lowercase Unicode alphanumeric words, ignoring punctuation. This can also rerun genuinely repeated phrases, and it does not catch every substitution caused by context; earlier mistranscriptions and unrelated context can still bias recognition. If clearing the context fails, the session state is uncertain and Parakit reports the error without a rerun. The rerun result is used as is, so a capture that stays empty is reported as no speech detected.

A test of the 1.7B model on about 100 minutes of English voice notes (228 captures of 15 to 40 seconds, mostly one technical conversation) supports requesting context only for related speech. Outputs were scored against the consensus of four models that never saw context. Context from the same conversation left the overall error rate essentially unchanged but corrected names and jargon that ordinary dictation got wrong, such as "cloud code" to "Claude Code" and "Flash Extension" to "Flash Attention". Context from an unrelated recording raised the error rate from 7.6% to 8.6%, with 52 captures worse and 29 better. Unrelated words rarely appeared in the output, but in about 1% of captures the model skipped the start of the audio and transcribed only the end, which the rerun check does not catch. Generic encyclopedia text had no clear effect. Context also lengthens inference: about 1,000 characters of it made inference roughly 40 to 55% slower, before any rerun. Leave `context_on_default_hotkey` off unless consecutive dictations usually share a topic.

Use `--verbose` or the [inference log fields](../logging.md#transcription-record-fields) to inspect the shortcut mode, context length, and whether a retry occurred.

The extra shortcut is inactive for other models and when `context_on_default_hotkey = true`. If its registered chord is already owned, Parakit warns and keeps the original shortcut usable; `context_on_default_hotkey = true` requests context there instead. Linux IBus upgrades need the matching `parakit-ibus` helper and an [engine reload](../linux-desktop.md#ibus-setup).

## Long Audio And Inference

By default, Qwen splits long audio into native chunks of at most 30 seconds, choosing quiet split points and resetting the decoder for each chunk. Every chunk receives the same prior-dictation context; a preceding chunk's new text is not passed to the next. The model's published 1,200-second input capability does not extend [Parakit's recording limit](../running.md#microphone).

An incomplete decode fails the dictation instead of inserting partial text. Language hints and token budgets are restored after the [readiness and idle-reload flow](../running.md#idle-model-offload). Windowed audio attention is enabled by default; its environment override and reference-comparison settings are covered by the [Qwen validation procedure](../dev/quality.md#qwen-and-moondream-comparisons).

For inactive history or shortcut conflicts, follow [context troubleshooting](../troubleshooting.md#qwen-context-has-no-effect). If recognition changes unexpectedly, compare ordinary dictation and the selected [cleaning profile](../cleaning-rules.md#profiles).

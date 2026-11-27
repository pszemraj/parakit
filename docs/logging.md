# Transcription And Insertion Logs

## Enable Logging

To log completed dictations for a daemon session, pass a directory at startup.

```bash
parakit start --log-dir "$HOME/.parakit/logs"
```

Set `logging.dir` in [config.toml](configuration.md) to keep logging across restarts. Logging is off by default. Files contain plaintext transcripts and insertion metadata. Audio and console output are excluded. Protect the directory and rotate or delete old files yourself.

The first write creates the directory. Both records append to `parakit-YYYY-MM-DD.jsonl` for the transcription's local date, even if insertion finishes after midnight. Each append and any rollback take an exclusive file lock. The worker flushes each line before continuing, without a filesystem sync guarantee. An interrupted write can leave a partial last line. Logging errors go to stderr when available. Dictation continues after a log or console write failure.

## How To Read A Dictation

A successful transcription normally produces two lines. The first records the text before insertion and the second records the delivery outcome. The example shows the fields that identify a pair.

```jsonl
{"session_id":"session-a","record_id":7,"raw":"the build is green.","cleaned":"The build is green"}
{"kind":"insertion","session_id":"session-a","ref_id":7,"outcome":"pasted"}
```

Match both `session_id` and the sequence number (`record_id` on the transcription, `ref_id` on the insertion). The sequence restarts at zero for each logger session. An interrupted insertion can leave an unmatched transcription. No-speech captures and failed transcriptions produce neither line.

## Transcription Record Fields

| Field | Type | Meaning |
| --- | --- | --- |
| `ts` | string | UTC RFC 3339 timestamp with milliseconds. |
| `session_id` | string | Opaque logger-session identifier shared by this transcription and its insertion outcome. |
| `record_id` | integer | Sequence number within `session_id`, starting at zero. |
| `parakit_version` | string | Cargo package version of the binary that wrote the record. |
| `audio_secs` | number | Length of the recorded utterance. |
| `infer_ms` | integer | Model inference time in milliseconds. |
| `raw` | string | Transcript as returned by the model. |
| `cleaned` | string | Transcript after cleaning passes. |
| `rules_active` | integer | Enabled pass count after profile and disable filtering. |
| `cleaner_version` | integer | Procedural cleaner behavior revision. |
| `cleaning_profile` | string | `safe`, `aggressive`, or `disabled`. |
| `ruleset_id` | string | Fingerprint of the cleaner revision, profile, and ordered active rules. Includes rule names, patterns, replacements and positions, enabled repetition exceptions, and the threshold when a numeric pass is active. Descriptions and disabled rules are excluded. Omitted when cleaning is disabled. |
| `drops_trailing_period` | boolean | Whether the messaging-style terminal-period pass was enabled. |
| `number_threshold` | number or null | The [number-conversion threshold](cleaning-rules.md#numbers) in effect for digit conversion; `null` when cleaning is disabled. |
| `rules_fired` | array | Passes that changed text, in application order, as `{"name":...,"matches":...}` objects. |
| `cleaning_failure` | string | Error text from a bounded matcher that exceeded its limit; the cleaner then keeps the original transcript instead of inserting a partial transformation. Omitted when cleaning succeeded. |
| `inference` | object | Model identity and controls for this transcription. Absent in older rows and when a library caller provides no metadata. |

The daemon records the following fields in `inference`.

| Field | Meaning |
| --- | --- |
| `model` | Loaded GGUF filename, including a custom model selected with `-m`. |
| `model_preset` | Named preset, when selected. Omitted for custom files, including locally generated Q6_K selected with `-m`. |
| `backend` | Model backend family, such as `parakeet` or `qwen3`. |
| `quantization` | GGUF file-level quantization label. If its file-type header is absent, the tensor type containing the most model elements; `unknown` when metadata cannot be read or identified. |
| `requested_device` | Original `auto`, `cpu` or `gpu` policy. |
| `session_device` | Policy used to open the current session, including CPU fallback after reload. |
| `confirmed_device` | `cpu` for explicitly opened CPU sessions; otherwise omitted when execution is unconfirmed. |
| `qwen_language` | Effective forced language, or `auto`; Qwen only. |
| `qwen_max_new_tokens` | Effective per-chunk generation budget; Qwen only. |
| `context_mode` | Shortcut intent latched on press: `plain` for the original shortcut or `with_context` for the context shortcut. The default-hotkey setting can enable context for `plain`. Qwen only. |
| `context_chars` | Unicode characters supplied as prior context, including separators. Zero when history is unavailable or discarded. After a retry, counts the initial context. The context text itself is omitted. Qwen only. |
| `context_retried` | Whether contextual inference that failed, returned no text, or copied the context triggered an ordinary transcription retry. `infer_ms` includes both attempts. Qwen only. |
| `native_api_version` | Linked native C API version, when reported. |

The quantization label describes the artifact as a whole. An `F16` model can contain F32 biases. For runtime device selection and the limits of GPU detection, see [device selection](running.md#device-selection). Qwen fields describe the dictation after warmup. The daemon also prints the three context fields with `--verbose`. Log readers should accept older records without `inference` and ignore unfamiliar fields.

## Insertion Record Fields

| Field | Type | Meaning |
| --- | --- | --- |
| `kind` | string | Always `insertion`; the transcription line carries no `kind`. |
| `ts` | string | UTC RFC 3339 timestamp with milliseconds. |
| `session_id` | string | Logger-session identifier copied from the correlated transcription record. |
| `ref_id` | integer | Sequence number matching the correlated transcription record's `record_id`. |
| `insertion_backend` | string or null | Attempted transport: `ibus`, `clipboard`, or `direct`; `null` when no insertion report recorded a route, such as WAV simulation. Older records omit this field. |
| `outcome` | string | [Delivery result](#insertion-outcomes). |
| `target_bundle_id` | string or null | macOS target bundle identifier when known; `null` on Linux and Windows. |
| `focus_verification` | string | `matched`, `changed`, `ax_unsupported`, `unavailable`, or `not_applicable`. The last value also covers a live focus-recheck error, not only paths that skipped the check. |
| `transcript_chars` | integer | Character count of the transcript offered for insertion. |
| `paste_event_posted` | boolean | Whether a paste chord or type event was sent; `false` for native IBus. |
| `pasteboard_requested` | null | Reserved for a clipboard read-back signal; always `null` today. |
| `acknowledgement_kind` | string | Evidence from the insertion backend, or `not_applicable` when no acknowledgement applies. See the [outcomes](#insertion-outcomes) below. |
| `acknowledgement_ms` | integer or null | Milliseconds of the post-paste wait: the Accessibility acknowledgement poll on macOS, or the clipboard-restore wait on Linux and Windows; `null` when that wait did not run. |
| `clipboard_restored` | boolean or null | `true` when supported prior contents were restored or an unsupported payload's staged replacement was cleared; `false` for retained transcript text or a failed restore; `null` when unknown or inapplicable, including successful native delivery or a competing or unreadable clipboard left untouched. |
| `failure_reason` | string or null | Error or degraded-outcome detail. An unreadable clipboard records the failed observation; when the previous clipboard could not be saved, the transcript is still pasted and kept on the clipboard (`clipboard_restored: false`). |

Current insertion records include every field above; absent values serialize as `null`. Most `error` records have no completed insertion report, so `clipboard_restored` is `null` and `paste_event_posted` is `false` even though the clipboard may have been touched. On Linux, a direct-typing block or error after partial input retains its completed character count in `failure_reason` and reports `paste_event_posted: true`.

## Insertion Outcomes

Read `outcome` with `paste_event_posted`, `acknowledgement_kind`, and `clipboard_restored`.

| Outcome | What happened |
| --- | --- |
| `pasted` | parakit sent a paste chord or direct input. On macOS, `ax_confirmed` means Accessibility supplied paste evidence. Direct typing and the Linux and Windows paste shortcuts have no application-level confirmation. |
| `submitted` | The IBus helper emitted `CommitText` for the recording target (`ibus_submitted`). Application reception is unconfirmed. |
| `uncertain` | The IBus helper may have submitted text before losing its reply (`ibus_uncertain`). A recovery copy leaves the outcome uncertain. Inspect the target before retrying. |
| `pasted_unverified` | macOS posted the chord but could not confirm insertion. The acknowledgement values below explain why. |
| `copied_only` | The transcript is available on the clipboard. With no posted chord, this can be a terminal-mode multiline capture, held modifiers, or a focus/backend rejection. On macOS, a posted chord with `no_evidence` may have landed. |
| `blocked` | A target, content, or clipboard guard prevented completion. IBus sets `ibus_rejected`; a successful recovery copy changes that outcome to `copied_only`. Linux direct typing can stop partway through the text. |
| `skipped` | Sanitization left no printable text, or a WAV simulation skipped desktop insertion. Empty text triggers the error tone and leaves history untouched. The simulation retains its transcript in enabled history. |
| `error` | An insertion or clipboard operation failed. Read `failure_reason` for the backend error. |

**Unconfirmed macOS pastes.** The [paste check](macos-desktop.md#what-counts-as-evidence) compares the target's Accessibility value before and after the chord.

| Acknowledgement | What the daemon observed |
| --- | --- |
| `unverified_timeout` | No focused Accessibility element was available to poll. |
| `unverified_no_baseline` | An element was available but neither the pre-chord read nor the immediate fallback supplied a baseline. |
| `unverified_short_transcript` | A transcript under 12 normalized characters reached the deadline without the exact insertion delta. |
| `unverified_focus_lost` | The element became unavailable and the frontmost app changed or could no longer be identified. Restoration is skipped. |
| `no_evidence` | There was no confirmation before the deadline. If the transcript is still on the clipboard, the outcome is `copied_only`. If the clipboard changed or cannot be verified, it is `pasted_unverified`. Inspect the target before [recovering the text](troubleshooting.md#macos-paste-could-not-be-confirmed). |

A restore failure after a completed or unverified paste leaves the delivery outcome intact. Look at `clipboard_restored`, `failure_reason`, or verbose output for restoration diagnostics. See [clipboard staging and restore](running.md#clipboard-staging-and-restore) for the policy.

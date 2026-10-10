# Transcription And Insertion Logs

parakit can write one JSON object per line (JSONL) for every dictation: what the model heard, what cleanup did to it, and what happened when parakit tried to insert it into the focused application. Logging is off by default. Turn it on with the `logging.dir` config key (see [configuration.md](configuration.md) and [config_reference.toml](config_reference.toml)) or `parakit start`'s `--log-dir` flag (see [running.md#logging-and-sounds](running.md#logging-and-sounds)).

Audio and redirected console output are never logged, but the raw and cleaned transcript text is, in plaintext, with no built-in retention or size cap. Protect the log directory and rotate or delete old files to match the sensitivity of your dictation.

One append-only `parakit-YYYY-MM-DD.jsonl` file is written per local day, rotating when the local date changes. Each JSON object is written and flushed synchronously before the worker continues. There is no filesystem sync guarantee; an interrupted write can leave a partial final line.

Independent logger instances sharing a daily file take an exclusive file lock for each append and any failed-write rollback, so their records cannot overwrite each other.

Terminal output is best-effort. A closed terminal or failed stdout/stderr write does not stop dictation or JSONL logging. JSONL write failures are reported when stderr is available; they do not stop the worker.

## How To Read A Dictation

A completed dictation normally produces two lines: a transcription line, written as soon as the model and cleaner finish, and a later insertion line, written once parakit knows what happened to the paste attempt.

```json
{"ts":"2026-07-27T14:02:11.482Z","session_id":"2026-07-27T14:02:10.981234000Z-p4312-l0","record_id":7,"parakit_version":"0.4.1","audio_secs":4.21,"infer_ms":187,"raw":"so the build is green now.","cleaned":"So the build is green now","rules_active":29,"cleaner_version":20,"cleaning_profile":"safe","ruleset_id":"v20-safe-29a0de74bc0bd1e4","drops_trailing_period":true,"number_threshold":4.0,"rules_fired":[{"name":"capitalize-sentence-starts","matches":1},{"name":"fix-trailing-period","matches":1}]}
{"kind":"insertion","ts":"2026-07-27T14:02:11.930Z","session_id":"2026-07-27T14:02:10.981234000Z-p4312-l0","ref_id":7,"outcome":"pasted","target_bundle_id":"com.mitchellh.ghostty","focus_verification":"matched","transcript_chars":25,"paste_event_posted":true,"pasteboard_requested":null,"acknowledgement_kind":"ax_confirmed","acknowledgement_ms":312,"clipboard_restored":true,"failure_reason":null}
```

A dictation where push-to-talk modifiers were still held down at paste time looks different: no chord is ever sent, and the transcript is deliberately left on the clipboard instead.

```json
{"kind":"insertion","ts":"2026-07-27T14:05:42.118Z","session_id":"2026-07-27T14:05:41.900123000Z-p4312-l0","ref_id":9,"outcome":"copied_only","target_bundle_id":"com.apple.Terminal","focus_verification":"matched","transcript_chars":18,"paste_event_posted":false,"pasteboard_requested":null,"acknowledgement_kind":"not_applicable","acknowledgement_ms":null,"clipboard_restored":false,"failure_reason":null}
```

Join a transcription line to its insertion line by matching (`session_id`, `record_id`) on the transcription record to (`session_id`, `ref_id`) on the insertion record. `session_id` identifies one logger instance (normally one per daemon start), and the numeric sequence restarts at zero on every new session, so the combined key stays unambiguous even after a daemon restart appends to the same daily file. The transcription line is written and flushed before insertion starts, so quitting mid-insertion can leave a transcription line with no matching insertion line; as noted above, this is not a filesystem-sync guarantee.

## Transcription Record Fields

Fields, in serialization order:

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
| `ruleset_id` | string | Identifier of the ordered enabled pass set and configurable cleaning behavior, including user rules and a non-default number threshold. Omitted when cleaning is disabled. |
| `drops_trailing_period` | boolean | Whether the messaging-style terminal-period pass was enabled. |
| `number_threshold` | number or null | The isolated-value cutoff for digit conversion that was actually in effect, 4 by default; `null` only when cleaning is disabled entirely. |
| `rules_fired` | array | Passes that changed text, in application order, as `{"name":...,"matches":...}` objects. |
| `cleaning_failure` | string | Error text from a bounded matcher that exceeded its limit; the cleaner then keeps the original transcript instead of inserting a partial transformation. Omitted when cleaning succeeded. |

`ruleset_id` and `cleaning_failure` are the only omittable fields on this line. Everything else is always present, and `number_threshold` carries whatever threshold was actually in effect, dropping to `null` only when cleaning was disabled. Pass semantics are in [cleaning-rules.md](cleaning-rules.md).

## Insertion Record Fields

Fields, in serialization order:

| Field | Type | Meaning |
| --- | --- | --- |
| `kind` | string | Always `insertion`; the transcription line carries no `kind`. |
| `ts` | string | UTC RFC 3339 timestamp with milliseconds. |
| `session_id` | string | Logger-session identifier copied from the correlated transcription record. |
| `ref_id` | integer | Sequence number matching the correlated transcription record's `record_id`. |
| `outcome` | string | `pasted`, `pasted_unverified`, `copied_only`, `blocked`, `skipped`, or `error`. See [Insertion Outcomes](#insertion-outcomes). |
| `target_bundle_id` | string or null | macOS target bundle identifier when known; `null` on Linux and Windows. |
| `focus_verification` | string | `matched`, `changed`, `ax_unsupported`, `unavailable`, or `not_applicable`. The last value also covers a live focus-recheck error, not only paths that skipped the check. |
| `transcript_chars` | integer | Character count of the transcript offered for insertion. |
| `paste_event_posted` | boolean | Whether a paste chord or type event was sent. |
| `pasteboard_requested` | null | Reserved for a clipboard read-back signal; always `null` today. |
| `acknowledgement_kind` | string | `ax_confirmed`, `unverified_timeout`, `unverified_no_baseline`, `unverified_short_transcript`, `unverified_focus_lost`, `no_evidence`, or `not_applicable`. |
| `acknowledgement_ms` | integer or null | Milliseconds spent waiting for acknowledgement; `null` when no wait occurred. |
| `clipboard_restored` | boolean or null | `true` when supported prior contents were restored or an unsupported payload's staged replacement was cleared; `false` for retained transcript text or a failed restore; `null` when unknown or inapplicable, including a competing or unreadable clipboard left untouched. |
| `failure_reason` | string or null | Error or degraded-outcome detail. On Linux, a blocked partial direct insertion records how many characters completed before typing stopped. An unreadable clipboard records the failed observation; when the previous clipboard could not be saved, the transcript is still pasted and kept on the clipboard (`clipboard_restored: false`). |

No field on the insertion line is omitted; absent values serialize as `null`. Most `error` records have no completed insertion report, so `clipboard_restored` is `null` and `paste_event_posted` is `false` even though the clipboard may have been touched. On Linux, a direct-typing backend error after partial input retains its completed character count and reports `paste_event_posted: true`.

## Insertion Outcomes

`outcome` collapses a lot of decision-making into one word. Reading it alongside `paste_event_posted`, `acknowledgement_kind`, and `clipboard_restored` tells you what actually happened:

- **`pasted`** - a paste chord or direct-typed input was sent and either positively confirmed or has no stronger confirmation signal on this platform (`acknowledgement_kind: ax_confirmed` on macOS; `not_applicable` on Linux and Windows). A competing clipboard write can leave this outcome unchanged with `clipboard_restored: null`; that value does not prove the clipboard was unused.
- **`pasted_unverified`** - a paste chord was sent, but macOS could not positively confirm it landed. `acknowledgement_kind` distinguishes five situations:
  - `unverified_timeout` - no pollable Accessibility value was available at all (no focused element was captured, or the field withholds its value, as secure/password fields do by design).
  - `unverified_no_baseline` - a pollable element existed, but neither the pre-chord read nor the immediate post-chord fallback produced a baseline value to compare later reads against, so confirmation was structurally impossible.
  - `unverified_short_transcript` - a transcript under 12 normalized characters reached the deadline without the exact insertion delta required for text this collision-prone; unrelated target churn may have hidden a paste that landed.
  - `unverified_focus_lost` - the captured element died mid-poll and the frontmost application then changed, or its focus state could no longer be read at all, before any evidence appeared.
  - `no_evidence` - no insertion evidence appeared and another application replaced the staged clipboard. The chord may still have landed, so parakit alerts you to inspect the target and, when history is enabled, recover the full transcript with `copy-last` instead of reporting a definite pre-paste block.

  `unverified_timeout`, `unverified_no_baseline`, and `unverified_short_transcript` apply the restore policy only while the staged clipboard still matches, as with `pasted`. `unverified_focus_lost` skips restoration: the transcript remains if still current (`clipboard_restored: false`), otherwise the competing or unreadable clipboard is preserved (`null`). This is not a restore failure and prints no restore warning.
- **`copied_only`** - the transcript remains active on the clipboard because automatic insertion did not complete. With `paste_event_posted: false`, this covers terminal-mode multiline text and pre-paste fallbacks such as held modifiers, an unavailable backend, or focus rejection when the transcript-retention policy applies. Pre-chord paths that restore or preserve another clipboard payload report `blocked` instead. With `paste_event_posted: true` on macOS and `acknowledgement_kind: no_evidence`, the chord was sent but never confirmed, so parakit keeps the transcript active, plays the error tone, and asks you to inspect the target before pasting manually.
- **`blocked`** - a focus or clipboard guard prevented completion, and parakit plays the error tone. A competing or unverifiable clipboard is preserved with `clipboard_restored: null`; otherwise restoration follows policy. On Linux, direct-typing guards also report `blocked`; `failure_reason` gives the completed character count, and the full transcript remains available through `copy-last` when history is enabled.
- **`skipped`** - insertion was intentionally omitted. This means either nothing printable survived sanitization or the WAV worker simulation disabled desktop insertion; no paste or clipboard action occurs.
- **`error`** - insertion actually failed: the platform backend could not be prepared, the paste chord could not be sent, or a clipboard restore failed on a path that never reached a landed paste. `failure_reason` carries the error text.

A clipboard-restore failure is not always an `error`. If restoration fails or is deliberately skipped *after* a paste already landed or was accepted as unverified, the outcome stays `pasted` or `pasted_unverified`. Clipboard details appear only with `--verbose` and in the insertion record's `clipboard_restored` and `failure_reason` fields; they do not print stderr warnings or fail the dictation.

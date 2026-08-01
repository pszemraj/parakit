# Transcription And Insertion Logs

parakit can write one JSON object per line (JSONL) for every dictation: what the model heard, what cleanup did to it, and what happened when parakit tried to insert it into the focused application. Logging is off by default. Turn it on with the `logging.dir` config key (see [configuration.md](configuration.md) and [config_reference.toml](config_reference.toml)) or `parakit start`'s `--log-dir` flag (see [running.md#logging-and-sounds](running.md#logging-and-sounds)).

Audio and redirected console output are never logged, but the raw and cleaned transcript text is, in plaintext, with no built-in retention or size cap. Protect the log directory and rotate or delete old files to match the sensitivity of your dictation.

One append-only `parakit-YYYY-MM-DD.jsonl` file is written per local day, rotating when the local date changes. Every line is an independent JSON object and is flushed synchronously before the worker continues, so a crash mid-dictation cannot corrupt an earlier line.

## How To Read A Dictation

A completed dictation normally produces two lines: a transcription line, written as soon as the model and cleaner finish, and a later insertion line, written once parakit knows what happened to the paste attempt.

```json
{"ts":"2026-07-27T14:02:11.482Z","session_id":"2026-07-27T14:02:10.981234000Z-p4312-l0","record_id":7,"parakit_version":"0.4.0","audio_secs":4.21,"infer_ms":187,"raw":"so the build is green now.","cleaned":"So the build is green now","rules_active":25,"cleaner_version":8,"cleaning_profile":"safe","ruleset_id":"v8-safe-697797d1e998a158","drops_trailing_period":true,"number_threshold":4.0,"rules_fired":[{"name":"capitalize-sentence-starts","matches":1},{"name":"fix-trailing-period","matches":1}]}
{"kind":"insertion","ts":"2026-07-27T14:02:11.930Z","session_id":"2026-07-27T14:02:10.981234000Z-p4312-l0","ref_id":7,"outcome":"pasted","target_bundle_id":"com.mitchellh.ghostty","focus_verification":"matched","transcript_chars":25,"paste_event_posted":true,"pasteboard_requested":null,"acknowledgement_kind":"ax_confirmed","acknowledgement_ms":312,"clipboard_restored":true,"failure_reason":null}
```

A dictation where push-to-talk modifiers were still held down at paste time looks different: no chord is ever sent, and the transcript is deliberately left on the clipboard instead.

```json
{"kind":"insertion","ts":"2026-07-27T14:05:42.118Z","session_id":"2026-07-27T14:05:41.900123000Z-p4312-l0","ref_id":9,"outcome":"copied_only","target_bundle_id":"com.apple.Terminal","focus_verification":"matched","transcript_chars":18,"paste_event_posted":false,"pasteboard_requested":null,"acknowledgement_kind":"not_applicable","acknowledgement_ms":null,"clipboard_restored":false,"failure_reason":null}
```

Join a transcription line to its insertion line by matching (`session_id`, `record_id`) on the transcription record to (`session_id`, `ref_id`) on the insertion record. `session_id` identifies one logger instance (normally one per daemon start), and the numeric sequence restarts at zero on every new session, so the combined key stays unambiguous even after a daemon restart appends to the same daily file. The transcription line is durable before insertion starts, so quitting mid-insertion can leave a transcription line with no matching insertion line.

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
| `clipboard_restored` | boolean or null | Whether the previous clipboard contents were restored; `null` when the result is unknown or inapplicable (for example, `direct` mode never touches the clipboard). |
| `failure_reason` | string or null | Error text when `outcome` is `error`. |

No field on the insertion line is omitted; absent values serialize as `null`. An `error` record is assembled without a completed insertion report, so `clipboard_restored` is `null` even though the clipboard may have been touched, and `paste_event_posted` is `false` whether or not an event was attempted.

## Insertion Outcomes

`outcome` collapses a lot of decision-making into one word. Reading it alongside `paste_event_posted`, `acknowledgement_kind`, and `clipboard_restored` tells you what actually happened:

- **`pasted`** - a paste chord or direct-typed input was sent and either positively confirmed or has no stronger confirmation signal on this platform (`acknowledgement_kind: ax_confirmed` on macOS; `not_applicable` on Linux and Windows). Read `clipboard_restored` for the clipboard result: `true` means the previous contents were restored, `false` means the transcript remains or the staged clipboard was cleared, and `null` means the path did not use the clipboard.
- **`pasted_unverified`** - a paste chord was sent, but macOS could not positively confirm it landed. `acknowledgement_kind` distinguishes four situations:
  - `unverified_timeout` - no pollable Accessibility value was available at all (no focused element was captured, or the field withholds its value, as secure/password fields do by design).
  - `unverified_no_baseline` - a pollable element existed, but neither the pre-chord read nor the immediate post-chord fallback produced a baseline value to compare later reads against, so confirmation was structurally impossible.
  - `unverified_short_transcript` - a transcript under 12 normalized characters reached the deadline without the exact insertion delta required for text this collision-prone; unrelated target churn may have hidden a paste that landed.
  - `unverified_focus_lost` - the captured element died mid-poll and the frontmost application then changed, or its focus state could no longer be read at all, before any evidence appeared.

  `unverified_timeout`, `unverified_no_baseline`, and `unverified_short_transcript` restore the previous clipboard per policy, same as `pasted`. `unverified_focus_lost` does not (`clipboard_restored: false`): once the target can never be re-observed, the chord very likely landed, but the transcript is kept as the only remaining copy rather than risking it on a target that can no longer be checked.
- **`copied_only`** - the transcript was staged but automatic insertion did not complete. With `paste_event_posted: false`, this covers sanitizer-driven manual copy (including terminal-mode multi-line or over-limit text) and pre-paste fallbacks such as held modifiers, an unavailable backend, focus/guard rejection under the keep-transcript policy, or an open insertion circuit breaker. Clipboard handling follows the reported policy result; it is not necessarily left active in every pre-chord case. With `paste_event_posted: true` on macOS and `acknowledgement_kind: no_evidence`, the chord was sent but never confirmed, so parakit keeps the transcript active, plays the error tone, and asks you to inspect the target before pasting manually.
- **`blocked`** - exclusively a pre-paste guard block, such as focus changing immediately before the paste chord (or before insertion became eligible at all), where the clipboard policy asked to restore the previous contents. The previous clipboard is restored and parakit plays the error tone. In `direct` mode, which never touches the clipboard, the same guard logs `blocked` with `clipboard_restored: null` since there was nothing to restore.
- **`skipped`** - nothing printable survived sanitization, so no insertion or clipboard action occurs.
- **`error`** - insertion actually failed: the platform backend could not be prepared, the paste chord could not be sent, or a clipboard restore failed on a path that never reached a landed paste. `failure_reason` carries the error text.

A clipboard-restore failure is not always an `error`. If the restore step fails *after* a paste already landed or was accepted as unverified, the outcome stays `pasted` or `pasted_unverified` with `clipboard_restored: false`; the daemon prints a warning ("paste succeeded, but could not restore previous clipboard contents; the transcript is likely still on the clipboard") instead of failing the dictation. Only a genuine `error` record counts as a failure toward the automatic-paste circuit breaker described in [running.md#repeated-failures](running.md#repeated-failures); `pasted`/`pasted_unverified` reset it, and `copied_only`/`blocked`/`skipped` leave it unchanged.

`pasted_unverified` with `acknowledgement_kind: unverified_focus_lost` also has `clipboard_restored: false`, but it is not a restore failure and prints no warning: the restore is never attempted, deliberately, because the target became unobservable before one could be trusted (see above). The daemon tells the two situations apart by `acknowledgement_kind` rather than by `clipboard_restored` alone.

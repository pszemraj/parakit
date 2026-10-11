# Troubleshooting

Start with diagnostics. Launch behavior and exit codes are in [running.md#first-run](running.md#first-run). `doctor` does not load the model.

```text
parakit doctor
parakit doctor --verbose
parakit doctor --deep
```

`parakit doctor` prints one status line each for `hotkey`, `daemon`, `mic`, and `insertion`. Whichever line reports `FAIL` tells you which section below to read.

If a daemon is already running, use its local control channel before starting another copy:

```text
parakit status
parakit stop
```

## Hotkey Problems

The push-to-talk chord does nothing, or something else on the desktop reacts instead.

1. Confirm the chord for your platform: `Ctrl+Space` on Linux and Windows, `Left Control+Space` on macOS. Modified chords such as `Ctrl+Shift+Space` or `Command+Space` are passed through to the desktop on purpose and never start a recording.
2. Run `parakit doctor` and read the `hotkey` line. It reports whether the chord could be registered at all.
3. Confirm only one parakit process owns the chord. A second copy fails the daemon lock rather than sharing the hotkey.

   ```text
   parakit status
   parakit stop
   ```

4. Free the chord if another application already owns it. Follow the conflict checks for [Linux](linux-desktop.md#shortcut-conflicts) or [macOS](macos-desktop.md#hotkey).

5. On macOS, follow the [permission recovery steps](macos-desktop.md#permissions), then restart parakit.
6. On Linux, start from the [current X11 session](linux-desktop.md#x11-sessions); a tmux server from an earlier login may have stale display credentials.
7. Rerun `parakit doctor`.

WSL is not the native Windows daemon path. Validate Windows hotkeys, focus checks, and paste behavior from native Windows PowerShell with the Windows bundle.

If that did not fix it, Linux backend selection and conflict hunting are in [linux-desktop.md#shortcut-conflicts](linux-desktop.md#shortcut-conflicts), and macOS event-tap behavior is in [macos-desktop.md#hotkey](macos-desktop.md#hotkey).

## Literal Space Appears

A literal space reaches the focused application while you hold the chord.

Normal dictation backends suppress the literal Space in the platform push-to-talk chord. Linux `x11-listen` is a passive debugging backend and deliberately does not suppress it, so switch off that backend first if you selected it. For any other backend:

1. Confirm only one parakit process is running with `parakit status`.
2. Confirm no desktop shortcut or input method also handles `Ctrl+Space` on Linux and Windows, or `Left Control+Space` on macOS. Platform conflict checks are linked from [Hotkey Problems](#hotkey-problems).
3. If you selected `evdev-proxy-experimental`, confirm `/dev/uinput` is writable and the input device can be grabbed. That backend suppresses the chord by grabbing a physical keyboard event device, so it needs both.

   ```bash
   test -w /dev/uinput
   parakit doctor --hotkey-backend evdev-proxy-experimental
   ```

4. Retry in foreground mode with `--verbose` for backend details. Errors and warnings remain on stderr even in quiet mode.
5. Use an X11 session for Linux insertion. Wayland is rejected at startup, and an XWayland `DISPLAY` is not enough.

If that did not fix it, backend behavior per Linux hotkey route is in [linux-desktop.md](linux-desktop.md).

## Text Does Not Insert

The transcript is printed or logged but nothing arrives in the focused application.

1. Reproduce it without the microphone. This runs clipboard staging, focus checks, paste sanitization, and the paste chord through the running daemon.

   ```text
   parakit test-paste "hello from parakit"
   ```

2. Run the active insertion smoke test. It opens a throwaway probe window, takes focus for a moment, and reads the result back.

   ```text
   parakit doctor --deep
   ```

   In `standard` and `terminal` modes this is a real paste round trip on all three platforms. In `direct` mode on Linux and Windows it performs backend preflight only and never types into the probe, so a passing `direct` deep check proves less than a passing clipboard-mode one. On macOS, `direct` mode still runs the suppressed key-event tap.

3. Check daemon history for the transcript. Recovery requires history to be enabled and the entry still retained.

   ```text
   parakit history
   parakit copy-last
   ```

   When the clipboard changes or cannot be read after staging, parakit re-copies the transcript once before a safe paste and keeps it on the clipboard. If held modifiers prevent the paste, it leaves the transcript there for manual insertion. Focus changes still block automatic insertion, and newer copies made after dispatch are preserved. Enabled OS clipboard history is another possible recovery source; on Windows, check `Win+V`. If transcription logging was enabled, its raw/clean text is another recovery source.

4. Try a different paste mode against the same target. The default is `terminal` on Linux and `standard` elsewhere.

   ```bash
   parakit start --paste-mode standard
   parakit start --paste-mode direct
   ```

   Choose the mode using the [insertion table](running.md#insertion).

5. For multi-line prose, switch from `terminal` to `standard`; see [paste modes](running.md#insertion).
6. On Linux, use an X11 session. Insertion goes through X11/XTest for every paste mode, including `direct`.
7. On macOS, check [permissions and deep diagnostics](macos-desktop.md#permissions) in an active GUI login.
8. On Windows, check whether the target runs elevated. A normal user process cannot inject into an administrator or elevated application, and parakit cannot work around that.
9. If focus changed between recording start and insertion, parakit skips automatic insertion on purpose. Recover the transcript through the sources above. Clipboard handling follows the selected policy and preserves detected competing writes. Hold focus on the target until the success cue.

If that did not fix it, paste modes, focus guards, and clipboard restore policy are in [running.md#insertion](running.md#insertion). Platform specifics are in [linux-desktop.md#deep-doctor-check](linux-desktop.md#deep-doctor-check), [macos-desktop.md#doctor---deep](macos-desktop.md#doctor---deep), and [windows-desktop.md#doctor---deep](windows-desktop.md#doctor---deep).

## macOS Paste Could Not Be Confirmed

macOS plays the error tone and shows a "Paste unconfirmed" notification when it cannot prove that a posted paste landed.

The target did not provide [paste acknowledgement](macos-desktop.md#insertion). Recover the text as follows.

> [!IMPORTANT]
> Look at the target application before you press `Cmd+V`. The paste may have already landed and parakit only failed to observe it. Pasting again over text that is already there inserts a second copy.

1. If the notification says the transcript was copied and the text is not in the target, press `Cmd+V`. This path deliberately leaves the transcript on the clipboard and skips restoration so an uncertain paste cannot destroy its only copy.
2. If the notification says the clipboard changed, do not paste blindly: another application owns the current clipboard. Inspect the target first, then use `parakit copy-last` when transcript history is enabled.
3. If it happens once against a busy or slow target, treat it as a timeout and move on. Confirmation has a fixed deadline, and a target that takes longer than that to update its accessibility value reports no evidence even on a successful paste.
4. If it repeats against the same application, reproduce it without dictating. Focus that application and run:

   ```text
   parakit test-paste "hello from parakit"
   ```

   If the text lands every time but is still reported unconfirmed, that application does not expose the pasted text through Accessibility in a form parakit can match. Restart the daemon with `parakit start --paste-mode direct` while you work in that application; direct typing never touches the clipboard and never runs the acknowledgement step.

5. If JSONL logging is enabled, `"acknowledgement_kind":"no_evidence"` identifies this case. The outcome is `copied_only` while the staged transcript remains current, or `pasted_unverified` when another application replaced the clipboard. Fields that withhold their value use one of the separate `unverified_*` acknowledgement kinds.

If the notification never appears at all, follow the macOS notification note in [macos-desktop.md#insertion](macos-desktop.md#insertion). Acknowledgement and clipboard outcomes are described there and in [logging.md#insertion-outcomes](logging.md#insertion-outcomes).

## Wrong Microphone

parakit records from a different input device than the one you expect. A typical symptom is the error tone right after you speak; without `--quiet`, the daemon also prints `parakit: no speech detected`.

1. Check what parakit selected. `parakit doctor` prints the `mic` line; a running daemon reports the same thing under `parakit status --verbose`.
2. Change the operating system default input device. parakit follows the OS default and has no device-selection flag. Use desktop sound settings, `pavucontrol` on Linux, System Settings > Sound > Input on macOS, or Settings > System > Sound on Windows.
3. On PipeWire or PulseAudio, confirm the change took effect at the audio-server level:

   ```bash
   pactl get-default-source
   pactl list sources | grep -E 'Description:|Sample Specification:' | grep -v monitor
   parakit doctor
   ```

4. Wait a few seconds and rerun `parakit doctor`. An idle daemon switches on its own when CPAL reports a changed default device and prints the new microphone unless `--quiet` is set.
5. Restart parakit if the audio server itself is not reporting the new default source.
6. If the selected device is a monitor, loopback, or virtual source, parakit picked it because no better input was available. Connect or enable a real input device and rerun `parakit doctor`.
7. If parakit warned about a Bluetooth microphone, that is not an error. Bluetooth microphones are allowed, but headset profiles add latency and reduce speech quality, so prefer a wired or USB microphone when transcription accuracy matters.

If that did not fix it, device following, downmixing, and the pre-roll buffer are described in [running.md#microphone](running.md#microphone).

If Windows cannot open any microphone, check [desktop microphone permissions](windows-desktop.md#microphone-permissions).

## Model Reload Fails After Idle

Run `parakit --verbose status` and read `model error`. Confirm that the model
path resolved at startup still exists and is readable, then check device
visibility with `parakit --verbose doctor`. Restore the file or device and
press PTT again; [reload behavior](running.md#idle-model-offload) retries without
a daemon restart. A changed model path or device setting requires a restart.

For unexpectedly high memory after offload, see the [allocation ownership notes](dev/memory.md#allocation-ownership)
for the distinction between session allocations and process-wide backend caches.

## Build And Model Issues

The build fails, the binary will not start, or the model will not load.

1. If the build fails on a missing [CrispASR](https://github.com/CrispStrobe/CrispASR) path dependency:

   ```text
   failed to read vendor\CrispASR\crispasr\Cargo.toml
   ```

   The git submodule is missing. Fix the existing checkout:

   ```bash
   git submodule update --init --recursive
   ```

2. For missing Vulkan headers or Linux shared-library load failures, follow the package list under [native dependencies](build.md#native-dependencies) and the [runtime library checks](build.md#runtime-library-paths).
3. On Windows, the executable needs its generated DLLs beside it. Build and install through the Windows scripts rather than copying `parakit.exe` on its own, then open a new terminal so the updated `PATH` applies.
4. If the model fails to download or open, inspect the cache and force a refetch:

   ```bash
   parakit cache dir
   parakit cache list
   parakit fetch --force
   ```

   `-m /path/to/model.gguf` overrides `daemon.model`; either custom-model setting disables automatic fetch. `PARAKIT_MODELS_DIR` only relocates the default model cache.

If that did not fix it, native dependencies are in [build.md#native-dependencies](build.md#native-dependencies), library path rules are in [build.md#runtime-library-paths](build.md#runtime-library-paths), the Windows scripts are in [../scripts/windows/README.md](../scripts/windows/README.md), and cache behavior is in [running.md#model-cache](running.md#model-cache).

## Downloads Behind A Corporate Proxy

`parakit fetch` (the hosted default, `--from-source`, a Hugging Face repo, or a direct URL) fails with a TLS or connection error, or cannot reach `huggingface.co` at all.

1. parakit builds `reqwest` with `rustls-tls-native-roots`, so the operating system's certificate store is used natively - no bundled CA list to fall out of date, and a corporate TLS-intercepting proxy's CA is trusted automatically once it is installed in that OS store. If you manage certificates through a separate PEM bundle instead, point `SSL_CERT_FILE` (or `SSL_CERT_DIR`) at it.
2. parakit also builds with the `system-proxy` feature, so `HTTPS_PROXY`, `HTTP_PROXY`, and `NO_PROXY` are honored the same way most other CLI tools read them. Set them if an egress proxy is required to reach the internet at all.
3. If the download fails with a certificate-shaped error (mentions `certificate`, an unknown issuer, an invalid peer certificate, or a failed TLS handshake), parakit appends a hint to the error covering steps 1 and 2 automatically - read the full error text, not just the first line.
4. If `huggingface.co` is blocked outright, set `HF_ENDPOINT` to an internal Nexus/Artifactory-style Hugging Face mirror. It is honored for the default hosted download, `--from-source`'s official `.nemo` checkpoint, and every `parakit fetch <owner>/<repo>` lookup.
5. If the mirror (or a specific repo) requires authentication, set `HF_TOKEN` to a bearer token. It is only ever attached to requests that target the resolved `HF_ENDPOINT` host, never to an arbitrary `parakit fetch <url>` host.

```bash
export HF_ENDPOINT=https://artifactory.example.com/huggingface
export HF_TOKEN=hf_...
parakit fetch
```

## Windows GPU Builds

A GPU bundle fails to start, or `parakit start --device gpu` fails before the model loads.

1. Open a new terminal. The installer updates persistent User `PATH` but does not broadcast the change to already-running shells.
2. If startup fails with `0xC0000135` or `STATUS_DLL_NOT_FOUND`, Windows could not resolve a load-time DLL. For a Vulkan bundle, `vulkan-1.dll` comes from the GPU driver, not from parakit: install or update the NVIDIA, AMD, or Intel driver. For a CUDA bundle, the CUDA runtime and cuBLAS DLLs must be reachable from the install directory or `PATH`, or the bundle has to be rebuilt with `--bundle-cuda-dlls`.
3. List the compute devices the bundled ggml can actually see:

   ```text
   parakit doctor --verbose
   ```

   The `compute:` block lists them. A GPU build with no GPU or iGPU listed usually means the driver is missing, is too old for the CUDA toolkit and driver ABI, or does not expose Vulkan on that machine.

4. Confirm the rest of the install works by forcing CPU inference:

   ```text
   parakit start --device cpu
   ```

   If that runs, the problem is device visibility, not the bundle.

5. If startup feels slow rather than broken, that is the intentional backend warmup. Use `--verbose` to see the warmup duration.

If that did not fix it, bundle requirements, CUDA runtime bundling, Vulkan loader behavior, and installer checks are in [../scripts/windows/README.md#runtime-manifest](../scripts/windows/README.md#runtime-manifest), and runtime `--device` behavior is in [running.md#device-selection](running.md#device-selection).

## macOS Metal Builds

Build and permission setup are in [macos-desktop.md](macos-desktop.md). Metal library checks are in [macos-desktop.md#metal-verification](macos-desktop.md#metal-verification).

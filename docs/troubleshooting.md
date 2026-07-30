# Troubleshooting

Start with diagnostics. Launch behavior and exit codes are in [running.md#first-run](running.md#first-run). `doctor` does not load the model.

```text
parakit doctor
parakit doctor --verbose
parakit doctor --deep
```

`parakit doctor` prints one status line each for `hotkey`, `daemon`, `mic`, and `insertion`. Whichever line reports `FAIL` tells you which section below to read.

If a daemon is already running, use the control socket before starting another copy:

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

4. Free the chord if another application already owns it. On GNOME/Ubuntu the usual owner is IBus input-source switching.

   ```bash
   gsettings get org.gnome.desktop.wm.keybindings switch-input-source
   gsettings get org.gnome.desktop.wm.keybindings switch-input-source-backward
   gsettings get org.gnome.desktop.input-sources xkb-options
   ```

   Output mentioning `<Control>space`, `<Ctrl>space`, or a left-control toggle is a conflict. Clear it in Settings > Keyboard > Keyboard Shortcuts, or run `ibus-setup` and remove `Ctrl+Space` from input-method switching. On macOS, check System Settings > Keyboard > Keyboard Shortcuts > Input Sources for `Control+Space` and `Control+Option+Space`.

5. On macOS, grant Accessibility and Input Monitoring to the terminal application that launches parakit, then restart parakit. The CoreGraphics event tap does not survive a privacy-setting change, so a hotkey that worked before you touched System Settings needs a restart to come back.
6. On Linux, start parakit from a terminal opened in the current graphical login. A tmux server that outlived a logout carries stale `DISPLAY` or `XAUTHORITY` values, and `doctor` then reports an X11 error such as `Connection refused`.
7. Rerun `parakit doctor`.

WSL is not the native Windows daemon path. Validate Windows hotkeys, focus checks, and paste behavior from native Windows PowerShell with the Windows bundle.

If that did not fix it, Linux backend selection and conflict hunting are in [linux-desktop.md#shortcut-conflicts](linux-desktop.md#shortcut-conflicts), and macOS event-tap behavior is in [macos-desktop.md#hotkey](macos-desktop.md#hotkey).

## Literal Space Appears

A literal space reaches the focused application while you hold the chord.

Normal dictation backends suppress the literal Space in the platform push-to-talk chord. Linux `x11-listen` is a passive debugging backend and deliberately does not suppress it, so switch off that backend first if you selected it. For any other backend:

1. Confirm only one parakit process is running with `parakit status`.
2. Confirm no desktop shortcut or input method also handles `Ctrl+Space` on Linux and Windows, or `Left Control+Space` on macOS. Use the `gsettings` checks in [Hotkey Problems](#hotkey-problems).
3. If you selected `evdev-proxy-experimental`, confirm `/dev/uinput` is writable and the input device can be grabbed. That backend suppresses the chord by grabbing a physical keyboard event device, so it needs both.

   ```bash
   test -w /dev/uinput
   parakit doctor --hotkey-backend evdev-proxy-experimental
   ```

4. Retry in foreground mode without `--quiet` to see the backend errors that background mode hides.
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

3. Recover the transcript before doing anything else. It is not lost.

   ```text
   parakit history
   parakit copy-last
   ```

   In non-direct paste modes the transcript was staged on the clipboard even when the paste was blocked, so OS clipboard history also has it. On Windows that is `Win+V`, which the user has to enable first.

4. Try a different paste mode against the same target. The default is `terminal` on Linux and `standard` elsewhere.

   ```bash
   parakit start --paste-mode standard
   parakit start --paste-mode direct
   ```

   Use `standard` for applications that only accept `Ctrl+V` and ignore `Ctrl+Shift+V`. Use `direct` only when an application refuses clipboard paste entirely; it types through the platform keyboard API, is slower, and is less reliable for non-ASCII text. On Linux it still requires an X11 session.

5. Check for a sanitization block rather than a backend failure. Terminal mode strips trailing newlines and refuses multi-line text outright, because pasting a newline into a shell submits the command. A multi-line dictation in `terminal` mode is copied, not pasted, by design; switch to `standard` for prose targets.
6. On Linux, use an X11 session. Insertion goes through X11/XTest for every paste mode, including `direct`.
7. On macOS, grant Accessibility and Input Monitoring to the terminal application that launches parakit, restart parakit, and rerun `parakit doctor --deep`. Stage 2 of the deep check needs an active GUI login, not an SSH-only session.
8. On Windows, check whether the target runs elevated. A normal user process cannot inject into an administrator or elevated application, and parakit cannot work around that.
9. If focus changed between the hotkey release and the paste, parakit skips the paste on purpose and leaves the transcript on the clipboard. Hold focus on the target until the success cue.
10. If paste stopped working after several failures in a row, the insertion circuit breaker is open. It retries automatically after a short cooldown, so wait rather than restarting the daemon.

If that did not fix it, paste modes, focus guards, and clipboard restore policy are in [running.md#insertion](running.md#insertion). Platform specifics are in [linux-desktop.md#deep-doctor-check](linux-desktop.md#deep-doctor-check), [macos-desktop.md#doctor---deep](macos-desktop.md#doctor---deep), and [windows-desktop.md#doctor---deep](windows-desktop.md#doctor---deep).

## macOS Paste Could Not Be Confirmed

macOS plays the error tone and shows a "Paste blocked" notification reading `Paste could not be confirmed; transcript copied. Press Cmd+V to insert it.`

parakit sent `Cmd+V`, then polled the focused Accessibility element's value for about 1.8 seconds looking for the transcript. The element exposed a value, but the transcript never appeared in it. parakit cannot tell whether the paste landed, so it left the transcript on the clipboard instead of restoring the previous clipboard contents. The paste may still have succeeded.

> [!IMPORTANT]
> Look at the target application before you press `Cmd+V`. The paste may have already landed and parakit only failed to observe it. Pasting again over text that is already there inserts a second copy.

1. If the text is not there, press `Cmd+V`. The transcript is the current clipboard contents.
2. Expect the previous clipboard contents to be gone. This path deliberately skips the clipboard restore so the transcript cannot be destroyed by a wrong guess. Recover the old contents from a clipboard manager if you need them.
3. If it happens once against a busy or slow target, treat it as a timeout and move on. Confirmation has a fixed deadline, and a target that takes longer than that to update its accessibility value reports no evidence even on a successful paste.
4. If it repeats against the same application, reproduce it without dictating. Focus that application and run:

   ```text
   parakit test-paste "hello from parakit"
   ```

   If the text lands every time but is still reported unconfirmed, that application does not expose the pasted text through Accessibility in a form parakit can match. Restart the daemon with `parakit start --paste-mode direct` while you work in that application; direct typing never touches the clipboard and never runs the acknowledgement step.

5. If JSONL logging is enabled, the insertion record for the dictation has `"outcome":"copied_only"` with `"acknowledgement_kind":"no_evidence"`. That distinguishes this case from `pasted_unverified`, which is the separate and quieter path taken when the field withholds its value entirely, as password and other secure fields do.

If the notification never appears at all, some macOS versions file `osascript`-originated notifications under "Script Editor" rather than under "parakit"; check System Settings > Notifications. The three acknowledgement tiers and what each does to the clipboard are in [running.md#insertion](running.md#insertion).

## Wrong Microphone

parakit records from a different input device than the one you expect.

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

2. If a Vulkan build fails on `spirv/unified1/spirv.hpp`, `spirv-headers` is missing. It is a different package from `spirv-tools`:

   ```bash
   sudo apt install libvulkan-dev vulkan-tools glslc spirv-tools spirv-headers mesa-vulkan-drivers
   ```

3. If the binary builds but fails to load shared libraries on Linux, check that the executable carries `RPATH` rather than `RUNPATH`, so `libcrispasr.so` can find its sibling `libggml*.so` files:

   ```bash
   ldd target/debug/parakit | grep -E "crispasr|ggml"
   readelf -d target/debug/parakit | grep -E "RPATH|RUNPATH"
   ```

4. On Windows, the executable needs its generated DLLs beside it. Build and install through the Windows scripts rather than copying `parakit.exe` on its own, then open a new terminal so the updated `PATH` applies.
5. If the model fails to download or open, inspect the cache and force a refetch:

   ```bash
   parakit cache dir
   parakit cache list
   parakit fetch --force
   ```

   `-m /path/to/model.gguf` always wins and disables automatic fetch, so an unexpected model usually means an `-m` flag or a `PARAKIT_MODELS_DIR` override is still set.

If that did not fix it, native dependencies are in [build.md#native-dependencies](build.md#native-dependencies), library path rules are in [build.md#runtime-library-paths](build.md#runtime-library-paths), the Windows scripts are in [../scripts/windows/README.md](../scripts/windows/README.md), and cache behavior is in [running.md#model-cache](running.md#model-cache).

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

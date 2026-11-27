# Troubleshooting

Run [`parakit doctor`](running.md#first-run) in the desktop session that launches parakit. Read the failing check first. For a running daemon, inspect [`parakit --verbose status`](running.md#daemon-control).

## Hotkey Problems

If push-to-talk does nothing or triggers another desktop action, check the `hotkey` result and the shortcut bindings for [Linux](linux-desktop.md#shortcut-conflicts), [macOS](macos-desktop.md#hotkey), or [Windows](windows-desktop.md#hotkey). If capture fails only in a particular session or launcher, check [X11 session access](linux-desktop.md#x11-sessions) or [macOS permissions](macos-desktop.md#permissions).

## IBus Dictation Stops After An Upgrade

If the helper fails to attach or `Ctrl+Space` passes through after an upgrade, follow the [IBus helper installation and reload steps](linux-desktop.md#ibus-setup).

## Qwen Context Has No Effect

Check the active model in `parakit --verbose status` and recent dictations in `parakit history --plain`. For Qwen3-ASR, check the [context shortcut and history requirements](models/qwen3-asr.md#prior-dictation-context). On Windows or X11 with the registered hotkey backend, `parakit --verbose doctor` can check the optional shortcut before startup.

## Literal Space Appears

Follow the checks for [passive X11](linux-desktop.md#passive-x11-listen), [focused-field IBus](linux-desktop.md#ibus-setup), or the [evdev proxy](linux-desktop.md#evdev-proxy), depending on the active Linux backend.

## Text Does Not Insert

1. Run `parakit test-paste "hello from parakit"` from a disposable text prompt with the daemon running. This exercises insertion without recording audio.
2. Check `parakit doctor --deep` on [Linux](linux-desktop.md#deep-doctor-check), [macOS](macos-desktop.md#doctor---deep), or [Windows](windows-desktop.md#doctor---deep). The check depends on the selected insertion backend and mode.
3. Inspect the target before retrying. For an unconfirmed paste, check the [logged outcome](logging.md#insertion-outcomes) and recover the transcript from [daemon history](running.md#daemon-control) or an enabled clipboard history manager.
4. If the shortcut is wrong for the target, stop the daemon and restart with [the appropriate paste mode](running.md#insertion). For a target switch, focus rejection, or elevated Windows application, see [focus changes](running.md#focus-changes).

## macOS Paste Could Not Be Confirmed

Inspect the target first. If the transcript is missing and the notification says it was copied, press `Cmd+V`. If the notification says the clipboard changed, recover the transcript with `parakit copy-last` when history is enabled.

For a recurring warning in one app, run `sleep 3 && parakit test-paste "hello from parakit"` in a terminal and focus an empty field in the app during the delay. If the text lands but [paste confirmation](macos-desktop.md#insertion) keeps timing out, stop the daemon and restart with `parakit start --paste-mode direct` for that application.

## Wrong Microphone

If speaking triggers the error tone or `parakit: no speech detected`, check `mic` in `parakit doctor` or `parakit --verbose status`. Select the desired default input in desktop sound settings, macOS System Settings > Sound > Input, or Windows Settings > System > Sound. On PipeWire or PulseAudio, compare the default source with the available inputs.

```bash
pactl get-default-source
pactl list sources
parakit doctor
```

See [microphone switching](running.md#microphone) for Bluetooth and device changes, or [Windows microphone permissions](windows-desktop.md#microphone-permissions) if access was denied.

## Model Reload Fails After Idle

Read `model error` and the model filename in `parakit --verbose status`. Locate a cached model with `parakit cache list`, or check the custom path supplied with `-m` or `daemon.model`. Use `parakit --verbose doctor` to inspect visible devices, then press the hotkey to retry. Restart after changing the model path or device setting. See [idle model offload](running.md#idle-model-offload) or, for high memory use after offload, [allocation ownership](dev/memory.md#allocation-ownership).

## Build And Model Issues

For a missing CrispASR path dependency, follow [submodule setup](build.md#install). For missing headers or libraries, check [native dependencies](build.md#native-dependencies). For model paths, cache inspection, or a fresh download, see [model cache](running.md#model-cache).

## Downloads Behind A Corporate Proxy

If `parakit fetch` reports a TLS or connection error, check the operating system's trust store and your proxy settings. For TLS interception, install your organization's CA or configure a certificate file or directory. If Hugging Face is blocked, set the mirror endpoint and supply a token if the mirror requires one. See the [download environment variables](configuration.md#environment-variables) for which requests use these settings.

```bash
export HF_ENDPOINT=https://artifactory.example.com/huggingface
parakit fetch
```

## Windows GPU Builds

Check the bundle's [runtime manifest and loader requirements](../scripts/windows/README.md#runtime-manifest), then [select the runtime device](running.md#device-selection).

## macOS Metal Builds

For Metal startup or library errors, check [Metal verification](macos-desktop.md#metal-verification) and [runtime library paths](build.md#runtime-library-paths).

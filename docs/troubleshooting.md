# Troubleshooting

Start with [parakit doctor](running.md#first-run). The `hotkey`, `daemon`, `mic`, and `insertion` status lines identify which checks need attention. For a running daemon, use [status and control commands](running.md#daemon-control).

## Hotkey Problems

If push-to-talk does nothing or another desktop action happens, read `hotkey` in `parakit doctor`, then follow the conflict checks for [Linux](linux-desktop.md#shortcut-conflicts), [macOS](macos-desktop.md#hotkey), or [Windows](windows-desktop.md#hotkey). Confirm only one daemon is running with `parakit status`; see [daemon control](running.md#daemon-control).

For desktop-session or permission failures, follow the [X11 session checks](linux-desktop.md#x11-sessions) or [macOS permission recovery](macos-desktop.md#permissions).

WSL is not the native Windows daemon path. Check Windows hotkeys, focus, and paste from native PowerShell with the Windows bundle.

## Qwen Context Has No Effect

Confirm a Qwen3-ASR model is active and that `parakit history --plain` contains recent dictations. Inspect `parakit config show` and the [context settings and eligibility rules](models/qwen3-asr.md#prior-dictation-context), then restart the daemon after any configuration change. On registered X11 or Windows, use `parakit --verbose doctor` before startup to inspect the optional context shortcut. On Linux IBus, install the matching helper and follow the [upgrade reload steps](linux-desktop.md#ibus-setup).

## Literal Space Appears

Follow the checks for [passive X11 capture](linux-desktop.md#passive-x11-listen), [focused-field IBus capture](linux-desktop.md#ibus-setup), or the [evdev proxy](linux-desktop.md#evdev-proxy), depending on the selected backend.

## Text Does Not Insert

1. Reproduce without the microphone; this exercises insertion through the running daemon:

   ```text
   parakit test-paste "hello from parakit"
   ```

2. Run `parakit doctor --deep` for the platform's insertion-backend check. Platform requirements and the limits of `direct` mode are in the [Linux](linux-desktop.md#deep-doctor-check), [macOS](macos-desktop.md#doctor---deep), and [Windows](windows-desktop.md#doctor---deep) sections.
3. Inspect the target before recovering the transcript; an unconfirmed paste may have landed. Use [daemon history](running.md#daemon-control), [JSONL logs](logging.md#how-to-read-a-dictation), or clipboard history if enabled.
4. Check the selected [insertion transport and paste mode](running.md#insertion). To try `standard`:

   ```text
   parakit stop
   parakit start --paste-mode standard
   ```

   To try `direct`, repeat the `parakit stop` and `parakit start` pair with `--paste-mode direct`, following the [Linux transport constraints](linux-desktop.md#insertion-selection-and-recovery) or [macOS permissions](macos-desktop.md#permissions). If focus changed before insertion or the target is elevated on Windows, see [focus changes](running.md#focus-changes).

## macOS Paste Could Not Be Confirmed

Inspect the target before pressing `Cmd+V`: the paste may have landed even though parakit could not confirm it.

1. If the notification says the transcript was copied and it is absent from the target, press `Cmd+V`.
2. If it says the clipboard changed, do not paste blindly. Check the target, then use `parakit copy-last` if history is enabled.
3. A one-off warning against a busy application may be a timeout; inspect the target before retrying. If it repeats with one application, reproduce with `parakit test-paste "hello from parakit"`; if the text lands but remains unconfirmed, use `parakit start --paste-mode direct` for that application.

See [macOS insertion](macos-desktop.md#insertion) for notification behavior and [insertion outcomes](logging.md#insertion-outcomes) for the logged acknowledgement states.

## Wrong Microphone

If speaking triggers the error tone or `parakit: no speech detected`, check the selected input device:

1. Check `mic` in `parakit doctor` or `parakit status --verbose`.
2. Change the operating system's default input device; parakit follows that device. Use desktop sound settings, `pavucontrol` on Linux, System Settings > Sound > Input on macOS, or Settings > System > Sound on Windows.
3. On PipeWire or PulseAudio, inspect the selected source and rerun doctor:

   ```bash
   pactl get-default-source
   pactl list sources | grep -E 'Description:|Sample Specification:' | grep -v monitor
   parakit doctor
   ```

Default-device switching and Bluetooth behavior are described under [microphone](running.md#microphone). For Windows microphone access, see [desktop microphone permissions](windows-desktop.md#microphone-permissions).

## Model Reload Fails After Idle

Read `model error` in `parakit --verbose status`, confirm the resolved model path and device are available with `parakit --verbose doctor`, then press PTT to retry. A changed model path or device setting requires a restart. See [idle model offload](running.md#idle-model-offload). For high memory after offload, see the [allocation ownership notes](dev/memory.md#allocation-ownership).

## Build And Model Issues

For a missing CrispASR path dependency, follow [submodule setup](build.md#install). Use [native dependencies](build.md#native-dependencies) for missing headers or shared libraries, and the [Windows bundle scripts](../scripts/windows/README.md) for Windows installs. For model paths, cache inspection, and refetch commands, see [model cache](running.md#model-cache).

## Downloads Behind A Corporate Proxy

`parakit fetch` (the hosted default, `--from-source`, a Hugging Face repo, or a direct URL) fails with a TLS or connection error, or cannot reach `huggingface.co` at all.

1. parakit builds `reqwest` with `rustls-tls-native-roots`, so the operating system's certificate store is used natively - no bundled CA list to fall out of date, and a corporate TLS-intercepting proxy's CA is trusted automatically once it is installed in that OS store. If you manage certificates through a separate PEM bundle instead, point `SSL_CERT_FILE` (or `SSL_CERT_DIR`) at it.
2. parakit also builds with the `system-proxy` feature, so `HTTPS_PROXY`, `HTTP_PROXY`, and `NO_PROXY` are honored the same way most other CLI tools read them. Set them if an egress proxy is required to reach the internet at all.
3. If the download fails with a certificate-shaped error (mentions `certificate`, an unknown issuer, an invalid peer certificate, or a failed TLS handshake), parakit appends a hint to the error covering steps 1 and 2 automatically - read the full error text, not just the first line.
4. If `huggingface.co` is blocked outright, set `HF_ENDPOINT` to an internal Nexus/Artifactory-style Hugging Face mirror. It is honored for hosted preset downloads, `--from-source` checkpoint downloads (including NVIDIA's `.nemo` with `--model-preset parakeet-v3`), and every `parakit fetch <owner>/<repo>` lookup.
5. If the mirror (or a specific repo) requires authentication, set `HF_TOKEN` to a bearer token. It is only ever attached to requests that target the resolved `HF_ENDPOINT` host, never to an arbitrary `parakit fetch <url>` host.

```bash
export HF_ENDPOINT=https://artifactory.example.com/huggingface
export HF_TOKEN=hf_...
parakit fetch
```

## Windows GPU Builds

Follow the [Windows loader and runtime checks](../scripts/windows/README.md#runtime-manifest) and [runtime device selection](running.md#device-selection).

## macOS Metal Builds

For Metal startup or library errors, check [Metal verification](macos-desktop.md#metal-verification) and [runtime library paths](build.md#runtime-library-paths).

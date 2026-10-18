# Troubleshooting

Start with `parakit doctor`; it does not load the model. Launch behavior and exit codes are in [running.md#first-run](running.md#first-run).

```text
parakit doctor
parakit doctor --verbose
parakit doctor --deep
```

The `hotkey`, `daemon`, `mic`, and `insertion` status lines identify which checks need attention. If a daemon is already running, use its local control channel:

```text
parakit status
parakit stop
```

## Hotkey Problems

If push-to-talk does nothing or another desktop action happens, read `hotkey` in `parakit doctor`, then follow the conflict checks for [Linux](linux-desktop.md#shortcut-conflicts), [macOS](macos-desktop.md#hotkey), or [Windows](windows-desktop.md#hotkey). Confirm only one daemon is running with `parakit status`; see [daemon control](running.md#daemon-control).

On Linux, an X11 connection error can mean the terminal or tmux server retained credentials from an earlier desktop login; start from the [current X11 session](linux-desktop.md#x11-sessions), then rerun `parakit doctor`. On macOS, recover a denied privacy prompt through [Permissions](macos-desktop.md#permissions), then restart parakit and rerun `parakit doctor`.

WSL is not the native Windows daemon path. Check Windows hotkeys, focus, and paste from native PowerShell with the Windows bundle.

## Literal Space Appears

Linux `x11-listen` is passive and does not suppress Space; switch to the registered backend for normal dictation. The [evdev proxy](linux-desktop.md#evdev-proxy) also needs a readable keyboard event device and writable `/dev/uinput`. Linux insertion requires X11, including in `direct` mode. Other backend details are under [Linux hotkeys](linux-desktop.md).

## Text Does Not Insert

1. Reproduce without the microphone; this exercises insertion through the running daemon:

   ```text
   parakit test-paste "hello from parakit"
   ```

2. Run `parakit doctor --deep` for the platform's insertion-backend check. Platform requirements and the limits of `direct` mode are in the [Linux](linux-desktop.md#deep-doctor-check), [macOS](macos-desktop.md#doctor---deep), and [Windows](windows-desktop.md#doctor---deep) sections.
3. Inspect the target before recovering the transcript; an unconfirmed paste may have landed. Use [daemon history](running.md#daemon-control), [JSONL logs](logging.md#how-to-read-a-dictation), or clipboard history if enabled.
4. Try another [paste mode](running.md#insertion):

   ```text
   parakit stop
   parakit start --paste-mode standard
   ```

   To try `direct`, repeat the `parakit stop` and `parakit start` pair with `--paste-mode direct`. Linux needs an X11 session even in `direct` mode; macOS needs the terminal's [Accessibility permission](macos-desktop.md#permissions). If focus changed before insertion or the target is elevated on Windows, see [focus changes](running.md#focus-changes).

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

An idle daemon follows a changed default device. Bluetooth headsets are supported, though their profiles can add latency and reduce speech quality. For Windows microphone access, see [desktop microphone permissions](windows-desktop.md#microphone-permissions). Device behavior is described under [microphone](running.md#microphone).

## Model Reload Fails After Idle

Read `model error` in `parakit --verbose status`, confirm the resolved model path and device are available with `parakit --verbose doctor`, then press PTT to retry. A changed model path or device setting requires a restart. See [idle model offload](running.md#idle-model-offload). For high memory after offload, see the [allocation ownership notes](dev/memory.md#allocation-ownership).

## Build And Model Issues

If a build cannot find the CrispASR path dependency, initialize the existing submodule:

```bash
git submodule update --init --recursive
```

Use [native dependencies](build.md#native-dependencies) for missing headers or shared libraries, and the [Windows bundle scripts](../scripts/windows/README.md) for Windows installs. Inspect the model cache or refetch a model with:

```bash
parakit cache dir
parakit cache list
parakit fetch --force
```

`-m /path/to/model.gguf` disables automatic fetch. Cache-directory settings affect downloaded models, not an explicit model path. Custom model paths and cache locations are described under [model cache](running.md#model-cache).

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

Open a new terminal after installing so the updated User `PATH` is available. If startup reports `0xC0000135` or `STATUS_DLL_NOT_FOUND`, install the graphics driver for a missing Vulkan loader, or make the CUDA runtime and cuBLAS DLLs available to the bundle. Run `parakit doctor --verbose` to inspect visible compute devices and `parakit start --device cpu` to check CPU inference. If startup is slow rather than broken, backend warmup is expected; `--verbose` shows timing. Bundle requirements are in the [Windows scripts guide](../scripts/windows/README.md#runtime-manifest); runtime device selection is in [running.md#device-selection](running.md#device-selection).

## macOS Metal Builds

For Metal startup or library errors, check [Metal verification](macos-desktop.md#metal-verification) and [runtime library paths](build.md#runtime-library-paths).

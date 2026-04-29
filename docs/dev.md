# Development Notes

## Model Artifact Policy

End-user startup uses the hosted Q8_0 GGUF from:

```text
https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf
```

The binary downloads `parakeet-tdt-0.6b-v3-Q8_0.gguf`, verifies the compiled-in
SHA256, writes it to the platform model cache, and then starts the daemon. The
default `parakit` command must not require Python, NeMo, PyTorch, or manual
model setup.

`-m <path>` is the escape hatch for local experiments and always disables
automatic model fetch.

## Hosted Repo Layout

The hosted Hugging Face repo should use standard GGUF file names:

```text
parakeet-tdt-0.6b-v3-F16.gguf
parakeet-tdt-0.6b-v3-Q8_0.gguf
parakeet-tdt-0.6b-v3-Q6_K.gguf
parakeet-tdt-0.6b-v3-Q5_K_M.gguf
parakeet-tdt-0.6b-v3-Q4_K_M.gguf
```

The current daemon default is Q8_0. F16 is the source GGUF for future local
re-quantization when a requested quant is not already hosted.

The vendored Parakeet converter currently writes an F16/F32 GGUF; the
Parakeet loader and `crispasr-quantize` path are also written around F16/F32
tensors. Treat BF16 as a future artifact format only after the converter,
loader, and quantizer have explicit BF16 support and validation.

Do not use unrelated file names, nested directories, or model-card-only links
for artifacts the CLI needs. The Rust side builds hosted names from the shared
stem `parakeet-tdt-0.6b-v3` and a quant suffix such as `Q8_0`.

## Future Quantization Flow

The intended later CLI behavior is:

1. If the requested quant exists in the hosted repo, download and verify that
   GGUF directly.
2. If it does not exist, download the hosted F16 GGUF, then run
   `crispasr-quantize` locally to create the requested quant in the cache.
3. Record the source F16 checksum, target quant, quantizer identity, and output
   checksum in `manifest.json`.

This keeps Python out of the normal user path. Python remains only for
rebuilding F16 from NVIDIA's `.nemo` when maintaining the hosted artifacts.

## Source Rebuild

The reproducible rebuild path is explicit:

```bash
python -m pip install -r requirements-convert.txt
parakit fetch --from-source --keep-f16 --keep-nemo
```

That path downloads NVIDIA's official `.nemo`, converts it with
`vendor/CrispASR/models/convert-parakeet-to-gguf.py`, and quantizes the
intermediate GGUF with `crispasr-quantize`.

After rebuilding a release artifact, update the hosted Hugging Face repo, then
update `HOSTED_Q8_SHA256` in `src/model.rs` if the Q8_0 bytes changed.

## Local Artifact Notes

The source-rebuilt upload candidates are written under
`target/tmp/source-cache/parakit/models/` when running:

```bash
XDG_CACHE_HOME=$PWD/target/tmp/source-cache parakit fetch --from-source --keep-f16 --keep-nemo
```

The source-built Q8_0 should be uploaded with the F16 artifact. The older
ignored local file `models/parakeet-tdt-0.6b-v3-q8_0.gguf` is not the canonical
hosted artifact.

The file `target/tmp/gguf-tests/q8.gguf` is only a tiny unit-test fixture for
GGUF dtype parsing.

## TODOs

- Port the `.nemo` to GGUF converter to Rust so source rebuilds do not need
  Python.
- Investigate whether Murmure starts partial inference before the hotkey is
  released and whether that explains perceived CPU latency differences.
- Audit CPU feature flags and ggml backend selection for release builds. Confirm
  AVX2/FMA/AVX-512 detection where available before assuming the CPU path is
  fully optimized.
- Revisit true streaming cursor insertion if streaming becomes a supported
  primary mode. Synthetic typing makes sense for incremental partials, but
  batch mode should remain a one-shot paste unless real-use testing shows that
  clipboard behavior is worse than typed insertion.
- Track clipboard-manager behavior for paste insertion. parakit restores the
  previous text clipboard, but clipboard history tools can still record the
  transient transcript as the previous history item.
- Add a release checklist for regenerating, validating, uploading, and checksum
  pinning hosted GGUF artifacts.
- Add `parakit fetch --quant <QUANT>` after the F16 artifact is hosted. Keep
  Q8_0 as the default unless quality, memory, or startup data justifies changing
  it.
- Add an empty-cache smoke test that runs against the hosted Hugging Face
  artifact after upload.

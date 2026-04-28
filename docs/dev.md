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
parakeet-tdt-0.6b-v3-BF16.gguf
parakeet-tdt-0.6b-v3-Q8_0.gguf
parakeet-tdt-0.6b-v3-Q6_K.gguf
parakeet-tdt-0.6b-v3-Q5_K_M.gguf
parakeet-tdt-0.6b-v3-Q4_K_M.gguf
```

The current daemon default is Q8_0. BF16 is the source GGUF for future local
re-quantization when a requested quant is not already hosted.

Do not use unrelated file names, nested directories, or model-card-only links
for artifacts the CLI needs. The Rust side builds hosted names from the shared
stem `parakeet-tdt-0.6b-v3` and a quant suffix such as `Q8_0`.

## Future Quantization Flow

The intended later CLI behavior is:

1. If the requested quant exists in the hosted repo, download and verify that
   GGUF directly.
2. If it does not exist, download the hosted BF16 GGUF, then run
   `crispasr-quantize` locally to create the requested quant in the cache.
3. Record the source BF16 checksum, target quant, quantizer identity, and output
   checksum in `manifest.json`.

This keeps Python out of the normal user path. Python remains only for
rebuilding BF16 from NVIDIA's `.nemo` when maintaining the hosted artifacts.

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

The ignored local file `models/parakeet-tdt-0.6b-v3-q8_0.gguf` was present
during the hosted-fetch migration and appeared to be a real 711 MB Q8_0 GGUF.
Its SHA256 was:

```text
e8bc983c89342a1f36a5bfa1a7a2dc6fab8f9ebdc2e305738f36e3ff60cbc313
```

The file `target/tmp/gguf-tests/q8.gguf` is only a tiny unit-test fixture for
GGUF dtype parsing.

## TODOs

- Port the `.nemo` to GGUF converter to Rust so source rebuilds do not need
  Python.
- Add a release checklist for regenerating, validating, uploading, and checksum
  pinning hosted GGUF artifacts.
- Add `parakit fetch --quant <QUANT>` after the BF16 artifact is hosted. Keep
  Q8_0 as the default unless quality, memory, or startup data justifies changing
  it.
- Add an empty-cache smoke test that runs against the hosted Hugging Face
  artifact after upload.

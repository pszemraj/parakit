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
- Add a release checklist for regenerating, validating, uploading, and
  checksum-pinning hosted GGUF artifacts.
- Add an empty-cache smoke test that runs against the hosted Hugging Face
  artifact after upload.

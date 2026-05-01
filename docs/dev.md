# Development Notes

## Model Artifacts

End-user startup uses the hosted Q8_0 GGUF:

```text
https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf
```

The binary downloads `parakeet-tdt-0.6b-v3-Q8_0.gguf`, verifies the compiled-in
SHA256, writes it to the platform model cache, and starts the daemon. The
default `parakit` command must not require Python, NeMo, PyTorch, or manual model
setup.

`-m <path>` is the escape hatch for local experiments and always disables
automatic model fetch.

Hosted files should use standard GGUF names:

| File | Role |
| --- | --- |
| `parakeet-tdt-0.6b-v3-Q8_0.gguf` | Default user artifact. |
| `parakeet-tdt-0.6b-v3-F16.gguf` | Source GGUF for future local re-quantization. |
| `parakeet-tdt-0.6b-v3-Q6_K.gguf`, etc. | Optional hosted quants. |

The Rust side builds hosted names from the shared stem
`parakeet-tdt-0.6b-v3` and a quant suffix. Avoid unrelated names, nested
directories, or model-card-only links for CLI artifacts.

The current Parakeet converter, loader, and `crispasr-quantize` path are built
around F16/F32 tensors. Treat BF16 as future work until it has explicit support
and validation.

## Future Quantization

The intended later CLI behavior is:

1. If the requested quant exists in the hosted repo, download and verify that
   GGUF directly.
2. If it does not exist, download the hosted F16 GGUF, then run
   `crispasr-quantize` locally to create the requested quant in the cache.
3. Record the source F16 checksum, target quant, quantizer identity, and output
   checksum in `manifest.json`.

Python should remain outside the normal user path.

## Source Rebuild

Maintainers can rebuild from NVIDIA's `.nemo` checkpoint:

```bash
python -m pip install -r scripts/requirements-convert.txt
parakit fetch --from-source --keep-f16 --keep-nemo
```

That path downloads NVIDIA's official `.nemo`, converts it with
`vendor/CrispASR/models/convert-parakeet-to-gguf.py`, and quantizes the
intermediate GGUF with `crispasr-quantize`.

After rebuilding a release artifact, upload F16 and Q8_0 to the hosted repo.
Update `HOSTED_Q8_SHA256` in `src/model.rs` if the Q8_0 bytes changed.

## Updating CrispASR

Keep submodule updates separate from parakit code changes:

```bash
cd vendor/CrispASR
git fetch
git checkout <tag-or-commit>
cd ../..
git add vendor/CrispASR
cargo build
```

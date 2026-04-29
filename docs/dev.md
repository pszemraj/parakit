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
python -m pip install -r requirements-convert.txt
parakit fetch --from-source --keep-f16 --keep-nemo
```

That path downloads NVIDIA's official `.nemo`, converts it with
`vendor/CrispASR/models/convert-parakeet-to-gguf.py`, and quantizes the
intermediate GGUF with `crispasr-quantize`.

After rebuilding a release artifact, upload F16 and Q8_0 to the hosted repo.
Update `HOSTED_Q8_SHA256` in `src/model.rs` if the Q8_0 bytes changed.

## TODOs

- Port the `.nemo` to GGUF converter to Rust so source rebuilds do not need
  Python.
- Investigate whether Murmure starts partial inference before the hotkey is
  released and whether that explains perceived CPU latency differences.
- Revisit true streaming cursor insertion if streaming becomes a supported
  primary mode. Synthetic typing makes sense for incremental partials, but
  batch mode should remain a one-shot paste unless real-use testing shows that
  clipboard behavior is worse than typed insertion.
- Track clipboard-manager behavior for paste insertion. The required behavior is
  that the user's original text clipboard is restored after insertion. Avoiding
  transcript entries in clipboard history entirely would be nice to have, but it
  is not required while clipboard managers record the transient paste payload as
  an earlier history item.
- Add `parakit fetch --quant <QUANT>` after the F16 artifact is hosted. Keep
  Q8_0 as the default unless quality, memory, or startup data justifies changing
  it.
- Add an empty-cache smoke test that runs against the hosted Hugging Face
  artifact after upload.

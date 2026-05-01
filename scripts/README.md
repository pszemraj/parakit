# Python Helpers

`transcribe_nemo_parakeet.py` is a validation helper, not part of the parakit daemon. It runs NVIDIA NeMo's official Parakeet model path so we can compare parakit's GGUF/CrispASR output against a known reference on the same real audio clips.

Model card: <https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3>

`requirements.txt` belongs to the NeMo reference helper. `requirements-convert.txt` belongs to maintainer source rebuilds described in [../docs/dev.md#source-rebuild](../docs/dev.md#source-rebuild).

## NeMo Reference Setup

Use a separate Python environment. NeMo and PyTorch are heavy dependencies and should not be installed just to run the Rust daemon.

```bash
python -m venv target/tmp/.venv-nemo
source target/tmp/.venv-nemo/bin/activate
python -m pip install --upgrade pip
python -m pip install -r scripts/requirements.txt
```

For CUDA, install the PyTorch build appropriate for the machine first if the default wheel resolver does not pick the right one.

## NeMo Reference Usage

```bash
python scripts/transcribe_nemo_parakeet.py clips/example.wav
python scripts/transcribe_nemo_parakeet.py --device cuda clips/*.wav
```

The script prints one JSON object per input file:

```json
{"audio":"clips/example.wav","model":"nvidia/parakeet-tdt-0.6b-v3","device":"cuda","infer_ms":1234,"text":"..."}
```

Missing `torch` or `nemo` imports fail immediately. That is intentional: this script has one job, and a broken reference environment should be obvious.

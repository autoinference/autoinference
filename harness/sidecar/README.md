# autoinference-sidecar

Out-of-process Python sidecar for the [autoinference](https://github.com/autoinference/autoinference)
CLI. It runs inside your inference-engine venv and answers, over `[u32 BE len][CBOR]` frames on stdio:

* **knob registry** queries over `inference-engine-KB` (3,275 machine-extracted knobs, 482 constraints,
  attention-backend matrix) — set `AUTOINFERENCE_KB_DIR`
* **hardware probes** via `nvidia-smi` (measured values override datasheet priors)

```bash
pip install autoinference-sidecar
python -m autoinference_sidecar   # spoken to by the Rust CLI; not meant to be driven by hand
```

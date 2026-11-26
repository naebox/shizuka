# Bundled Neural Model Notices & Licenses

All neural network weights bundled in `src-tauri/models/` are distributed under permissive open-source licenses compatible with Shizuka's [MIT License](../../LICENSE):

1. **`DeepFilterNet3_onnx.tar.gz`** — Real-time full-band speech enhancement (48 kHz)
   - **Upstream:** [Rikorose/DeepFilterNet](https://github.com/Rikorose/DeepFilterNet) (Hendrik Schröter et al.)
   - **License:** MIT OR Apache-2.0

2. **`campplus_sv_16k_fp16.onnx.gz`** — Speaker verification & voiceprint embedding (`CAM++`, 16 kHz, 80-dim Fbank -> 192-dim embedding, FP16-compressed weights)
   - **Upstream:** [modelscope/3D-Speaker](https://github.com/modelscope/3D-Speaker) (`iic/speech_campplus_sv_zh_en_16k-common_advanced`, Alibaba Group) via [k2-fsa/sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)
   - **License:** Apache License 2.0 (`http://www.apache.org/licenses/LICENSE-2.0`)

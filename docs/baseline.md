# 阶段 0 基线资产

当前仓库中的 Python 参考实现作为数值移植来源，Rust 后端接入前必须补齐以下固定资产。

## 模型文件清单

| 模型 | 必需文件 | 状态 |
| --- | --- | --- |
| SenseVoice-Small | `SenseVoice-Encoder.fp16.onnx`、`SenseVoice-CTC.fp16.onnx`、`tokenizer.bpe.model` | `Sensevoice-Small-ONNX.zip`, SHA-256 `3948B5761F12DB1C01D7A7E596294B43B0316AA5C7A8DF77981E78573997DCBB` |
| Fun-ASR-Nano | `Fun-ASR-Nano-Encoder-Adaptor.fp16.onnx`、`Fun-ASR-Nano-CTC.fp16.onnx`、`Fun-ASR-Nano-Decoder.q5_k.gguf`、`tokens.txt` | `Fun-ASR-Nano-GGUF.zip`, SHA-256 `26A557923AEDC44F1A3033D0A9B9C7B13CBB551F57FB9FD4B15A67BB4B57F998` |
| Qwen3-ASR | `qwen3_asr_encoder_frontend.int4.onnx`、`qwen3_asr_encoder_backend.int4.onnx`、`qwen3_asr_llm.q4_k.gguf` | `Qwen3-ASR-1.7B-gguf.zip`, SHA-256 `080746B8235D55A2F6BFAF1A8BF21F2107C4A29045A48D95C79E454CD42F5BF0` |

## Golden 样本矩阵

每种模型需要使用相同语义类别的 16 kHz mono WAV：

- 普通中文短句。
- 中英混合和产品名。
- 连续数字、日期和金额。
- 至少 20 个专业热词。
- 静音、极短音频和背景噪声。

每个样本需要保存特征张量摘要、encoder 输出 shape、CTC Top-K、tokenizer 输出、最终文本和耗时。热词样本必须同时保存关闭与开启热词的结果，不能用最终文本替换充当模型热词效果。

## Release smoke baseline

2026-07-13 使用 `qwen_test.wav`（SHA-256 `2A28077C4154842E30DC3069D746F3B4F5186182EF9CAB4B4F61E11433425AC9`，3.291 秒）在便携 release 目录执行真实推理：

| 模型 | 加载耗时 | 推理耗时 | 输出 |
| --- | ---: | ---: | --- |
| SenseVoice-Small | 2478 ms | 154 ms | `先生、今日もの全力あなたをアシスしますね。` |
| Fun-ASR-Nano | 2689 ms | 528 ms | `先生、今日もの全力あなたをアシストしますね。` |
| Qwen3-ASR | 1610 ms | 1127 ms | `先生、今日もの全力あなたをアシストしますね。` |

命令格式：`dictate-in.exe --smoke-model <sensevoice|fun-asr-nano|qwen3-asr> <wav>`。

## 参考实现路径

- SenseVoice：`CapsWriter-Offline/core/server/engines/sensevoice_onnx/`
- Fun-ASR-Nano：`CapsWriter-Offline/core/server/engines/fun_asr_gguf/`
- Qwen3-ASR：`CapsWriter-Offline/core/server/engines/qwen_asr_gguf/`

# DictateIn

本地语音输入法。按住说话，松开上屏，全程离线运行，不开放任何网络端口。

基于 [CapsWriter-Offline](https://github.com/HaujetZhao/CapsWriter-Offline) 的 Rust 重写。三个 ASR 引擎的数值实现从 Python 参考实现逐行移植，模型文件直接复用原项目的量化版本。

---

## 特性

- **单进程架构** — 不分 Server / Client，一个二进制搞定一切。多线程通过 channel 通信，无 WebSocket / HTTP / UDP 监听。
- **三引擎切换** — SenseVoice-Small、Fun-ASR-Nano、Qwen3-ASR，运行时热切换。
- **完全离线** — ONNX Runtime + llama.cpp，CPU 或 GPU 推理，零网络请求。
- **Push-to-Talk** — 按住快捷键开始录音，松开上屏。支持键盘组合键和鼠标侧键。
- **热词增强** — 每个引擎按自身能力（CTC Top-K / LLM Prompt）应用热词，不是事后文本替换。
- **灵动岛浮层** — 屏幕底部药丸形悬浮窗，Direct2D + DirectWrite 渲染，绿色波动边缘，宽度按文本自适应。
- **跨平台 GPU** — Windows / macOS / Linux 三平台，每平台提供 CPU + 适配的 GPU 加速后端。

---

## 致谢

本项目站在巨人的肩膀上：

- **[CapsWriter-Offline](https://github.com/HaujetZhao/CapsWriter-Offline)** — by [HaujetZhao](https://github.com/HaujetZhao)。这是本项目的一切基础。三个 ASR 引擎（SenseVoice、Fun-ASR-Nano、Qwen3-ASR）的 Python 参考实现、量化模型文件、热词系统设计、Mel 特征提取算法、CTC 解码逻辑、GGUF embedding 注入方案，全部来自原项目。DictateIn 的 Rust 代码是对照这些 Python 实现逐行数值移植的，模型 SHA-256 校验值也直接引用自原项目的发布。没有 CapsWriter-Offline 的开源，就没有本项目。
- **[SenseVoice](https://github.com/FunAudioLLM/SenseVoice)** — by FunAudioLLM，SenseVoice-Small 模型。
- **[Fun-ASR-Nano](https://github.com/modelscope/FunASR)** — by ModelScope，Fun-ASR-Nano 模型。
- **[Qwen3-ASR](https://github.com/QwenLM/Qwen3)** — by QwenLM，Qwen3-ASR 模型。
- **[llama.cpp](https://github.com/ggml-org/llama.cpp)** — GGUF 推理引擎，Fun-ASR-Nano 和 Qwen3-ASR 的 LLM decoder 通过它运行。
- **[ONNX Runtime](https://github.com/microsoft/onnxruntime)** — ONNX 模型推理引擎，三个引擎的 encoder / CTC 部分通过它运行。
- **[Slint](https://slint.dev)** — 设置窗口 UI 框架。

---

## 下载

从 [Releases](../../releases) 页面下载对应平台的包：

| 平台 | 包名 | GPU 后端 |
|------|------|----------|
| Windows x64 | `dictate-in-windows-x64-cpu.zip` | CPU |
| Windows x64 | `dictate-in-windows-x64-vulkan.zip` | Vulkan（NVIDIA / AMD / Intel） |
| Windows x64 | `dictate-in-windows-x64-cuda.zip` | CUDA + Vulkan（NVIDIA） |
| macOS Apple Silicon | `dictate-in-macos-arm64-cpu.tar.gz` | CPU |
| macOS Apple Silicon | `dictate-in-macos-arm64-metal.tar.gz` | Metal |
| macOS Intel | `dictate-in-macos-x86_64-cpu.tar.gz` | CPU |
| Linux x64 | `dictate-in-linux-x64-cpu.tar.gz` | CPU |
| Linux x64 | `dictate-in-linux-x64-vulkan.tar.gz` | Vulkan |
| Linux x64 | `dictate-in-linux-x64-cuda.tar.gz` | CUDA + Vulkan |

### 模型文件

模型不包含在发布包中。从 CapsWriter-Offline 的 [models release](https://github.com/HaujetZhao/CapsWriter-Offline/releases/tag/models) 下载，解压到程序同目录的 `models/` 下：

```
dictate-in/
  dictate-in.exe
  models/
    sensevoice-small/    SenseVoice-Encoder.fp16.onnx, SenseVoice-CTC.fp16.onnx, tokenizer.bpe.model
    fun-asr-nano/        Fun-ASR-Nano-Encoder-Adaptor.fp16.onnx, Fun-ASR-Nano-CTC.fp16.onnx, Fun-ASR-Nano-Decoder.q5_k.gguf, tokens.txt
    qwen3-asr/           qwen3_asr_encoder_frontend.int4.onnx, qwen3_asr_encoder_backend.int4.onnx, qwen3_asr_llm.q4_k.gguf
  config/
  logs/
  cache/
```

---

## 使用

1. 下载发布包，解压到任意目录。
2. 下载模型文件，放入 `models/` 目录。
3. 运行 `dictate-in`。
4. 系统托盘出现图标后，右键选择「设置」。
5. 在「模型」页面选择 ASR 引擎。
6. 在「输入」页面录制快捷键（支持键盘组合键和鼠标侧键）。
7. 在「热词」页面添加热词（每行一个）。
8. 保存设置。
9. 按住快捷键说话，松开后文本自动上屏到当前前台窗口。

关闭设置窗口只隐藏，程序继续在后台运行。右键托盘图标选择「退出」才是真正的退出。

---

## 架构

```
                    ┌──────────┐
                    │  Slint   │ 设置窗口
                    │   UI     │
                    └────┬─────┘
                         │ channel
    ┌────────────────────┼────────────────────┐
    │                    │                    │
    ▼                    ▼                    ▼
┌─────────┐      ┌─────────────┐      ┌────────────┐
│  Audio  │      │     ASR     │      │  Output    │
│ Worker  │─────▶│   Worker    │─────▶│  Worker    │
│ (cpal)  │ 音频  │ (ort+llama) │ 文本  │ (SendInput)│
└─────────┘      └─────────────┘      └────────────┘
    ▲                    ▲                    │
    │                    │                    ▼
┌─────────┐      ┌─────────────┐      ┌────────────┐
│ Hotkey  │      │   Overlay   │      │ Foreground │
│ Hook    │─────▶│   Window    │◀─────│  Detector  │
└─────────┘ 命令  └─────────────┘ 状态  └────────────┘
```

**线程模型** — 单进程多线程，通过 crossbeam channel 通信：
- Main / UI — Slint 事件循环
- asr-worker — 持有模型，执行推理
- audio-worker — cpal 采集 + 重采样 + ring buffer
- output-worker — SendInput / 剪贴板
- native-overlay — Direct2D 灵动岛浮层
- hotkey-coordinator — 快捷键状态机

**ASR 引擎** — 三个引擎共享 `AsrEngine` + `AsrSession` trait：

| 引擎 | 编码器 | 解码器 | 量化 | 热词能力 |
|------|--------|--------|------|----------|
| SenseVoice-Small | ONNX fp16 | ONNX CTC fp16 | fp16 | CTC Top-K 偏置 |
| Fun-ASR-Nano | ONNX fp16 | ONNX CTC + GGUF Q5_K | fp16 + Q5_K | LLM Prompt 上下文 |
| Qwen3-ASR | ONNX INT4 | GGUF Q4_K LLM | INT4 + Q4_K | LLM Prompt 上下文 |

---

## 从源码构建

### 依赖

- Rust stable（edition 2024）
- CMake（llama.cpp 编译需要）
- C++ 编译器（MSVC / clang / gcc）

### 编译

```bash
# CPU only
cargo build --release

# GPU 加速（按平台选择一个）
cargo build --release --features gpu-vulkan     # Windows / Linux — Vulkan
cargo build --release --features gpu-cuda       # Windows / Linux — CUDA + Vulkan
cargo build --release --features gpu-directml   # Windows — DirectML + Vulkan
cargo build --release --features gpu-metal      # macOS — Metal
cargo build --release --features gpu-rocm       # Linux — ROCm
```

### GPU 后端说明

| Feature | 平台 | ONNX EP | llama.cpp 后端 |
|---------|------|---------|---------------|
| `gpu-vulkan` | Windows / Linux | CPU | Vulkan |
| `gpu-cuda` | Windows / Linux | CUDA | CUDA |
| `gpu-directml` | Windows | DirectML | Vulkan |
| `gpu-metal` | macOS | CoreML | Metal（自动） |
| `gpu-rocm` | Linux | ROCm | ROCm |

ONNX EP 和 llama.cpp 后端是独立配置的。`gpu-directml` 组合 DirectML（ONNX）+ Vulkan（llama.cpp），因为 llama.cpp 没有直接的 DirectML 后端。

### 测试

```bash
cargo test
```

### 冒烟测试

```bash
# 麦克风采集测试（2 秒，无 ASR）
cargo run -- --audio-smoke

# 端到端 ASR 测试（需要模型文件）
cargo run -- --smoke-model sensevoice path/to/test.wav
cargo run -- --smoke-model fun-asr-nano path/to/test.wav
cargo run -- --smoke-model qwen3-asr path/to/test.wav
```

---

## 配置

配置文件位于程序同目录的 `config/settings.toml`，TOML 格式，原子写入。

```toml
schema_version = 2

[general]
launch_at_login = false
minimize_to_tray = true

[hotkey]
keys = ["Ctrl", "Space"]
suppress = false

[audio]
device_id = ""
device_name = ""

[asr]
model = "sense_voice"   # sense_voice | fun_asr_nano | qwen3_asr
provider = "cpu"
partial_interval_ms = 800

[hotwords]
items = ["CapsWriter", "Claude", "CUDA", ...]
boost = 1.0

[output]
mode = "unicode"        # unicode | paste | copy

[overlay]
enabled = true
monitor = "foreground"
```

---

## CI

GitHub Actions 自动构建三平台九个目标，见 `.github/workflows/ci.yml`。tag 推送时自动创建 Release。

---

## 许可证

同 CapsWriter-Offline。

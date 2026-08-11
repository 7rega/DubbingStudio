<div align="center">

<img src="frontend/public/favicon.svg" width="72" alt="Dub Studio"/>

# Dub Studio

**面向 Windows 的免费离线 AI 视频配音工作室 —— 用克隆的声音、翻译字幕和画面文字本地化，把任意视频重新配音成另一种语言。100% 本地运行，零 Python：一个原生 `.exe`（Rust + C++/CUDA），所有模型与引擎一键下载。**

[![License](https://img.shields.io/github/license/timoncool/dub-studio?style=flat-square)](LICENSE)
[![Stars](https://img.shields.io/github/stars/timoncool/dub-studio?style=flat-square)](https://github.com/timoncool/dub-studio/stargazers)
[![Latest release](https://img.shields.io/github/v/release/timoncool/dub-studio?include_prereleases&style=flat-square)](https://github.com/timoncool/dub-studio/releases)
[![Downloads](https://img.shields.io/github/downloads/timoncool/dub-studio/total?style=flat-square)](https://github.com/timoncool/dub-studio/releases)

[English](README.md) · [Русский](README.ru.md) · **中文** · [Español](README.es.md) · [Português](README.pt.md) · [Français](README.fr.md)

### [🌐 在线演示与前后对比展示 →](https://timoncool.github.io/dub-studio/)



</div>

## 实际效果

**[▶ 在网站上观看前后对比视频展示 →](https://timoncool.github.io/dub-studio/#showcase)** — 真实片段，全部在本地 GPU 上端到端完成：不同的视频、模式和语言。

| ![dub](docs/shots/mode-dub-ru.png) | ![voiceover](docs/shots/mode-voiceover-es.png) | ![dub CJK](docs/shots/mode-dub-zh.png) |
|:--:|:--:|:--:|
| 🎙️ **配音** · EN→RU | 🗣️ **旁白配音** · EN→ES | 🈶 **配音** · 画面上的中文 |
| ![subtitles](docs/shots/mode-subtitles-ru.png) | ![widescreen](docs/shots/mode-dub-cinema-fr.png) | ![transcript](docs/shots/mode-transcribe-pt.png) |
| 📝 **字幕** · 原语言 | 🎬 **配音** · 宽屏 16:9 | 🔤 **转录** · 说话人分离 |

## 这是什么

**Dub Studio** 把任意视频变成另一种语言的配音版本 —— **克隆说话人本人的音色、翻译字幕、并在画面上就地本地化嵌入文字**。拖入一个片段，智能自动流程先出初稿；随后实时编辑器让你掌控**每一条字幕、声音、模糊框、字体和标题**，即时预览。

默认一切都在**你自己的电脑上本地运行** —— 无云端、无订阅：你的素材和声纹绝不离开电脑。如果电脑较弱（跑不动本地 Gemma/Higgs）或你想要更快更好的效果，繁重的部分（翻译、视觉、TTS、语音识别）可以**可选地**通过 **OpenRouter** 交给云端 —— 每个引擎单独选择（本地 ↔ 云端），并按说话人性别自动分配声音（测试版）。密钥保存在本地，默认全部关闭。

这是**完全原生重写**。没有内嵌 Python、没有 torch、没有 CUDA wheel。整条流水线是 **Rust + 原生 C++/CUDA 引擎（GGUF/ONNX）**：单进程、启动快、显存占用低。模型、引擎、CUDA/VC++ 运行库和 ffmpeg 都由应用在首次运行时**自行一键下载安装**。**推荐 NVIDIA，但并非必需** —— 分离有 CPU 版本，说话人分离与语音识别可在 CPU 上运行，繁重部分（翻译、视觉、TTS）交给云端，因此在完全没有 NVIDIA 的机器上也能完成配音。

## 五种模式，随时切换

| 模式 | 作用 |
|------|------|
| 🎙️ **配音** | 完整重新配音到目标语言，**克隆原始音色** —— 按说话人自动分配或自选声音 |
| 🗣️ **旁白（画外音）** | 翻译人声**叠加在减弱的原声之上** —— 原声仍在下方可听，平衡可调 |
| 📝 **字幕** | 烧录**原语言**字幕、保留原声 —— 不配音、不翻译 |
| ✨ **趣味改编** | 给个主题（“像海盗”“像新闻播报”）→ 模型**重写整个脚本**再配音 |
| 🎬 **转录** | 干净的**说话人分离转录**、逐说话人排布、卡拉OK跟随播放、一键生成声音、导出 `.srt`/`.txt` |

加载一次片段，即可在编辑器里送入任意模式。

## 功能

- **声音克隆** —— 克隆原始音色并说出新语言（原生 [Higgs Audio v3](https://huggingface.co/bosonai) 引擎，GGUF）。按说话人自动分配或使用自带声音包。
- **说话人分离** —— 谁在何时说话（NVIDIA **Sortformer** v2，最多 4 个声音），每个说话人不同声音。
- **角色选角（测试版）** —— 一个角色就是**「人脸 + 声音」的配对**。应用在整段视频中收集人脸、识别同一个人，并**按共同出现把他绑定到某个说话人**（近景出镜者获得声音，背景旁听者则否）；自动挑选最清晰的一帧作头像，并**为整部剧集保存选角档案** —— 声音和角色描述只需指定一次，**下一集自动套用**。**「真实人脸 / 卡通·动漫」**开关按内容切换人脸识别。
- **可选 ASR 引擎** —— 用 **Parakeet-TDT**（GPU，默认）或 **Whisper**（[Purfview faster-whisper 独立版](https://github.com/Purfview/whisper-standalone-win)，可在 CPU 上运行）转写 —— 在设置里直接选择模型大小（tiny … large-v3-turbo）和量化（compute type）。
- **导入现成字幕** —— 用你自己的 `.srt`/`.ass` 作为精确文稿：文本和时间轴直接取自文件，而非自动识别（说话人仍由声纹分离自动分配）。勾选 **“字幕已是目标语言”** 可连翻译一起跳过 —— 英文视频 + 你的俄语字幕 → 直接生成俄语配音，无需识别与翻译。
- **多语言导出** —— 导出按钮旁的 **▾** 可把一个视频一次导出为多种语言；每种都继承你的全部编辑（字幕排版、样式、模糊框、克隆音色）—— 只重新翻译与配音。
- **保存与重开项目** —— 自动保存、启动页的近期项目列表，一键回到未完成的工作。
- **语音与语言列表可搜索** —— 输入名称的一部分即可从数百个语音或 100+ 种语言中筛选；语言也可按界面语言中的名称匹配。
- **可组合流水线** —— 输入端独立开关：音频（原声 / 配音 / 旁白 / 转写）× 字幕（无 / 原文 / 翻译）× 是否烧录到视频 × 搞笑改写。任意组合 —— 配音但不加字幕、翻译字幕但不配音、用自己的声音做搞笑配音 —— 批量和编辑器中同样适用。
- **画面文字本地化** —— OCR 检测嵌入文字（**PP-OCR** ONNX），**模糊原文**并以匹配风格叠印本地化标题 —— 其他工具没有的功能。
- **翻译 + 视觉风格分析** —— 通过 **Gemma-4 12B**（GGUF，llama.cpp）本地翻译整段转录；视觉流程解析画面排布：字幕风格、标题、品牌、文字区域。
- **SOTA 人声分离** —— **Mel-Band Roformer**（CUDA 上的原生 BSRoformer.cpp）将人声与音乐分离：背景音乐**得以保留**，克隆锁定干净语音。
- **26 种字幕预设** —— karaoke / 逐词 / hormozi / 霓虹等，直接**在你的画面上**渲染（所见即所得，JASSUB 覆盖同一份 ffmpeg 烧录的 `.ass`）。
- **卡拉OK转录** —— 播放视频，转录中当前行与当前**词**同步高亮。
- **实时编辑器** —— 编辑转录、声音、字幕风格、模糊框、标题；**约 0.17 秒/帧预览**，每次修改即时可见。
- **智能重生成** —— 导出时只重新合成你改动过的片段，而非整段。
- **自定义台词** —— 在转录中插入自己的短语；每句都用说话人的克隆声音配音并显示在字幕中。
- **批量处理** —— 文件队列，统一设置，逐文件进度。
- **前后对比** —— 原片与配音并排。
- **100+ 种语言** —— 配音到任何主要语言（西班牙语、中文、日语、阿拉伯语、印地语等），自动检测源语言。
- **任意视频格式** —— MP4、MOV、MKV、WEBM、AVI 等（ffmpeg 解码）。
- **一键安装 + 应用内自动更新** —— 首次运行下载模型、引擎、运行库与 ffmpeg；应用自我更新。
- **可续传下载** —— 大模型（10GB+）断线后从中断处续传，而非重新开始。
- **想在哪算就在哪算** —— 分离、说话人分离和 ASR 各自在 **GPU、CPU 和云端**之间独立切换，随意组合。把繁重模型（翻译、视觉、TTS）交给 **OpenRouter** 后，整条流水线即使在**没有 NVIDIA** 的机器上也能运行。
- **按硬件调优** —— 每个引擎都有多种量化（TTS Q8/Q6/Q4、翻译 Q4…Q8、ASR int8/fp32 或 Whisper tiny…large-v3-turbo、分离 Q8/Q5/Q4），在设置中切换；可限制 prefill 批大小与参考片段时长，以适配 8–12 GB 显卡和 32 GB 内存。
- **完全便携** —— 不写入用户配置；删除文件夹不留痕迹。

## 截图

主界面 —— 五种模式、所选视频预览、语言选择、任意视频格式：

![Dub Studio 主界面](docs/screenshot-home.png)

转录模式 —— 说话人分离转录、逐说话人排布、卡拉OK跟随播放、一键从每个说话人生成声音：

![Dub Studio 转录模式](docs/screenshot-transcribe.png)

## 环境要求

- **系统：** Windows 10 / 11（x64）
- **显卡：** 推荐 NVIDIA 8–16 GB 显存 —— **或无显卡**：分离、说话人分离和 ASR 在 CPU 上运行，繁重模型交给云端
- **WebView2** —— Windows 11 预装（Windows 10 自动安装）
- **磁盘：** 约 15 GB 用于模型、引擎与运行库（首次运行下载），外加项目空间

在有 NVIDIA 的机器上，唯一需要手动安装的是较新的 **[NVIDIA 驱动](https://www.nvidia.com/Download/index.aspx)**。其余一切 —— 模型（Higgs Audio v3、Gemma-4 12B + vision、Parakeet-TDT、Sortformer、Mel-Band Roformer）、引擎、CUDA 运行库与 ffmpeg —— 应用在首次运行时一键下载。

## 快速开始

1. 从 [Releases](https://github.com/timoncool/dub-studio/releases) **下载**便携版并解压到任意文件夹（或用 `-setup.exe` / `.msi` 安装）。
2. **运行** `Dub Studio.exe`。
3. 在**首次运行**面板点击**全部下载** —— 应用获取模型、引擎与运行库（约 15 GB，一次）。若缺 NVIDIA 驱动，按钮会打开下载页。
4. **拖入视频**，选择目标语言 → 自动流程出初稿。在编辑器里微调后点击**导出**。

> 一切都下载并存放在**应用文件夹内**。模型、缓存与项目不会去别处。

## 工作原理

`analyze()` 是固定的第一遍：分离 → 带词级时间戳的 ASR → 说话人分离 → 上下文翻译 + 视觉（字幕风格 / 标题 / 品牌）→ OCR（排布 / 模糊框）。产出一个可编辑的 **Project** 文档。每次编辑都是对它的补丁，约 0.17 秒/帧预览；导出只重跑**被弄脏的阶段**。

**技术栈：** 原生 **Tauri 2（Rust）** 外壳在本地端口启动 `dub-server`（axum），并把窗口打开到 SPA —— React 19 + Vite + Tailwind + react-konva 覆盖 JASSUB。引擎：Parakeet-TDT 或 Whisper（ASR）· Sortformer（分离）· Gemma-4-12B GGUF（翻译 + 视觉，llama.cpp）· Higgs Audio v3（TTS）· Mel-Band Roformer（人声分离，BSRoformer.cpp）· PP-OCR（ONNX）· ffmpeg/NVENC。**运行时没有任何 Python 进程。**

### 从源码构建

```bash
git clone https://github.com/timoncool/dub-studio.git
cd dub-studio

cd frontend && npm install && npm run build && cd ..   # 1) SPA
cargo build --release -p dub-server                     # 2) 原生服务器 (axum)
cd desktop && npm install && npx tauri build            # 3) 桌面外壳 (Tauri)
```

需要 Node 20+、Rust（MSVC 工具链）与 WebView2。原生引擎无需重建 —— 应用会下载预编译二进制。

## 参与贡献与分支

**非常欢迎协作者。** 我会由衷高兴看到 Dub Studio 被移植到其他平台和显卡上 —— 架构完全支持，我只是没有精力亲自做这些移植。如果你想让它跑在 **AMD / Intel 显卡、macOS 或 Linux** 上，尽管 fork —— 欢迎 PR。

**额外的本地化**同样欢迎：目前应用和落地页支持 6 种语言 —— 翻译语言文件（`frontend/src/locales/` 及 `docs/index.html` 中的字典）并提交 PR 加入你的语言。

## 作者

- **Nerual Dreming** —— [Telegram](https://t.me/nerual_dreming) | [neuro-cartel.com](https://neuro-cartel.com) | [ArtGeneration.me](https://artgeneration.me) 创始人
- **Neuro-Soft** —— [Telegram](https://t.me/neuroport) | 便携 AI 应用

## 致谢

- **[Boson AI](https://huggingface.co/bosonai)** —— Higgs Audio v3 模型；**[drbaph / Higgs-Audio-v3-Studio](https://huggingface.co/drbaph/Higgs-Audio-v3-Studio)** —— GGUF 量化与原生 `audiocpp_engine.dll`。
- **[NVIDIA Parakeet](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3)** 与 **[Sortformer](https://huggingface.co/nvidia)** —— ASR 与说话人分离；ONNX 权重来自 [istupakov/parakeet-tdt-0.6b-v3-onnx](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx) 与 [altunenes/parakeet-rs](https://github.com/altunenes/parakeet-rs)。
- **[Google Gemma](https://huggingface.co/google/gemma-4-12b-it-qat-q4_0-gguf)** —— Gemma-4 12B（翻译 + 视觉），经 [llama.cpp](https://github.com/ggml-org/llama.cpp)。
- **[chenmozhijin / BSRoformer.cpp](https://github.com/chenmozhijin/BSRoformer.cpp)** 与 **[GaboxR67](https://huggingface.co/GaboxR67)** —— 原生引擎与 Mel-Band Roformer 模型。

## 支持作者

我做开源软件与 AI 研究，绝大部分成果都公开。捐助让我能做和研究更多。

**[所有支持方式](DONATE.md)** | **[dalink.to/nerual_dreming](https://dalink.to/nerual_dreming)** | **[boosty.to/neuro_art](https://boosty.to/neuro_art)**

- **BTC:** `1E7dHL22RpyhJGVpcvKdbyZgksSYkYeEBC`
- **ETH (ERC20):** `0xb5db65adf478983186d4897ba92fe2c25c594a0c`
- **USDT (TRC20):** `TQST9Lp2TjK6FiVkn4fwfGUee7NmkxEE7C`

## 许可证

应用代码采用 [MIT](LICENSE)。模型权重保留各自许可证（Higgs Audio v3 —— Boson AI 研究/非商业；Gemma —— Gemma Terms 等）—— 每次发布前审核。

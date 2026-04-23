# speak-it

`speak-it` 是一个面向 Linux X11 的 Rust CLI 工具，用来实现“按住热键说话输入”。

当前流程是：

- 按下 `F8` 后立即开始录音
- 松开 `F8` 后立即停止录音
- 将音频上传到 BigModel 语音转文本接口，模型固定为 `glm-asr-2512`
- 等说完后再进行整段转写
- 通过 `xdotool` 把最终识别结果一次性注入当前聚焦的 X11 窗口

当前实现不会再劫持空格键，正常输入空格不会受影响。

## 当前状态

这是一个可运行的首版实现，采用了偏务实的依赖方案：

- 仅支持 Linux
- 仅支持 X11
- 录音依赖外部工具：`ffmpeg` 或 `arecord`
- 文本注入依赖 `xdotool`
- API Key 从环境变量 `ZHIPUAI_API_KEY` 读取

暂不支持 Wayland。

## 命令

```bash
speak-it daemon
speak-it doctor
speak-it once <audio-file>
```

## 依赖要求

- Rust 工具链
- X11 桌面会话
- `xdotool`
- `ffmpeg` 或 `arecord`
- BigModel API Key，并写入 `ZHIPUAI_API_KEY`

Debian / Ubuntu 示例安装：

```bash
sudo apt install xdotool ffmpeg
```

如果你想使用 ALSA 录音工具：

```bash
sudo apt install xdotool alsa-utils
```

## 构建

```bash
cargo build --release
```

二进制输出路径：

```bash
target/release/speak-it
```

## 环境准备

先设置 API Key：

```bash
export ZHIPUAI_API_KEY="your_api_key"
```

运行环境检查：

```bash
cargo run -- doctor
```

`doctor` 会检查：

- `ZHIPUAI_API_KEY`
- `DISPLAY`
- 是否处于 X11 会话
- `xdotool`
- 是否存在可用录音工具

## 使用方式

### 1. 用现有音频文件调试转写

```bash
cargo run -- once ./sample.wav
```

这个命令会调用：

- `POST https://open.bigmodel.cn/api/paas/v4/audio/transcriptions`
- model: `glm-asr-2512`
- `stream=false`

输入文件格式以 BigModel 接口支持范围为准。当前实现优先按 `.wav` 场景设计。

### 2. 启动常驻监听

```bash
cargo run -- daemon
```

然后：

1. 把焦点切到任意 X11 文本输入区域
2. 按住 `F8`
3. 开始说话
4. 松开 `F8`
5. 等待整段文本在转写完成后注入到当前应用

## 当前行为说明

- 热键固定为 `F8`
- 模型固定为 `glm-asr-2512`
- 录音结果会写入当前工作目录下的 `speak-it-<uuid>.wav`
- 当前目录最多保留最近 10 次录音，更早的会自动删除
- 不再显示流式预览
- 转写完成后一次性注入最终文本

## 已知限制

- 不支持 Wayland
- 还没有配置文件
- 还不能自定义热键
- 还没有取消录音或静音流程
- 对特殊键盘布局没有做充分兼容
- 当前热键固定为 `F8`

## 开发

格式化和测试：

```bash
cargo fmt
cargo test
```

## 手工验证建议

建议至少在这些场景下测试：

- 终端输入框
- 浏览器输入框
- 编辑器文本区域

建议覆盖这些异常路径：

- 缺少 `ZHIPUAI_API_KEY`
- 缺少 `xdotool`
- `ffmpeg` 和 `arecord` 都不存在
- 非 X11 会话
- 空白语音
- 网络失败
- API 鉴权失败

## 实现说明

主要文件：

- [src/main.rs](./src/main.rs)
- [src/lib.rs](./src/lib.rs)

当前实现使用：

- `x11rb` 做 X11 热键监听
- `reqwest` 调用 BigModel API
- `tokio` 提供异步运行时
- 当前目录保存录音文件，文件名使用随机 UUID

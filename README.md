# spout-transparent

`spout-transparent` 是一个 Windows 透明桌面窗口，用来显示 Spout 的 Direct3D 11 共享纹理。窗口默认鼠标穿透，也可以随时切换为可拖动、可缩放的交互模式。

接收端完全由 Rust 实现，直接使用 Win32 共享内存和 Direct3D 11 API；运行时不需要 `SpoutLibrary.dll`，构建时也不需要 CMake、LLVM/libclang 或 C++ Spout SDK。

## 功能

- 自动连接当前排序后的第一个有效 Spout Sender
- Sender 重启、切换或共享纹理重建后自动重新连接
- 支持普通共享纹理和 `IDXGIKeyedMutex` 纹理同步
- DirectComposition 透明无边框窗口
- 默认鼠标穿透；交互模式下透明像素仍可把点击传递给下层窗口
- 鼠标拖动、滚轮缩放、窗口置顶和帧率限制
- 可通过 JSON 自定义全局快捷键

## 系统要求

- Windows 10/11 x64
- 支持 Direct3D 11 的显卡与驱动
- 一个输出 DX11 共享纹理的 Spout Sender

程序当前只实现本项目所需的 DX11 共享纹理接收路径，不包含 Spout Sender、OpenGL 互操作、CPU sharing 或 DirectX 9 兼容接口。

## 快速开始

1. 启动 Spout Sender，并确认它正在输出纹理。
2. 运行 `spout-transparent.exe`。
3. 程序会连接名称排序后的第一个有效 Sender，并创建与其纹理尺寸一致的透明窗口。

程序启动时最多等待 Sender 5 秒。若没有可用 Sender，程序会退出；请先启动 Sender 后再重试。运行期间 Sender 暂时关闭时，接收端会继续轮询并在 Sender 恢复后重连。

## 默认快捷键

| 快捷键 | 功能 |
| --- | --- |
| `Ctrl+Shift+M` | 切换鼠标穿透/交互模式 |
| `Ctrl+Shift+F` | 循环切换帧率：Unlimited → 120 → 60 → 30 |
| `Ctrl+Shift+T` | 切换窗口置顶 |
| `Ctrl+Shift+Q` | 退出程序 |
| `Esc` | 窗口获得键盘焦点时退出 |

交互模式开启后：

- 按住鼠标左键拖动窗口。
- 使用滚轮缩放窗口，范围为原始尺寸的 10%～500%。
- 透明区域的按键与滚轮事件会继续传递给下层窗口。

## 自定义快捷键

在当前工作目录或可执行文件目录创建 `hotkeys.json`。若两处都存在，优先读取当前工作目录中的文件。

```json
{
  "hotkeys": [
    {
      "modifiers": ["CTRL", "SHIFT"],
      "key": "M",
      "action": "toggle_interaction"
    },
    {
      "modifiers": ["CTRL", "ALT"],
      "key": "Q",
      "action": "quit"
    }
  ]
}
```

配置规则：

- `modifiers` 支持 `CTRL`、`CONTROL`、`SHIFT` 和 `ALT`。
- `key` 必须是单个 ASCII 字母或数字。
- `action` 可取 `toggle_interaction`、`cycle_framerate`、`toggle_topmost` 或 `quit`。
- 自定义配置会整体替换默认快捷键；格式错误或快捷键被其他程序占用时，程序会报告错误并退出。

## 从源码构建

安装以下环境：

- Rust stable，目标为 `x86_64-pc-windows-msvc`
- Visual Studio 2022 Build Tools（MSVC 链接器和 Windows 10/11 SDK）

在项目根目录执行：

```powershell
cargo build --release --locked
```

产物位于：

```text
target\release\spout-transparent.exe
```

可执行文件可单独复制运行，无需附带项目文件或额外 Spout DLL。

## 开发与测试

```powershell
cargo fmt --all -- --check
cargo test --locked
cargo clippy --all-targets --locked --no-deps -- -D warnings
cargo build --release --locked
```

Debug 构建会打开日志控制台。需要更详细的日志时：

```powershell
$env:RUST_LOG = 'debug'
cargo run --locked
```

采集 Sender 纹理和合成 BackBuffer 的诊断位图：

```powershell
$env:SPOUT_DEBUG_CAPTURE = '1'
cargo run --locked
```

诊断位图写入可执行文件所在目录，并已由 `.gitignore` 排除。

## 常见问题

### 启动后立即退出

先确认 Sender 已启动并使用 DX11 共享纹理。程序只等待 5 秒；Debug 构建的控制台会显示具体错误。

### 找到 Sender，但无法打开共享纹理

确保 Sender 与接收端运行在同一 Windows 会话，并尽量让两者使用同一块 GPU。多显卡系统中，共享纹理通常不能跨图形适配器直接打开。

### 全局快捷键注册失败

对应组合键可能已被其他程序占用。关闭冲突程序，或通过 `hotkeys.json` 更换组合键。

### 窗口无法拖动或缩放

窗口默认处于鼠标穿透状态。先按 `Ctrl+Shift+M` 开启交互模式。

## 第三方声明

纯 Rust 接收端与 Spout2 的公开共享内存和纹理同步协议兼容。相关版权与许可证见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

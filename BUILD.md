# spout-transparent 构建与运行

## 环境要求

- Windows 10/11 x64
- Rust stable（建议通过 `rustup` 安装）
- Visual Studio 2022，并安装“使用 C++ 的桌面开发”工作负载
- CMake
- LLVM/libclang（`autocxx 0.26` 建议使用 LLVM 18；LLVM 22 的 AST 变化可能导致绑定生成失败）

项目已经包含 Spout2 SDK 源码和运行所需的 `SpoutLibrary.dll`。

## 构建

在项目根目录打开 PowerShell：

```powershell
$env:SPOUT2_DIR = (Resolve-Path .\Spout2).Path
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
cargo build --locked
```

Debug 产物位于 `target\debug\spout-transparent.exe`。Release 构建：

```powershell
$env:SPOUT2_DIR = (Resolve-Path .\Spout2).Path
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
cargo build --release --locked
```

Release 产物位于 `target\release\spout-transparent.exe`。程序启动时会在可执行文件目录中确保存在 `SpoutLibrary.dll`。

## 运行

先启动一个 Spout Sender，然后执行：

```powershell
.\target\debug\spout-transparent.exe
```

Debug 构建默认不会执行昂贵的 GPU 回读或写入诊断位图。需要采集 Sender 与 BackBuffer 时可显式开启：

```powershell
$env:SPOUT_DEBUG_CAPTURE = '1'
cargo run --locked
```

## 质量检查

```powershell
cargo fmt --all -- --check
$env:SPOUT2_DIR = (Resolve-Path .\Spout2).Path
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
```

## 快捷键

| 快捷键 | 功能 |
| --- | --- |
| `Ctrl+Shift+M` | 切换交互模式（拖拽、缩放或鼠标穿透） |
| `Ctrl+Shift+F` | 切换帧率档位（Unlimited → 120 → 60 → 30） |
| `Ctrl+Shift+T` | 切换窗口置顶 |
| `Ctrl+Shift+Q` | 退出程序 |
| `Esc` | 退出程序 |

可在工作目录或可执行文件目录放置 `hotkeys.json` 来覆盖默认全局快捷键。键值必须是单个 ASCII 字母或数字；支持的修饰键为 `CTRL`、`SHIFT` 和 `ALT`。全局快捷键已启用防重复触发。

示例：

```json
{
  "hotkeys": [
    {
      "modifiers": ["CTRL", "SHIFT"],
      "key": "M",
      "action": "toggle_interaction"
    }
  ]
}
```

可用 action：`toggle_interaction`、`cycle_framerate`、`toggle_topmost`、`quit`。

## LLVM 22 兼容性

当前锁定的 `rust-spout2 0.1.3` / `autocxx 0.26.0` 在 LLVM 22 下可能把 Win32 `POINT` 解析成不兼容的匿名数组，从而导致代码生成失败。请优先切换到 LLVM 18，并将 `LIBCLANG_PATH` 指向对应的 `bin` 目录；不要直接修改 Cargo registry 中的 crate 源码，因为清理缓存或换机后这些修改会丢失。

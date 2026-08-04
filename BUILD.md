# spout-transparent 构建与运行

## 环境要求

- Windows 10/11 x64
- Rust stable（MSVC 工具链）
- Visual Studio Build Tools 2022 或 Visual Studio 2022，并安装 Windows 10/11 SDK

接收端现已由 Rust 直接调用 Win32 共享内存和 Direct3D 11 API。构建与运行均不再需要 CMake、LLVM/libclang、`autocxx` 或 `SpoutLibrary.dll`。仓库中的 `Spout2` 目录仅保留为协议实现参考及许可证来源，不参与编译。

## 构建

在项目根目录打开 PowerShell：

```powershell
cargo build --locked
```

Release 构建：

```powershell
cargo build --release --locked
```

产物分别位于 `target\debug\spout-transparent.exe` 和 `target\release\spout-transparent.exe`，无需在可执行文件旁部署额外 DLL。

## 运行

先启动一个使用 Direct3D 11 共享纹理的 Spout Sender，然后执行：

```powershell
.\target\debug\spout-transparent.exe
```

接收端会读取 `SpoutSenderNames` 共享内存，连接排序后的第一个有效 Sender，并直接用共享句柄调用 `ID3D11Device::OpenSharedResource`。Sender 重启、句柄改变或切换后会自动重新发现并重建纹理资源。

当前纯 Rust 接收路径面向本程序实际使用的 DX11 共享纹理模式，不实现 Spout 的 OpenGL 互操作、发送端、CPU 共享和 DirectX 9 兼容路径。

Debug 构建默认不会执行昂贵的 GPU 回读或写入诊断位图。需要采集 Sender 与 BackBuffer 时可显式开启：

```powershell
$env:SPOUT_DEBUG_CAPTURE = '1'
cargo run --locked
```

## 质量检查

```powershell
cargo fmt --all -- --check
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

可在工作目录或可执行文件目录放置 `hotkeys.json` 覆盖默认全局快捷键。键值必须是单个 ASCII 字母或数字；支持 `CTRL`、`SHIFT` 和 `ALT` 修饰键。

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

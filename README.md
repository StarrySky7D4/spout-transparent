# spout-transparent

`spout-transparent` 是一个 Windows 透明桌面窗口，用来显示 Spout 的 Direct3D 11 共享纹理。窗口默认鼠标穿透，也可以随时切换为可拖动、可缩放的交互模式。

接收端完全由 Rust 实现，直接使用 Win32 共享内存和 Direct3D 11 API；运行时不需要 `SpoutLibrary.dll`，构建时也不需要 CMake、LLVM/libclang 或 C++ Spout SDK。

项目结构、资源开销模型和优化验证记录见 [`PERFORMANCE.md`](PERFORMANCE.md)。
当前模块职责、状态不变量和修改入口见 [`ARCHITECTURE.md`](ARCHITECTURE.md)。

## 功能

- 自动连接当前排序后的第一个有效 Spout Sender，也可从托盘手动切换
- Sender 重启、切换或共享纹理重建后自动重新连接
- 支持普通共享纹理和 `IDXGIKeyedMutex` 纹理同步
- DirectComposition 透明无边框窗口
- 自动裁切透明边缘，缩小窗体范围；支持稳定收缩与托盘开关
- 托盘常驻，不显示任务栏图标；右键可快速调整运行设置
- 默认鼠标穿透；交互模式下透明像素仍可把点击传递给下层窗口
- 鼠标拖动、滚轮缩放、窗口置顶和帧率限制
- 可通过 JSON 自定义全局快捷键

## 系统要求

- Windows 10/11 x64
- 支持 Direct3D 11 的显卡与驱动
- 一个输出 DX11 共享纹理的 Spout Sender

程序当前只实现本项目所需的 DX11 共享纹理接收路径，不包含 Spout Sender、OpenGL 互操作、CPU sharing 或 DirectX 9 兼容接口。

## 快速开始

1. 运行 `spout-transparent.exe`。
2. 启动 Spout Sender，并确认它正在输出纹理。
3. 程序会连接名称排序后的第一个有效 Sender，先创建与纹理尺寸一致的透明窗口，再按可见内容自动收窄范围。

没有可用 Sender 时，程序不会退出，而是隐藏窗口并常驻系统托盘。Sender 出现后会自动连接并显示窗口；运行期间 Sender 暂时关闭时，窗口会再次隐藏并等待重连。

程序不会显示任务栏图标。需要隐藏、恢复或退出窗口时，请使用系统托盘中的 `Spout Transparent` 图标。

## 托盘菜单

右键单击托盘图标可使用以下选项：

- 显示或隐藏透明窗口
- 从当前有效 Sender 列表中切换来源
- 开关交互模式
- 开关窗口置顶
- 开关“自动裁切透明边缘”（默认开启）
- 将帧率设为不限制、120 FPS、60 FPS 或 30 FPS
- 退出程序

双击托盘图标也可以快速显示或隐藏窗口。窗口隐藏后会暂停纹理渲染，并降低后台轮询频率。
托盘菜单在独立线程中跟踪，右键菜单保持打开时不会阻塞纹理接收和画面呈现。

托盘、快捷键和鼠标穿透通过事件通知主循环，连续切换操作按顺序处理。
无来源时主窗口等待事件，后台线程继续发现 Sender。来源查询、重连、Alpha 回读、裁切布局和绘制
分别调度，窗口隐藏后取消显示与回读任务，保留来源监控。

## 稳定自动裁切

自动裁切按非零 Alpha 像素的外接矩形确定范围，保留 8 个源像素的边距，以保留淡阴影和抗锯齿边缘。
采样发现内容超出当前范围时扩大窗口；较小边界稳定 500 ms 后才收缩，拖动期间暂停调整。
窗口的位置与合成偏移同时补偿，裁切本身保持内容在桌面上的位置，不会把图像拉伸到新窗体大小。
鼠标命中检测使用裁切偏移和实际缩放，从原始 Alpha 掩码采样。

来源切换会重新检测；完全透明的帧保留上次范围并继续接收，避免空白过渡时反复缩成小窗。
关闭托盘中的“自动裁切透明边缘”可恢复完整画布。

边界随 Alpha 回读以约 10 Hz 更新，实际频率受来源帧率、限帧和 GPU 回读影响。快速出现于裁切范围外的内容
可能在下一次有效采样前短暂被裁掉；对这种动画可关闭自动裁切。
当前实现缩小的是窗体和合成显示范围，交换链保持源分辨率。自动裁切也需要 Alpha 回读，因此关闭交互模式
不会单独释放这部分资源；同时关闭裁切和交互时才停止 Alpha 提取。

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

在支持 D3D11 的 Windows 环境中，可额外运行像素级渲染、共享纹理、Alpha 会话与事件顺序验证：

```powershell
cargo test --locked -- --ignored
```

一次运行全部测试，包括这些平台测试：

```powershell
cargo test --locked -- --include-ignored
```

运行独立 Release 性能测量（CPU 边界扫描、GPU 时间戳、Alpha 回读和事件递送）：

```powershell
cargo bench --bench performance --locked --target-dir target/performance-build
```

结果默认写入 `target/performance`；可通过 `SPOUT_PERF_OUTPUT` 指定结果目录。
测试使用合成纹理，不会注册新 Sender 或修改正在运行的接收端。
本机四轮结果与范围说明见 [性能测试记录](Knowledge/2026-10-11-performance-results.md)。

采集 Sender 纹理和合成 BackBuffer 的诊断位图：

```powershell
$env:SPOUT_DEBUG_CAPTURE = '1'
cargo run --locked
```

诊断位图写入可执行文件所在目录，并已由 `.gitignore` 排除。

## 常见问题

### 没有 Sender 时看不到窗口

这是预期行为。程序会隐藏窗口并留在系统托盘中等待；启动 DX11 Spout Sender 后会自动连接，也可以右键托盘图标，从“来源”菜单中手动选择。

### 找到 Sender，但无法打开共享纹理

确保 Sender 与接收端运行在同一 Windows 会话，并尽量让两者使用同一块 GPU。多显卡系统中，共享纹理通常不能跨图形适配器直接打开。

### 全局快捷键注册失败

对应组合键可能已被其他程序占用。关闭冲突程序，或通过 `hotkeys.json` 更换组合键。

### 窗口无法拖动或缩放

窗口默认处于鼠标穿透状态。先按 `Ctrl+Shift+M` 开启交互模式。

### 任务栏中找不到程序

这是预期行为。程序作为托盘应用运行，请在系统托盘或其折叠区域中查找 `Spout Transparent` 图标；右键可显示窗口或退出程序。

## 许可证

本项目原创代码采用 [Apache License 2.0](LICENSE) 许可。第三方组件和兼容性实现仍遵循各自的许可证及归属要求。

## 第三方声明

纯 Rust 接收端与 Spout2 的公开共享内存和纹理同步协议兼容。相关版权与许可证见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

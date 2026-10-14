# spout-transparent-app

桌面应用包，使用同一 workspace 中的 `spout-transparent` 库。
负责透明窗口、任务栏隐藏、托盘菜单、热键、鼠标转发和应用事件协调。
生成的可执行文件名仍是 `spout-transparent.exe`。

## 构建与运行

在仓库根目录运行，要求 Windows 10/11 x64、Rust MSVC 工具链和 Windows SDK：

```powershell
cargo build -p spout-transparent-app --release --locked
cargo run -p spout-transparent-app --locked
```

Release 产物位于 `target/release/spout-transparent.exe`。Debug 应用会分配日志控制台；
Release 使用 GUI 子系统。启动第二个实例可能因全局热键冲突而失败，更新程序时先从托盘退出旧实例。

没有 Sender 时隐藏显示窗口并等待发现；发现有效来源后连接并显示。隐藏窗口暂停绘制和回读，
已连接来源的元数据监控继续运行。托盘菜单在自己的线程中打开，菜单操作通过事件通知 UI。

## 任务栏与交互

创建窗口时通过 winit 配置跳过任务栏与无重定向位图；窗口策略在样式更新前持续保留
`WS_EX_TOOLWINDOW` 并移除 `WS_EX_APPWINDOW`。工具窗口策略早于 HWND 释放。
Explorer 的任务栏重建通知由 winit 的跳过任务栏状态处理；托盘图标由托盘线程恢复。

默认鼠标穿透。交互开启后可拖动和滚轮缩放，透明像素按 Alpha 掩码继续转发给下层窗口。
自动裁切默认开启，保留 8 个源像素边距，较小范围稳定 500 ms 后收缩；拖动时暂停布局调整。
完整功能、快捷键和菜单说明见 [项目 README](../README.md)。

## 配置与诊断

`hotkeys.json` 优先从进程当前工作目录读取，其次从可执行文件目录读取。
未找到时使用内置配置。可执行文件目录是部署后稳定的配置位置。
错误 JSON 或被占用的热键会导致启动报错；配置格式见 [项目 README](../README.md)。

```powershell
$env:RUST_LOG = 'debug'
cargo run -p spout-transparent-app --locked
```

Debug 应用支持 `SPOUT_DEBUG_CAPTURE=1`，将 Sender 和 BackBuffer 诊断位图写到程序目录。
库自身不设置日志后端，应用在入口初始化 env_logger。

## 验证和性能基准

```powershell
cargo test -p spout-transparent-app --locked -- --include-ignored
cargo clippy --workspace --all-targets --locked --no-deps -- -D warnings
cargo bench -p spout-transparent-app --bench performance --locked --no-run
```

应用包含 20 项单元测试和 1 项独立窗口集成测试；2026-10-15 均显式运行通过。
GPU 接收和 Alpha 测试在库包中，应使用 `cargo test --workspace --locked -- --include-ignored`
完成整个项目的验证。独立窗口测试保持单独进程，避免 winit 的事件循环单例冲突。

移除基准命令中的 `--no-run` 可实际测量。默认输出在仓库根目录的 `target/performance`；
自定义 `SPOUT_PERF_OUTPUT` 时建议使用绝对路径。运行中的文件被 Windows 锁定时，使用
`--target-dir target/performance-build` 构建到独立目录。
测量范围和历史结果见 [PERFORMANCE.md](../PERFORMANCE.md)。

## 模块与 SDK

应用模块位于 `src/app/`、`src/events.rs`、`src/interaction/`、`src/tray.rs` 与 `src/tray/`。
基准通过 Cargo 依赖调用真实库，不嵌入库源码。复用接收与渲染能力时应依赖库包，
而不是复制应用内部模块；入口见 [SDK 指南](../lib/README.md)。

分发时同时保留仓库的 [LICENSE](../LICENSE)、[NOTICE](../NOTICE) 与
[THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md)。

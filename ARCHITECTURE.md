# 模块契约与维护指南

本文描述当前代码结构。历史审计和性能记录保留其测量时的文件名与结论；当前模块以本文为准。

## 从哪里开始阅读

仓库根目录是 Cargo workspace：`lib/` 是库包 `spout-transparent`，`bin/` 是应用包
`spout-transparent-app`。应用生成的文件名仍为 `spout-transparent.exe`。
第三方集成从 [库指南](lib/README.md) 和 `lib/src/lib.rs` 的公开 API 开始。
查看应用流程时，从 `bin/src/events.rs` 进入 `bin/src/app/runtime.rs`，再追踪它对库的调用。协议细节与原生资源清理由相应子模块负责。

| 文件或目录 | 负责的事情 | 主要边界 |
| --- | --- | --- |
| `bin/src/main.rs` | 日志、入口、最终错误退出 | 调用 `app::run` |
| `bin/src/app.rs` | winit 生命周期适配 | 只向 Runtime 分派通知和窗口事件 |
| `bin/src/app/runtime.rs` | 操作处理、显示状态、来源安装、独立任务协调 | GPU 资源由 Renderer 所有；合成更新交给 DCompResources |
| `bin/src/app/platform.rs` | 窗口属性、任务栏隐藏策略与 UI 线程 COM 生命周期 | 样式策略早于窗口释放；COM guard 晚于 COM 对象释放 |
| `bin/src/app/diagnostics.rs` | Debug 窗口样式日志 | 与库内的 GPU 位图诊断分开 |
| `bin/src/events.rs` | 托盘、热键、发现和鼠标通知 | 操作使用事件代理，连续操作保留顺序 |
| `lib/src/source.rs` | 元数据轮询与按资源身份约束的重连重试 | 不创建 GPU 资源 |
| `lib/src/spout/protocol.rs` | Sender 名称、280 字节元数据和资源身份 | 对象名保留原始字节；有损 UTF-8 仅用于显示 |
| `lib/src/spout/receiver.rs` | 来源选择、资源代次和轮询期限 | 同一共享句柄被重用也需要检查完整身份 |
| `lib/src/spout/discovery.rs` | 后台发现、变化快照和通知 | 查询结果相同不会唤醒 UI |
| `lib/src/spout/frame_counter.rs` | 非阻塞新帧探测与旧 Sender 兼容回退 | 零帧计数不能冻结接收端 |
| `lib/src/spout/shared_memory.rs` | 映射、句柄、命名互斥量的所有权 | 构造失败和正常退出都走 Drop 清理 |
| `lib/src/renderer.rs` | 已安装资源、Draw/Present、Alpha 回读和裁切状态 | D3D11 immediate context 的唯一生产访问入口 |
| `lib/src/renderer/source_texture.rs` | 打开共享纹理、SRV 与同步资源 | 返回完整资源组或错误 |
| `lib/src/frame_pacer.rs` | 限帧与重试退避 | 帧预算从绘制开始计时 |
| `lib/src/scheduler.rs` | 每种维护任务的独立期限 | 消费期限后必须显式安排下一次任务 |
| `lib/src/crop.rs` | Alpha 外接范围、稳定收缩和坐标换算 | 源像素与窗口物理像素的偏移分别处理 |
| `lib/src/dx/` | 设备、管线、交换链、合成、纹理锁与回读原语 | 原生 API 细节集中在此层 |
| `bin/src/interaction/mod.rs` | 交互状态、拖动和缩放 | 掩码交给鼠标钩子线程使用 |
| `bin/src/interaction/hotkeys.rs` | 纯配置解析、注册与热键生命周期 | 先完整验证配置，再调用注册 API |
| `bin/src/interaction/hook_thread.rs` | 透明命中与下层窗口事件转发 | 掩码邮箱只保留最新样本，并回收旧缓冲 |
| `bin/src/tray.rs` / `bin/src/tray/menu.rs` | 托盘线程与菜单构建 | 菜单来源命令按打开时的快照解释 |
| `lib/src/error.rs` | 子系统上下文与错误原因链 | 原生错误保留类型，便于定位 HRESULT / OS 错误 |
| `lib/src/diagnostics.rs` | Debug 日志和可选位图诊断 | 不编入 Release 应用 |

较大的原生渲染测试单独放在 `bin/src/app/tests.rs` 和 `lib/src/renderer/tests.rs`。
纯逻辑测试仍与实现靠近，避免阅读日常流程时穿过大量 GPU 测试装配代码。

## 线程与调度流程

应用 UI 线程运行 Runtime、SourceController 和 Renderer。库中的发现线程接受框架无关的通知回调；
桌面应用将回调适配为 `EventLoopProxy<AppEvent>`。托盘与鼠标钩子线程也使用此代理通知 UI。它们不共享 immediate context，也不调用生产 Draw/Map/Present。

`AboutToWait` 依次消费来源、Alpha 资源配置、回读、布局和帧尝试期限。每个阶段使用新的时间戳，
因为打开纹理或驱动调用可能令后续期限到期。帧任务只排队一次 `request_redraw`；真正绘制发生在
`RedrawRequested`，随后由 FramePacer 安排下一次尝试。

隐藏窗口会取消显示相关任务并丢弃待处理掩码，来源监控继续运行。没有可用来源时 UI 等待事件；
后台发现线程保持自己的节奏。拖动期间暂停 Alpha 提取和布局调整，释放鼠标后再安排布局。

## 必须维持的不变量

1. **资源安装完整。** `Renderer::installed` 是一个 `Option<PreparedSource>`，将身份、纹理、
   交换链和 RTV 作为整体替换。先 prepare，再安装合成内容，成功后才 install；失败时保留旧资源组。
2. **身份完整匹配。** 名称、共享句柄和 generation 一起决定当前来源是否对应已安装资源。
   重连重试期限只约束失败的身份，不延迟新代次的来源。
3. **请求帧不能丢失。** 纹理锁忙或来源计数不变时，明确请求的帧仍然保留；仅成功 Present 后清除。
4. **裁切提交有顺序。** DComp 缩放、偏移和窗口位置/尺寸更新成功后，才调用 `commit_viewport`。
   原生更新失败时回退合成参数，并恢复交互缩放状态。
5. **坐标约定一致。** DComp 的视觉偏移发生在缩放之前，需要除以 scale；桌面位置补偿和命中检测
   都使用同一份量化后的 CropLayout 偏移，避免不同模块各自取整。
6. **完成回读不等于发布掩码。** `AlphaReadResult::Completed` 可以没有 mask，例如交互已关闭或
   样本属于旧 epoch。裁切仍可使用样本；鼠标掩码仅发布给匹配的当前会话。
7. **资源生命周期覆盖错误路径。** 原生句柄、映射视图、热键和菜单都有所有者。子菜单只在
   AppendMenuW 成功后转交父菜单。Runtime 字段释放顺序保持线程、GPU/合成、窗口、COM guard 的约束。

## 如何调整参数

| 参数 | 所在位置 | 当前值与含义 |
| --- | --- | --- |
| 内容边距 / 稳定收缩 | `lib/src/crop.rs` | 8 源像素 / 500 ms |
| Alpha 提取周期 | `lib/src/renderer.rs` | 100 ms，实际受新帧、限帧和 GPU 就绪影响 |
| 回读 / 配置失败 / 布局失败重试 | `bin/src/app/runtime.rs` | 2 / 500 / 100 ms |
| 来源重连重试 | `lib/src/source.rs` | 500 ms，仅当前失败身份 |
| 有连接 / 断开元数据轮询 | `lib/src/spout/receiver.rs` | 100 / 500 ms |
| 来源发现周期 | `lib/src/spout/discovery.rs` | 500 ms，变化时才通知 |
| 锁忙 / 无新帧重试 | `lib/src/frame_pacer.rs` | 锁忙 1 ms；无新帧按 1、2、4、8 ms 退避 |
| 鼠标透明阈值 | `bin/src/interaction/mod.rs` | Alpha 10，与裁切的非零 Alpha 判定分开 |

修改频率时分别考虑响应速度、CPU 唤醒、GPU 提取和内存带宽。缩小窗体不改变源分辨率交换链；
同时关闭交互和裁切才会释放 Alpha 资源。

## 修改与验证

新增用户操作时，从 `AppAction` 和 Runtime 分派入口扩展。新增维护任务时，同时更新 Scheduler
任务槽位、Runtime 的期限消费顺序，以及隐藏/失联时的取消规则。修改 GPU 或 Alpha 流程时，
保留锁忙重试、新旧会话隔离和完整像素覆盖测试。

```powershell
cargo fmt --all -- --check
cargo test --workspace --locked -- --include-ignored
cargo clippy --workspace --all-targets --locked --no-deps -- -D warnings
cargo clippy --workspace --release --all-targets --locked --no-deps --target-dir target/performance-build -- -D warnings
cargo build --workspace --release --locked --target-dir target/performance-build
cargo bench -p spout-transparent-app --bench performance --locked --target-dir target/performance-build --no-run
```

独立基准位于 `bin/benches/`，通过包依赖链接实际库，不再复制生产模块或使用整文件警告豁免。
公开 API 集成测试位于 `lib/tests/`，可运行示例位于 `lib/examples/`。
窗口回归测试 `bin/tests/taskbar_visibility.rs` 单独编译私有窗口策略以隔离 winit 的单例事件循环。
`--no-run` 验证基准能构建；需要测量时移除此参数。单独的 target 目录可以避免覆盖 Windows 已锁定的
运行中程序。事件循环测试和基准保持独立进程，以满足 winit 的单事件循环限制。

本轮重构保留现有线程与采样、退避算法。同步释放与 GPU 提交时序、实际呈现 FPS 和完整点击延迟，
仍需要专门的端到端测量；历史性能数据不能直接作为本轮重构的速度提升证明。

## 第三方集成的边界

库依赖只包含 log、thiserror、windows 和 windows-sys；窗口框架、日志后端和 JSON 热键配置只属于应用。
来源发现使用回调而非 AppEvent，合成位置使用物理像素元组而非 winit 类型，避免公开 API 绑定单一 UI 框架。
库不设置全局日志、不调用进程退出，也不自动注册应用窗口或全局热键。

添加可复用功能时放入 `lib/`，并通过 `lib/tests/` 或 rustdoc 示例验证外部可达性；应用交互放在 `bin/`。
不要恢复跨包的源码路径引用来访问库私有实现。包内的原生句柄和 COM 生命周期仍需遵守对应接口契约。

本地验证 SDK 打包：

```powershell
cargo doc -p spout-transparent --no-deps --locked
cargo package -p spout-transparent --allow-dirty --locked --offline
```

`cargo package` 只在本地生成并校验 crate，不会发布。共享依赖版本和锁文件由 workspace 根目录维护。
历史性能与审计记录保留重构前的路径；结构调整本身不作为性能提升的测量证明。

## 当前验证快照（2026-10-15）

库单元测试 34 项、库公开 API 集成测试 2 项、应用单元测试 20 项、独立任务栏测试 1 项，
全部显式运行后共 57 项通过；另有 3 个 rustdoc 示例编译通过。
Debug/Release 的 workspace 全目标严格 Clippy、Release 程序、独立基准和 SDK 打包校验通过。
外部依赖版本未升级。历史测量原始文件没有改写；测试明细与范围见 [PERFORMANCE.md](PERFORMANCE.md)。

原文件对应关系：根目录 `src/` 中的接收、渲染、裁切与调度进入 `lib/src/`，应用事件、窗口、
鼠标热键与托盘进入 `bin/src/`。`config.rs` 的职责已由 `lib/src/frame_pacer.rs` 承担。
原根目录 `benches/` 进入 `bin/benches/`，原窗口集成测试进入 `bin/tests/`。
重新克隆后直接使用 workspace 根目录命令，不需要旧的源码路径。

# 模块契约与维护指南

本文描述当前代码结构。历史审计和性能记录保留其测量时的文件名与结论；当前模块以本文为准。

## 从哪里开始阅读

先读 `events.rs` 中的操作与通知，再沿 `app.rs → app/runtime.rs → source.rs / renderer.rs`
查看事件如何改变状态、安排维护任务和触发绘制。协议细节与原生资源清理由相应子模块负责。

| 文件或目录 | 负责的事情 | 主要边界 |
| --- | --- | --- |
| `src/main.rs` | 日志、入口、最终错误退出 | 调用 `app::run` |
| `src/app.rs` | winit 生命周期适配 | 只向 Runtime 分派通知和窗口事件 |
| `src/app/runtime.rs` | 操作处理、显示状态、来源安装、独立任务协调 | GPU 资源由 Renderer 所有；合成更新交给 DCompResources |
| `src/app/platform.rs` | UI 线程 COM apartment 生命周期 | 其 guard 必须晚于所有 COM 对象释放 |
| `src/events.rs` | 托盘、热键、发现和鼠标通知 | 操作使用事件代理，连续操作保留顺序 |
| `src/source.rs` | 元数据轮询与按资源身份约束的重连重试 | 不创建 GPU 资源 |
| `src/spout/protocol.rs` | Sender 名称、280 字节元数据和资源身份 | 对象名保留原始字节；有损 UTF-8 仅用于显示 |
| `src/spout/receiver.rs` | 来源选择、资源代次和轮询期限 | 同一共享句柄被重用也需要检查完整身份 |
| `src/spout/discovery.rs` | 后台发现、变化快照和通知 | 查询结果相同不会唤醒 UI |
| `src/spout/frame_counter.rs` | 非阻塞新帧探测与旧 Sender 兼容回退 | 零帧计数不能冻结接收端 |
| `src/spout/shared_memory.rs` | 映射、句柄、命名互斥量的所有权 | 构造失败和正常退出都走 Drop 清理 |
| `src/renderer.rs` | 已安装资源、Draw/Present、Alpha 回读和裁切状态 | D3D11 immediate context 的唯一生产访问入口 |
| `src/renderer/source_texture.rs` | 打开共享纹理、SRV 与同步资源 | 返回完整资源组或错误 |
| `src/frame_pacer.rs` | 限帧与重试退避 | 帧预算从绘制开始计时 |
| `src/scheduler.rs` | 每种维护任务的独立期限 | 消费期限后必须显式安排下一次任务 |
| `src/crop.rs` | Alpha 外接范围、稳定收缩和坐标换算 | 源像素与窗口物理像素的偏移分别处理 |
| `src/dx/` | 设备、管线、交换链、合成、纹理锁与回读原语 | 原生 API 细节集中在此层 |
| `src/interaction/mod.rs` | 交互状态、拖动和缩放 | 掩码交给鼠标钩子线程使用 |
| `src/interaction/hotkeys.rs` | 纯配置解析、注册与热键生命周期 | 先完整验证配置，再调用注册 API |
| `src/interaction/hook_thread.rs` | 透明命中与下层窗口事件转发 | 掩码邮箱只保留最新样本，并回收旧缓冲 |
| `src/tray.rs` / `src/tray/menu.rs` | 托盘线程与菜单构建 | 菜单来源命令按打开时的快照解释 |
| `src/error.rs` | 子系统上下文与错误原因链 | 原生错误保留类型，便于定位 HRESULT / OS 错误 |
| `src/diagnostics.rs` | Debug 日志和可选位图诊断 | 不编入 Release 应用 |

较大的原生渲染测试单独放在 `src/app/tests.rs` 和 `src/renderer/tests.rs`。
纯逻辑测试仍与实现靠近，避免阅读日常流程时穿过大量 GPU 测试装配代码。

## 线程与调度流程

UI 线程运行 Runtime、SourceController 和 Renderer。发现线程、托盘线程和鼠标钩子线程通过
`EventLoopProxy<AppEvent>` 通知 UI。它们不共享 immediate context，也不调用生产 Draw/Map/Present。

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
| 内容边距 / 稳定收缩 | `crop.rs` | 8 源像素 / 500 ms |
| Alpha 提取周期 | `renderer.rs` | 100 ms，实际受新帧、限帧和 GPU 就绪影响 |
| 回读 / 配置失败 / 布局失败重试 | `app/runtime.rs` | 2 / 500 / 100 ms |
| 来源重连重试 | `source.rs` | 500 ms，仅当前失败身份 |
| 有连接 / 断开元数据轮询 | `spout/receiver.rs` | 100 / 500 ms |
| 来源发现周期 | `spout/discovery.rs` | 500 ms，变化时才通知 |
| 锁忙 / 无新帧重试 | `frame_pacer.rs` | 锁忙 1 ms；无新帧按 1、2、4、8 ms 退避 |
| 鼠标透明阈值 | `interaction/mod.rs` | Alpha 10，与裁切的非零 Alpha 判定分开 |

修改频率时分别考虑响应速度、CPU 唤醒、GPU 提取和内存带宽。缩小窗体不改变源分辨率交换链；
同时关闭交互和裁切才会释放 Alpha 资源。

## 修改与验证

新增用户操作时，从 `AppAction` 和 Runtime 分派入口扩展。新增维护任务时，同时更新 Scheduler
任务槽位、Runtime 的期限消费顺序，以及隐藏/失联时的取消规则。修改 GPU 或 Alpha 流程时，
保留锁忙重试、新旧会话隔离和完整像素覆盖测试。

```powershell
cargo fmt --all -- --check
cargo test --locked -- --include-ignored
cargo clippy --all-targets --locked --no-deps -- -D warnings
cargo clippy --release --all-targets --locked --no-deps --target-dir target/performance-build -- -D warnings
cargo build --release --locked --target-dir target/performance-build
cargo bench --bench performance --locked --target-dir target/performance-build --no-run
```

独立基准通过路径引用生产模块，源文件移动时也需要检查 `benches/performance.rs`。
`--no-run` 验证基准能构建；需要测量时移除此参数。单独的 target 目录可以避免覆盖 Windows 已锁定的
运行中程序。事件循环测试和基准保持独立进程，以满足 winit 的单事件循环限制。

本轮重构保留现有线程与采样、退避算法。同步释放与 GPU 提交时序、实际呈现 FPS 和完整点击延迟，
仍需要专门的端到端测量；历史性能数据不能直接作为本轮重构的速度提升证明。

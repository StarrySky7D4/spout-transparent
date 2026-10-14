# C / C++ SDK（Windows x64，ABI 1）

通过 C ABI 复用 Rust 库的 Spout DX11 接收、DirectComposition 渲染、Alpha 回读和稳定裁切能力。
接口声明和完整参数契约见 [spout_transparent.h](include/spout_transparent.h)，可运行的来源枚举示例见
[list_senders.c](examples/list_senders.c)。使用 DLL 的宿主不需要 Rust 或 SpoutLibrary.dll。

## 在源码仓库中构建

需要 Rust stable 的 `x86_64-pc-windows-msvc` 工具链及 MSVC / Windows SDK。在仓库根目录执行：

```powershell
cargo build -p spout-transparent-capi --release --locked
& ./lib/c-api/build-sdk.ps1
```

生成 `target/c-api-sdk/release/`：

```text
bin/spout_transparent_c.dll
lib/spout_transparent_c.dll.lib
include/spout_transparent.h
examples/list_senders.c
examples/CMakeLists.txt
README.md, LICENSE, NOTICE, THIRD_PARTY_NOTICES.md
```

DLL 放在宿主可执行文件旁；MSVC 链接 `.dll.lib`，包含头文件即可。当前提供动态库，没有静态库产物。
`build-sdk.ps1 -Configuration Debug` 可生成 Debug SDK。普通 `cargo build` 的默认成员仍是 Rust 库与桌面应用；
`cargo build --workspace` 才同时构建 C ABI 包。C ABI 包不发布到 crates.io。

## 从生成的 SDK 编译示例

在 x64 MSVC 开发者终端中，进入生成的 SDK 根目录。以下命令需要 CMake 3.20 或更新版本；
CMake 仅用于这个可选的 C 示例，不是构建 Rust 库或 DLL 的前置条件。

```powershell
cmake -S examples -B example-build -G "NMake Makefiles" -DSPT_SDK_ROOT="$PWD"
cmake --build example-build
& ./example-build/spout_c_example.exe
```

示例枚举当前来源并读取元数据；没有来源时正常退出。名字保留 Spout 原始字节，不承诺 UTF-8。
头文件同时支持 C11 和 C++；入口是 `extern "C"` / `__cdecl`，ABI 主版本可通过 `spt_abi_version()` 检查。

## 接口与所有权

| 接口组 | 用途 |
| --- | --- |
| `spt_sender_list_*` / `spt_sender_info_read` | 不可变来源快照、指定名字的元数据查询 |
| `spt_receiver_*` | 自动/手动选择、连接轮询、来源身份与下次期限 |
| `spt_renderer_*` | 绑定宿主 HWND、安装来源、绘制、限帧、Alpha 与布局 |
| `spt_visible_bounds` / `spt_crop_*` | 紧密 R8 Alpha 检测、保留 8 像素边距、稳定 500 ms 收缩 |
| `spt_last_*` / `spt_release` | 当前线程诊断、统一释放句柄 |

`spt_handle` 是不透明的 64 位编号；创建、使用、释放必须在同一 OS 线程。
失效、类型错误及跨线程句柄返回 `SPT_INVALID_HANDLE`，`spt_release(0)` 成功。
在线程退出和卸载 DLL 前显式释放所有句柄；不要从 DllMain 调用 API。

宿主提供并释放所有输入/输出缓冲区，DLL 不向宿主转交 Rust 分配的内存。指针必须有效、正确对齐，
实际内存长度足够，各输入与输出区域不得重叠。字符串输出所需长度包含末尾 NUL，输入名字长度不包含 NUL。
`spt_sender_info.struct_size` 必须先设置为 `sizeof(spt_sender_info)`；当前结构为 40 字节。
更大的结构只写入已知前缀，未知尾部保持不变。头文件会独立保持结构对齐，不受宿主 `pack(1)` 影响。

字符串与 Alpha 使用相同的容量探测方式：传 `NULL, 0`，得到 `SPT_BUFFER_TOO_SMALL` 和所需容量，
再用宿主缓冲区重试。Alpha 探测还返回宽高，样本保留到成功复制、关闭 Alpha 或重新安装来源；
容量不足不会部分写入缓冲区。返回 `SPT_WOULD_BLOCK` 表示尚无可用 Alpha 样本。

普通 API 成功时清除当前线程错误；失败后立即查询 `spt_last_error_code()`、`spt_last_native_error()`
和 `spt_last_error_message()`，查询及版本函数不会覆盖错误。消息是借用的 UTF-8 指针，需在同线程
下一次普通 API 前复制。可恢复的 Rust unwind 转为 `SPT_PANIC`；非法内存和进程中止不在恢复范围内。

## 宿主渲染与调度流程

1. 宿主初始化 COM，在同线程创建适合 DirectComposition 的无边框 HWND（含 `WS_EX_NOREDIRECTIONBITMAP`），
   并根据自身需求设置透明、穿透、任务栏等窗口样式。调用 `spt_receiver_create`、`spt_renderer_create`。
2. 自动选源时先 `spt_receiver_refresh`；指定名字时用 `spt_receiver_select`。调用 `spt_receiver_poll`
   检查连接，断开是 `SPT_OK` 且 `connected=0`。`spt_receiver_info` 返回来源属性和 generation。
3. 来源连接或身份变化后调用 `spt_renderer_attach`。比较完整名字、共享句柄和 generation；
   绘制拒绝已失效的来源资源，宿主可在查询到新身份后重试安装。
4. 到帧期限时调用 `spt_renderer_render`，根据 `spt_renderer_next_frame_ms` 安排下一次尝试。
   返回状态与帧结果分开：成功调用也可能是无新帧、纹理锁忙或尚未到期。
5. 如需鼠标命中或自动裁切，显式 `spt_renderer_set_alpha(renderer, 1)`。
   绘制后用 `spt_renderer_has_readback` 检查是否有待处理槽位或已保留样本，仅此时约 2 ms 重试
   `spt_renderer_read_alpha`，完成后交给宿主的命中检测或 `spt_crop_observe`。
   裁切时间为该句柄从零开始的单调毫秒数；收缩延迟到期可只调用 `spt_crop_rect`，无需重新提交样本。
   用 `spt_renderer_set_layout` 提交源像素裁切和 0.1～5.0 倍缩放，窗口位置补偿保持画布桌面锚点。
6. 停止时先释放 renderer / receiver / crop，再销毁 HWND，最后反初始化 COM。

C 宿主显式控制 Alpha、裁切和事件循环；新 renderer 默认关闭 Alpha 和自动布局调整。
`spt_renderer_layout` 返回提交后的裁切、缩放、量化偏移和窗体物理尺寸，用于保持鼠标坐标映射一致。
更换来源应重建对应尺寸的 crop 状态，清理宿主缓存的旧掩码。
DLL 不创建宿主窗口、托盘、快捷键和全局日志，也不转发鼠标输入。

发现查询是同步操作，注册表互斥量每次等待上限 67 ms，建议约 500 ms 刷新。
接收轮询内部遵守连接时 100 ms、断开时 500 ms 的元数据期限，可查询 `spt_receiver_next_poll_ms`；
显式选择来源会使查询立即到期。所有句柄保留在创建线程，UI 宿主也可把整个接收/渲染流程放入其自有线程。
帧等待按 0/30/60/120 FPS 与锁忙/无新帧退避计算；Present 可能等待 vsync，因此这些函数不是硬实时接口。
Alpha 提取约 10 Hz，受新帧和 GPU 就绪影响；显式 request_frame 会强制尝试新采样。
查询返回毫秒期限向上取整，`SPT_NO_DEADLINE` 表示无需安排该任务。

## 原生测试（源码仓库）

以下脚本需在 x64 MSVC 开发者终端、仓库根目录执行：

```powershell
& ./lib/c-api/tests/run.ps1
& ./lib/c-api/tests/run.ps1 -Configuration Debug
cargo test --workspace --locked -- --include-ignored
```

每个 profile 都编译并运行真正链接 DLL 的 C11 和 C++17 客户端，开启 `/W4 /WX`。
覆盖 ABI 结构布局、错误/线程隔离、句柄失效、缓冲区探测、元数据锁忙、来源代次、稳定裁切，
以及 16×16 共享 keyed texture、完整 Alpha 回读、绘制锁忙重试、原生 HWND 裁切与位置补偿。
夹具使用进程独有名字且不写入全局 Sender 名单。`-SkipGpu` 可只运行非图形部分，不能替代完整 GPU 验证。

2026-10-15：Debug / Release 的 C 与 C++ GPU 客户端均通过；workspace 共 59 项 Rust 测试通过，
另有 3 个 rustdoc 示例。这里验证功能与边界，没有新增真实 Sender 的端到端性能测量。

分发时保留 [LICENSE](https://github.com/StarrySky7D4/spout-transparent/blob/main/LICENSE)、
[NOTICE](https://github.com/StarrySky7D4/spout-transparent/blob/main/NOTICE) 和
[THIRD_PARTY_NOTICES.md](https://github.com/StarrySky7D4/spout-transparent/blob/main/THIRD_PARTY_NOTICES.md)。
源码仓库中的这些文件位于仓库根目录；SDK 构建脚本将它们复制到生成的 SDK 根目录。

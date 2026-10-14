# spout-transparent

可复用的 Windows Spout DX11 接收库。提供来源发现、共享纹理接收、渲染与 Alpha 回读、稳定裁切和调度原语。
支持 Windows 10/11、D3D11；不包含 OpenGL、DX9 或 CPU sharing 接收路径。

库不依赖 winit、serde 或 env_logger，不创建应用窗口、注册全局热键或初始化全局日志。
窗口、事件循环和生命周期由宿主程序控制；可在自己的 UI 框架中集成。

C / C++ 宿主可使用独立的
[C ABI DLL 与头文件](https://github.com/StarrySky7D4/spout-transparent/blob/main/lib/c-api/README.md)。
该封装位于仓库 `lib/c-api/`，有独立构建脚本与原生客户端测试，不增加本 Rust 库的直接依赖。

## 引入

在第三方项目的 `Cargo.toml` 添加本地路径依赖（指向仓库中的 `lib/`）：

```toml
[dependencies]
spout-transparent = { path = "../spout-transparent/lib" }
```

包名是 `spout-transparent`，Rust 导入名是 `spout_transparent`。当前尚未发布到 crates.io。
本次 workspace 拆分推送后，也可直接使用 Git 依赖：

```toml
[dependencies]
spout-transparent = { git = "https://github.com/StarrySky7D4/spout-transparent.git", branch = "main" }
```

需要固定源码版本时，用已验证提交的 `rev` 替换 `branch`。Cargo 会从 workspace 中选择库包，
不需要把应用包作为依赖。

库重导出同版本的 `windows` crate，宿主可使用 `spout_transparent::windows` 避免 COM 类型版本不匹配。

## 同步查询来源

```rust,no_run
use spout_transparent::{discover_sender_names, read_sender_info, SenderName};

fn main() -> std::io::Result<()> {
    for name in discover_sender_names()? {
        // 0 表示不等待繁忙的元数据互斥量。
        match read_sender_info(&name, 0) {
            Ok(info) => println!("{name}: {}x{}", info.width, info.height),
            Err(error) => eprintln!("{name}: {error}"),
        }
    }
    let selected: SenderName = "VTubeStudioSpout".parse()?;
    assert_eq!(selected.as_bytes(), b"VTubeStudioSpout");
    Ok(())
}
```

名称构造支持 1–255 个非 NUL 字节；`from_bytes` 保留非 UTF-8 原始名称。
`display_name` 和 Display 用于显示，不能拿有损 UTF-8 结果重新打开原生对象。

`discover_sender_names` 最多等待 67 ms 获取名称表锁；UI 线程应使用后台发现。
`read_sender_info` 使用毫秒超时；锁忙返回 `WouldBlock`，映射不存在返回 `NotFound`。

## 接入自己的事件循环

```rust,no_run
use spout_transparent::SenderDiscovery;
use std::sync::mpsc;

fn main() -> std::io::Result<()> {
    let (notify, events) = mpsc::channel();
    let discovery = SenderDiscovery::start(move || notify.send(()).is_ok())?;
    events.recv().expect("discovery notification");
    if let Some(snapshot) = discovery.take_latest() {
        println!("{:?}", snapshot?);
    }
    drop(discovery); // 停止并 join 发现线程。
    Ok(())
}
```

通知回调在发现线程执行，快照先存入邮箱，再调用回调。回调应快速返回；返回 `false` 表示宿主已经断开，
结束发现线程。也可以在回调中唤醒自己的事件循环。邮箱只保留最新快照，连续变化可能合并，
不能把它当作每个中间状态都会到达的事件日志。

`SpoutReceiver` 保存元数据和资源 generation；`SourceController` 额外提供按资源身份约束的重连重试。
收到发现快照后调用 `update_discovered`，按 `next_poll` 或 `next_wake` 安排维护。
`poll` 返回连接状态，`sender_info` 借用缓存的元数据，不会再次访问共享内存。

## 在已有 HWND 中接入渲染

下面展示资源准备和安装。调用者应先在此线程初始化 COM，并确保 HWND、COM apartment、
返回的 Renderer 与 DCompResources 按正确顺序释放。

```rust,no_run
use spout_transparent::{CropLayout, CropRect, Renderer, SenderIdentity};
use spout_transparent::dx::{composition::{setup_dcomp, DCompResources}, device::create_dx11_device_auto};
use spout_transparent::windows::{core::Interface, Win32::Foundation::HWND};

fn attach(
    hwnd: HWND,
    desktop_position: (i32, i32),
    identity: SenderIdentity,
) -> Result<(Renderer, DCompResources), Box<dyn std::error::Error>> {
    let (device, context) = create_dx11_device_auto()?;
    let composition = setup_dcomp(&device.cast()?, hwnd)?;
    let mut renderer = Renderer::new(device, context)?;
    let candidate = renderer.prepare(identity)?;
    let (width, height) = candidate.size();
    let layout = CropLayout::new(CropRect::full(width, height), 1.0);
    composition.install_content(candidate.swapchain(), renderer.swapchain(), hwnd,
        desktop_position, renderer.layout(), layout)?;
    renderer.install(candidate);
    // 默认启用自动裁切；与交互模式共享同一套 Alpha 资源。
    renderer.configure_alpha(false)?;
    Ok((renderer, composition))
}
```

宿主窗口需支持 DirectComposition：使用无重定向位图的透明窗口，并正确配置输入与窗口样式。
库中的合成操作可能修改调用者指定的 HWND，但不会替调用者建立托盘、热键或窗口事件循环。

集成时应维持以下顺序和约定：

1. 保持 Sender 与接收设备在兼容的 GPU/会话中。`prepare` 成功后先安装合成内容，再 `install`。
2. 使用完整 SenderIdentity 检查资源代次，失联时暂停使用旧资源；句柄相同不代表纹理仍是同一代。
3. 在同一提交线程调用 Draw、Map 和 Present；不与 Renderer 并发使用同一个 immediate context。
   `render` 内的 Present 可能等待垂直同步，事件宿主需要考虑这一点。
4. 使用 `frame_due` / `next_frame` 安排绘制，锁忙和来源无新帧是正常状态，不应使用紧密重试循环。
5. `configure_alpha` 在交互或裁切需要时配置回读。`has_readback` 为真时按宿主期限调用 `read_alpha`，
   `NotReady` 表示等待下一次尝试。epoch 不匹配时不会发布旧鼠标掩码，但仍可更新裁切。
6. 用 `target_crop` / `crop_deadline` 安排布局；原生布局成功后才 `commit_viewport`。
   显示和鼠标命中应共用同一个 CropLayout。`set_auto_crop(false)` 可显式关闭裁切。

## 示例和开发

从仓库根目录运行：

```powershell
cargo run -p spout-transparent --example list_senders --locked
cargo run -p spout-transparent --example watch_senders --locked
cargo test -p spout-transparent --locked
cargo doc -p spout-transparent --no-deps --locked
```

普通测试不需要 Sender；GPU 和窗口相关测试默认忽略，在有 Windows 桌面和 D3D11 的环境中使用
`cargo test --workspace --locked -- --include-ignored` 全量验证。

只运行库的 34 项单元测试与 2 项公开 API 测试：

```powershell
cargo test -p spout-transparent --locked -- --include-ignored
```

2026-10-15 验证：上述库测试及 3 个 rustdoc 示例通过；独立 `.crate` 包已在本地编译校验。
这是本地 SDK 产物，尚未发布到 crates.io。docs.rs 元数据指定 Windows MSVC 文档目标。

原创代码使用 Apache-2.0。独立库包保留 `LICENSE`、`NOTICE` 和 `THIRD_PARTY_NOTICES.md`；
Spout 协议兼容性实现的归属与许可见
[第三方声明](https://github.com/StarrySky7D4/spout-transparent/blob/main/lib/THIRD_PARTY_NOTICES.md)。

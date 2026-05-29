# spout-transparent 构建与运行说明

## 前置条件

- Rust 工具链（当前使用 `rustc 1.95.0`）
- Visual Studio 2022 (含 C++ 桌面开发工作负载)
- CMake（已在 VS2022 中附带）
- 系统已安装 LLVM（本例为 LLVM 22.1.0，位于 `C:\Program Files\LLVM`）

## 环境变量

构建前必须设置 `SPOUT2_DIR` 指向 Spout2 SDK 源码目录：

```powershell
$env:SPOUT2_DIR = "$PWD\Spout2"
```

或直接使用相对路径：

```powershell
$env:SPOUT2_DIR = ".\Spout2"
```

## 构建

在项目根目录执行：

```powershell
$env:SPOUT2_DIR = ".\Spout2"
cargo build
```

构建产物：`.\target\debug\spout-transparent.exe`

## 运行

```powershell
.\target\debug\spout-transparent.exe
```

或直接双击可执行文件。

> **注意**：运行前需确保同目录下存在 `SpoutLibrary.dll`（构建过程会自动拷贝至 `.\target\debug\`）。

## Release 构建

```powershell
cargo build --release
```

产物位于 `.\target\release\spout-transparent.exe`

## Clippy 检查

```powershell
cargo clippy
```

---

## 快捷键

| 快捷键 | 功能 |
|--------|------|
| `Ctrl+Shift+M` | 切换交互模式（拖拽/缩放/穿透） |
| `Ctrl+Shift+F` | 切换帧率档位（Unlimited → 120 → 60 → 30 → …） |
| `Ctrl+Shift+T` | 切换窗口置顶 |
| `Ctrl+Shift+Q` | 退出程序 |
| `Esc` | 退出程序 |

可通过项目根目录下的 `hotkeys.json` 自定义快捷键。

---

## LLVM 22 兼容性补丁说明

LLVM 22.1.0 的 `libclang` 与 `autocxx` 0.26 存在兼容性问题：`POINT (tagPOINT)` 在 AST 中被表示为匿名数组 `[u32; 2]` 而非命名结构体，导致 autoccxx 代码生成失败。

已对注册表中的以下 crate 源码进行了补丁（无需修改系统 LLVM）：

### 1. `autocxx-engine` 0.26.0 (`type_to_cpp.rs`)

`Type::Array` 原行为直接报错 `UnsupportedType`，改为输出 `std::array<T,N>` C++ 类型。

文件路径：
```
%USERPROFILE%\.cargo\registry\src\index.crates.io-*\autocxx-engine-0.26.0\src\conversion\codegen_cpp\type_to_cpp.rs
```

### 2. `rust-spout2` 0.1.3 (`build.rs`)

在 autocxx 代码生成后追加后处理步骤：
- 为 `autocxxgen_ffi.h` 添加 `#include <windows.h>`（提供 `POINT` 定义）
- 将 `SpoutMessageBoxPosition` 包装器的 `arg1` 参数加上 `reinterpret_cast<POINT*>` 强制转换

文件路径：
```
%USERPROFILE%\.cargo\registry\src\index.crates.io-*\rust-spout2-0.1.3\build.rs
```

### 重新构建注意事项

- 若清除 Cargo 缓存（`cargo clean` 或删除 `target\`），上述补丁仍然有效（修改的是 registry 源码）
- 若更新了 `Cargo.lock` 或执行了 `cargo update`，需重新确认 `type_to_cpp.rs` 补丁
- `build.rs` 原始备份位于同目录下的 `build.rs.bak`

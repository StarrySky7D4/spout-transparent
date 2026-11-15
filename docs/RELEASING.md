# Windows 预构建与发布

工作流：.github/workflows/windows-prebuilt.yml。

## 日常 CI

main 的普通推送与指向 main 的 Pull Request 会运行 Windows x64 CI：

1. Rust 格式检查、默认 workspace 测试、严格 Clippy。
2. 使用 Cargo.lock 构建 Release 应用和 C ABI DLL。
3. 用 MSVC 编译、运行 C11 / C++17 原生客户端，跳过 GPU 路径。
4. 打包 C SDK，编译并运行 SDK 自带的 CMake 查询示例。
5. 检查最终 EXE / DLL 的导入依赖，生成两个 ZIP、SHA-256 和构建来源记录。
6. 将分发文件保留为当前 CI run 的 artifact，保存 30 天。

普通代码推送不会创建 Release。PR 没有发布权限。
GPU / 桌面相关的默认忽略测试保留为手动验证，CI 不将这些测试记为通过。
产物未作代码签名。

## 请求一次预发布

修改 .github/release.json 中的 tag 并提交到 main，例如从 v0.1.0-pre.1 改为
v0.1.0-pre.2。tag 的主版本必须对应 Cargo workspace 版本。

修改版本清单的这次 push 会先执行完整 CI，通过后：

- 仅从同一次 run 下载已验证的分发文件。
- 再次核对 SHA-256 与源码 commit。
- 检查版本是否已存在；不会移动已有 tag、覆盖已有 Release 或替换其附件。
- 用确切源码 commit 建立 Release 草稿并上传附件，然后发布为 prerelease。

GitHub Actions 页面也可以在 main 上手动运行工作流，发布当前版本清单。
其他分支的手动运行只构建和验证。

## 重试

CI 在创建 Release 前失败：修复问题后，可以在 main 手动运行工作流。
也可以将 release.json 的 request 整数加 1 与修复一起提交，以明确重试同一版本。
request 只负责标识一次新的发布请求，不影响二进制版本。

若已创建草稿、已有同名 tag 或已经发布，工作流会拒绝覆盖。
应先检查现有版本状态，再选择新的 tag；不要靠重复运行覆盖正式产物。

## 下载内容

- spout-transparent-vX.Y.Z-pre.N-windows-x64.zip：桌面程序。
- spout-transparent-sdk-vX.Y.Z-pre.N-windows-x64.zip：C/C++ SDK。
- SHA256SUMS.txt：两个 ZIP 与构建记录的 SHA-256。
- build-info.json：源码、工具链、Cargo.lock、CI 来源与验证边界。

PowerShell 可以使用 Get-FileHash -Algorithm SHA256 检查 ZIP；Linux/macOS
可以在附件目录运行 sha256sum --check SHA256SUMS.txt。

如最终产物导入 Visual C++ 运行库，发布说明会给出微软官方 x64 安装地址。
无需分发 SpoutLibrary.dll。应用与 SDK 的包内均保留项目与第三方许可证。

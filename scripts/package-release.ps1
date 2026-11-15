param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^v[0-9]+\.[0-9]+\.[0-9]+-pre\.[0-9]+$')]
    [string]$Tag
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$taskWorkspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$taskDist = Join-Path $taskWorkspace 'dist'
$taskStage = Join-Path $taskWorkspace 'target/release-package'
$taskAppName = "spout-transparent-$Tag-windows-x64"
$taskSdkName = "spout-transparent-sdk-$Tag-windows-x64"
$taskApp = Join-Path $taskStage $taskAppName
$taskSdk = Join-Path $taskStage $taskSdkName

Push-Location $taskWorkspace
try {
    foreach ($taskRequired in @(
        'target/release/spout-transparent.exe',
        'target/release/spout_transparent_c.dll',
        'target/c-api-sdk/release/bin/spout_transparent_c.dll',
        'target/c-api-sdk/release/lib/spout_transparent_c.dll.lib',
        'target/c-api-sdk/release/include/spout_transparent.h',
        'target/runtime-dependencies.txt'
    )) {
        if (-not (Test-Path -LiteralPath $taskRequired -PathType Leaf)) {
            throw "Missing required build output: $taskRequired"
        }
    }
    foreach ($taskDirectory in @($taskDist, $taskStage)) {
        if (Test-Path -LiteralPath $taskDirectory) {
            Remove-Item -LiteralPath $taskDirectory -Recurse -Force
        }
        New-Item -ItemType Directory -Path $taskDirectory -Force | Out-Null
    }
    New-Item -ItemType Directory -Path $taskApp -Force | Out-Null
    New-Item -ItemType Directory -Path $taskSdk -Force | Out-Null

    $taskCommit = (& git rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve the source commit.' }
    $taskRust = @(& rustc -Vv)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read the Rust compiler version.' }
    $taskCargo = (& cargo -V).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read the Cargo version.' }
    $taskMetadataJson = & cargo metadata --no-deps --format-version 1 --locked
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read workspace metadata.' }
    $taskMetadata = $taskMetadataJson | ConvertFrom-Json
    $taskVersion = ($taskMetadata.packages | Where-Object { $_.name -eq 'spout-transparent-app' }).version
    if (-not $Tag.StartsWith("v$taskVersion-pre.")) {
        throw 'The release tag must match the Cargo workspace version.'
    }

    $taskDependencies = Get-Content -LiteralPath target/runtime-dependencies.txt -Raw
    $taskVcRuntime = @(
        [regex]::Matches($taskDependencies, '(?im)^\s*((?:VCRUNTIME|MSVCP|CONCRT)[a-z0-9_.-]*\.dll)\s*$') |
            ForEach-Object { $_.Groups[1].Value } |
            Sort-Object -Unique
    )
    $taskRunUrl = if ($env:GITHUB_RUN_ID) {
        "https://github.com/$env:GITHUB_REPOSITORY/actions/runs/$env:GITHUB_RUN_ID"
    } else { $null }
    $taskBuildInfo = [ordered]@{
        release_tag = $Tag
        workspace_version = $taskVersion
        commit = $taskCommit
        target = 'x86_64-pc-windows-msvc'
        profile = 'release'
        built_at_utc = [DateTime]::UtcNow.ToString('o')
        rustc = $taskRust
        cargo = $taskCargo
        cargo_lock_sha256 = (Get-FileHash -LiteralPath Cargo.lock -Algorithm SHA256).Hash.ToLowerInvariant()
        ci_run_url = $taskRunUrl
        ci_run_attempt = $env:GITHUB_RUN_ATTEMPT
        visual_cpp_runtime_dlls = $taskVcRuntime
        validation = @(
            'cargo fmt --all -- --check',
            'cargo test --workspace --locked (default ignored platform tests excluded)',
            'cargo clippy --workspace --all-targets --locked --no-deps -- -D warnings',
            'cargo build --workspace --release --locked',
            'C11 and C++17 Release DLL client tests with -SkipGpu',
            'Packaged C SDK example built and executed'
        )
        not_validated_in_ci = @(
            'Real Spout Sender interoperability and GPU shared textures',
            'Interactive desktop, transparency, taskbar behavior and global hotkeys'
        )
    }
    $taskInfoPath = Join-Path $taskDist 'build-info.json'
    $taskBuildInfo | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $taskInfoPath -Encoding utf8NoBOM

    Copy-Item -LiteralPath target/release/spout-transparent.exe -Destination $taskApp
    foreach ($taskFile in @('README.md', 'LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.md')) {
        Copy-Item -LiteralPath $taskFile -Destination $taskApp
    }
    Copy-Item -LiteralPath bin/README.md -Destination (Join-Path $taskApp 'APPLICATION.md')
    Copy-Item -Path target/c-api-sdk/release/* -Destination $taskSdk -Recurse
    foreach ($taskDirectory in @($taskApp, $taskSdk)) {
        Copy-Item -LiteralPath $taskInfoPath -Destination $taskDirectory
        Copy-Item -LiteralPath target/runtime-dependencies.txt -Destination $taskDirectory
    }

    $taskRuntimeNote = if ($taskVcRuntime.Count -gt 0) {
        '此构建依赖 Microsoft Visual C++ x64 运行库。若启动时提示缺少 ' +
            ($taskVcRuntime -join ', ') +
            '，请安装微软官方 x64 Redistributable：https://aka.ms/vs/17/release/vc_redist.x64.exe'
    } else {
        '依赖检查未发现额外的 VCRUNTIME、MSVCP 或 CONCRT DLL 导入；仍需要 Windows 自带的系统图形组件。'
    }
    $taskQuickStart = @"
spout-transparent $Tag / Windows x64

1. 将整个压缩包解压到可写目录，运行 spout-transparent.exe。
2. 启动一个输出 DX11 共享纹理的 Spout Sender。
3. 没有 Sender 时程序会隐藏窗口，并在系统托盘中等待。
4. Ctrl+Shift+M 切换鼠标穿透/交互；交互模式下可拖动并用滚轮缩放。
5. 右键托盘图标可选择来源、设置帧率、置顶、裁切和退出。

要求：Windows 10/11 x64，支持 Direct3D 11 的显卡和驱动。
Sender 与接收端应位于同一 Windows 会话，并尽量使用同一块 GPU。
不需要 SpoutLibrary.dll。hotkeys.json 是可选配置，未随包预设。
$taskRuntimeNote

源码与完整文档：https://github.com/StarrySky7D4/spout-transparent
源码提交：$taskCommit
CI：$taskRunUrl

本版为 CI 预构建测试版。真实 Sender、GPU 共享纹理与桌面交互需要在实际 Windows 设备验证。
"@
    $taskQuickStart | Set-Content -LiteralPath (Join-Path $taskApp 'QUICKSTART.txt') -Encoding utf8NoBOM

    foreach ($taskArchive in @(
        @{ Directory = $taskApp; Name = $taskAppName; Required = @('spout-transparent.exe', 'LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.md', 'build-info.json', 'QUICKSTART.txt') },
        @{ Directory = $taskSdk; Name = $taskSdkName; Required = @('bin/spout_transparent_c.dll', 'lib/spout_transparent_c.dll.lib', 'include/spout_transparent.h', 'examples/list_senders.c', 'examples/CMakeLists.txt', 'README.md', 'LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.md', 'build-info.json') }
    )) {
        $taskZipPath = Join-Path $taskDist ($taskArchive.Name + '.zip')
        Compress-Archive -LiteralPath $taskArchive.Directory -DestinationPath $taskZipPath -CompressionLevel Optimal
        $taskZip = [System.IO.Compression.ZipFile]::OpenRead($taskZipPath)
        try {
            foreach ($taskRequired in $taskArchive.Required) {
                $taskEntryName = $taskArchive.Name + '/' + $taskRequired
                $taskEntry = $taskZip.GetEntry($taskEntryName)
                if (-not $taskEntry -or $taskEntry.Length -eq 0) {
                    throw "Missing or empty archive entry: $taskEntryName"
                }
            }
        } finally {
            $taskZip.Dispose()
        }
    }

    $taskSums = foreach ($taskFile in @(Get-ChildItem -LiteralPath $taskDist -File | Sort-Object Name)) {
        $taskHash = (Get-FileHash -LiteralPath $taskFile.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        "$taskHash  $($taskFile.Name)"
    }
    [System.IO.File]::WriteAllText(
        (Join-Path $taskDist 'SHA256SUMS.txt'),
        ($taskSums -join [char]10) + [char]10,
        [System.Text.Encoding]::ASCII
    )

    $taskNotes = @"
## Windows x64 预构建版

此版本由 GitHub Actions 从提交 $taskCommit 构建，Cargo workspace 版本为 $taskVersion。

### 下载

- $taskAppName.zip：桌面应用、使用说明、许可证和构建记录。解压后运行 spout-transparent.exe。
- $taskSdkName.zip：C ABI DLL、MSVC 导入库、C 头文件、CMake 示例、许可证和构建记录。
- SHA256SUMS.txt：两个 ZIP 和 build-info.json 的 SHA-256 校验值。
- build-info.json：源码提交、Rust 工具链、Cargo.lock 校验值、目标平台和本次 CI 链接。

要求 Windows 10/11 x64、支持 Direct3D 11 的显卡与驱动，以及 DX11 Spout Sender。
不需要额外的 SpoutLibrary.dll。没有 Sender 时，应用会在系统托盘等待。
$taskRuntimeNote

### 本次 CI 验证范围

- Rust 格式检查、workspace 默认测试、严格 Clippy、Release 构建。
- 链接真实 Release DLL 的 C11 / C++17 客户端测试，使用 -SkipGpu。
- 打包 SDK 的 CMake 示例编译与运行。
- ZIP 必需文件完整性、SHA-256 校验，以及发布前源码提交一致性检查。

默认忽略的 GPU / 桌面测试未在本次 CI 执行。本版未签名，作为预发布供测试；
实际 Sender、GPU 共享纹理、透明合成和桌面交互仍需在真实 Windows 设备验证。

[查看本次 CI]($taskRunUrl)
[查看源码](https://github.com/StarrySky7D4/spout-transparent/tree/$taskCommit)
"@
    $taskNotes | Set-Content -LiteralPath (Join-Path $taskDist 'release-notes.md') -Encoding utf8NoBOM
    if ($env:GITHUB_STEP_SUMMARY) {
        $taskNotes | Out-File -FilePath $env:GITHUB_STEP_SUMMARY -Append -Encoding utf8
    }
    Get-ChildItem -LiteralPath $taskDist -File | Select-Object Name, Length
} finally {
    Pop-Location
}

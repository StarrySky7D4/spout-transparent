param([ValidateSet('Debug', 'Release')][string]$Configuration = 'Release')
$ErrorActionPreference = 'Stop'
$taskWorkspace = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$taskProfile = $Configuration.ToLowerInvariant()
$taskBuild = Join-Path $taskWorkspace "target/$taskProfile"
$taskSdk = Join-Path $taskWorkspace "target/c-api-sdk/$taskProfile"
Push-Location $taskWorkspace
try {
    $taskArguments = @('build', '-p', 'spout-transparent-capi', '--locked')
    if ($Configuration -eq 'Release') { $taskArguments += '--release' }
    & cargo @taskArguments
    if ($LASTEXITCODE -ne 0) { throw 'C ABI DLL build failed.' }
    foreach ($taskSubdirectory in @('bin', 'lib', 'include', 'examples')) {
        New-Item -ItemType Directory -Path (Join-Path $taskSdk $taskSubdirectory) -Force | Out-Null
    }
    Copy-Item -LiteralPath (Join-Path $taskBuild 'spout_transparent_c.dll') -Destination (Join-Path $taskSdk 'bin') -Force
    Copy-Item -LiteralPath (Join-Path $taskBuild 'spout_transparent_c.dll.lib') -Destination (Join-Path $taskSdk 'lib') -Force
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'include/spout_transparent.h') -Destination (Join-Path $taskSdk 'include') -Force
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'README.md') -Destination $taskSdk -Force
    foreach ($taskFile in @('LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.md')) {
        Copy-Item -LiteralPath (Join-Path $taskWorkspace $taskFile) -Destination $taskSdk -Force
    }
    foreach ($taskFile in @('list_senders.c', 'CMakeLists.txt')) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot "examples/$taskFile") -Destination (Join-Path $taskSdk 'examples') -Force
    }
    Write-Output "C ABI SDK: $taskSdk"
} finally {
    Pop-Location
}

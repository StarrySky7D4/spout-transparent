param(
    [ValidateSet('Debug', 'Release')][string]$Configuration = 'Release',
    [switch]$SkipGpu
)
$ErrorActionPreference = 'Stop'
$taskWorkspace = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
$taskApiRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$taskProfile = $Configuration.ToLowerInvariant()
$taskBuild = Join-Path $taskWorkspace "target/$taskProfile"
$taskOutput = Join-Path $taskWorkspace "target/c-api-tests/$taskProfile"
if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue) -or -not $env:INCLUDE -or -not $env:LIB) {
    throw 'Run from an x64 Visual Studio Developer PowerShell (cl.exe, INCLUDE and LIB are required).'
}
New-Item -ItemType Directory -Path $taskOutput -Force | Out-Null
Push-Location $taskWorkspace
try {
    $taskCargoArguments = @('build', '-p', 'spout-transparent-capi', '--locked')
    if ($Configuration -eq 'Release') { $taskCargoArguments += '--release' }
    & cargo @taskCargoArguments
    if ($LASTEXITCODE -ne 0) { throw 'C ABI DLL build failed.' }
    Copy-Item -LiteralPath (Join-Path $taskBuild 'spout_transparent_c.dll') -Destination $taskOutput -Force
    foreach ($taskLanguage in @('c', 'cpp')) {
        $taskExecutable = Join-Path $taskOutput "client-$taskLanguage.exe"
        $taskObject = Join-Path $taskOutput "client-$taskLanguage.obj"
        $taskLanguageArguments = if ($taskLanguage -eq 'c') { @('/TC', '/std:c11') } else { @('/TP', '/std:c++17') }
        $taskCompilerArguments = @('/nologo', '/W4', '/WX', '/MD') + $taskLanguageArguments + @(
            "/I$(Join-Path $taskApiRoot 'include')", "/Fo$taskObject", "/Fe$taskExecutable",
            (Join-Path $PSScriptRoot 'client.c'), '/link', "/LIBPATH:$taskBuild",
            'spout_transparent_c.dll.lib', 'd3d11.lib', 'dxgi.lib', 'dxguid.lib', 'ole32.lib', 'user32.lib'
        )
        & cl.exe @taskCompilerArguments
        if ($LASTEXITCODE -ne 0) { throw "$taskLanguage client compilation failed." }
        $taskClientArguments = if ($SkipGpu) { @('--no-gpu') } else { @() }
        & $taskExecutable @taskClientArguments
        if ($LASTEXITCODE -ne 0) { throw "$taskLanguage ABI tests failed." }
    }
} finally {
    Pop-Location
}

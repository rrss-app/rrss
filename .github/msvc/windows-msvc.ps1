# Windows MSVC 发布构建的环境准备，供 .github/workflows/release-msvc.yml 调用。
#   windows-msvc.ps1 vcvars                     载入 Visual Studio 的 x64 开发环境（INCLUDE / LIB / rc.exe 等）
#   windows-msvc.ps1 llvm-version <版本|latest>  解析 LLVM 版本，写 LLVM_VERSION（缓存键要用）
#   windows-msvc.ps1 llvm <版本>                 缓存里没有就下载官方 LLVM；设 clang-cl / llvm-lib / lld-link
#                                               为 C 编译器、归档器与链接器
# 结果写进 GITHUB_ENV / GITHUB_PATH，对之后的步骤生效。LLVM 解到 C:\llvm-ci，由 workflow 按版本缓存。
param(
    [Parameter(Mandatory = $true)][ValidateSet('vcvars', 'llvm-version', 'llvm')][string]$Step,
    [string]$Version = 'latest'
)
$ErrorActionPreference = 'Stop'
$target = 'X86_64_PC_WINDOWS_MSVC'

function Add-Env([string]$Name, [string]$Value) {
    "$Name=$Value" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
}

if ($Step -eq 'vcvars') {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if (-not $vs) { throw '找不到带 MSVC x64 工具的 Visual Studio' }
    $bat = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'
    Write-Host "Visual Studio: $vs"
    $before = @{}
    Get-ChildItem env: | ForEach-Object { $before[$_.Name] = $_.Value }
    $oldPath = $before['Path'].Split(';')
    cmd /c "`"$bat`" >nul && set" | ForEach-Object {
        if ($_ -notmatch '^([^=]+)=(.*)$') { return }
        $name = $Matches[1]; $value = $Matches[2]
        if ($name -ieq 'Path') {
            # 只追加 vcvars 新加的目录；GITHUB_PATH 里的目录排在原 PATH 前面。
            $value.Split(';') | Where-Object { $_ -and ($oldPath -notcontains $_) } |
                ForEach-Object { $_ | Out-File -FilePath $env:GITHUB_PATH -Append -Encoding utf8 }
        } elseif ($before[$name] -ne $value) {
            Add-Env $name $value
        }
    }
    exit 0
}

if ($Step -eq 'llvm-version') {
    if ($Version -eq 'latest' -or -not $Version) {
        $headers = @{ 'User-Agent' = 'rrss-ci' }
        if ($env:GH_TOKEN) { $headers['Authorization'] = "Bearer $env:GH_TOKEN" }
        $release = Invoke-RestMethod -Headers $headers -Uri 'https://api.github.com/repos/llvm/llvm-project/releases/latest'
        $Version = $release.tag_name -replace '^llvmorg-', ''
    }
    Write-Host "LLVM $Version"
    Add-Env 'LLVM_VERSION' $Version
    exit 0
}

# llvm：缓存命中时 C:\llvm-ci 里已有这个版本，跳过下载。
$root = 'C:\llvm-ci'
function Install-Llvm([string]$Version, [string]$Root) {
    $base = "https://github.com/llvm/llvm-project/releases/download/llvmorg-$Version"
    $tmp = Join-Path $env:RUNNER_TEMP 'llvm'
    $full = Join-Path $tmp 'full'
    New-Item -ItemType Directory -Force -Path $full | Out-Null
    $clock = [System.Diagnostics.Stopwatch]::StartNew()

    # 用 MSI 的管理安装只解出文件：不注册、不覆盖镜像预装的 LLVM，也避开 tar 包里的符号链接。
    $msi = Join-Path $tmp "LLVM-$Version-win64.msi"
    curl.exe -fsSL --retry 3 -o $msi "$base/LLVM-$Version-win64.msi"
    if ($LASTEXITCODE -eq 0) {
        Write-Host ("下载 MSI {0:N0} MB，用时 {1:N0} 秒" -f ((Get-Item $msi).Length / 1MB), $clock.Elapsed.TotalSeconds)
        # 不用 Start-Process -Wait：它连子进程一起等，msiexec 解完文件后这一步会一直不返回。
        # 直接起进程、只等 msiexec 本身，并设上限；失败时打印 MSI 日志末尾。
        $log = Join-Path $tmp 'msiexec.log'
        $psi = [System.Diagnostics.ProcessStartInfo]::new('msiexec.exe', "/a `"$msi`" /qn /l*v `"$log`" TARGETDIR=`"$full`"")
        $psi.UseShellExecute = $false
        $p = [System.Diagnostics.Process]::Start($psi)
        if (-not $p.WaitForExit(20 * 60 * 1000)) {
            $p.Kill($true)
            if (Test-Path $log) { Get-Content $log -Tail 40 }
            throw 'msiexec /a 20 分钟没有退出'
        }
        if ($p.ExitCode -ne 0) {
            if (Test-Path $log) { Get-Content $log -Tail 40 }
            throw "msiexec /a 失败，退出码 $($p.ExitCode)"
        }
    } else {
        Write-Host '没有 MSI，改用 tar.xz'
        $name = "clang+llvm-$Version-x86_64-pc-windows-msvc"
        $xz = Join-Path $tmp "$name.tar.xz"
        curl.exe -fsSL --retry 3 -o $xz "$base/$name.tar.xz"
        if ($LASTEXITCODE -ne 0) { throw "下载 LLVM $Version 失败" }
        7z x $xz "-o$tmp" -y | Out-Null
        7z x (Join-Path $tmp "$name.tar") "-o$full" -snl -y | Out-Null
    }
    Write-Host ("解包完成，累计 {0:N0} 秒" -f $clock.Elapsed.TotalSeconds)

    # 只留构建用到的三个工具、它们旁边的 DLL 与 clang 自带的头文件（clang-cl 按 <bin>\..\lib\clang 找），
    # 缓存从几 GB 降到几百 MB。
    $src = (Get-ChildItem -Path $full -Recurse -Filter clang-cl.exe | Select-Object -First 1).DirectoryName
    if (-not $src) { throw '解出的 LLVM 里没有 clang-cl.exe' }
    New-Item -ItemType Directory -Force -Path (Join-Path $Root 'bin'), (Join-Path $Root 'lib') | Out-Null
    foreach ($tool in 'clang-cl.exe', 'lld-link.exe', 'llvm-lib.exe') {
        Copy-Item (Join-Path $src $tool) (Join-Path $Root 'bin')
    }
    Get-ChildItem -Path $src -Filter *.dll | Copy-Item -Destination (Join-Path $Root 'bin')
    Copy-Item -Recurse (Join-Path (Split-Path $src -Parent) 'lib\clang') (Join-Path $Root 'lib')
    Remove-Item -Recurse -Force $tmp
    $size = (Get-ChildItem -Path $Root -Recurse -File | Measure-Object Length -Sum).Sum / 1MB
    Write-Host ("留下 {0:N0} MB，累计 {1:N0} 秒" -f $size, $clock.Elapsed.TotalSeconds)
}

if (Test-Path (Join-Path $root 'bin\clang-cl.exe')) {
    Write-Host "LLVM $Version 来自缓存"
} else {
    Install-Llvm $Version $root
}

$bin = Join-Path $root 'bin'
foreach ($tool in 'clang-cl.exe', 'lld-link.exe', 'llvm-lib.exe') {
    if (-not (Test-Path (Join-Path $bin $tool))) { throw "LLVM 缺少 $tool" }
}
$reported = & (Join-Path $bin 'clang-cl.exe') --version | Select-Object -First 1
Write-Host $reported
if ($reported -notmatch [regex]::Escape($Version)) { throw "clang-cl 报告的版本不是 $Version" }

$bin | Out-File -FilePath $env:GITHUB_PATH -Append -Encoding utf8
Add-Env "CC_$($target.ToLower())" (Join-Path $bin 'clang-cl.exe')
Add-Env "CXX_$($target.ToLower())" (Join-Path $bin 'clang-cl.exe')
Add-Env "AR_$($target.ToLower())" (Join-Path $bin 'llvm-lib.exe')
Add-Env "CARGO_TARGET_${target}_LINKER" (Join-Path $bin 'lld-link.exe')

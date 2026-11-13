# 编鸿蒙用的 Rust 库（crates/harmony-ffi），放进 entry/libs/<架构>/libp2pcli.so；
# 顺便把界面翻译（桌面的 en.json）和版本号（桌面的 tauri.conf.json）同步过来。在 DevEco Studio 里打包前先跑一次。
#
# 用法（Windows，在 PowerShell 里）：
#   powershell -ExecutionPolicy Bypass -File mobile\harmony\build-rust.ps1             真机和模拟器都编
#   powershell -ExecutionPolicy Bypass -File mobile\harmony\build-rust.ps1 -Target emulator   只编模拟器（x86_64）
#   powershell -ExecutionPolicy Bypass -File mobile\harmony\build-rust.ps1 -Target device     只编真机（arm64）
#
# 要装 DevEco Studio（带鸿蒙 SDK），要 Rust 的编译目标：
#   rustup target add aarch64-unknown-linux-ohos x86_64-unknown-linux-ohos
# SDK 默认按 DevEco Studio 的安装位置找；装在别处时设环境变量 OHOS_NDK_HOME 指向 SDK 里的 openharmony 目录。
param([ValidateSet("all", "emulator", "device")][string]$Target = "all")

$here = $PSScriptRoot
$repo = (Resolve-Path "$here\..\..").Path

# 一、找鸿蒙 SDK 的 openharmony 目录（里面有 native\llvm 编译器和 sysroot）
$sdk = $env:OHOS_NDK_HOME
if (-not $sdk) {
    $roots = @()
    if ($env:DEVECO_SDK_HOME) { $roots += $env:DEVECO_SDK_HOME }
    $keys = "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*", "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*"
    Get-ItemProperty $keys -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName -match "DevEco" -and $_.InstallLocation } |
        ForEach-Object { $roots += (Join-Path $_.InstallLocation "sdk") }
    $roots += "C:\Program Files\Huawei\DevEco Studio\sdk"
    foreach ($r in $roots) {
        $c = Join-Path $r "default\openharmony"
        if (Test-Path (Join-Path $c "native\llvm\bin\clang.exe")) { $sdk = $c; break }
    }
}
if (-not $sdk) { throw "找不到鸿蒙 SDK：装 DevEco Studio，或者设 OHOS_NDK_HOME 指向 SDK 里的 openharmony 目录" }
Write-Host "鸿蒙 SDK：$sdk"
$bin = Join-Path $sdk "native\llvm\bin"

# 二、编译器包装：SDK 里带的是 Linux 的 shell 脚本，Windows 上换成 .cmd。
# 编出来的东西放在纯英文的目录：路径里有中文时，经 cmd 转一道参数会乱码，链接器找不到文件
$work = Join-Path $env:LOCALAPPDATA "p2pcli-ohos"
$wrap = Join-Path $work "cc"
New-Item -ItemType Directory -Force $wrap | Out-Null
$env:CARGO_TARGET_DIR = Join-Path $work "target"
$env:OHOS_NDK_HOME = $sdk

$targets = @()
if ($Target -ne "device") { $targets += , @("x86_64-unknown-linux-ohos", "x86_64-linux-ohos", "x86_64") }
if ($Target -ne "emulator") { $targets += , @("aarch64-unknown-linux-ohos", "aarch64-linux-ohos", "arm64-v8a") }

foreach ($t in $targets) {
    $rustTarget, $clangTarget, $abi = $t
    $flags = "-target $clangTarget --sysroot=`"$sdk\native\sysroot`" -D__MUSL__"
    Set-Content -Encoding ascii (Join-Path $wrap "$rustTarget-clang.cmd") "@`"$bin\clang.exe`" $flags %*"
    Set-Content -Encoding ascii (Join-Path $wrap "$rustTarget-clang++.cmd") "@`"$bin\clang++.exe`" $flags %*"
    $u = $rustTarget.Replace("-", "_")
    Set-Item "env:CC_$u" (Join-Path $wrap "$rustTarget-clang.cmd")
    Set-Item "env:CXX_$u" (Join-Path $wrap "$rustTarget-clang++.cmd")
    Set-Item "env:AR_$u" (Join-Path $bin "llvm-ar.exe")
    Set-Item "env:CARGO_TARGET_$($u.ToUpper())_LINKER" (Join-Path $wrap "$rustTarget-clang.cmd")

    Write-Host "编 $rustTarget ..."
    Push-Location $repo
    & cargo build --locked --release -p p2pcli-harmony-ffi --target $rustTarget
    $code = $LASTEXITCODE
    Pop-Location
    if ($code -ne 0) { throw "编 $rustTarget 失败" }
    $out = Join-Path $here "entry\libs\$abi"
    New-Item -ItemType Directory -Force $out | Out-Null
    Copy-Item -Force (Join-Path $env:CARGO_TARGET_DIR "$rustTarget\release\libp2pcli.so") (Join-Path $out "libp2pcli.so")
}

# 三、界面翻译：和桌面、安卓、iOS 同一份
$i18n = Join-Path $here "entry\src\main\resources\rawfile\i18n"
New-Item -ItemType Directory -Force $i18n | Out-Null
Copy-Item -Force (Join-Path $repo "app\src\i18n\locales\en.json") (Join-Path $i18n "en.json")

# 四、版本号：和桌面端同一个。1.2.3 → versionCode 1002003，和安卓、iOS 一样算
$version = (Get-Content -Raw -Encoding UTF8 (Join-Path $repo "app\src-tauri\tauri.conf.json") | ConvertFrom-Json).version
$parts = ($version -split "-")[0].Split(".") | ForEach-Object { [int]$_ }
$versionCode = $parts[0] * 1000000 + $parts[1] * 1000 + $parts[2]
$appJson = Join-Path $here "AppScope\app.json5"
$text = [IO.File]::ReadAllText($appJson)
$new = $text -replace '"versionCode": \d+', "`"versionCode`": $versionCode" -replace '"versionName": "[^"]*"', "`"versionName`": `"$version`""
if ($new -ne $text) { [IO.File]::WriteAllText($appJson, $new, (New-Object Text.UTF8Encoding $false)) }

Write-Host "Rust 库、翻译、版本号（$version）已同步：$here\entry"

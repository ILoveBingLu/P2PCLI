# 命令行打 HAP（不开 DevEco Studio 时用；在 DevEco Studio 里直接点运行也一样）。先跑过 build-rust.ps1。
# 用法：powershell -ExecutionPolicy Bypass -File mobile\harmony\build-hap.ps1
# 产物：entry\build\default\outputs\default\ 下的 .hap（配了签名是 entry-default-signed.hap）
$here = $PSScriptRoot

# 找 DevEco Studio：工具（node、ohpm、hvigor）和 SDK 都在它的安装目录里
$deveco = $env:DEVECO_HOME
if (-not $deveco) {
    $keys = "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*", "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*"
    $deveco = Get-ItemProperty $keys -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName -match "DevEco" -and $_.InstallLocation } |
        Select-Object -First 1 -ExpandProperty InstallLocation
}
if (-not $deveco -or -not (Test-Path "$deveco\tools\hvigor\bin\hvigorw.js")) { throw "找不到 DevEco Studio：设环境变量 DEVECO_HOME 指向它的安装目录" }
$env:DEVECO_SDK_HOME = "$deveco\sdk"
$node = "$deveco\tools\node\node.exe"
$env:PATH = "$deveco\tools\node;$env:PATH"

# hvigor 不认带中文（等非英文字符）的工程路径，目录链接、subst 虚拟盘符也绕不开（它会找到真实路径）：
# 代码要放在纯英文的路径下，DevEco Studio 里打开也一样
if ($here -match '[^\x00-\x7F]') { throw "工程路径里有中文等非英文字符，鸿蒙的打包工具不支持：把代码挪到纯英文的路径下。现在的路径：$here" }

Push-Location $here
try {
    & "$deveco\tools\ohpm\bin\ohpm.bat" install --all
    if ($LASTEXITCODE -ne 0) { throw "ohpm install 失败" }
    & $node "$deveco\tools\hvigor\bin\hvigorw.js" --mode module -p product=default -p module=entry@default -p buildMode=debug assembleHap --no-daemon
    if ($LASTEXITCODE -ne 0) { throw "打包失败" }
    Get-ChildItem "entry\build\default\outputs\default\*.hap" | ForEach-Object { Write-Host "已生成：$($_.FullName)" }
} finally {
    Pop-Location
}

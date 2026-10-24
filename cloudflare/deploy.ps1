# 一键部署 P2PCLI 牵线服务到 Cloudflare Workers（Windows）
# 用法：.\deploy.ps1 [域名]      默认 p2pcli.yscopy.cn，域名必须托管在你的 Cloudflare 账号里
#       .\deploy.ps1 -Check      只装依赖、试打包，不部署
param(
  [string]$Domain = $(if ($env:P2PCLI_DOMAIN) { $env:P2PCLI_DOMAIN } else { "p2pcli.yscopy.cn" }),
  [switch]$Check
)
$ErrorActionPreference = "Stop"
Set-Location -LiteralPath $PSScriptRoot

if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) {
  Write-Host "需要先装 Node 和 pnpm（npm i -g pnpm）"
  exit 1
}
$env:WRANGLER_SEND_METRICS = "false"

Write-Host "==> 安装部署工具"
pnpm install --frozen-lockfile
if ($LASTEXITCODE -ne 0) { exit 1 }

if ($Check) {
  pnpm exec wrangler deploy --dry-run
  if ($LASTEXITCODE -ne 0) { exit 1 }
  Write-Host "检查通过，没有部署"
  exit 0
}

$who = (pnpm exec wrangler whoami 2>&1 | Out-String)
if ($who -notmatch "You are logged in") {
  Write-Host "==> 第一次用，在浏览器里登录 Cloudflare 账号"
  pnpm exec wrangler login
  if ($LASTEXITCODE -ne 0) { exit 1 }
}

Write-Host "==> 部署到 $Domain"
pnpm exec wrangler deploy --domain $Domain
if ($LASTEXITCODE -ne 0) { exit 1 }

Write-Host "==> 检查能不能访问"
for ($i = 0; $i -lt 30; $i++) {
  try {
    $r = Invoke-WebRequest -UseBasicParsing -TimeoutSec 5 "https://$Domain/"
    if ($r.Content -match "P2PCLI") {
      Write-Host "部署好了。软件设置里的牵线服务器填：wss://$Domain"
      exit 0
    }
  } catch {}
  Start-Sleep -Seconds 2
}
Write-Host "已经部署，但 https://$Domain/ 暂时还打不开；新域名的证书要几分钟才生效，稍后再试"

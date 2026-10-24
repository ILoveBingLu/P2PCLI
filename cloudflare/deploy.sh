#!/usr/bin/env bash
# 一键部署 P2PCLI 牵线服务到 Cloudflare Workers（macOS / Linux）
# 用法：./deploy.sh [域名]        默认 p2pcli.yscopy.cn，域名必须托管在你的 Cloudflare 账号里
#       ./deploy.sh --check       只装依赖、试打包，不部署
set -euo pipefail
cd "$(dirname "$0")"

CHECK=0
DOMAIN="${P2PCLI_DOMAIN:-p2pcli.yscopy.cn}"
for arg in "$@"; do
  case "$arg" in
    --check) CHECK=1 ;;
    *) DOMAIN="$arg" ;;
  esac
done

command -v pnpm >/dev/null || { echo "需要先装 Node 和 pnpm（npm i -g pnpm）"; exit 1; }
export WRANGLER_SEND_METRICS=false

echo "==> 安装部署工具"
pnpm install --frozen-lockfile

if [ "$CHECK" = 1 ]; then
  pnpm exec wrangler deploy --dry-run
  echo "检查通过，没有部署"
  exit 0
fi

if ! pnpm exec wrangler whoami 2>/dev/null | grep -q "You are logged in"; then
  echo "==> 第一次用，在浏览器里登录 Cloudflare 账号"
  pnpm exec wrangler login
fi

echo "==> 部署到 $DOMAIN"
pnpm exec wrangler deploy --domain "$DOMAIN"

echo "==> 检查能不能访问"
for _ in $(seq 1 30); do
  if curl -fsS --max-time 5 "https://$DOMAIN/" 2>/dev/null | grep -q "P2PCLI"; then
    echo "部署好了。软件设置里的牵线服务器填：wss://$DOMAIN"
    exit 0
  fi
  sleep 2
done
echo "已经部署，但 https://$DOMAIN/ 暂时还打不开；新域名的证书要几分钟才生效，稍后再试"

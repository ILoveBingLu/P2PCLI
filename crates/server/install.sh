#!/usr/bin/env bash
# P2PCLI 牵线服务器一键安装、升级、卸载。Linux + systemd，root 运行：
#
#   curl -fsSL https://raw.githubusercontent.com/ILoveBingLu/P2PCLI/main/crates/server/install.sh | sudo bash
#
# 参数写在 `bash -s --` 后面，例如 `... | sudo bash -s -- --port 9000`，--help 看全部。
# 已经装过再跑一次就是升级：只换程序、重启，原来的服务配置不动；带了 --port / --ws-port 才按这次的参数重写配置。
set -euo pipefail

REPO="${P2PCLI_REPO:-ILoveBingLu/P2PCLI}"
BIN=/opt/p2pcli/p2pcli-server
UNIT=/etc/systemd/system/p2pcli-server.service
# systemd 的 DynamicUser 实际把数据放在 /var/lib/private/p2pcli-server，这里是它的链接
DATA=/var/lib/p2pcli-server

usage() {
  cat <<'EOF'
P2PCLI 牵线服务器一键安装（Linux + systemd，root 运行）

  --port 端口        UDP 端口，默认 7788
  --ws-port 端口     另开明文 WebSocket 入口，给本机的 HTTPS 反向代理或 CDN 回源用，默认不开
  --version 版本     装指定版本，例如 0.1.0，默认装最新
  --mirror 前缀      GitHub 下载慢时用的代理前缀，下载地址变成 前缀+原地址（代理是第三方的，自己判断可不可信）
  --uninstall        停服务、删程序；设备库和服务器私钥留着，加 --purge 一起删
  -h, --help         显示这段说明
EOF
}

die() { echo "错误：$*" >&2; exit 1; }
say() { echo "==> $*"; }
is_port() { [[ "$1" =~ ^[0-9]+$ ]] && [ "$1" -ge 1 ] && [ "$1" -le 65535 ]; }

port=7788
ws_port=""
version=""
mirror=""
uninstall=0
purge=0
reconfigure=0
while [ $# -gt 0 ]; do
  case "$1" in
    --port) is_port "${2:-}" || die "--port 后面要写 1-65535 的端口"; port=$2; reconfigure=1; shift 2 ;;
    --ws-port) is_port "${2:-}" || die "--ws-port 后面要写 1-65535 的端口"; ws_port=$2; reconfigure=1; shift 2 ;;
    --version) [ -n "${2:-}" ] || die "--version 后面要写版本，例如 0.1.0"; version=${2#server-v}; version=${version#v}; shift 2 ;;
    --mirror) [ -n "${2:-}" ] || die "--mirror 后面要写代理前缀"; mirror=$2; shift 2 ;;
    --uninstall) uninstall=1; shift ;;
    --purge) purge=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "不认识的参数：$1" ;;
  esac
done

[ "$(id -u)" -eq 0 ] || die "要用 root 运行，命令里的 bash 前面加 sudo"
[ "$(uname -s)" = Linux ] || die "只支持 Linux"
command -v systemctl >/dev/null && [ -d /run/systemd/system ] || die "需要 systemd"

if [ "$uninstall" = 1 ]; then
  systemctl disable --now p2pcli-server >/dev/null 2>&1 || true
  rm -f "$UNIT" "$BIN"
  rmdir /opt/p2pcli 2>/dev/null || true
  systemctl daemon-reload
  if [ "$purge" = 1 ]; then
    rm -rf /var/lib/private/p2pcli-server /var/lib/p2pcli-server
    say "已卸载，设备库和服务器私钥也删了"
  else
    say "已卸载。设备库和服务器私钥还在 /var/lib/private/p2pcli-server，重装后服务器公钥不变；要一起删就加 --purge"
  fi
  exit 0
fi

case "$(uname -m)" in
  x86_64|amd64) arch=x86_64 ;;
  aarch64|arm64) arch=aarch64 ;;
  *) die "不支持这种 CPU：$(uname -m)，只有 x86_64 和 aarch64 的版本" ;;
esac

if command -v curl >/dev/null; then
  fetch() { curl -fsSL --retry 3 --connect-timeout 15 -o "$2" "$1"; }
  peek() { curl -fsS4 --max-time 5 "$1"; }
elif command -v wget >/dev/null; then
  fetch() { wget -q -T 15 -t 3 -O "$2" "$1"; }
  peek() { wget -q -4 -T 5 -O - "$1"; }
else
  die "需要 curl 或 wget"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

if [ -z "$version" ]; then
  say "查最新版本"
  fetch "https://api.github.com/repos/$REPO/releases?per_page=30" "$tmp/releases.json" \
    || die "连不上 api.github.com 查版本，可以用 --version 指定，例如 --version 0.1.0"
  tag=$(grep -o '"tag_name": *"server-v[^"]*"' "$tmp/releases.json" | awk -F'"' 'NR==1 {print $4}' || true)
  [ -n "$tag" ] || die "发布页上还没有牵线服务器的版本"
else
  tag="server-v$version"
fi

base="${mirror}https://github.com/$REPO/releases/download/$tag"
say "下载 $tag（$arch）"
fetch "$base/p2pcli-server-linux-$arch" "$tmp/p2pcli-server" || die "下载失败：$base/p2pcli-server-linux-$arch"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || die "下载校验和失败"
want=$(awk -v f="p2pcli-server-linux-$arch" '$2 == f {print $1}' "$tmp/SHA256SUMS")
got=$(sha256sum "$tmp/p2pcli-server" | awk '{print $1}')
[ -n "$want" ] && [ "$want" = "$got" ] || die "校验和对不上，下载的文件可能坏了或被改过"
chmod 755 "$tmp/p2pcli-server"
"$tmp/p2pcli-server" --version >/dev/null || die "下载的程序在这台机器上跑不起来"

old=""
[ -x "$BIN" ] && old=$("$BIN" --version 2>/dev/null | awk '{print $2}' || true)
install -d -m 755 /opt/p2pcli
# 先放到旁边再改名，正在跑的旧程序不受影响
install -m 755 "$tmp/p2pcli-server" "$BIN.new"
mv -f "$BIN.new" "$BIN"

if [ ! -f "$UNIT" ] || [ "$reconfigure" = 1 ]; then
  public_ip=""
  for u in https://4.ipw.cn https://api.ipify.org https://ifconfig.me/ip; do
    ip=$(peek "$u" 2>/dev/null | tr -d '[:space:]' || true)
    if [[ "$ip" =~ ^[0-9]+(\.[0-9]+){3}$ ]]; then public_ip=$ip; break; fi
  done
  args="--data-dir $DATA --port $port"
  [ -n "$ws_port" ] && args="$args --ws-port $ws_port"
  [ -n "$public_ip" ] && args="$args --public-ip $public_ip"
  cat >"$UNIT" <<EOF
[Unit]
Description=P2PCLI rendezvous server
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=$BIN $args
DynamicUser=yes
StateDirectory=p2pcli-server
Restart=always
RestartSec=3
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF
  say "写好服务配置 $UNIT"
else
  # 升级：端口以现有配置为准
  port=$(grep -o -- '--port [0-9]*' "$UNIT" | awk 'NR==1 {print $2}' || true)
  port=${port:-7788}
  ws_port=$(grep -o -- '--ws-port [0-9]*' "$UNIT" | awk 'NR==1 {print $2}' || true)
fi

if command -v ufw >/dev/null && ufw status 2>/dev/null | grep -q "Status: active"; then
  ufw allow "$port/udp" >/dev/null && say "ufw 已放行 UDP $port"
fi
if command -v firewall-cmd >/dev/null && firewall-cmd --state >/dev/null 2>&1; then
  firewall-cmd --permanent --add-port="$port/udp" >/dev/null && firewall-cmd --reload >/dev/null && say "firewalld 已放行 UDP $port"
fi

systemctl daemon-reload
systemctl enable p2pcli-server >/dev/null 2>&1
since=$(date '+%Y-%m-%d %H:%M:%S')
systemctl restart p2pcli-server
addr=""
for _ in $(seq 1 30); do
  addr=$(journalctl -u p2pcli-server --since "$since" -o cat 2>/dev/null | grep -oE '^[0-9a-f]{64}@[^ ]+' | awk 'END {print}' || true)
  [ -n "$addr" ] && break
  sleep 0.5
done
if ! systemctl is-active --quiet p2pcli-server || [ -z "$addr" ]; then
  journalctl -u p2pcli-server -n 20 --no-pager || true
  die "服务没起来，上面是它的日志"
fi

new=$("$BIN" --version | awk '{print $2}')
echo
if [ -n "$old" ] && [ "$old" != "$new" ]; then
  echo "P2PCLI 牵线服务器已从 $old 升级到 $new，正在运行。"
else
  echo "P2PCLI 牵线服务器 $new 正在运行，开机自动启动，出错自动重启。"
fi
cat <<EOF

在 P2PCLI 的 设置 → 牵线服务器 里填：

  $addr

@ 后面的 IP 也可以换成直接解析到这台服务器的域名。
还要在云服务器控制台的安全组（或防火墙）里放行 UDP $port，不然外面连不进来。
EOF
if [ -n "$ws_port" ]; then
  cat <<EOF

WebSocket 入口开在 $ws_port 端口，是明文的，别直接对外开放。
在本机用 nginx、Caddy 加 HTTPS 转发过来，或者放在 CDN 后面，软件里填 wss://你的域名。
EOF
fi
echo
echo "看日志：journalctl -u p2pcli-server -f    卸载：本命令加 --uninstall"

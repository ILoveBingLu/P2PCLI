#!/bin/sh
# 把桌面端的 logo、各家 AI 图标渲染成透明底 PNG 放进 Android 资源（它们用了遮罩、渐变，安卓的矢量图表达不了）。
# 要本机装了 Chrome。在仓库根目录跑：sh mobile/android/tools/render_assets.sh
set -eu
repo=$(pwd)
res="$repo/mobile/android/app/src/main/res/drawable-nodpi"
chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
tmp=$(mktemp -d)
mkdir -p "$res"

render() { # 源 SVG、输出名、边长
    cat > "$tmp/page.html" <<EOF
<!doctype html><html><body style="margin:0;background:transparent">
<img src="file://$1" style="display:block;width:${3}px;height:${3}px">
</body></html>
EOF
    "$chrome" --headless=new --disable-gpu --hide-scrollbars --default-background-color=00000000 \
        --user-data-dir="$tmp/profile" --allow-file-access-from-files \
        --window-size="$3,$3" --screenshot="$res/$2.png" "file://$tmp/page.html" >/dev/null 2>&1
}

render "$repo/app/src/assets/logo.svg" logo 192
for svg in "$repo"/app/src/assets/agents/*.svg; do
    kind=$(basename "$svg" .svg)
    render "$svg" "agent_$kind" 96
done
rm -rf "$tmp"
ls "$res"

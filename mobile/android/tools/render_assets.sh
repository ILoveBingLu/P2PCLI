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
# 桌面图标：自适应图标（安卓 8 以上）的前景是 logo，放在 108dp 里中间 60dp 的安全区；
# 背景是渐变（res/drawable/ic_launcher_background.xml，颜色取自桌面图标）。旧系统直接用桌面的 icon.png
main="$repo/mobile/android/app/src/main/res"
for d in mdpi:1 hdpi:1.5 xhdpi:2 xxhdpi:3 xxxhdpi:4; do
    name=${d%%:*}; scale=${d##*:}
    fg=$(awk "BEGIN{print int(108*$scale)}"); logo=$(awk "BEGIN{print int(60*$scale)}"); legacy=$(awk "BEGIN{print int(48*$scale)}")
    mkdir -p "$main/mipmap-$name"
    printf '%s' "<!doctype html><html><body style=\"margin:0;background:transparent;width:${fg}px;height:${fg}px;display:grid;place-items:center\"><img src=\"file://$repo/app/src/assets/logo.svg\" style=\"width:${logo}px;height:${logo}px\"></body></html>" > "$tmp/page.html"
    "$chrome" --headless=new --disable-gpu --hide-scrollbars --default-background-color=00000000 \
        --user-data-dir="$tmp/profile" --allow-file-access-from-files \
        --window-size="$fg,$fg" --screenshot="$main/mipmap-$name/ic_launcher_foreground.png" "file://$tmp/page.html" >/dev/null 2>&1
    sips -z "$legacy" "$legacy" "$repo/app/src-tauri/icons/icon.png" --out "$main/mipmap-$name/ic_launcher.png" >/dev/null
    cp "$main/mipmap-$name/ic_launcher.png" "$main/mipmap-$name/ic_launcher_round.png"
done
rm -rf "$tmp"
ls "$res"

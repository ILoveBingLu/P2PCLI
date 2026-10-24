#!/usr/bin/env bash
# 从 AppIcon.icon（Icon Composer 做的分层液态玻璃图标）生成 macOS 要用的两份图标：
#   Assets.car  macOS 26 起系统用它，分层、液态玻璃，跟着浅色、深色、着色、透明外观变
#   icon.icns   更早的系统用它，从同一个图标渲染，按苹果的边距（1024 画布里图形占 824）
# 两份都提交进仓库，打包时不需要 Xcode。改了 AppIcon.icon 跑一下这个脚本。
# 需要 Xcode 26 以上（自带 Icon Composer、ictool、actool）。
set -euo pipefail
cd "$(dirname "$0")"

ICTOOL="/Applications/Xcode.app/Contents/Applications/Icon Composer.app/Contents/Executables/ictool"
[ -x "$ICTOOL" ] || { echo "找不到 ictool，需要装 Xcode 26 以上"; exit 1; }

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# 一、Assets.car
mkdir -p "$WORK/car"
xcrun actool "$(pwd)/AppIcon.icon" --compile "$WORK/car" \
  --output-format human-readable-text --notices --warnings \
  --output-partial-info-plist "$WORK/car/info.plist" \
  --app-icon AppIcon --include-all-app-icons --enable-on-demand-resources NO \
  --development-region en --target-device mac --minimum-deployment-target 11.0 \
  --platform macosx > "$WORK/actool.log" 2>&1 || true
if [ ! -f "$WORK/car/Assets.car" ]; then
  cat "$WORK/actool.log"
  echo "actool 没生成 Assets.car。常见原因是 Xcode 的后台服务 ibtoold 状态坏了，运行 killall ibtoold 后再试一次"
  exit 1
fi
cp "$WORK/car/Assets.car" Assets.car

# 二、icon.icns
SET="$WORK/icon.iconset"
mkdir -p "$SET"
render() { # 画布边长 输出文件
  local canvas=$1 out=$2
  local art=$(( canvas * 824 / 1024 ))
  [ "$canvas" -eq 16 ] && art=14
  "$ICTOOL" AppIcon.icon --export-image --output-file "$WORK/art.png" --platform macOS \
    --rendition Default --width "$art" --height "$art" --scale 1 >/dev/null
  # 居中贴到透明画布上
  osascript -l JavaScript - "$WORK/art.png" "$canvas" "$out" <<'JS' >/dev/null
ObjC.import("AppKit");
function run(argv) {
  const [src, size, dst] = [argv[0], Number(argv[1]), argv[2]];
  const art = $.NSImage.alloc.initWithContentsOfFile(src);
  const a = art.representations.objectAtIndex(0).pixelsWide;
  const rep = $.NSBitmapImageRep.alloc.initWithBitmapDataPlanesPixelsWidePixelsHighBitsPerSampleSamplesPerPixelHasAlphaIsPlanarColorSpaceNameBytesPerRowBitsPerPixel(
    null, size, size, 8, 4, true, false, $.NSDeviceRGBColorSpace, 0, 0);
  $.NSGraphicsContext.saveGraphicsState;
  $.NSGraphicsContext.setCurrentContext($.NSGraphicsContext.graphicsContextWithBitmapImageRep(rep));
  const off = (size - a) / 2;
  art.drawInRectFromRectOperationFraction($.NSMakeRect(off, off, a, a), $.NSZeroRect, $.NSCompositingOperationSourceOver, 1);
  $.NSGraphicsContext.restoreGraphicsState;
  rep.representationUsingTypeProperties($.NSBitmapImageFileTypePNG, $({})).writeToFileAtomically(dst, true);
}
JS
}
for s in 16 32 128 256 512; do
  render "$s" "$SET/icon_${s}x${s}.png"
  render $(( s * 2 )) "$SET/icon_${s}x${s}@2x.png"
done
iconutil --convert icns "$SET" -o icon.icns

echo "已生成 $(pwd)/Assets.car 和 $(pwd)/icon.icns"

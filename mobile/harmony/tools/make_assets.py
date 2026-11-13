#!/usr/bin/env python3
"""生成鸿蒙版的图标资源（entry/src/main/resources/base/media）：和桌面、安卓、iOS 同一套。

在仓库根目录跑：python3 mobile/harmony/tools/make_assets.py（要先 pnpm install）。
- lucide 图标（ISC 许可）：从 node_modules/lucide-react 转成 SVG。ArkUI 里不方便给 SVG 改色，
  每个图标按用到的颜色各出一份：lucide_<名字>（白）、lucide_<名字>_accent（蓝）、_muted（灰）、_danger（红）、_warning（黄）
- 系统图标（苹果、Windows、Linux、安卓、鸿蒙）：桌面的 SVG，改成白色
- 各家 AI 的图标、logo：直接用安卓、iOS 已经渲染好的 PNG
- 标题栏、工具栏按钮的图标（menu_<名字>.png）：系统给这些位置的 SVG 统一上色时会把封闭的图形填满，
  所以这几个用 Chrome 渲染成 PNG（系统不给 PNG 上色），线条还是 lucide 的
- 终端：xterm.js 和样式拷进 rawfile/terminal（和桌面同一个版本）
"""
import os
import re
import shutil
import subprocess
import tempfile

REPO = os.getcwd()
MEDIA = os.path.join(REPO, "mobile/harmony/entry/src/main/resources/base/media")
RAW = os.path.join(REPO, "mobile/harmony/entry/src/main/resources/rawfile")

LUCIDE = [
    "plus", "refresh-cw", "scan-line", "square-terminal", "x", "ellipsis", "file-up", "trash", "unplug",
    "send-horizontal", "keyboard", "triangle-alert", "square-arrow-out-up-right", "key-round", "arrow-up",
    "arrow-down", "arrow-left", "arrow-right", "bot", "plug", "settings", "copy", "check", "chevron-right",
    "clipboard-paste", "languages", "folder-open", "lock-keyhole", "image", "info",
]
COLORS = {"": "#FCFCFC", "_accent": "#0485F7", "_muted": "#9F9FA9", "_danger": "#DB3B3E", "_warning": "#F5A524"}
PLATFORMS = ["apple", "windows", "linux", "android", "harmonyos"]
MENU = ["scan-line", "settings", "plus", "file-up", "refresh-cw", "square-terminal", "x", "check"]
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"


def num(a, k):
    return float(a.get(k, 0))


def to_path(tag, a):
    if tag == "path":
        return a["d"]
    if tag in ("circle", "ellipse"):
        cx, cy = num(a, "cx"), num(a, "cy")
        rx = float(a.get("r", a.get("rx", 0)))
        ry = float(a.get("r", a.get("ry", 0)))
        return f"M{cx - rx},{cy}a{rx},{ry} 0 1,0 {2 * rx},0a{rx},{ry} 0 1,0 {-2 * rx},0"
    if tag == "line":
        return f"M{a['x1']},{a['y1']}L{a['x2']},{a['y2']}"
    if tag in ("polyline", "polygon"):
        return "M" + " L".join(a["points"].split()) + ("Z" if tag == "polygon" else "")
    if tag == "rect":
        x, y, w, h = num(a, "x"), num(a, "y"), num(a, "width"), num(a, "height")
        rx = float(a.get("rx", a.get("ry", 0)))
        ry = float(a.get("ry", rx))
        if rx == 0:
            return f"M{x},{y}h{w}v{h}h{-w}Z"
        return (f"M{x + rx},{y}h{w - 2 * rx}a{rx},{ry} 0 0 1 {rx},{ry}v{h - 2 * ry}a{rx},{ry} 0 0 1 {-rx},{ry}"
                f"h{-(w - 2 * rx)}a{rx},{ry} 0 0 1 {-rx},{-ry}v{-(h - 2 * ry)}a{rx},{ry} 0 0 1 {rx},{-ry}Z")
    raise ValueError(tag)


def lucide_paths(name):
    src = open(os.path.join(REPO, f"node_modules/lucide-react/dist/esm/icons/{name}.mjs")).read()
    node = src[src.index("node: ["):src.index("};", src.index("node: ["))]
    paths = []
    for m in re.finditer(r'\[\s*"(\w+)",\s*\{([^}]*)\}\s*\]', node):
        attrs = dict(re.findall(r'(\w+): "([^"]*)"', m.group(2)))
        attrs.pop("key", None)
        paths.append(to_path(m.group(1), attrs))
    expected = len(re.findall(r'\[\s*"(?:path|circle|ellipse|line|polyline|polygon|rect)"', node))
    if len(paths) != expected:
        raise SystemExit(f"{name}：源文件里有 {expected} 个图形，只认出 {len(paths)} 个")
    return paths


def main():
    os.makedirs(MEDIA, exist_ok=True)
    for f in os.listdir(MEDIA):
        if f.startswith(("lucide_", "platform_", "agent_", "menu_")) or f == "logo.png":
            os.remove(os.path.join(MEDIA, f))
    for name in LUCIDE:
        for suffix, color in COLORS.items():
            # 每条路径都写明不填充、描边颜色：ArkUI 画 SVG 时封闭的图形不继承外层的 fill="none"，会被填成实心
            body = "".join(f'<path d="{p}" fill="none" stroke="{color}" stroke-width="2" stroke-linecap="round" '
                           f'stroke-linejoin="round"/>' for p in lucide_paths(name))
            svg = (f'<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none">'
                   f'{body}</svg>\n')
            with open(os.path.join(MEDIA, f"lucide_{name.replace('-', '_')}{suffix}.svg"), "w") as f:
                f.write(svg)
    for name in PLATFORMS:
        svg = open(os.path.join(REPO, f"app/src/assets/platforms/{name}.svg")).read()
        svg = re.sub(r'\s(width|height)="1em"', "", svg)
        svg = svg.replace('fill="currentColor"', 'fill="#FCFCFC"')
        if 'fill="#FCFCFC"' not in svg:
            svg = svg.replace("<svg ", '<svg fill="#FCFCFC" ', 1)
        with open(os.path.join(MEDIA, f"platform_{name}.svg"), "w") as f:
            f.write(svg)
    nodpi = os.path.join(REPO, "mobile/android/app/src/main/res/drawable-nodpi")
    for f in os.listdir(nodpi):
        if f.startswith("agent_") and f.endswith(".png"):
            shutil.copy(os.path.join(nodpi, f), os.path.join(MEDIA, f))
    # logo：和 Mac、iOS 同一个液态玻璃图标（iOS 那边由 Icon Composer 渲染好的）
    shutil.copy(os.path.join(REPO, "mobile/ios/P2PCLI/Assets.xcassets/logo.imageset/logo.png"), os.path.join(MEDIA, "logo.png"))

    # 标题栏、工具栏的图标：渲染成 72px 的 PNG（24vp × 3）
    with tempfile.TemporaryDirectory() as tmp:
        for name in MENU:
            src = os.path.join(MEDIA, f"lucide_{name.replace('-', '_')}_accent.svg")
            page = os.path.join(tmp, "p.html")
            with open(page, "w") as f:
                f.write(f'<!doctype html><html><body style="margin:0;background:transparent">'
                        f'<img src="file://{src}" style="display:block;width:72px;height:72px"></body></html>')
            subprocess.run([CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars", "--default-background-color=00000000",
                            f"--user-data-dir={tmp}/profile", "--allow-file-access-from-files", "--window-size=72,72",
                            f"--screenshot={os.path.join(MEDIA, 'menu_' + name.replace('-', '_') + '.png')}", f"file://{page}"],
                           check=True, capture_output=True)

    term = os.path.join(RAW, "terminal")
    os.makedirs(term, exist_ok=True)
    xterm = os.path.join(REPO, "node_modules/@xterm/xterm")
    shutil.copy(os.path.join(xterm, "lib/xterm.js"), os.path.join(term, "xterm.js"))
    shutil.copy(os.path.join(xterm, "css/xterm.css"), os.path.join(term, "xterm.css"))
    print(f"{len(LUCIDE)} 个 lucide 图标 × {len(COLORS)} 种颜色、{len(PLATFORMS)} 个系统图标 → {MEDIA}")


main()

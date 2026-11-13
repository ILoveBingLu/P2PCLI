#!/usr/bin/env python3
"""生成 iOS 的资源目录（P2PCLI/Assets.xcassets）：和桌面、安卓同一套图标。

在仓库根目录跑：python3 mobile/ios/tools/make_assets.py（要先 pnpm install，要本机装了 Chrome）。
- lucide 图标（ISC 许可）：从 node_modules/lucide-react 转成 SVG，当模板图用，颜色跟着界面走。
  要加图标就在 LUCIDE 里加一个名字，再跑一遍。
- 系统图标（苹果、Windows、Linux、安卓、鸿蒙）：直接用桌面的 SVG。
- 各家 AI 的图标：用了遮罩和渐变，用 Chrome 渲染成 PNG（和安卓一样）。
- 软件里的 logo：Mac 版的液态玻璃图标（app/src-tauri/icons/AppIcon.icon），用 Xcode 的 ictool 渲染；
  没装 Xcode 的电脑上保留已经生成好的那张。
App 图标不在这里：工程直接用 AppIcon.icon（见 project.yml），和 Mac 版同一个文件。
"""
import json
import os
import re
import shutil
import subprocess
import tempfile

REPO = os.getcwd()
OUT = os.path.join(REPO, "mobile/ios/P2PCLI/Assets.xcassets")
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
ICTOOL = "/Applications/Xcode.app/Contents/Applications/Icon Composer.app/Contents/Executables/ictool"
ICON = os.path.join(REPO, "app/src-tauri/icons/AppIcon.icon")

LUCIDE = [
    "plus", "refresh-cw", "scan-line", "square-terminal", "x", "ellipsis", "file-up", "trash", "unplug",
    "send-horizontal", "keyboard", "triangle-alert", "square-arrow-out-up-right", "key-round", "arrow-up",
    "arrow-down", "arrow-left", "arrow-right", "bot", "plug", "settings", "copy", "check", "info",
    "clipboard-paste", "chevron-right", "image", "undo-2", "folder-open", "languages", "circle-check", "lock-keyhole", "flashlight", "folder", "house", "corner-down-left", "play", "circle-pause", "repeat-2", "pencil-line", "keyboard-off",
]
PLATFORMS = ["apple", "windows", "linux", "android", "harmonyos"]

CONTENTS_INFO = {"author": "xcode", "version": 1}


def write_json(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        json.dump(data, f, indent=2, ensure_ascii=False)
        f.write("\n")


def vector_set(name, svg, template=True):
    """一个矢量图：放大缩小都清楚；模板图的颜色由界面决定"""
    d = os.path.join(OUT, f"{name}.imageset")
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, f"{name}.svg"), "w") as f:
        f.write(svg)
    props = {"preserves-vector-representation": True}
    if template:
        props["template-rendering-intent"] = "template"
    write_json(os.path.join(d, "Contents.json"), {
        "images": [{"filename": f"{name}.svg", "idiom": "universal"}],
        "info": CONTENTS_INFO,
        "properties": props,
    })


def png_set(name, png):
    d = os.path.join(OUT, f"{name}.imageset")
    os.makedirs(d, exist_ok=True)
    shutil.copy(png, os.path.join(d, f"{name}.png"))
    write_json(os.path.join(d, "Contents.json"), {
        "images": [{"filename": f"{name}.png", "idiom": "universal", "scale": "3x"}],
        "info": CONTENTS_INFO,
    })


def num(a, k):
    return float(a.get(k, 0))


def to_path(tag, a):
    """lucide 的 rect、circle 这些都换成 path，和安卓那份转换一样"""
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


def lucide_svg(name):
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
    body = "".join(f'<path d="{p}"/>' for p in paths)
    return ('<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" '
            'stroke="#000000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">' + body + "</svg>\n")


def render(html, size, out):
    """用 Chrome 把一段 HTML 截成 PNG"""
    with tempfile.TemporaryDirectory() as tmp:
        page = os.path.join(tmp, "page.html")
        with open(page, "w") as f:
            f.write(html)
        subprocess.run([CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars",
                        "--default-background-color=00000000", f"--user-data-dir={tmp}/profile",
                        "--allow-file-access-from-files", f"--window-size={size},{size}",
                        f"--screenshot={out}", f"file://{page}"], check=True, capture_output=True)


def color_set(name, hex_color):
    r, g, b = (int(hex_color[i:i + 2], 16) / 255 for i in (1, 3, 5))
    write_json(os.path.join(OUT, f"{name}.colorset", "Contents.json"), {
        "colors": [{"idiom": "universal", "color": {"color-space": "srgb", "components": {
            "alpha": "1.000", "red": f"{r:.3f}", "green": f"{g:.3f}", "blue": f"{b:.3f}"}}}],
        "info": CONTENTS_INFO,
    })


def main():
    # 没有 ictool 时沿用上次渲染的 logo
    old_logo = os.path.join(OUT, "logo.imageset", "logo.png")
    kept_logo = open(old_logo, "rb").read() if os.path.exists(old_logo) else None
    if os.path.isdir(OUT):
        shutil.rmtree(OUT)
    write_json(os.path.join(OUT, "Contents.json"), {"info": CONTENTS_INFO})

    for name in LUCIDE:
        vector_set(f"lucide.{name}", lucide_svg(name))
    for name in PLATFORMS:
        svg = open(os.path.join(REPO, f"app/src/assets/platforms/{name}.svg")).read()
        # 桌面那份有的写死了 1em 大小；去掉，按 viewBox 算
        svg = re.sub(r'\s(width|height)="1em"', "", svg)
        vector_set(f"platform.{name}", svg)

    with tempfile.TemporaryDirectory() as tmp:
        def img_page(src, size):
            return (f'<!doctype html><html><body style="margin:0;background:transparent">'
                    f'<img src="file://{src}" style="display:block;width:{size}px;height:{size}px"></body></html>')

        # 软件里的 logo：72pt 以内都清楚
        if os.path.exists(ICTOOL):
            subprocess.run([ICTOOL, ICON, "--export-image", "--output-file", f"{tmp}/logo.png", "--platform", "iOS",
                            "--rendition", "Default", "--width", "216", "--height", "216", "--scale", "1"],
                           check=True, capture_output=True)
        elif kept_logo:
            with open(f"{tmp}/logo.png", "wb") as f:
                f.write(kept_logo)
        else:
            raise SystemExit("找不到 ictool（要 Xcode 26 以上），也没有之前生成的 logo")
        png_set("logo", f"{tmp}/logo.png")
        agents = os.path.join(REPO, "app/src/assets/agents")
        for f in sorted(os.listdir(agents)):
            if f.endswith(".svg"):
                kind = f[:-4]
                render(img_page(os.path.join(agents, f), 96), 96, f"{tmp}/{kind}.png")
                png_set(f"agent.{kind}", f"{tmp}/{kind}.png")


    color_set("AccentColor", "#0485F7")
    color_set("LaunchBackground", "#060607")
    print(f"{len(LUCIDE)} 个 lucide 图标、{len(PLATFORMS)} 个系统图标 → {OUT}")


main()

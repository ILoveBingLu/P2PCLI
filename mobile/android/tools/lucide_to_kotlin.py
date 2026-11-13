#!/usr/bin/env python3
"""把桌面端用的 lucide 图标（node_modules/lucide-react）转成 Compose 的 ImageVector：
python3 mobile/android/tools/lucide_to_kotlin.py  （在仓库根目录跑，要先 pnpm install）
要加图标就在 NAMES 里加一行，再跑一遍。"""
import re

NAMES = {
    "ChevronRight": "chevron-right", "ChevronLeft": "chevron-left", "Plus": "plus", "RefreshCw": "refresh-cw",
    "ScanLine": "scan-line", "SquareTerminal": "square-terminal", "X": "x", "Ellipsis": "ellipsis",
    "EllipsisVertical": "ellipsis-vertical", "FileUp": "file-up", "Trash2": "trash", "Unplug": "unplug",
    "SendHorizontal": "send-horizontal", "Keyboard": "keyboard", "TriangleAlert": "triangle-alert",
    "LoaderCircle": "loader-circle", "SquareArrowOutUpRight": "square-arrow-out-up-right", "KeyRound": "key-round",
    "LockKeyhole": "lock-keyhole", "ArrowUp": "arrow-up", "ArrowDown": "arrow-down", "Bot": "bot", "Plug": "plug",
    "Settings": "settings", "Copy": "copy", "Check": "check", "Info": "info",
}
OUT = "mobile/android/app/src/main/java/io/github/ilovebinglu/p2pcli/ui/Lucide.kt"


def num(a, k):
    return float(a.get(k, 0))


def to_path(tag, a):
    """lucide 的 rect、circle 这些都换成 path"""
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


lines = []
for kname, name in NAMES.items():
    src = open(f"node_modules/lucide-react/dist/esm/icons/{name}.mjs").read()
    node = src[src.index("node: ["):src.index("};", src.index("node: ["))]
    paths = []
    for m in re.finditer(r'\["(\w+)", \{([^}]*)\}\]', node):
        attrs = dict(re.findall(r'(\w+): "([^"]*)"', m.group(2)))
        attrs.pop("key", None)
        paths.append(to_path(m.group(1), attrs))
    args = ", ".join(f'"{p}"' for p in paths)
    lines.append(f'    val {kname}: ImageVector by lazy {{ icon("{name}", {args}) }}')

open(OUT, "w").write('''package io.github.ilovebinglu.p2pcli.ui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.dp

/**
 * 和桌面端同一套图标：lucide（ISC 许可），路径由 mobile/android/tools/lucide_to_kotlin.py 从 lucide-react 生成，别手改。
 * 线宽 2、圆头圆角，颜色由 Icon 的 tint 决定。
 */
object Lucide {
    private fun icon(name: String, vararg paths: String): ImageVector {
        val b = ImageVector.Builder(name = name, defaultWidth = 24.dp, defaultHeight = 24.dp, viewportWidth = 24f, viewportHeight = 24f)
        for (d in paths) {
            b.addPath(
                pathData = PathParser().parsePathString(d).toNodes(),
                fill = null,
                stroke = SolidColor(Color.Black),
                strokeLineWidth = 2f,
                strokeLineCap = StrokeCap.Round,
                strokeLineJoin = StrokeJoin.Round,
            )
        }
        return b.build()
    }

''' + "\n".join(lines) + "\n}\n")
print(f"{len(lines)} 个图标 → {OUT}")

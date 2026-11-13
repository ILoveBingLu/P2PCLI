package io.github.ilovebinglu.p2pcli.ui

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

    val ChevronRight: ImageVector by lazy { icon("chevron-right", "m9 18 6-6-6-6") }
    val ChevronLeft: ImageVector by lazy { icon("chevron-left", "m15 18-6-6 6-6") }
    val Plus: ImageVector by lazy { icon("plus", "M5 12h14", "M12 5v14") }
    val RefreshCw: ImageVector by lazy { icon("refresh-cw", "M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8", "M21 3v5h-5", "M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16", "M8 16H3v5") }
    val ScanLine: ImageVector by lazy { icon("scan-line", "M3 7V5a2 2 0 0 1 2-2h2", "M17 3h2a2 2 0 0 1 2 2v2", "M21 17v2a2 2 0 0 1-2 2h-2", "M7 21H5a2 2 0 0 1-2-2v-2", "M7 12h10") }
    val SquareTerminal: ImageVector by lazy { icon("square-terminal", "m7 11 2-2-2-2", "M11 13h4", "M5.0,3.0h14.0a2.0,2.0 0 0 1 2.0,2.0v14.0a2.0,2.0 0 0 1 -2.0,2.0h-14.0a2.0,2.0 0 0 1 -2.0,-2.0v-14.0a2.0,2.0 0 0 1 2.0,-2.0Z") }
    val X: ImageVector by lazy { icon("x", "M18 6 6 18", "m6 6 12 12") }
    val Ellipsis: ImageVector by lazy { icon("ellipsis", "M11.0,12.0a1.0,1.0 0 1,0 2.0,0a1.0,1.0 0 1,0 -2.0,0", "M18.0,12.0a1.0,1.0 0 1,0 2.0,0a1.0,1.0 0 1,0 -2.0,0", "M4.0,12.0a1.0,1.0 0 1,0 2.0,0a1.0,1.0 0 1,0 -2.0,0") }
    val EllipsisVertical: ImageVector by lazy { icon("ellipsis-vertical", "M11.0,12.0a1.0,1.0 0 1,0 2.0,0a1.0,1.0 0 1,0 -2.0,0", "M11.0,5.0a1.0,1.0 0 1,0 2.0,0a1.0,1.0 0 1,0 -2.0,0", "M11.0,19.0a1.0,1.0 0 1,0 2.0,0a1.0,1.0 0 1,0 -2.0,0") }
    val FileUp: ImageVector by lazy { icon("file-up", "M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z", "M14 2v5a1 1 0 0 0 1 1h5", "M12 12v6", "m15 15-3-3-3 3") }
    val Trash2: ImageVector by lazy { icon("trash", "M10 11v6", "M14 11v6", "M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6", "M3 6h18", "M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2") }
    val Unplug: ImageVector by lazy { icon("unplug", "m19 5 3-3", "m2 22 3-3", "M6.3 20.3a2.4 2.4 0 0 0 3.4 0L12 18l-6-6-2.3 2.3a2.4 2.4 0 0 0 0 3.4Z", "M7.5 13.5 10 11", "M10.5 16.5 13 14", "m12 6 6 6 2.3-2.3a2.4 2.4 0 0 0 0-3.4l-2.6-2.6a2.4 2.4 0 0 0-3.4 0Z") }
    val SendHorizontal: ImageVector by lazy { icon("send-horizontal", "M3.714 3.048a.498.498 0 0 0-.683.627l2.843 7.627a2 2 0 0 1 0 1.396l-2.842 7.627a.498.498 0 0 0 .682.627l18-8.5a.5.5 0 0 0 0-.904z", "M6 12h16") }
    val Keyboard: ImageVector by lazy { icon("keyboard", "M10 8h.01", "M12 12h.01", "M14 8h.01", "M16 12h.01", "M18 8h.01", "M6 8h.01", "M7 16h10", "M8 12h.01", "M4.0,4.0h16.0a2.0,2.0 0 0 1 2.0,2.0v12.0a2.0,2.0 0 0 1 -2.0,2.0h-16.0a2.0,2.0 0 0 1 -2.0,-2.0v-12.0a2.0,2.0 0 0 1 2.0,-2.0Z") }
    val TriangleAlert: ImageVector by lazy { icon("triangle-alert", "m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3", "M12 9v4", "M12 17h.01") }
    val LoaderCircle: ImageVector by lazy { icon("loader-circle", "M21 12a9 9 0 1 1-6.219-8.56") }
    val SquareArrowOutUpRight: ImageVector by lazy { icon("square-arrow-out-up-right", "M21 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h6", "m21 3-9 9", "M15 3h6v6") }
    val KeyRound: ImageVector by lazy { icon("key-round", "M2.586 17.414A2 2 0 0 0 2 18.828V21a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h1a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h.172a2 2 0 0 0 1.414-.586l.814-.814a6.5 6.5 0 1 0-4-4z", "M16.0,7.5a0.5,0.5 0 1,0 1.0,0a0.5,0.5 0 1,0 -1.0,0") }
    val LockKeyhole: ImageVector by lazy { icon("lock-keyhole", "M11.0,16.0a1.0,1.0 0 1,0 2.0,0a1.0,1.0 0 1,0 -2.0,0", "M5.0,10.0h14.0a2.0,2.0 0 0 1 2.0,2.0v8.0a2.0,2.0 0 0 1 -2.0,2.0h-14.0a2.0,2.0 0 0 1 -2.0,-2.0v-8.0a2.0,2.0 0 0 1 2.0,-2.0Z", "M7 10V7a5 5 0 0 1 10 0v3") }
    val ArrowUp: ImageVector by lazy { icon("arrow-up", "m5 12 7-7 7 7", "M12 19V5") }
    val ArrowDown: ImageVector by lazy { icon("arrow-down", "M12 5v14", "m19 12-7 7-7-7") }
    val Bot: ImageVector by lazy { icon("bot", "M12 8V4H8", "M6.0,8.0h12.0a2.0,2.0 0 0 1 2.0,2.0v8.0a2.0,2.0 0 0 1 -2.0,2.0h-12.0a2.0,2.0 0 0 1 -2.0,-2.0v-8.0a2.0,2.0 0 0 1 2.0,-2.0Z", "M2 14h2", "M20 14h2", "M15 13v2", "M9 13v2") }
    val Plug: ImageVector by lazy { icon("plug", "M12 22v-5", "M15 8V2", "M17 8a1 1 0 0 1 1 1v4a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V9a1 1 0 0 1 1-1z", "M9 8V2") }
    val Settings: ImageVector by lazy { icon("settings", "M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915", "M9.0,12.0a3.0,3.0 0 1,0 6.0,0a3.0,3.0 0 1,0 -6.0,0") }
    val Copy: ImageVector by lazy { icon("copy", "M10.0,8.0h10.0a2.0,2.0 0 0 1 2.0,2.0v10.0a2.0,2.0 0 0 1 -2.0,2.0h-10.0a2.0,2.0 0 0 1 -2.0,-2.0v-10.0a2.0,2.0 0 0 1 2.0,-2.0Z", "M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2") }
    val Check: ImageVector by lazy { icon("check", "M20 6 9 17l-5-5") }
    val Info: ImageVector by lazy { icon("info", "M2.0,12.0a10.0,10.0 0 1,0 20.0,0a10.0,10.0 0 1,0 -20.0,0", "M12 16v-4", "M12 8h.01") }
    val ArrowLeft: ImageVector by lazy { icon("arrow-left", "m12 19-7-7 7-7", "M19 12H5") }
    val ArrowRight: ImageVector by lazy { icon("arrow-right", "M5 12h14", "m12 5 7 7-7 7") }
    val ArrowBigUp: ImageVector by lazy { icon("arrow-big-up", "M9 19a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1v-6a1 1 0 0 1 1-1h3.293a.707.707 0 0 0 .5-1.207l-7.086-7.086a1 1 0 0 0-1.414 0l-7.086 7.086a.707.707 0 0 0 .5 1.207H8a1 1 0 0 1 1 1z") }
    val ClipboardPaste: ImageVector by lazy { icon("clipboard-paste", "M11 14h10", "M16 4h2a2 2 0 0 1 2 2v1.344", "m17 18 4-4-4-4", "M8 4H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 1.793-1.113", "M9.0,2.0h6.0a1.0,1.0 0 0 1 1.0,1.0v2.0a1.0,1.0 0 0 1 -1.0,1.0h-6.0a1.0,1.0 0 0 1 -1.0,-1.0v-2.0a1.0,1.0 0 0 1 1.0,-1.0Z") }
    val PanelLeftClose: ImageVector by lazy { icon("panel-left-close", "M5.0,3.0h14.0a2.0,2.0 0 0 1 2.0,2.0v14.0a2.0,2.0 0 0 1 -2.0,2.0h-14.0a2.0,2.0 0 0 1 -2.0,-2.0v-14.0a2.0,2.0 0 0 1 2.0,-2.0Z", "M9 3v18", "m16 15-3-3 3-3") }
    val PanelLeftOpen: ImageVector by lazy { icon("panel-left-open", "M5.0,3.0h14.0a2.0,2.0 0 0 1 2.0,2.0v14.0a2.0,2.0 0 0 1 -2.0,2.0h-14.0a2.0,2.0 0 0 1 -2.0,-2.0v-14.0a2.0,2.0 0 0 1 2.0,-2.0Z", "M9 3v18", "m14 9 3 3-3 3") }
    val MousePointerClick: ImageVector by lazy { icon("mouse-pointer-click", "M14 4.1 12 6", "m5.1 8-2.9-.8", "m6 12-1.9 2", "M7.2 2.2 8 5.1", "M9.037 9.69a.498.498 0 0 1 .653-.653l11 4.5a.5.5 0 0 1-.074.949l-4.349 1.041a1 1 0 0 0-.74.739l-1.04 4.35a.5.5 0 0 1-.95.074z") }
    val Folder: ImageVector by lazy { icon("folder", "M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z") }
    val FolderOpen: ImageVector by lazy { icon("folder-open", "m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2") }
    val House: ImageVector by lazy { icon("house", "M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8", "M3 10a2 2 0 0 1 .709-1.528l7-6a2 2 0 0 1 2.582 0l7 6A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z") }
    val CornerDownLeft: ImageVector by lazy { icon("corner-down-left", "M20 4v7a4 4 0 0 1-4 4H4", "m9 10-5 5 5 5") }
    val Play: ImageVector by lazy { icon("play", "M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z") }
}

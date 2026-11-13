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
    val FileUp: ImageVector by lazy { icon("file-up", "M14 2v5a1 1 0 0 0 1 1h5", "M12 12v6", "m15 15-3-3-3 3") }
    val Trash2: ImageVector by lazy { icon("trash", "M10 11v6", "M14 11v6", "M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6", "M3 6h18", "M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2") }
    val Unplug: ImageVector by lazy { icon("unplug", "m19 5 3-3", "m2 22 3-3", "M7.5 13.5 10 11", "M10.5 16.5 13 14") }
    val SendHorizontal: ImageVector by lazy { icon("send-horizontal", "M6 12h16") }
    val Keyboard: ImageVector by lazy { icon("keyboard", "M10 8h.01", "M12 12h.01", "M14 8h.01", "M16 12h.01", "M18 8h.01", "M6 8h.01", "M7 16h10", "M8 12h.01", "M4.0,4.0h16.0a2.0,2.0 0 0 1 2.0,2.0v12.0a2.0,2.0 0 0 1 -2.0,2.0h-16.0a2.0,2.0 0 0 1 -2.0,-2.0v-12.0a2.0,2.0 0 0 1 2.0,-2.0Z") }
    val TriangleAlert: ImageVector by lazy { icon("triangle-alert", "M12 9v4", "M12 17h.01") }
    val LoaderCircle: ImageVector by lazy { icon("loader-circle", "M21 12a9 9 0 1 1-6.219-8.56") }
    val SquareArrowOutUpRight: ImageVector by lazy { icon("square-arrow-out-up-right", "M21 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h6", "m21 3-9 9", "M15 3h6v6") }
    val KeyRound: ImageVector by lazy { icon("key-round", "M16.0,7.5a0.5,0.5 0 1,0 1.0,0a0.5,0.5 0 1,0 -1.0,0") }
    val LockKeyhole: ImageVector by lazy { icon("lock-keyhole", "M11.0,16.0a1.0,1.0 0 1,0 2.0,0a1.0,1.0 0 1,0 -2.0,0", "M5.0,10.0h14.0a2.0,2.0 0 0 1 2.0,2.0v8.0a2.0,2.0 0 0 1 -2.0,2.0h-14.0a2.0,2.0 0 0 1 -2.0,-2.0v-8.0a2.0,2.0 0 0 1 2.0,-2.0Z", "M7 10V7a5 5 0 0 1 10 0v3") }
    val ArrowUp: ImageVector by lazy { icon("arrow-up", "m5 12 7-7 7 7", "M12 19V5") }
    val ArrowDown: ImageVector by lazy { icon("arrow-down", "M12 5v14", "m19 12-7 7-7-7") }
    val Bot: ImageVector by lazy { icon("bot", "M12 8V4H8", "M6.0,8.0h12.0a2.0,2.0 0 0 1 2.0,2.0v8.0a2.0,2.0 0 0 1 -2.0,2.0h-12.0a2.0,2.0 0 0 1 -2.0,-2.0v-8.0a2.0,2.0 0 0 1 2.0,-2.0Z", "M2 14h2", "M20 14h2", "M15 13v2", "M9 13v2") }
    val Plug: ImageVector by lazy { icon("plug", "M12 22v-5", "M15 8V2", "M9 8V2") }
}

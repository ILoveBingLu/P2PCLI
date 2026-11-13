package io.github.ilovebinglu.p2pcli.ui

import android.os.Build
import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import uniffi.p2pcli_mobile.AgentState
import uniffi.p2pcli_mobile.AgentView
import uniffi.p2pcli_mobile.DeviceView
import uniffi.p2pcli_mobile.PathKind
import uniffi.p2pcli_mobile.PathView
import uniffi.p2pcli_mobile.Presence
import uniffi.p2pcli_mobile.SessionView

/** 安卓 12 以上跟着壁纸取色；深浅跟系统 */
@Composable
fun P2pTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val context = LocalContext.current
    val colors = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S -> if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        dark -> darkColorScheme()
        else -> lightColorScheme()
    }
    MaterialTheme(colorScheme = colors, content = content)
}

/** 直连绿、中继黄 */
val Ok = Color(0xFF22A06B)
val Warn = Color(0xFFE2A400)

/** 设备图标：系统的缩写放在圆角方块里 */
@Composable
fun PlatformBadge(platform: String, connected: Boolean = false) {
    val label = when (platform) {
        "macos" -> "mac"
        "windows" -> "Win"
        "linux" -> "Linux"
        "android" -> "And"
        "ios" -> "iOS"
        else -> platform.take(3)
    }
    Box(contentAlignment = Alignment.Center) {
        Box(
            Modifier.size(44.dp).background(MaterialTheme.colorScheme.secondaryContainer, RoundedCornerShape(14.dp)),
            contentAlignment = Alignment.Center,
        ) {
            Text(label, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, color = MaterialTheme.colorScheme.onSecondaryContainer)
        }
        if (connected) {
            Box(
                Modifier.align(Alignment.BottomEnd).size(12.dp)
                    .background(MaterialTheme.colorScheme.surface, RoundedCornerShape(6.dp)),
                contentAlignment = Alignment.Center,
            ) { Box(Modifier.size(8.dp).background(Ok, RoundedCornerShape(4.dp))) }
        }
    }
}

fun pathText(path: PathView?): String {
    if (path == null) return "连接中"
    val kind = when (path.kind) {
        PathKind.IPV6 -> "IPv6 直连"
        PathKind.IPV4 -> "IPv4 直连"
        PathKind.RELAY -> "中继"
        PathKind.LOCAL -> "本机"
        PathKind.UNKNOWN -> return "连接中"
    }
    return path.rttMs?.let { "$kind · $it ms" } ?: kind
}

/** 设备一行的状态：文字和颜色 */
fun presenceOf(device: DeviceView, connected: Boolean): Pair<String, Color?> = when {
    connected -> "已连接" to Ok
    device.presence == Presence.ONLINE -> {
        val rtt = device.path?.rttMs?.let { " · $it ms" } ?: ""
        (if (device.lanOnline) "局域网在线$rtt" else "在线$rtt") to (if (device.path?.kind == PathKind.RELAY) Warn else Ok)
    }
    device.presence == Presence.CHECKING -> "检测中…" to null
    device.presence == Presence.OLD_VERSION -> "对方需要更新 P2PCLI" to Warn
    else -> "离线" to null
}

fun agentText(agent: AgentView): String = when {
    agent.stalled -> "可能卡住"
    agent.state == AgentState.WORKING -> "干活中"
    agent.state == AgentState.WAITING -> "等你确认"
    else -> "空闲"
}

fun agentColor(agent: AgentView): Color? = when {
    agent.stalled || agent.state == AgentState.WAITING -> Warn
    agent.state == AgentState.WORKING -> Color(0xFF3B82F6)
    else -> null
}

/** 会话的名字：有备注用备注；跑着 AI 用 AI 的名字（同名的加编号）；不然按创建先后叫“会话 1、会话 2” */
fun sessionTitles(sessions: List<SessionView>): Map<String, String> {
    val numbers = sessions.sortedWith(compareBy({ it.createdAt }, { it.id }))
        .mapIndexed { i, s -> s.id to "会话 ${i + 1}" }.toMap()
    val agentCounts = sessions.mapNotNull { it.agent?.name }.groupingBy { it }.eachCount()
    return sessions.associate { s ->
        val number = numbers[s.id] ?: "会话"
        val agent = s.agent
        s.id to when {
            !s.remark.isNullOrBlank() -> s.remark!!
            agent == null -> number
            (agentCounts[agent.name] ?: 0) > 1 -> "${agent.name} · $number"
            else -> agent.name
        }
    }
}

/** 刚刚、3 分钟前、2 小时前、5 天前 */
fun relativeTime(secs: ULong): String {
    val diff = System.currentTimeMillis() / 1000 - secs.toLong()
    return when {
        diff < 60 -> "刚刚"
        diff < 3600 -> "${diff / 60} 分钟前"
        diff < 86400 -> "${diff / 3600} 小时前"
        else -> "${diff / 86400} 天前"
    }
}

/** /Users/me/Projects/x → ~/Projects/x，太长只留最后两段 */
fun shortPath(path: String?): String {
    if (path.isNullOrBlank()) return ""
    val home = path.replace(Regex("^/(Users|home)/[^/]+"), "~").replace(Regex("^[A-Za-z]:\\\\Users\\\\[^\\\\]+"), "~")
    val parts = home.split('/', '\\').filter { it.isNotEmpty() }
    return if (parts.size > 3) "…/" + parts.takeLast(2).joinToString("/") else home
}

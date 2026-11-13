package io.github.ilovebinglu.p2pcli.ui

import io.github.ilovebinglu.p2pcli.t
import io.github.ilovebinglu.p2pcli.tr
import androidx.compose.ui.graphics.Color
import uniffi.p2pcli_mobile.AgentState
import uniffi.p2pcli_mobile.AgentView
import uniffi.p2pcli_mobile.DeviceView
import uniffi.p2pcli_mobile.PathKind
import uniffi.p2pcli_mobile.PathView
import uniffi.p2pcli_mobile.Presence
import uniffi.p2pcli_mobile.SessionView

fun pathText(path: PathView?): String {
    if (path == null) return t("连接中")
    val kind = when (path.kind) {
        PathKind.IPV6 -> t("IPv6 直连")
        PathKind.IPV4 -> t("IPv4 直连")
        PathKind.RELAY -> t("中继")
        PathKind.LOCAL -> t("本机")
        PathKind.UNKNOWN -> return t("连接中")
    }
    return path.rttMs?.let { "$kind · $it ms" } ?: kind
}

/** 设备一行的状态：文字和颜色 */
fun presenceOf(device: DeviceView, connected: Boolean): Pair<String, Color?> = when {
    connected -> t("已连接") to Hero.Success
    device.presence == Presence.ONLINE -> {
        val rtt = device.path?.rttMs?.let { " · $it ms" } ?: ""
        (if (device.lanOnline) t("局域网在线") + rtt else t("在线") + rtt) to (if (device.path?.kind == PathKind.RELAY) Hero.Warning else Hero.Success)
    }
    device.presence == Presence.CHECKING -> t("检测中…") to null
    device.presence == Presence.OLD_VERSION -> t("对方需要更新 P2PCLI") to Hero.Warning
    else -> t("离线") to null
}

fun agentText(agent: AgentView): String = when {
    agent.stalled -> t("可能卡住")
    agent.state == AgentState.WORKING -> t("干活中")
    agent.state == AgentState.WAITING -> t("等你确认")
    else -> t("空闲")
}

fun agentColor(agent: AgentView): Color? = when {
    agent.stalled || agent.state == AgentState.WAITING -> Hero.Warning
    agent.state == AgentState.WORKING -> Hero.Accent
    else -> null
}

/** 会话的名字：有备注用备注；跑着 AI 用 AI 的名字（同名的加编号）；不然按创建先后叫“会话 1、会话 2” */
fun sessionTitles(sessions: List<SessionView>): Map<String, String> {
    val numbers = sessions.sortedWith(compareBy({ it.createdAt }, { it.id }))
        .mapIndexed { i, s -> s.id to t("会话 {{value}}", "value" to i + 1) }.toMap()
    val agentCounts = sessions.mapNotNull { it.agent?.name }.groupingBy { it }.eachCount()
    return sessions.associate { s ->
        val number = numbers[s.id] ?: t("会话")
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
        diff < 60 -> t("刚刚")
        diff < 3600 -> t("{{value}} 分钟前", "value" to diff / 60)
        diff < 86400 -> t("{{value}} 小时前", "value" to diff / 3600)
        else -> t("{{value}} 天前", "value" to diff / 86400)
    }
}

/** /Users/me/Projects/x → ~/Projects/x，太长只留最后两段 */
fun shortPath(path: String?): String {
    if (path.isNullOrBlank()) return ""
    val home = path.replace(Regex("^/(Users|home)/[^/]+"), "~").replace(Regex("^[A-Za-z]:\\\\Users\\\\[^\\\\]+"), "~")
    val parts = home.split('/', '\\').filter { it.isNotEmpty() }
    return if (parts.size > 3) "…/" + parts.takeLast(2).joinToString("/") else home
}

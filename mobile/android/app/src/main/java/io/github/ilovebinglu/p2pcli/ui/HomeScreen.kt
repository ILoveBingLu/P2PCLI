package io.github.ilovebinglu.p2pcli.ui

import io.github.ilovebinglu.p2pcli.t
import io.github.ilovebinglu.p2pcli.tr
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.ilovebinglu.p2pcli.ConnectPrompt
import io.github.ilovebinglu.p2pcli.Core
import io.github.ilovebinglu.p2pcli.R
import io.github.ilovebinglu.p2pcli.Screen
import io.github.ilovebinglu.p2pcli.terminal.RemoteTerminal
import uniffi.p2pcli_mobile.AgentState
import uniffi.p2pcli_mobile.DeviceView
import uniffi.p2pcli_mobile.ServerState
import uniffi.p2pcli_mobile.SessionView
import uniffi.p2pcli_mobile.TransferState
import uniffi.p2pcli_mobile.TransferView

@Composable
fun HomeScreen(core: Core, selected: Screen? = null) {
    val overview by core.overview.collectAsStateWithLifecycle()
    val conns by core.conns.collectAsStateWithLifecycle()
    val terms by core.terms.collectAsStateWithLifecycle()
    val sessions by core.sessions.collectAsStateWithLifecycle()
    val connecting by core.connecting.collectAsStateWithLifecycle()
    val connectingName by core.connectingName.collectAsStateWithLifecycle()
    val transfers by core.transfers.collectAsStateWithLifecycle()
    val devices = overview?.devices.orEmpty()
    val nearby = overview?.nearby.orEmpty()
    val empty = overview != null && devices.isEmpty() && nearby.isEmpty()
    // 宽屏时右边正显示哪台设备、哪个终端：列表里标出来
    val selectedTerm = (selected as? Screen.Term)?.key
    val selectedDevice = when (selected) {
        is Screen.Device -> selected.endpointId
        is Screen.Term -> terms.firstOrNull { it.key == selected.key }?.endpointId
        else -> null
    }

    Column(Modifier.fillMaxSize().background(Hero.Background).statusBarsPadding()) {
        // 大标题
        Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 8.dp, top = 14.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Image(painterResource(R.drawable.logo), null, Modifier.size(30.dp))
            Text("P2PCLI", color = Hero.Foreground, fontSize = 26.sp, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f).padding(start = 10.dp))
            HeroIconButton(Lucide.ScanLine, t("扫码连接"), { core.openScan() })
            HeroIconButton(Lucide.Settings, t("设置"), { core.openSettings() })
        }
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 28.dp)) {
            transfersSection(transfers)
            // 扫码连的设备列表里还没有：顶上放一张“正在连接”的卡片
            val pending = connecting
            if (pending != null && devices.none { it.endpointId == pending } && nearby.none { it.endpointId == pending }) {
                item {
                    Row(
                        Modifier.fillMaxWidth().padding(top = 10.dp).clip(RoundedCornerShape(16.dp)).background(Hero.Surface).padding(horizontal = 14.dp, vertical = 12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        Box(Modifier.size(36.dp).clip(RoundedCornerShape(18.dp)).background(Hero.AccentSoft), contentAlignment = Alignment.Center) { Spinner(18.dp, Hero.Accent) }
                        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                            RowTitle(t("正在连接 {{name}}", "name" to (connectingName ?: t("电脑"))))
                            SubText(t("等对方在电脑上点同意"))
                        }
                    }
                }
            }
            when (val server = overview?.server) {
                is ServerState.Offline, ServerState.None -> item {
                    Banner(if (server is ServerState.None) t("还没配置牵线服务器，只能连同一个局域网里的设备") else t("牵线服务器没连上，只能连同一个局域网里的设备"))
                }
                else -> {}
            }

            if (empty) {
                item {
                    EmptyMessage(t("连接你的电脑"), t("在电脑上打开 P2PCLI，点标题栏的手机图标，用这里扫它显示的二维码。")) {
                        HeroButton(t("扫码连接"), { core.openScan() }, Modifier.fillMaxWidth(), icon = Lucide.ScanLine)
                        HeroButton(t("输入设备码"), { core.prompt.value = ConnectPrompt() }, Modifier.fillMaxWidth(), kind = ButtonKind.Secondary, icon = Lucide.KeyRound)
                        HeroButton(t("找找附近的设备"), { core.scanNearby() }, Modifier.fillMaxWidth(), kind = ButtonKind.Ghost, icon = Lucide.RefreshCw)
                    }
                }
                return@LazyColumn
            }

            if (terms.isNotEmpty()) {
                item { SectionHeader(t("正在使用")) }
                item {
                    Group {
                        terms.forEachIndexed { i, term ->
                            if (i > 0) GroupDivider()
                            val list = sessions[term.endpointId]?.list.orEmpty()
                            val agent = list.firstOrNull { it.id == term.sessionId }?.agent?.takeIf { term.state == RemoteTerminal.State.OPEN }
                            val device = devices.firstOrNull { it.endpointId == term.endpointId }
                            HeroRow(
                                onClick = { core.openTerminal(term.endpointId, term.sessionId) },
                                selected = term.key == selectedTerm,
                                leading = {
                                    Tile(dimmed = term.state == RemoteTerminal.State.CLOSED) {
                                        if (agent != null) AgentIcon(agent.kind) else Icon(Lucide.SquareTerminal, null, Modifier.size(20.dp), tint = Hero.Foreground)
                                    }
                                },
                                title = { RowTitle(term.sessionId?.let { sessionTitles(list)[it] } ?: t("新终端"), modifier = Modifier.weight(1f, fill = false)) },
                                subtitle = {
                                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                                        SubText((device?.remark ?: device?.name ?: "") + if (term.state == RemoteTerminal.State.CLOSED) " · " + t("已断开") else "")
                                        agent?.let { AgentStateLabel(it) }
                                    }
                                },
                            )
                        }
                    }
                }
            }

            if (devices.isNotEmpty()) {
                item { SectionHeader(t("我的设备")) }
                item {
                    Group {
                        devices.forEachIndexed { i, d ->
                            if (i > 0) GroupDivider()
                            DeviceRow(d, conns.containsKey(d.endpointId), connecting, sessions[d.endpointId]?.list, d.endpointId == selectedDevice && selectedTerm == null) { core.openDevice(d) }
                        }
                    }
                }
            }

            item {
                SectionHeader(t("附近设备")) {
                    HeroIconButton(Lucide.RefreshCw, t("刷新附近设备"), { core.scanNearby() }, tint = Hero.Muted, size = 18.dp)
                }
            }
            item {
                if (nearby.isEmpty()) {
                    Text(t("同一个局域网里没找到别的设备"), color = Hero.Muted, fontSize = 13.5.sp,
                        modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(Hero.Surface).padding(18.dp))
                } else {
                    Group {
                        nearby.forEachIndexed { i, d ->
                            if (i > 0) GroupDivider()
                            DeviceRow(d, conns.containsKey(d.endpointId), connecting, null, d.endpointId == selectedDevice && selectedTerm == null) { core.openDevice(d) }
                        }
                    }
                }
            }
        }
        if (!empty) {
            BottomBar {
                HeroButton(t("连接新设备"), { core.prompt.value = ConnectPrompt() }, Modifier.fillMaxWidth(), icon = Lucide.Plus)
            }
        }
    }
}

@Composable
private fun DeviceRow(d: DeviceView, connected: Boolean, connecting: String?, live: List<SessionView>?, selected: Boolean, onClick: () -> Unit) {
    val busy = connecting == d.endpointId
    val (status, color) = presenceOf(d, connected)
    val sessions = live ?: d.sessions
    val waiting = sessions.count { it.agent?.state == AgentState.WAITING }
    HeroRow(
        onClick = onClick,
        selected = selected,
        enabled = connecting == null || busy,
        leading = { PlatformTile(d.platform, connected) },
        title = { RowTitle(d.remark ?: d.name) },
        subtitle = {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                if (busy) SubText(t("等待对方同意…"), Hero.Accent)
                else {
                    SubText(status, color ?: Hero.Muted)
                    if (d.lanOnline && !connected && d.known) Chip(t("局域网"))
                    if (sessions.isNotEmpty()) SubText(t("{{count}} 个会话", "count" to sessions.size))
                }
            }
            if (waiting > 0 && !busy) Text(t("{{count}} 个 AI 在等你确认", "count" to waiting), color = Hero.Warning, fontSize = 12.5.sp, fontWeight = FontWeight.Medium)
        },
        trailing = {
            if (busy) Spinner(18.dp, Hero.Accent)
            else Icon(Lucide.ChevronRight, null, Modifier.size(18.dp), tint = Hero.Muted.copy(alpha = 0.7f))
        },
    )
}

/** AI 的状态：点加字，干活中蓝、等确认黄 */
@Composable
fun AgentStateLabel(agent: uniffi.p2pcli_mobile.AgentView, size: androidx.compose.ui.unit.TextUnit = 12.sp) {
    val color = agentColor(agent) ?: Hero.Muted
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        Dot(color, 6.dp)
        Text(agentText(agent), color = color, fontSize = size)
    }
}

/** 正在收发的文件和进度 */
fun LazyListScope.transfersSection(transfers: List<TransferView>) {
    val active = transfers.filter { it.state != TransferState.DONE && it.state != TransferState.FAILED }
    items(active.size) { i ->
        val x = active[i]
        val progress = if (x.total > 0uL) (x.done.toDouble() / x.total.toDouble()).toFloat().coerceIn(0f, 1f) else 0f
        Row(
            Modifier.fillMaxWidth().padding(top = 10.dp).clip(RoundedCornerShape(16.dp)).background(Hero.Surface).padding(horizontal = 14.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Box(Modifier.size(36.dp).clip(RoundedCornerShape(18.dp)).background(Hero.AccentSoft), contentAlignment = Alignment.Center) {
                Icon(if (x.outgoing) Lucide.ArrowUp else Lucide.ArrowDown, null, Modifier.size(18.dp), tint = Hero.Accent)
            }
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                RowTitle(x.label)
                SubText(
                    when (x.state) {
                        TransferState.WAITING -> t("等 {{name}} 同意", "name" to x.peerName)
                        TransferState.CONNECTING -> t("正在连接 {{name}}", "name" to x.peerName)
                        else -> (if (x.outgoing) t("发给 {{name}}", "name" to x.peerName) else t("来自 {{name}}", "name" to x.peerName)) + " · ${(progress * 100).toInt()}%"
                    },
                )
                LinearProgressIndicator(
                    progress = { progress },
                    modifier = Modifier.fillMaxWidth().padding(top = 4.dp).clip(RoundedCornerShape(2.dp)),
                    color = Hero.Accent,
                    trackColor = Hero.Tile,
                    drawStopIndicator = {},
                )
            }
        }
    }
}

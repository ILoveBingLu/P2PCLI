package io.github.ilovebinglu.p2pcli.ui

import io.github.ilovebinglu.p2pcli.t
import io.github.ilovebinglu.p2pcli.tr
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.ilovebinglu.p2pcli.Core
import io.github.ilovebinglu.p2pcli.terminal.RemoteTerminal
import kotlinx.coroutines.delay
import uniffi.p2pcli_mobile.PathKind
import uniffi.p2pcli_mobile.SessionView

@Composable
fun DeviceScreen(core: Core, endpointId: String) {
    val conns by core.conns.collectAsStateWithLifecycle()
    val overview by core.overview.collectAsStateWithLifecycle()
    val sessionsMap by core.sessions.collectAsStateWithLifecycle()
    val terms by core.terms.collectAsStateWithLifecycle()
    val transfers by core.transfers.collectAsStateWithLifecycle()
    val connecting by core.connecting.collectAsStateWithLifecycle()
    val conn = conns[endpointId]
    val device = overview?.devices?.firstOrNull { it.endpointId == endpointId } ?: overview?.nearby?.firstOrNull { it.endpointId == endpointId }
    val name = device?.remark ?: device?.name ?: conn?.name ?: t("设备")
    val platform = device?.platform ?: conn?.platform ?: ""
    val state = sessionsMap[endpointId]
    val list = state?.list.orEmpty().sortedByDescending { it.createdAt }
    val titles = sessionTitles(state?.list.orEmpty())
    val openIds = terms.filter { it.endpointId == endpointId && it.state != RemoteTerminal.State.CLOSED }.mapNotNull { it.sessionId }.toSet()
    var menuFor by remember { mutableStateOf<SessionView?>(null) }
    var confirmEnd by remember { mutableStateOf(false) }
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.GetMultipleContents()) { uris -> core.sendFiles(endpointId, uris) }

    // 会话列表和 AI 状态隔几秒刷新一次
    LaunchedEffect(endpointId, conn?.connId) {
        while (conn != null) {
            core.loadSessions(endpointId)
            delay(3000)
        }
    }

    Column(Modifier.fillMaxSize().background(Hero.Background).statusBarsPadding()) {
        NavBar(
            title = { NavTitle(name) },
            subtitle = {
                if (conn != null) SubText(t("已连接") + " · " + pathText(conn.path), if (conn.path.kind == PathKind.RELAY) Hero.Warning else Hero.Success, size = 12.5.sp)
                else SubText(t("没有连着"), size = 12.5.sp)
            },
            onBack = { core.back() },
            actions = { if (conn != null) HeroIconButton(Lucide.FileUp, t("发送文件"), { pickFiles.launch("*/*") }) },
        )
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 28.dp)) {
            transfersSection(transfers)
            item {
                Box(Modifier.fillMaxWidth().padding(top = 18.dp, bottom = 2.dp), contentAlignment = Alignment.Center) {
                    PlatformTile(platform, conn != null, 76.dp)
                }
            }
            if (conn == null) {
                item {
                    EmptyMessage(t("没有连着这台设备"), t("连接断开了，后台会话还在对方电脑上运行，重新连上就能接着用。")) {
                        val busy = connecting == endpointId
                        HeroButton(if (busy) t("等待对方同意…") else t("重新连接"), { device?.let { core.openDevice(it) } }, Modifier.fillMaxWidth(),
                            icon = Lucide.Plug, loading = busy, enabled = device != null && connecting == null)
                    }
                }
                return@LazyColumn
            }
            item {
                SectionHeader(t("会话")) {
                    HeroIconButton(Lucide.RefreshCw, t("刷新会话"), { core.refreshSessions(endpointId) }, tint = Hero.Muted, size = 18.dp)
                }
            }
            state?.error?.let { err ->
                item { Text(t("读不到会话列表：{{error}}", "error" to tr(err)), color = Hero.Danger, fontSize = 13.5.sp, modifier = Modifier.padding(bottom = 10.dp)) }
            }
            item {
                when {
                    state?.list == null && state?.error == null -> Placeholder(t("正在读取会话…"))
                    list.isEmpty() -> Placeholder(t("这台设备上还没有会话，新建一个终端开始用。"))
                    else -> Group {
                        list.forEachIndexed { i, s ->
                            if (i > 0) GroupDivider()
                            HeroRow(
                                onClick = { core.openTerminal(endpointId, s.id) },
                                leading = {
                                    Tile { s.agent?.let { AgentIcon(it.kind) } ?: Icon(Lucide.SquareTerminal, null, Modifier.size(20.dp), tint = Hero.Foreground) }
                                },
                                title = {
                                    RowTitle(titles[s.id] ?: t("会话"), modifier = Modifier.weight(1f, fill = false))
                                    if (s.id in openIds) Chip(t("已打开"), accent = true)
                                },
                                subtitle = {
                                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                                        s.agent?.let { AgentStateLabel(it, 13.sp) } ?: s.command?.let { SubText(it, mono = true, size = 12.sp) }
                                        SubText(shortPath(s.currentPath), mono = true, size = 12.sp)
                                    }
                                    SubText(t("{{time}}创建", "time" to relativeTime(s.createdAt)), Hero.Muted.copy(alpha = 0.8f), size = 12.sp)
                                },
                                trailing = { HeroIconButton(Lucide.EllipsisVertical, t("会话操作"), { menuFor = s; confirmEnd = false }, tint = Hero.Muted) },
                            )
                        }
                    }
                }
            }
            item {
                Row(
                    Modifier.fillMaxWidth().padding(top = 28.dp),
                    horizontalArrangement = Arrangement.Center,
                ) {
                    Row(
                        Modifier.clip(RoundedCornerShape(12.dp)).clickable { core.disconnect(endpointId) }.padding(horizontal = 16.dp, vertical = 10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Icon(Lucide.Unplug, null, Modifier.size(18.dp), tint = Hero.Danger)
                        Text(t("断开连接"), color = Hero.Danger, fontSize = 15.sp)
                    }
                }
            }
        }
        if (conn != null) {
            BottomBar { HeroButton(t("新建终端"), { core.openTerminal(endpointId, null) }, Modifier.fillMaxWidth(), icon = Lucide.Plus) }
        }
    }

    menuFor?.let { s ->
        val title = titles[s.id] ?: t("会话")
        HeroSheet(onDismiss = { menuFor = null }, title = title) {
            if (!confirmEnd) {
                SheetAction(Lucide.SquareArrowOutUpRight, t("打开"), { menuFor = null; core.openTerminal(endpointId, s.id) })
                SheetAction(Lucide.Trash2, t("结束会话"), { confirmEnd = true }, description = t("关掉对方电脑上的这个会话和里面运行的程序"), danger = true)
            } else {
                Text(t("结束后里面运行的程序会被关掉，没保存的内容会丢失。"), color = Hero.Muted, fontSize = 14.sp,
                    modifier = Modifier.fillMaxWidth().padding(bottom = 16.dp), textAlign = androidx.compose.ui.text.style.TextAlign.Center)
                HeroButton(t("结束会话"), { menuFor = null; core.endSession(endpointId, s.id) }, Modifier.fillMaxWidth(), kind = ButtonKind.Danger, icon = Lucide.Trash2)
                Spacer(Modifier.height(10.dp))
                HeroButton(t("取消"), { confirmEnd = false }, Modifier.fillMaxWidth(), kind = ButtonKind.Ghost)
            }
        }
    }
}

@Composable
private fun Placeholder(text: String) {
    Text(text, color = Hero.Muted, fontSize = 13.5.sp, textAlign = androidx.compose.ui.text.style.TextAlign.Center,
        modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(Hero.Surface).padding(18.dp))
}

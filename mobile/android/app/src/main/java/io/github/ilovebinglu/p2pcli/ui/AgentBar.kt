package io.github.ilovebinglu.p2pcli.ui

import io.github.ilovebinglu.p2pcli.t
import io.github.ilovebinglu.p2pcli.tr
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.ilovebinglu.p2pcli.Core
import io.github.ilovebinglu.p2pcli.terminal.RemoteTerminal
import io.github.ilovebinglu.p2pcli.terminal.errorText
import kotlinx.coroutines.launch
import uniffi.p2pcli_mobile.DirListingView
import uniffi.p2pcli_mobile.InstalledAgentView
import uniffi.p2pcli_mobile.SessionView

/**
 * 终端下面一行：最左边是当前工作目录（点开选目录），后面是对方电脑上装了的命令行 AI，点一下在这个会话里启动。
 * 和桌面一样，终端停在空闲的命令提示符、还没在跑 AI 时才能启动；跑着程序时目录只能看不能切。
 * 这个会话里在跑 AI 时整行不显示
 */
@Composable
fun AgentBar(core: Core, term: RemoteTerminal, session: SessionView?, onPickDir: () -> Unit) {
    val agentsMap by core.agents.collectAsStateWithLifecycle()
    LaunchedEffect(term.connId) { core.scanAgents(term.connId) }
    val state = agentsMap[term.connId]
    val idle = session?.shellReady == true && session.agent == null
    var launching by remember { mutableStateOf<String?>(null) }
    var menuFor by remember { mutableStateOf<InstalledAgentView?>(null) }
    val launch = { a: InstalledAgentView, bypass: Boolean ->
        if (launching == null) {
            launching = a.kind
            core.launchAgent(term, a.kind, bypass) { launching = null }
        }
    }

    if (session?.agent != null) return

    Row(
        Modifier.fillMaxWidth().background(Hero.Background).horizontalScroll(rememberScrollState())
            .padding(start = 8.dp, end = 8.dp, top = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val path = shortPath(session?.currentPath).ifEmpty { "~" }
        Pill(t("当前目录：{{path}}，点击选择目录", "path" to path), onClick = onPickDir, icon = Lucide.FolderOpen, mono = true) {
            Text(path, color = Hero.Foreground, fontSize = 12.5.sp, fontFamily = Hero.Mono, maxLines = 1,
                overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 180.dp))
        }
        if (idle) {
            when {
                state == null || (state.scanning && state.list == null) -> Row(
                    Modifier.padding(horizontal = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Spinner(14.dp)
                    Text(t("正在检测 AI"), color = Hero.Muted, fontSize = 12.5.sp)
                }
                state.list == null -> Pill(t("AI 检测失败，重试"), onClick = { core.scanAgents(term.connId, true) }, icon = Lucide.RefreshCw) {
                    Text(t("检测失败 · 重试"), color = Hero.Foreground, fontSize = 12.5.sp)
                }
                state.list.isEmpty() -> Pill(t("未检测到 AI，点击重新检测"), onClick = { core.scanAgents(term.connId, true) }, icon = Lucide.RefreshCw) {
                    Text(t("未检测到 AI，点击重新检测"), color = Hero.Muted, fontSize = 12.5.sp)
                }
                else -> {
                    state.list.forEach { a ->
                        Pill(
                            t("启动 {{name}}", "name" to a.name),
                            onClick = { launch(a, false) },
                            // Claude Code 长按可以选“允许之后切换全部放行”，和桌面的右键菜单一样
                            onLongClick = if (a.kind == "claude") ({ menuFor = a }) else null,
                            enabled = launching == null,
                        ) {
                            if (launching == a.kind) Spinner(16.dp, Hero.Accent) else AgentIcon(a.kind, 16.dp)
                            Text(a.name, color = Hero.Foreground, fontSize = 12.5.sp, maxLines = 1)
                        }
                    }
                    Box(
                        Modifier.size(32.dp).clip(RoundedCornerShape(16.dp)).clickable(enabled = launching == null && !state.scanning) {
                            core.scanAgents(term.connId, true)
                        },
                        contentAlignment = Alignment.Center,
                    ) {
                        if (state.scanning) Spinner(14.dp) else Icon(Lucide.RefreshCw, t("刷新已安装的 AI"), Modifier.size(15.dp), tint = Hero.Muted)
                    }
                }
            }
        }
    }

    menuFor?.let { a ->
        HeroSheet(onDismiss = { menuFor = null }, title = a.name) {
            SheetAction(Lucide.Play, t("正常启动"), { menuFor = null; launch(a, false) })
            SheetAction(Lucide.Play, t("启动并允许之后切换全部放行"), { menuFor = null; launch(a, true) })
        }
    }
}

/** 一个圆角按钮：图标加内容；读屏念 label */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun Pill(
    label: String,
    onClick: () -> Unit,
    icon: ImageVector? = null,
    mono: Boolean = false,
    onLongClick: (() -> Unit)? = null,
    enabled: Boolean = true,
    content: @Composable () -> Unit,
) {
    Row(
        Modifier.height(34.dp).clip(RoundedCornerShape(17.dp)).background(Hero.Tile)
            .combinedClickable(enabled = enabled, onClickLabel = label, onClick = onClick, onLongClick = onLongClick)
            .padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        icon?.let { Icon(it, null, Modifier.size(15.dp), tint = if (mono) Hero.Muted else Hero.Foreground) }
        content()
    }
}

/**
 * 选工作目录：对方电脑上的目录列表，点子目录进去，点“切换到这里”让终端 cd 过去。
 * 终端跑着程序（包括 AI）时只能看，免得 cd 被当成输入发给程序
 */
@Composable
fun DirPickerSheet(core: Core, term: RemoteTerminal, session: SessionView?, onDismiss: () -> Unit) {
    val scope = rememberCoroutineScope()
    var listing by remember { mutableStateOf<DirListingView?>(null) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var filter by remember { mutableStateOf("") }
    var changing by remember { mutableStateOf(false) }
    // Windows 电脑上在盘的根目录点上一级，是“所有磁盘”这一层（路径是 \），只能选盘进去，不能切过去
    val atDrives = listing?.path == "\\"
    val canChange = session?.shellReady == true && session.agent == null && !atDrives
    val browse = { path: String ->
        scope.launch {
            loading = true
            error = null
            try {
                val next = core.listDir(term.connId, path)
                if (next.path != listing?.path) filter = ""
                listing = next
            } catch (e: Exception) {
                error = errorText(e)
            } finally {
                loading = false
            }
        }
    }
    LaunchedEffect(Unit) { browse(session?.currentPath ?: "") }
    val current = listing
    val dirs = current?.dirs.orEmpty().filter { filter.isBlank() || it.contains(filter.trim(), ignoreCase = true) }

    HeroSheet(onDismiss = onDismiss, title = t("选择工作目录")) {
        Text(if (atDrives) t("所有磁盘") else current?.path ?: session?.currentPath ?: "", color = Hero.Muted, fontSize = 12.5.sp, fontFamily = Hero.Mono,
            modifier = Modifier.fillMaxWidth().padding(bottom = 10.dp))
        Row(Modifier.fillMaxWidth().padding(bottom = 10.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            HeroButton(t("上一级"), { current?.parent?.let { browse(it) } }, Modifier.weight(1f), kind = ButtonKind.Secondary,
                icon = Lucide.ArrowUp, enabled = current?.parent != null && !loading, height = 40.dp)
            HeroButton(t("用户目录"), { browse("") }, Modifier.weight(1f), kind = ButtonKind.Secondary,
                icon = Lucide.House, enabled = !loading, height = 40.dp)
        }
        HeroTextField(filter, { filter = it }, t("筛选子目录"))
        error?.let { Text(tr(it), color = Hero.Danger, fontSize = 13.sp, modifier = Modifier.padding(top = 8.dp)) }
        Box(Modifier.fillMaxWidth().heightIn(min = 80.dp, max = 320.dp).padding(top = 8.dp)) {
            if (loading && current == null) {
                Spinner(20.dp)
            } else {
                LazyColumn {
                    items(dirs, key = { it }) { name ->
                        Row(
                            Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp))
                                .clickable(enabled = !loading) { current?.let { browse(joinPath(it.path, name)) } }
                                .padding(horizontal = 8.dp, vertical = 11.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            Icon(Lucide.Folder, null, Modifier.size(18.dp), tint = Hero.Muted)
                            Text(name, color = Hero.Foreground, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                }
            }
        }
        if (current?.truncated == true) {
            Text(t("子目录太多，只列出了前 {{shown}} 个，可以输入名字筛选", "shown" to current.dirs.size), color = Hero.Muted, fontSize = 12.sp)
        }
        if (atDrives) {
            Text(t("先选一个盘"), color = Hero.Muted, fontSize = 12.5.sp, modifier = Modifier.padding(top = 8.dp))
        } else if (!canChange) {
            val agent = session?.agent
            Text(
                if (agent != null) t("{{name}} 正在运行，退出后才能切换目录", "name" to agent.name)
                else t("终端不在空闲提示符，完成当前输入或退出程序后才能切换"),
                color = Hero.Warning, fontSize = 12.5.sp, modifier = Modifier.padding(top = 8.dp),
            )
        }
        HeroButton(
            t("切换到这里"),
            {
                val target = current ?: return@HeroButton
                changing = true
                core.changeDir(term, target.path) { err ->
                    changing = false
                    if (err == null) onDismiss() else error = err
                }
            },
            Modifier.fillMaxWidth().padding(top = 12.dp),
            icon = Lucide.CornerDownLeft,
            enabled = canChange && current != null && !changing && !loading,
            loading = changing,
        )
    }
}

/** 拼对方电脑上的路径：Windows 用反斜杠 */
private fun joinPath(dir: String, name: String): String {
    val sep = if (dir.contains('\\') && !dir.contains('/')) "\\" else "/"
    return if (dir.endsWith(sep)) dir + name else dir + sep + name
}

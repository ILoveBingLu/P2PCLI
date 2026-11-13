package io.github.ilovebinglu.p2pcli.ui

import android.content.ClipboardManager
import android.content.Context
import android.view.KeyEvent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.ilovebinglu.p2pcli.Core
import io.github.ilovebinglu.p2pcli.terminal.RemoteTerminal
import io.github.ilovebinglu.p2pcli.terminal.TerminalCanvasView
import kotlinx.coroutines.delay
import uniffi.p2pcli_mobile.AgentState
import uniffi.p2pcli_mobile.PathKind

@Composable
fun TerminalScreen(core: Core, key: Int) {
    val terms by core.terms.collectAsStateWithLifecycle()
    val sessionsMap by core.sessions.collectAsStateWithLifecycle()
    val overview by core.overview.collectAsStateWithLifecycle()
    val conns by core.conns.collectAsStateWithLifecycle()
    val term = terms.firstOrNull { it.key == key } ?: return
    val context = LocalContext.current
    val list = sessionsMap[term.endpointId]?.list.orEmpty()
    val agent = if (term.state == RemoteTerminal.State.OPEN) list.firstOrNull { it.id == term.sessionId }?.agent else null
    val title = term.sessionId?.let { sessionTitles(list)[it] } ?: "新终端"
    fun deviceName(endpointId: String) = overview?.devices?.firstOrNull { it.endpointId == endpointId }?.let { it.remark ?: it.name }
        ?: conns[endpointId]?.name ?: ""
    var ctrl by remember { mutableStateOf(false) }
    var alt by remember { mutableStateOf(false) }
    var composing by remember { mutableStateOf(false) }
    var draft by remember { mutableStateOf("") }
    var panel by remember { mutableStateOf<String?>(null) }

    // AI 状态隔几秒刷新一次
    LaunchedEffect(term.endpointId) {
        while (true) {
            core.loadSessions(term.endpointId)
            delay(3000)
        }
    }

    val modifiers = remember {
        object : TerminalCanvasView.Modifiers {
            override fun takeCtrl(): Boolean = ctrl.also { ctrl = false }
            override fun takeAlt(): Boolean = alt.also { alt = false }
        }
    }
    val view = remember { TerminalCanvasView(context, modifiers) }
    val live = term.state == RemoteTerminal.State.OPEN
    val path = conns[term.endpointId]?.path

    Column(Modifier.fillMaxSize().background(Hero.Terminal).imePadding()) {
        Column(Modifier.background(Hero.Background).statusBarsPadding()) {
            NavBar(
                title = {
                    agent?.let { AgentIcon(it.kind, 16.dp) }
                    NavTitle(title)
                },
                subtitle = {
                    Dot(if (!live) Hero.Danger else if (path?.kind == PathKind.RELAY) Hero.Warning else Hero.Success)
                    SubText(deviceName(term.endpointId), size = 12.5.sp)
                    agent?.let { AgentStateLabel(it) }
                },
                onBack = { core.back() },
                actions = {
                    // 开着几个终端：点开切换
                    Box(
                        Modifier.size(44.dp).clip(RoundedCornerShape(14.dp)).clickable { panel = "switch" },
                        contentAlignment = Alignment.Center,
                    ) {
                        Box(
                            Modifier.widthIn(min = 22.dp).height(22.dp).border(1.8.dp, Hero.Foreground, RoundedCornerShape(7.dp)).padding(horizontal = 5.dp),
                            contentAlignment = Alignment.Center,
                        ) { Text("${terms.size}", color = Hero.Foreground, fontSize = 12.sp, fontWeight = FontWeight.Bold) }
                    }
                    HeroIconButton(Lucide.Ellipsis, "更多", { panel = "more" })
                },
            )
        }

        Box(Modifier.weight(1f).fillMaxWidth()) {
            AndroidView(
                factory = { view },
                update = { it.terminal = term },
                modifier = Modifier.fillMaxSize().padding(start = 6.dp, end = 2.dp, top = 4.dp),
            )
            when (term.state) {
                RemoteTerminal.State.OPENING -> Row(
                    Modifier.align(Alignment.Center),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Spinner(18.dp)
                    Text("正在打开终端…", color = Hero.Muted, fontSize = 14.sp)
                }
                RemoteTerminal.State.CLOSED -> Box(
                    Modifier.fillMaxSize().background(Hero.Terminal.copy(alpha = 0.82f)).padding(20.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Column(
                        Modifier.widthIn(max = 340.dp).clip(RoundedCornerShape(20.dp)).background(Hero.Surface).padding(20.dp),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        Text(if (term.exited) "这个终端已经结束" else "连接已断开", color = Hero.Foreground, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
                        term.note?.let { Text(it, color = Hero.Foreground, fontSize = 13.5.sp) }
                        Text("会话还在对方电脑上运行的话，重新连上就能接着用。", color = Hero.Muted, fontSize = 13.5.sp)
                        HeroButton(if (term.exited) "新建终端" else "重新连接", { core.reopen(term) }, Modifier.fillMaxWidth())
                        HeroButton("关闭", { core.closeTerminal(term.key) }, Modifier.fillMaxWidth(), kind = ButtonKind.Ghost)
                    }
                }
                else -> {}
            }
        }

        // AI 等你确认：选项直接点，不用到终端里按方向键
        val approval = agent?.approval
        if (agent != null && agent.state == AgentState.WAITING && approval != null) {
            Box(Modifier.background(Hero.Background).topLine().padding(start = 8.dp, end = 8.dp, top = 8.dp)) {
                Column(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(Hero.Surface).heightIn(max = 340.dp)
                        .verticalScroll(rememberScrollState()).padding(10.dp),
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Text("${agent.name} · 等待确认", color = Hero.Muted, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 4.dp))
                    Text(approval.context, color = Hero.Foreground, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp))
                    approval.options.forEach { opt ->
                        Row(
                            Modifier.fillMaxWidth().heightIn(min = 46.dp).clip(RoundedCornerShape(10.dp))
                                .clickable { core.chooseOption(term, approval.id, opt.id) }.padding(8.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(9.dp),
                        ) {
                            Box(Modifier.size(22.dp).clip(RoundedCornerShape(7.dp)).background(Hero.SurfaceSecondary), contentAlignment = Alignment.Center) {
                                Text("${opt.id + 1u}", color = Hero.Foreground, fontSize = 12.sp)
                            }
                            Text(opt.label, color = Hero.Foreground, fontSize = 14.sp, modifier = Modifier.weight(1f))
                            Icon(Lucide.SquareArrowOutUpRight, null, Modifier.size(15.dp), tint = Hero.Muted)
                        }
                    }
                }
            }
        }

        if (composing && live) {
            val send = {
                if (draft.isNotEmpty()) {
                    term.paste(draft)
                    view.postDelayed({ term.send("\r") }, 80)
                    draft = ""
                }
            }
            Row(
                Modifier.fillMaxWidth().background(Hero.Background).topLine().padding(start = 8.dp, end = 8.dp, top = 8.dp),
                verticalAlignment = Alignment.Bottom,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                HeroTextField(
                    draft, { draft = it }, agent?.let { "发给 ${it.name}" } ?: "输入命令，发送后自动回车",
                    modifier = Modifier.weight(1f), imeAction = ImeAction.Send, onImeAction = send,
                    singleLine = false, radius = 21.dp, background = Hero.Surface,
                )
                Box(
                    Modifier.size(44.dp).clip(RoundedCornerShape(22.dp)).background(if (draft.isNotEmpty()) Hero.Accent else Hero.Default)
                        .clickable(enabled = draft.isNotEmpty(), onClick = send),
                    contentAlignment = Alignment.Center,
                ) { Icon(Lucide.SendHorizontal, "发送", Modifier.size(20.dp), tint = Color.White) }
            }
        }

        if (live) {
            KeyBar(
                ctrl = ctrl, alt = alt, composing = composing,
                onCtrl = { ctrl = !ctrl }, onAlt = { alt = !alt }, onCompose = { composing = !composing },
                onBytes = { term.send(it) },
                onKey = { view.sendKeyCode(it, 0) },
                onPaste = {
                    val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                    cm.primaryClip?.getItemAt(0)?.coerceToText(context)?.toString()?.takeIf { it.isNotEmpty() }?.let { term.paste(it) }
                },
            )
        } else {
            Spacer(Modifier.navigationBarsPadding())
        }
    }

    when (panel) {
        "switch" -> HeroSheet(onDismiss = { panel = null }, title = "开着的终端") {
            terms.forEach { t ->
                val l = sessionsMap[t.endpointId]?.list.orEmpty()
                val a = l.firstOrNull { it.id == t.sessionId }?.agent?.takeIf { t.state == RemoteTerminal.State.OPEN }
                Row(
                    Modifier.fillMaxWidth().padding(vertical = 2.dp).clip(RoundedCornerShape(16.dp))
                        .background(if (t.key == key) Hero.AccentSoft.copy(alpha = 0.12f) else Color.Transparent)
                        .clickable { panel = null; core.openTerminal(t.endpointId, t.sessionId) }
                        .padding(start = 10.dp, top = 10.dp, bottom = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Tile(dimmed = t.state == RemoteTerminal.State.CLOSED) {
                        if (a != null) AgentIcon(a.kind) else Icon(Lucide.SquareTerminal, null, Modifier.size(20.dp), tint = Hero.Foreground)
                    }
                    Column(Modifier.weight(1f)) {
                        RowTitle(t.sessionId?.let { sessionTitles(l)[it] } ?: "新终端")
                        SubText(deviceName(t.endpointId) + if (t.state == RemoteTerminal.State.CLOSED) " · 已断开" else "")
                    }
                    a?.let { AgentStateLabel(it) }
                    HeroIconButton(Lucide.X, "关闭", { if (terms.size == 1) panel = null; core.closeTerminal(t.key) }, tint = Hero.Muted)
                }
            }
            Spacer(Modifier.height(8.dp))
            SheetAction(Lucide.Plus, "在 ${deviceName(term.endpointId)} 上新建终端", { panel = null; core.openTerminal(term.endpointId, null) })
        }
        "more" -> HeroSheet(onDismiss = { panel = null }, title = title) {
            SheetAction(Lucide.Plus, "新建终端", { panel = null; core.openTerminal(term.endpointId, null) })
            SheetAction(Lucide.X, "关闭这个终端", { panel = null; core.closeTerminal(term.key) }, description = "会话在对方电脑上接着运行，之后还能打开")
        }
    }
}

/** 手机键盘上没有、终端里又常用的键；横着能滑。按的时候不抢终端的焦点，软键盘不会收起 */
@Composable
private fun KeyBar(
    ctrl: Boolean,
    alt: Boolean,
    composing: Boolean,
    onCtrl: () -> Unit,
    onAlt: () -> Unit,
    onCompose: () -> Unit,
    onBytes: (String) -> Unit,
    onKey: (Int) -> Unit,
    onPaste: () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().background(Hero.Background).navigationBarsPadding().padding(8.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Key("", active = composing, icon = { Icon(Lucide.Keyboard, "输入栏", Modifier.size(20.dp), tint = if (composing) Hero.Accent else Hero.Foreground) }, onClick = onCompose)
        Row(Modifier.weight(1f).horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Key("Esc") { onBytes("\u001b") }
            Key("Tab") { onBytes("\t") }
            Key("Ctrl", active = ctrl, onClick = onCtrl)
            Key("Alt", active = alt, onClick = onAlt)
            Key("^C") { onBytes("\u0003") }
            Key("↑") { onKey(KeyEvent.KEYCODE_DPAD_UP) }
            Key("↓") { onKey(KeyEvent.KEYCODE_DPAD_DOWN) }
            Key("←") { onKey(KeyEvent.KEYCODE_DPAD_LEFT) }
            Key("→") { onKey(KeyEvent.KEYCODE_DPAD_RIGHT) }
            Key("⇧Tab") { onBytes("\u001b[Z") }
            Key("粘贴", onClick = onPaste)
            Key("^D") { onBytes("\u0004") }
            Key("^Z") { onBytes("\u001a") }
            Key("^L") { onBytes("\u000c") }
            Key("^R") { onBytes("\u0012") }
            Key("|") { onBytes("|") }
            Key("~") { onBytes("~") }
            Key("/") { onBytes("/") }
            Key("-") { onBytes("-") }
        }
    }
}

@Composable
private fun Key(label: String, active: Boolean = false, icon: (@Composable () -> Unit)? = null, onClick: () -> Unit) {
    // 不要涟漪、不抢焦点：按下时只换底色
    val source = remember { MutableInteractionSource() }
    Box(
        Modifier.widthIn(min = 44.dp).height(38.dp).clip(RoundedCornerShape(10.dp))
            .background(if (active) Hero.AccentSoft.copy(alpha = 0.22f) else Hero.Tile)
            .clickable(interactionSource = source, indication = null, onClick = onClick)
            .padding(horizontal = 10.dp),
        contentAlignment = Alignment.Center,
    ) {
        if (icon != null) icon()
        else Text(label, color = if (active) Hero.Accent else Hero.Foreground, fontFamily = Hero.Mono, fontSize = 14.sp, fontWeight = FontWeight.Medium,
            maxLines = 1, overflow = TextOverflow.Clip)
    }
}

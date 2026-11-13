package io.github.ilovebinglu.p2pcli.ui

import android.content.ClipboardManager
import android.content.Context
import android.view.KeyEvent
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
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

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TerminalScreen(core: Core, key: Int) {
    val terms by core.terms.collectAsStateWithLifecycle()
    val sessionsMap by core.sessions.collectAsStateWithLifecycle()
    val overview by core.overview.collectAsStateWithLifecycle()
    val term = terms.firstOrNull { it.key == key } ?: return
    val context = LocalContext.current
    val list = sessionsMap[term.endpointId]?.list.orEmpty()
    val session = list.firstOrNull { it.id == term.sessionId }
    val agent = if (term.state == RemoteTerminal.State.OPEN) session?.agent else null
    val title = term.sessionId?.let { sessionTitles(list)[it] } ?: "新终端"
    val device = overview?.devices?.firstOrNull { it.endpointId == term.endpointId }
    var ctrl by remember { mutableStateOf(false) }
    var alt by remember { mutableStateOf(false) }
    var composing by remember { mutableStateOf(false) }
    var draft by remember { mutableStateOf("") }

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

    Column(Modifier.fillMaxSize().background(Color.Black).imePadding()) {
        TopAppBar(
            title = {
                Column {
                    Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    val sub = (device?.remark ?: device?.name ?: "") + (agent?.let { " · " + agentText(it) } ?: "")
                    Text(sub, style = MaterialTheme.typography.bodySmall, color = agent?.let { agentColor(it) } ?: MaterialTheme.colorScheme.onSurfaceVariant)
                }
            },
            navigationIcon = { IconButton(onClick = { core.back() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "返回") } },
            actions = {
                IconButton(onClick = { core.closeTerminal(term.key) }) { Icon(Icons.Default.Close, contentDescription = "关闭这个终端") }
            },
        )

        Box(Modifier.weight(1f).fillMaxWidth()) {
            AndroidView(
                factory = { view },
                update = { it.terminal = term },
                modifier = Modifier.fillMaxSize().padding(start = 4.dp),
            )
            when (term.state) {
                RemoteTerminal.State.OPENING -> Row(
                    Modifier.align(Alignment.Center),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp, color = Color.White)
                    Text("正在打开终端…", color = Color.White)
                }
                RemoteTerminal.State.CLOSED -> Card(Modifier.align(Alignment.Center).padding(24.dp)) {
                    Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        Text(if (term.exited) "这个终端已经结束" else "连接已断开", style = MaterialTheme.typography.titleMedium)
                        term.note?.let { Text(it) }
                        Text("会话还在对方电脑上运行的话，重新连上就能接着用。", color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Button(onClick = { core.reopen(term) }, modifier = Modifier.fillMaxWidth()) { Text(if (term.exited) "新建终端" else "重新连接") }
                        OutlinedButton(onClick = { core.closeTerminal(term.key) }, modifier = Modifier.fillMaxWidth()) { Text("关闭") }
                    }
                }
                else -> {}
            }
        }

        // AI 等你确认：选项直接点，不用到终端里按方向键
        val approval = agent?.approval
        if (agent != null && agent.state == AgentState.WAITING && approval != null) {
            Card(Modifier.fillMaxWidth().padding(8.dp)) {
                Column(Modifier.padding(12.dp).heightIn(max = 320.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("${agent.name} 等你确认", style = MaterialTheme.typography.titleSmall)
                    Text(approval.context, fontFamily = FontFamily.Monospace, fontSize = 13.sp)
                    approval.options.forEach { opt ->
                        OutlinedButton(onClick = { core.chooseOption(term, approval.id, opt.id) }, modifier = Modifier.fillMaxWidth()) {
                            Text("${opt.id + 1u}. ${opt.label}", modifier = Modifier.fillMaxWidth())
                        }
                    }
                }
            }
        }

        if (composing && term.state == RemoteTerminal.State.OPEN) {
            val send = {
                if (draft.isNotEmpty()) {
                    term.paste(draft)
                    view.postDelayed({ term.send("\r") }, 80)
                    draft = ""
                }
            }
            Row(
                Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface).padding(8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                OutlinedTextField(
                    value = draft,
                    onValueChange = { draft = it },
                    placeholder = { Text(agent?.let { "发给 ${it.name}" } ?: "输入命令，发送后自动回车") },
                    modifier = Modifier.weight(1f),
                    maxLines = 4,
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                    keyboardActions = KeyboardActions(onSend = { send() }),
                )
                FilledIconButton(onClick = { send() }, enabled = draft.isNotEmpty()) {
                    Icon(Icons.AutoMirrored.Filled.Send, contentDescription = "发送")
                }
            }
        }

        if (term.state == RemoteTerminal.State.OPEN) {
            KeyBar(
                ctrl = ctrl,
                alt = alt,
                composing = composing,
                onCtrl = { ctrl = !ctrl },
                onAlt = { alt = !alt },
                onCompose = { composing = !composing },
                onBytes = { term.send(it) },
                onKey = { view.sendKeyCode(it, 0) },
                onPaste = {
                    val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                    cm.primaryClip?.getItemAt(0)?.coerceToText(context)?.toString()?.takeIf { it.isNotEmpty() }?.let { term.paste(it) }
                },
            )
        }
    }
}

/** 手机键盘上没有、终端里又常用的键；横着能滑 */
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
        Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface).navigationBarsPadding()
            .horizontalScroll(rememberScrollState()).padding(horizontal = 6.dp, vertical = 6.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Key("", active = composing, icon = { Icon(Icons.Default.Edit, contentDescription = "输入栏", modifier = Modifier.size(18.dp)) }, onClick = onCompose)
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

@Composable
private fun Key(label: String, active: Boolean = false, icon: (@Composable () -> Unit)? = null, onClick: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    Surface(
        shape = RoundedCornerShape(10.dp),
        color = if (active) colors.primary else colors.surfaceVariant,
        contentColor = if (active) colors.onPrimary else colors.onSurfaceVariant,
        modifier = Modifier.widthIn(min = 44.dp).clickable(onClick = onClick),
    ) {
        Box(Modifier.padding(horizontal = 12.dp, vertical = 10.dp), contentAlignment = Alignment.Center) {
            if (icon != null) icon() else Text(label, fontFamily = FontFamily.Monospace, fontSize = 14.sp)
        }
    }
}

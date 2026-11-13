package io.github.ilovebinglu.p2pcli.ui

import androidx.compose.runtime.SideEffect
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.animation.shrinkHorizontally
import androidx.compose.animation.expandHorizontally
import androidx.compose.animation.AnimatedVisibility
import io.github.ilovebinglu.p2pcli.t
import io.github.ilovebinglu.p2pcli.tr
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
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
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.ilovebinglu.p2pcli.ConnectPrompt
import io.github.ilovebinglu.p2pcli.Core
import io.github.ilovebinglu.p2pcli.R
import io.github.ilovebinglu.p2pcli.Screen
import io.github.ilovebinglu.p2pcli.StartState

@Composable
fun Root(core: Core) {
    val start by core.start.collectAsStateWithLifecycle()
    val stack by core.stack.collectAsStateWithLifecycle()
    val prompt by core.prompt.collectAsStateWithLifecycle()
    val fileRequest by core.fileRequest.collectAsStateWithLifecycle()
    val toasts = remember { SnackbarHostState() }
    LaunchedEffect(core) { core.notices.collect { toasts.showSnackbar(it) } }

    // 系统返回键、返回手势：退一页；在首页时交给系统（退到桌面）
    BackHandler(enabled = stack.size > 1) { core.back() }

    Box(Modifier.fillMaxSize().background(Hero.Background)) {
        when (val s = start) {
            StartState.Starting -> Centered {
                androidx.compose.foundation.Image(painterResource(R.drawable.logo), null, Modifier.size(68.dp))
                Spacer(Modifier.height(20.dp))
                Spinner(20.dp)
            }
            is StartState.Failed -> Centered {
                Text(t("启动失败"), color = Hero.Foreground, fontSize = 22.sp, fontWeight = FontWeight.Bold)
                Text(s.message, color = Hero.Muted, fontSize = 14.5.sp, modifier = Modifier.padding(top = 8.dp, bottom = 24.dp))
                HeroButton(t("重试"), { core.start() }, Modifier.width(200.dp))
            }
            StartState.Ready -> BoxWithConstraints(Modifier.fillMaxSize()) {
                // 宽屏（折叠屏展开、平板、横屏）左右两栏；窄屏一页一页进
                val wide = maxWidth >= 700.dp
                SideEffect { core.wide = wide }
                val sidebarHidden by core.sidebarHidden.collectAsStateWithLifecycle()
                if (wide) {
                    val top = stack.last()
                    Row(Modifier.fillMaxSize()) {
                        val collapse = sidebarHidden && top is Screen.Term
                        AnimatedVisibility(!collapse, enter = expandHorizontally(), exit = shrinkHorizontally()) {
                            Row {
                                Box(Modifier.width((maxWidth * 0.38f).coerceIn(320.dp, 420.dp)).fillMaxHeight()) {
                                    HomeScreen(core, selected = top)
                                }
                                Box(Modifier.width(1.dp).fillMaxHeight().background(Hero.Line))
                            }
                        }
                        Box(Modifier.weight(1f).fillMaxHeight()) {
                            Screens(core, stack, detail = true)
                        }
                    }
                } else {
                    Screens(core, stack, detail = false)
                }
            }
        }

        // 提示：从顶上冒出来的小卡片，和桌面一样
        SnackbarHost(toasts, Modifier.align(Alignment.TopCenter).statusBarsPadding().padding(top = 8.dp)) { data ->
            Row(
                Modifier.padding(horizontal = 16.dp).shadow(12.dp, RoundedCornerShape(16.dp)).clip(RoundedCornerShape(16.dp))
                    .background(Hero.Surface).padding(horizontal = 16.dp, vertical = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Text(data.visuals.message, color = Hero.Foreground, fontSize = 14.5.sp)
            }
        }
    }

    prompt?.let { ConnectSheet(core, it) }
    fileRequest?.let { req ->
        HeroSheet(onDismiss = { core.respondFiles(false) }, title = t("{{name}} 想发文件给你", "name" to req.name)) {
            Text(t("{{count}} 个文件：{{items}}", "count" to req.files, "items" to req.items.joinToString(t("、"))), color = Hero.Muted, fontSize = 14.sp,
                modifier = Modifier.padding(bottom = 16.dp))
            HeroButton(t("接收"), { core.respondFiles(true) }, Modifier.fillMaxWidth())
            Spacer(Modifier.height(10.dp))
            HeroButton(t("拒绝"), { core.respondFiles(false) }, Modifier.fillMaxWidth(), kind = ButtonKind.Ghost)
        }
    }
}

/** 一页一页的内容；宽屏时这是右边那一栏，首页的位置放一句提示 */
@Composable
private fun Screens(core: Core, stack: List<Screen>, detail: Boolean) {
    // 进新页从右边滑进来，退回时从左边
    var depth by remember { mutableStateOf(stack.size) }
    val forward = stack.size >= depth
    LaunchedEffect(stack.size) { depth = stack.size }
    AnimatedContent(
        targetState = stack.last(),
        transitionSpec = {
            val dir = if (forward) 1 else -1
            (slideInHorizontally(tween(260)) { it * dir / 4 } + fadeIn(tween(200)))
                .togetherWith(slideOutHorizontally(tween(260)) { -it * dir / 6 } + fadeOut(tween(160)))
        },
        label = "screen",
    ) { top ->
        when (top) {
            Screen.Home -> if (detail) DetailPlaceholder() else HomeScreen(core)
            is Screen.Device -> DeviceScreen(core, top.endpointId)
            is Screen.Term -> TerminalScreen(core, top.key)
            Screen.Settings -> SettingsScreen(core)
        }
    }
}

/** 宽屏右边还没选东西时 */
@Composable
private fun DetailPlaceholder() {
    Column(
        Modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        androidx.compose.foundation.Image(painterResource(R.drawable.logo), null, Modifier.size(64.dp))
        Spacer(Modifier.height(18.dp))
        Text(t("从左边选一台设备，或者连接新设备。"), color = Hero.Muted, fontSize = 15.sp)
    }
}

@Composable
private fun Centered(content: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) { content() }
}

/** 连接面板：手输设备码和密码，或者给列表里的设备填密码 */
@Composable
private fun ConnectSheet(core: Core, prompt: ConnectPrompt) {
    val connecting by core.connecting.collectAsStateWithLifecycle()
    val busy = connecting != null
    val knownDevice = prompt.endpointId != null
    var code by remember(prompt) { mutableStateOf("") }
    var password by remember(prompt) { mutableStateOf("") }
    val canSubmit = !busy && (knownDevice || code.length == 9)
    val submit = { if (canSubmit) core.submitPrompt(if (knownDevice) prompt.target else code, password) }
    HeroSheet(
        onDismiss = { if (!busy) core.prompt.value = null },
        title = if (knownDevice) t("连接 {{name}}", "name" to (prompt.name ?: t("设备"))) else t("连接新设备"),
    ) {
        Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
            if (!knownDevice) {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Label(t("设备码"))
                    CodeField(code, { code = it }, enabled = !busy)
                }
            }
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Label(t("配对密码"))
                HeroTextField(
                    password, { password = it }, t("输入对方的配对密码"),
                    password = true, keyboardType = KeyboardType.Password, imeAction = ImeAction.Go, onImeAction = submit, enabled = !busy,
                )
            }
            prompt.error?.let {
                Text(it, color = Hero.Danger, fontSize = 14.sp, modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp))
                    .background(Hero.DangerSoft).padding(horizontal = 14.dp, vertical = 10.dp))
            }
            HeroButton(
                if (busy) t("等待对方同意…") else t("连接"), submit, Modifier.fillMaxWidth(),
                kind = if (knownDevice) ButtonKind.Primary else ButtonKind.Secondary,
                icon = Lucide.SquareArrowOutUpRight, enabled = canSubmit || busy, loading = busy,
            )
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
                Icon(Lucide.LockKeyhole, null, Modifier.size(14.dp), tint = Hero.Muted)
                Spacer(Modifier.width(6.dp))
                Text(t("连接时对方电脑上会弹窗问要不要同意"), color = Hero.Muted, fontSize = 12.5.sp)
            }
        }
    }
}

@Composable
private fun Label(text: String) {
    Text(text, color = Hero.Foreground, fontSize = 14.sp, fontWeight = FontWeight.Medium)
}

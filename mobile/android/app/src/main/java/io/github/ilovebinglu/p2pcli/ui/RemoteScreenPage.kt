package io.github.ilovebinglu.p2pcli.ui

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.view.ViewGroup
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.ilovebinglu.p2pcli.Core
import io.github.ilovebinglu.p2pcli.screen.PointerMode
import io.github.ilovebinglu.p2pcli.screen.RemoteScreen
import io.github.ilovebinglu.p2pcli.t
import io.github.ilovebinglu.p2pcli.tr
import uniffi.p2pcli_mobile.ScreenQuality

/**
 * 远程控制页：铺满屏幕的对方画面，底下一条浮着的工具栏；弹出键盘时换成按键条（Esc、Tab、修饰键、方向键）。
 * 盖在所有页面上面，藏起状态栏和导航栏；系统返回键关掉这一页（键盘弹着时先收键盘）。
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun RemoteScreenPage(core: Core, screen: RemoteScreen) {
    val context = LocalContext.current
    val conns by core.conns.collectAsStateWithLifecycle()
    val keyboardUp = WindowInsets.isImeVisible
    var menu by remember { mutableStateOf(false) }

    BackHandler { core.closeScreen() }
    // 全屏、不锁屏；离开时还原
    DisposableEffect(Unit) {
        val window = context.findActivity()?.window
        val controller = window?.let { WindowCompat.getInsetsController(it, it.decorView) }
        controller?.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        controller?.hide(WindowInsetsCompat.Type.systemBars())
        screen.canvas.keepScreenOn = true
        onDispose {
            controller?.show(WindowInsetsCompat.Type.systemBars())
            screen.canvas.keepScreenOn = false
            screen.canvas.hideKeyboard()
        }
    }
    // 从后台回来：要一个关键帧
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) { screen.resume() }

    Box(Modifier.fillMaxSize().background(Color.Black)) {
        AndroidView(
            factory = { (screen.canvas.parent as? ViewGroup)?.removeView(screen.canvas); screen.canvas },
            modifier = Modifier.fillMaxSize(),
        )
        Status(core, screen, reconnectable = conns.containsKey(screen.endpointId))
        Box(Modifier.fillMaxSize().imePadding(), contentAlignment = Alignment.BottomCenter) {
            when {
                keyboardUp && screen.canControl -> KeyBar(screen)
                screen.toolbarCollapsed -> Box(Modifier.fillMaxWidth().navigationBarsPadding().padding(16.dp), contentAlignment = Alignment.BottomEnd) {
                    RoundButton(Lucide.Ellipsis, t("显示工具栏")) { screen.toolbarCollapsed = false }
                }
                else -> Toolbar(core, screen) { menu = true }
            }
        }
    }

    if (menu) {
        HeroSheet(onDismiss = { menu = false }, title = screen.name) {
            val displays = screen.started?.displays.orEmpty()
            if (displays.size > 1) {
                SheetLabel(t("屏幕"))
                displays.forEach { d ->
                    SheetAction(if (d.id == screen.display) Lucide.Check else Lucide.Monitor, d.name, { menu = false; screen.switchDisplay(d.id) })
                }
            }
            SheetLabel(t("画质"))
            listOf(ScreenQuality.SHARP, ScreenQuality.BALANCED, ScreenQuality.SMOOTH).forEach { q ->
                SheetAction(if (q == screen.quality) Lucide.Check else Lucide.Monitor, qualityName(q), { menu = false; screen.setQuality(q) },
                    description = qualityNote(q))
            }
            screen.stats?.let { s ->
                Text("${s.fps} fps · ${"%.1f".format(s.bitrate.toDouble() / 1_000_000)} Mbps", color = Hero.Muted, fontSize = 12.5.sp,
                    fontFamily = Hero.Mono, modifier = Modifier.padding(start = 10.dp, top = 6.dp, bottom = 6.dp))
            }
            Spacer(Modifier.height(6.dp))
            SheetAction(Lucide.ArrowDown, t("收起工具栏"), { menu = false; screen.toolbarCollapsed = true })
        }
    }
}

/** 画面上面的提示：等同意、暂停、结束 */
@Composable
private fun Status(core: Core, screen: RemoteScreen, reconnectable: Boolean) {
    when (val p = screen.phase) {
        RemoteScreen.Phase.Opening -> Card {
            Spinner(28.dp, Color.White)
            Text(t("等待对方同意…"), color = Color.White, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
            Text(t("对方电脑上会弹窗问要不要让你看、能不能操作。"), color = Hero.Muted, fontSize = 14.sp, textAlign = TextAlign.Center)
            HeroButton(t("取消"), { core.closeScreen() }, Modifier.fillMaxWidth())
        }
        RemoteScreen.Phase.Live -> {
            val note = screen.started?.controlNote
            if (!screen.hasVideo) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Spinner(36.dp, Color.White) }
            } else if (note != null && !screen.canControl) {
                Box(Modifier.fillMaxSize().statusBarsPadding().padding(top = 12.dp), contentAlignment = Alignment.TopCenter) {
                    Text(t("只能看，不能操作：{{reason}}", "reason" to tr(note)), color = Color.White, fontSize = 13.sp,
                        modifier = Modifier.padding(horizontal = 24.dp).clip(RoundedCornerShape(999.dp)).background(Color.Black.copy(alpha = 0.6f))
                            .padding(horizontal = 14.dp, vertical = 8.dp))
                }
            }
        }
        is RemoteScreen.Phase.Paused -> Card {
            Icon(Lucide.CirclePause, null, Modifier.size(32.dp), tint = Color.White)
            Text(if (p.relay) t("现在这条连接走的是中继，画面暂停") else t("正在找直连的路，画面暂停"),
                color = Color.White, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
            Text(t("远程画面只走直连，直连接通后自动接着放。"), color = Hero.Muted, fontSize = 14.sp, textAlign = TextAlign.Center)
        }
        is RemoteScreen.Phase.Closed -> Card {
            Icon(Lucide.Monitor, null, Modifier.size(32.dp), tint = Color.White)
            Text(if (p.reason == null) t("远程画面已结束") else t("远程画面断开了"), color = Color.White, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
            p.reason?.let { Text(it, color = Hero.Muted, fontSize = 14.sp, textAlign = TextAlign.Center) }
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                HeroButton(t("关闭"), { core.closeScreen() }, Modifier.weight(1f), kind = ButtonKind.Secondary)
                if (reconnectable) HeroButton(t("重新打开"), { screen.open() }, Modifier.weight(1f))
            }
        }
    }
}

@Composable
private fun Card(content: @Composable () -> Unit) {
    Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
        Column(
            Modifier.widthIn(max = 340.dp).clip(RoundedCornerShape(22.dp)).background(Hero.Surface.copy(alpha = 0.92f)).padding(24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) { content() }
    }
}

/** 平时的工具栏：关闭、键盘、操作方式、更多（换屏幕、画质、收起） */
@Composable
private fun Toolbar(core: Core, screen: RemoteScreen, onMore: () -> Unit) {
    Row(
        Modifier.navigationBarsPadding().padding(bottom = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        RoundButton(Lucide.X, t("关闭远程画面")) { core.closeScreen() }
        if (screen.canControl) {
            RoundButton(Lucide.Keyboard, t("键盘")) { screen.canvas.showKeyboard() }
            val trackpad = screen.mode == PointerMode.TRACKPAD
            RoundButton(if (trackpad) Lucide.MousePointer2 else Lucide.Pointer, if (trackpad) t("触控板模式") else t("直接点模式")) {
                screen.mode = if (trackpad) PointerMode.TOUCH else PointerMode.TRACKPAD
                core.notices.tryEmit(
                    if (screen.mode == PointerMode.TRACKPAD) t("触控板模式：手指滑动挪指针，点一下点在指针上")
                    else t("直接点模式：手指点哪儿就点哪儿"),
                )
            }
        }
        RoundButton(Lucide.Ellipsis, t("更多"), onMore)
    }
}

/** 画面上浮着的圆按钮：半透明黑底，不抢画面 */
@Composable
private fun RoundButton(icon: ImageVector, label: String, onClick: () -> Unit) {
    Box(
        Modifier.size(48.dp).clip(RoundedCornerShape(24.dp)).background(Color.Black.copy(alpha = 0.55f)).clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { Icon(icon, label, Modifier.size(21.dp), tint = Color.White) }
}

/** 键盘弹着时的按键条：手机键盘上没有的键。修饰键按一下粘住，下一个键或点击带上它 */
@Composable
private fun KeyBar(screen: RemoteScreen) {
    Row(
        Modifier.fillMaxWidth().background(Hero.Background).horizontalScroll(rememberScrollState()).padding(horizontal = 6.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Key("Esc") { screen.tapKey("Escape") }
        Key("Tab") { screen.tapKey("Tab") }
        ModKey("Ctrl", "ControlLeft", screen)
        ModKey(if (screen.isMac) "⌥" else "Alt", "AltLeft", screen)
        ModKey(if (screen.isMac) "⌘" else "Win", "MetaLeft", screen)
        ModKey("⇧", "ShiftLeft", screen)
        Key("←", Lucide.ArrowLeft) { screen.tapKey("ArrowLeft") }
        Key("↑", Lucide.ArrowUp) { screen.tapKey("ArrowUp") }
        Key("↓", Lucide.ArrowDown) { screen.tapKey("ArrowDown") }
        Key("→", Lucide.ArrowRight) { screen.tapKey("ArrowRight") }
        Key(t("删除"), Lucide.Delete) { screen.tapKey("Delete") }
        Key(t("收起键盘"), Lucide.KeyboardOff) { screen.canvas.hideKeyboard() }
    }
}

@Composable
private fun ModKey(label: String, code: String, screen: RemoteScreen) {
    Key(label, active = code in screen.modifiers) { screen.toggleModifier(code) }
}

/** 一个键：有图标画图标（读屏念 label），没有就写 label。不要涟漪、不抢焦点，键盘不会收起 */
@Composable
private fun Key(label: String, icon: ImageVector? = null, active: Boolean = false, onClick: () -> Unit) {
    val source = remember { MutableInteractionSource() }
    val color = if (active) Hero.Accent else Hero.Foreground
    Box(
        Modifier.height(40.dp).widthIn(min = 46.dp).clip(RoundedCornerShape(10.dp))
            .background(if (active) Hero.AccentSoft.copy(alpha = 0.22f) else Hero.Tile)
            .clickable(interactionSource = source, indication = null, onClick = onClick)
            .padding(horizontal = 10.dp),
        contentAlignment = Alignment.Center,
    ) {
        if (icon != null) Icon(icon, label, Modifier.size(18.dp), tint = color)
        else Text(label, color = color, fontFamily = Hero.Mono, fontSize = 13.sp, fontWeight = FontWeight.Medium, maxLines = 1)
    }
}

@Composable
private fun SheetLabel(text: String) {
    Text(text, color = Hero.Muted, fontSize = 12.5.sp, modifier = Modifier.padding(start = 10.dp, top = 6.dp, bottom = 2.dp))
}

fun qualityName(q: ScreenQuality): String = when (q) {
    ScreenQuality.SHARP -> t("清晰")
    ScreenQuality.BALANCED -> t("均衡")
    ScreenQuality.SMOOTH -> t("流畅")
}

private fun qualityNote(q: ScreenQuality): String = when (q) {
    ScreenQuality.SHARP -> t("原始分辨率，看小字")
    ScreenQuality.BALANCED -> t("最高 1080p")
    ScreenQuality.SMOOTH -> t("720p、30 帧，网差时用")
}

private fun Context.findActivity(): Activity? {
    var c: Context? = this
    while (c is ContextWrapper) {
        if (c is Activity) return c
        c = c.baseContext
    }
    return null
}

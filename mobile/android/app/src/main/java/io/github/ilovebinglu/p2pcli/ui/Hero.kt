package io.github.ilovebinglu.p2pcli.ui

import android.annotation.SuppressLint
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.scale
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.ilovebinglu.p2pcli.R

/**
 * 和桌面端同一套样式：颜色取自桌面用的 HeroUI 深色主题（@heroui/styles 里的 OKLCH 换成 sRGB），
 * 圆角、间距、字号和手机网页版（app/src/mobile/mobile.css）一致。桌面只有深色，这里也只有深色。
 */
object Hero {
    val Background = Color(0xFF060607)
    val Surface = Color(0xFF18181B)
    val SurfaceSecondary = Color(0xFF232325)
    /** 次要按钮、输入框的底色 */
    val Default = Color(0xFF27272A)
    val Foreground = Color(0xFFFCFCFC)
    val Muted = Color(0xFF9F9FA9)
    val Accent = Color(0xFF0485F7)
    val Success = Color(0xFF17C964)
    val Warning = Color(0xFFF5A524)
    val Danger = Color(0xFFDB3B3E)
    val Border = Color(0xFF28282C)
    val Terminal = Color(0xFF111114)
    /** 分隔线、按下时的底色、图标块的底色：前景色加一点透明度 */
    val Line = Foreground.copy(alpha = 0.09f)
    val Press = Foreground.copy(alpha = 0.07f)
    val Tile = Foreground.copy(alpha = 0.08f)
    val AccentSoft = Accent.copy(alpha = 0.16f)
    val DangerSoft = Danger.copy(alpha = 0.16f)
    val WarningSoft = Warning.copy(alpha = 0.14f)
    val Mono = FontFamily.Monospace
}

@Composable
fun HeroTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = darkColorScheme(
            primary = Hero.Accent,
            onPrimary = Color.White,
            background = Hero.Background,
            onBackground = Hero.Foreground,
            surface = Hero.Surface,
            onSurface = Hero.Foreground,
            surfaceVariant = Hero.Default,
            onSurfaceVariant = Hero.Muted,
            surfaceContainer = Hero.Surface,
            surfaceContainerHigh = Hero.Surface,
            surfaceContainerLow = Hero.Surface,
            secondaryContainer = Hero.Default,
            onSecondaryContainer = Hero.Foreground,
            error = Hero.Danger,
            outline = Hero.Border,
            outlineVariant = Hero.Line,
        ),
        content = content,
    )
}

/** 按下时缩一点，和 HeroUI 的按钮一样 */
@Composable
private fun Modifier.pressable(enabled: Boolean, onClick: () -> Unit, pressedColor: Color? = null, shape: RoundedCornerShape? = null): Modifier {
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val scale by animateFloatAsState(if (pressed && enabled) 0.97f else 1f, tween(120), label = "press")
    var m = this.scale(scale)
    if (pressed && pressedColor != null) m = m.background(pressedColor, shape ?: RoundedCornerShape(0.dp))
    return m.clickable(interactionSource = source, indication = null, enabled = enabled, onClick = onClick)
}

enum class ButtonKind { Primary, Secondary, Ghost, Danger }

@Composable
fun HeroButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    kind: ButtonKind = ButtonKind.Primary,
    icon: ImageVector? = null,
    enabled: Boolean = true,
    loading: Boolean = false,
    height: Dp = 48.dp,
) {
    val (bg, fg) = when (kind) {
        ButtonKind.Primary -> Hero.Accent to Color.White
        ButtonKind.Secondary -> Hero.Default to Hero.Foreground
        ButtonKind.Ghost -> Color.Transparent to Hero.Foreground
        ButtonKind.Danger -> Hero.DangerSoft to Hero.Danger
    }
    val shape = RoundedCornerShape(14.dp)
    Row(
        modifier
            .heightIn(min = height)
            .clip(shape)
            .background(if (enabled) bg else bg.copy(alpha = bg.alpha * 0.5f))
            .pressable(enabled && !loading, onClick, Hero.Press, shape)
            .padding(horizontal = 18.dp),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val color = if (enabled) fg else fg.copy(alpha = 0.5f)
        if (loading) Spinner(18.dp, color) else icon?.let { Icon(it, null, Modifier.size(19.dp), tint = color) }
        if (loading || icon != null) Spacer(Modifier.width(8.dp))
        Text(text, color = color, fontSize = 15.5.sp, fontWeight = FontWeight.Medium)
    }
}

/** 顶栏、行尾的图标按钮：44dp 好点 */
@Composable
fun HeroIconButton(icon: ImageVector, description: String, onClick: () -> Unit, tint: Color = Hero.Foreground, enabled: Boolean = true, size: Dp = 22.dp) {
    val shape = RoundedCornerShape(14.dp)
    Box(
        Modifier.size(44.dp).clip(shape).pressable(enabled, onClick, Hero.Press, shape),
        contentAlignment = Alignment.Center,
    ) { Icon(icon, description, Modifier.size(size), tint = if (enabled) tint else tint.copy(alpha = 0.4f)) }
}

@Composable
fun Spinner(size: Dp = 18.dp, color: Color = Hero.Muted) {
    CircularProgressIndicator(Modifier.size(size), color = color, strokeWidth = 2.dp, trackColor = Color.Transparent)
}

/** 二级页面的顶栏：返回、标题和一行状态、右边的按钮 */
@Composable
fun NavBar(
    title: @Composable RowScope.() -> Unit,
    onBack: () -> Unit,
    subtitle: (@Composable RowScope.() -> Unit)? = null,
    actions: @Composable RowScope.() -> Unit = {},
) {
    Row(
        Modifier.fillMaxWidth().heightIn(min = 56.dp).bottomLine().padding(horizontal = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        HeroIconButton(Lucide.ChevronLeft, "返回", onBack)
        Column(Modifier.weight(1f).padding(start = 2.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp), content = title)
            subtitle?.let { Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp), content = it) }
        }
        actions()
    }
}

@Composable
fun NavTitle(text: String) {
    Text(text, color = Hero.Foreground, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
}

fun Modifier.bottomLine(): Modifier = drawBehind {
    drawLine(Hero.Line, Offset(0f, size.height - 0.5f), Offset(size.width, size.height - 0.5f), 1f)
}

fun Modifier.topLine(): Modifier = drawBehind {
    drawLine(Hero.Line, Offset(0f, 0.5f), Offset(size.width, 0.5f), 1f)
}

/** 页面底下固定的主按钮区 */
@Composable
fun BottomBar(content: @Composable ColumnScope.() -> Unit) {
    Column(
        Modifier.fillMaxWidth().topLine().background(Hero.Background).navigationBarsPadding().padding(horizontal = 16.dp, vertical = 10.dp),
        content = content,
    )
}

/** 分组标题，右边可以放一个小按钮 */
@Composable
fun SectionHeader(title: String, action: (@Composable () -> Unit)? = null) {
    Row(
        Modifier.fillMaxWidth().padding(start = 4.dp, top = 22.dp, bottom = if (action != null) 2.dp else 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(title, color = Hero.Muted, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
        action?.invoke()
    }
}

/** 一组圆角卡片里的一行行 */
@Composable
fun Group(content: @Composable ColumnScope.() -> Unit) {
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(Hero.Surface), content = content)
}

@Composable
fun GroupDivider() {
    Box(Modifier.fillMaxWidth().height(1.dp).background(Hero.Line))
}

/** 分组里的一行：左边图标，中间几行字，右边箭头或按钮 */
@Composable
fun HeroRow(
    onClick: (() -> Unit)?,
    leading: @Composable () -> Unit,
    title: @Composable RowScope.() -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    subtitle: (@Composable ColumnScope.() -> Unit)? = null,
    trailing: (@Composable () -> Unit)? = { Icon(Lucide.ChevronRight, null, Modifier.size(18.dp), tint = Hero.Muted.copy(alpha = 0.7f)) },
) {
    val base = modifier.fillMaxWidth().heightIn(min = 64.dp)
    Row(
        (if (onClick != null) base.pressable(enabled, onClick, Hero.Press) else base)
            .alpha(if (enabled) 1f else 0.45f)
            .padding(start = 14.dp, end = 12.dp, top = 11.dp, bottom = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        leading()
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp), content = title)
            subtitle?.invoke(this)
        }
        trailing?.invoke()
    }
}

@Composable
fun RowTitle(text: String, color: Color = Hero.Foreground, modifier: Modifier = Modifier) {
    Text(text, color = color, fontSize = 15.5.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = modifier)
}

@Composable
fun SubText(text: String, color: Color = Hero.Muted, mono: Boolean = false, size: TextUnit = 13.sp, maxLines: Int = 1) {
    Text(text, color = color, fontSize = size, fontFamily = if (mono) Hero.Mono else null, maxLines = maxLines, overflow = TextOverflow.Ellipsis)
}

/** 圆角方块里放一个图标；connected 时右下角一个绿点 */
@Composable
fun Tile(size: Dp = 42.dp, connected: Boolean = false, dimmed: Boolean = false, content: @Composable BoxScope.() -> Unit) {
    val radius = if (size > 60.dp) 24.dp else 13.dp
    Box {
        Box(
            Modifier.size(size).clip(RoundedCornerShape(radius)).background(Hero.Tile),
            contentAlignment = Alignment.Center,
            content = content,
        )
        if (connected) {
            val dot = if (size > 60.dp) 18.dp else 12.dp
            Box(
                Modifier.align(Alignment.BottomEnd).padding(0.dp).size(dot)
                    .clip(RoundedCornerShape(dot / 2)).background(Hero.Surface)
                    .padding(if (size > 60.dp) 3.5.dp else 2.5.dp).clip(RoundedCornerShape(dot / 2)).background(Hero.Success),
            )
        }
        if (dimmed) Box(Modifier.size(size).clip(RoundedCornerShape(radius)).background(Hero.Background.copy(alpha = 0.4f)))
    }
}

/** 系统图标（苹果、Windows、Linux），和桌面同一套 */
@Composable
fun PlatformTile(platform: String, connected: Boolean = false, size: Dp = 42.dp) {
    Tile(size, connected) {
        val res = when (platform) {
            "macos", "ios" -> R.drawable.ic_platform_apple
            "windows" -> R.drawable.ic_platform_windows
            "linux" -> R.drawable.ic_platform_linux
            else -> null
        }
        val iconSize = if (size > 60.dp) 36.dp else 20.dp
        if (res != null) Icon(painterResource(res), platform, Modifier.size(iconSize), tint = Hero.Foreground)
        else Text(platform.take(3), color = Hero.Foreground, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
    }
}

/** 命令行 AI 的图标（和桌面同一套）；没收录的用机器人图标 */
@SuppressLint("DiscouragedApi")
@Composable
fun AgentIcon(kind: String, size: Dp = 20.dp) {
    val context = LocalContext.current
    val id = remember(kind) { context.resources.getIdentifier("agent_$kind", "drawable", context.packageName) }
    if (id != 0) Image(painterResource(id), kind, Modifier.size(size))
    else Icon(Lucide.Bot, kind, Modifier.size(size), tint = Hero.Foreground)
}

/** 小标签：“已打开”“局域网” */
@Composable
fun Chip(text: String, accent: Boolean = false) {
    Text(
        text,
        color = if (accent) Hero.Accent else Hero.Muted,
        fontSize = 11.sp,
        fontWeight = FontWeight.Medium,
        modifier = Modifier.clip(RoundedCornerShape(999.dp)).background(if (accent) Hero.AccentSoft else Hero.Tile)
            .padding(horizontal = 7.dp, vertical = 1.dp),
    )
}

@Composable
fun Dot(color: Color, size: Dp = 7.dp) {
    Box(Modifier.size(size).clip(RoundedCornerShape(size / 2)).background(color))
}

/** 从屏幕底部弹出的面板 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HeroSheet(onDismiss: () -> Unit, title: String? = null, content: @Composable ColumnScope.() -> Unit) {
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = Hero.Surface,
        contentColor = Hero.Foreground,
        scrimColor = Color.Black.copy(alpha = 0.66f),
        shape = RoundedCornerShape(topStart = 24.dp, topEnd = 24.dp),
        dragHandle = {
            Box(Modifier.padding(top = 8.dp, bottom = 12.dp).size(38.dp, 5.dp).clip(RoundedCornerShape(3.dp)).background(Hero.Foreground.copy(alpha = 0.22f)))
        },
    ) {
        Column(Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, bottom = 18.dp).navigationBarsPadding()) {
            title?.let {
                Text(it, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center, maxLines = 1,
                    overflow = TextOverflow.Ellipsis, modifier = Modifier.fillMaxWidth().padding(bottom = 14.dp))
            }
            content()
        }
    }
}

/** 面板里一行可以点的操作 */
@Composable
fun SheetAction(icon: ImageVector, label: String, onClick: () -> Unit, description: String? = null, danger: Boolean = false, enabled: Boolean = true) {
    val color = if (danger) Hero.Danger else Hero.Foreground
    val shape = RoundedCornerShape(14.dp)
    Row(
        Modifier.fillMaxWidth().heightIn(min = 54.dp).clip(shape).pressable(enabled, onClick, Hero.Press, shape).padding(10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Icon(icon, null, Modifier.size(21.dp), tint = if (enabled) color else color.copy(alpha = 0.45f))
        Column {
            Text(label, color = if (enabled) color else color.copy(alpha = 0.45f), fontSize = 15.5.sp)
            description?.let { Text(it, color = Hero.Muted, fontSize = 12.5.sp) }
        }
    }
}

/** 输入框：深灰底、圆角，没有边框（桌面端也是这样） */
@Composable
fun HeroTextField(
    value: String,
    onValueChange: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier,
    password: Boolean = false,
    keyboardType: KeyboardType = KeyboardType.Text,
    imeAction: ImeAction = ImeAction.Done,
    onImeAction: () -> Unit = {},
    enabled: Boolean = true,
    singleLine: Boolean = true,
    radius: Dp = 12.dp,
    background: Color = Hero.Default,
) {
    BasicTextField(
        value = value,
        onValueChange = onValueChange,
        enabled = enabled,
        singleLine = singleLine,
        maxLines = if (singleLine) 1 else 4,
        textStyle = TextStyle(color = Hero.Foreground, fontSize = 16.sp),
        cursorBrush = SolidColor(Hero.Accent),
        visualTransformation = if (password) PasswordVisualTransformation() else VisualTransformation.None,
        keyboardOptions = KeyboardOptions(keyboardType = keyboardType, imeAction = imeAction),
        keyboardActions = KeyboardActions(onAny = { onImeAction() }),
        modifier = modifier,
        decorationBox = { inner ->
            Box(
                Modifier.fillMaxWidth().heightIn(min = 46.dp).clip(RoundedCornerShape(radius)).background(background)
                    .padding(horizontal = 14.dp, vertical = 12.dp),
                contentAlignment = Alignment.CenterStart,
            ) {
                if (value.isEmpty()) Text(placeholder, color = Hero.Muted, fontSize = 16.sp)
                inner()
            }
        },
    )
}

/** 9 位设备码：三格一组，和桌面的输入框一样 */
@Composable
fun CodeField(value: String, onValueChange: (String) -> Unit, enabled: Boolean = true) {
    BasicTextField(
        value = value,
        onValueChange = { v -> onValueChange(v.filter { it.isDigit() }.take(9)) },
        enabled = enabled,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword, imeAction = ImeAction.Next),
        textStyle = TextStyle(color = Color.Transparent),
        cursorBrush = SolidColor(Color.Transparent),
        decorationBox = { _ ->
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                for (group in 0 until 3) {
                    if (group > 0) Box(Modifier.size(8.dp, 2.dp).background(Hero.Muted.copy(alpha = 0.6f)))
                    for (i in group * 3 until group * 3 + 3) {
                        val ch = value.getOrNull(i)
                        val focused = i == value.length
                        Box(
                            Modifier.weight(1f).height(48.dp).clip(RoundedCornerShape(12.dp)).background(Hero.Default)
                                .then(if (focused && enabled) Modifier.border(1.5.dp, Hero.Accent, RoundedCornerShape(12.dp)) else Modifier),
                            contentAlignment = Alignment.Center,
                        ) { Text(ch?.toString() ?: "", color = Hero.Foreground, fontSize = 20.sp, fontWeight = FontWeight.Medium) }
                    }
                }
            }
        },
    )
}

/** 黄色提醒条 */
@Composable
fun Banner(text: String, onClick: (() -> Unit)? = null) {
    val shape = RoundedCornerShape(14.dp)
    Row(
        Modifier.fillMaxWidth().padding(top = 10.dp).clip(shape).background(Hero.WarningSoft)
            .then(if (onClick != null) Modifier.pressable(true, onClick, Hero.Press, shape) else Modifier)
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(Lucide.TriangleAlert, null, Modifier.size(18.dp), tint = Hero.Warning)
        Text(text, color = Hero.Warning, fontSize = 13.5.sp, modifier = Modifier.weight(1f))
    }
}

/** 空的时候：一句标题、一段说明 */
@Composable
fun EmptyMessage(title: String, text: String, content: @Composable ColumnScope.() -> Unit = {}) {
    Column(
        Modifier.fillMaxWidth().padding(top = 40.dp, start = 8.dp, end = 8.dp, bottom = 24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(title, color = Hero.Foreground, fontSize = 22.sp, fontWeight = FontWeight.Bold)
        Text(text, color = Hero.Muted, fontSize = 14.5.sp, textAlign = TextAlign.Center, lineHeight = 22.sp,
            modifier = Modifier.padding(top = 8.dp, bottom = 28.dp).width(300.dp))
        Column(Modifier.width(320.dp), verticalArrangement = Arrangement.spacedBy(10.dp), content = content)
    }
}

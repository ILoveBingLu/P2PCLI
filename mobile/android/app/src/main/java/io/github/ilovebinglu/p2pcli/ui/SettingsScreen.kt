package io.github.ilovebinglu.p2pcli.ui

import io.github.ilovebinglu.p2pcli.t
import io.github.ilovebinglu.p2pcli.tr
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
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
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.ilovebinglu.p2pcli.Core
import io.github.ilovebinglu.p2pcli.I18n
import uniffi.p2pcli_mobile.MobileSettings
import uniffi.p2pcli_mobile.SettingsView

private const val REPO_URL = "https://github.com/ILoveBingLu/P2PCLI"

/**
 * 设置：只放手机用得上的。手机只当连接的一方，桌面上给“被连的电脑”用的设置
 * （AI 启动、多机互动、受信设备、配对密码、局域网里显示本机、系统权限）这里都没有。
 */
@Composable
fun SettingsScreen(core: Core) {
    val view by core.settings.collectAsStateWithLifecycle()
    val saving by core.savingSettings.collectAsStateWithLifecycle()
    val v = view
    Column(Modifier.fillMaxSize().background(Hero.Background).statusBarsPadding().imePadding()) {
        NavBar(title = { NavTitle(t("设置")) }, onBack = { core.back() })
        if (v == null) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Spinner() }
            return@Column
        }
        SettingsForm(core, v, saving)
    }
}

@Composable
private fun ColumnScope.SettingsForm(core: Core, v: SettingsView, saving: Boolean) {
    val context = LocalContext.current
    val s = v.settings
    var name by remember(v) { mutableStateOf(s.deviceName ?: "") }
    // 牵线服务器：选了预设就是它的地址；不在预设里的算“自定义”
    val initialServer = s.server ?: v.defaultServer
    val isPreset = v.serverPresets.any { it.address == initialServer }
    var server by remember(v) { mutableStateOf(if (isPreset) initialServer else "") }
    var custom by remember(v) { mutableStateOf(!isPreset) }
    var customServer by remember(v) { mutableStateOf(if (isPreset) "" else initialServer) }
    var relay by remember(v) { mutableStateOf(s.relay ?: "") }
    var proxyOn by remember(v) { mutableStateOf(s.proxy != null) }
    var proxy by remember(v) { mutableStateOf(s.proxy ?: "") }
    var error by remember(v) { mutableStateOf<String?>(null) }

    val chosenServer = if (custom) customServer.trim() else server
    val next = MobileSettings(
        deviceName = name.trim().ifEmpty { null },
        // 选的就是默认服务器时不记，以后默认换了跟着换
        server = chosenServer.ifEmpty { null }.takeIf { it != v.defaultServer },
        relay = relay.trim().ifEmpty { null },
        proxy = if (proxyOn) proxy.trim().ifEmpty { null } else null,
    )
    val dirty = next != s

    LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 28.dp)) {
        item { SectionHeader(t("界面语言")) }
        item {
            // 改了马上生效，不用点保存（和桌面一样）
            Group {
                val options = listOf(
                    I18n.Language.SYSTEM to t("跟随系统"),
                    I18n.Language.CHINESE to "简体中文",
                    I18n.Language.ENGLISH to "English",
                )
                options.forEachIndexed { i, (lang, label) ->
                    if (i > 0) GroupDivider()
                    ChoiceRow(
                        selected = I18n.language == lang,
                        title = label,
                        lines = if (lang == I18n.Language.SYSTEM) listOf(t("跟随系统时，中文环境用简体中文，其他环境用英文。")) else emptyList(),
                        onClick = { I18n.setLanguage(context, lang) },
                    )
                }
            }
        }

        item { SectionHeader(t("本机")) }
        item {
            FieldCard(t("设备名称"), t("对方电脑上看到的名字，留空用手机型号。")) {
                HeroTextField(name, { name = it; error = null }, v.defaultDeviceName, enabled = !saving)
            }
        }

        item { SectionHeader(t("牵线服务器")) }
        item {
            Group {
                v.serverPresets.forEachIndexed { i, p ->
                    if (i > 0) GroupDivider()
                    ChoiceRow(
                        selected = !custom && server == p.address,
                        title = p.label,
                        badge = if (p.address == v.defaultServer) t("默认") else null,
                        lines = listOf(p.note, p.address),
                        onClick = { custom = false; server = p.address; error = null },
                    )
                }
                GroupDivider()
                ChoiceRow(
                    selected = custom,
                    title = t("自定义"),
                    lines = listOf(t("填自己部署的牵线服务器：公钥@IP或域名:端口 走 UDP，wss://域名 走 WebSocket。")),
                    onClick = { custom = true; error = null },
                )
                if (custom) {
                    Box(Modifier.padding(start = 14.dp, end = 14.dp, bottom = 14.dp)) {
                        HeroTextField(customServer, { customServer = it; error = null }, t("公钥@IP:端口 或 wss://域名"),
                            keyboardType = KeyboardType.Uri, enabled = !saving)
                    }
                }
            }
        }

        item { SectionHeader(t("中继服务器")) }
        item {
            FieldCard(null, t("直连失败时转发加密数据，多个地址用逗号分隔。留空用公共中继。")) {
                HeroTextField(relay, { relay = it; error = null }, "https://relay.example.com", keyboardType = KeyboardType.Uri, enabled = !saving)
            }
        }

        item { SectionHeader(t("代理")) }
        item {
            Group {
                Row(
                    Modifier.fillMaxWidth().clickable(enabled = !saving) { proxyOn = !proxyOn; error = null }.padding(14.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                        RowTitle(t("使用 HTTP 代理"))
                        SubText(t("用于 WebSocket 牵线和 HTTP(S) 中继连接，点对点直连不经过代理。"), maxLines = 3)
                    }
                    HeroSwitch(proxyOn)
                }
                if (proxyOn) {
                    Column(Modifier.padding(start = 14.dp, end = 14.dp, bottom = 14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        HeroTextField(proxy, { proxy = it; error = null }, "http://127.0.0.1:7890", keyboardType = KeyboardType.Uri, enabled = !saving)
                        SubText(t("填 http:// 开头的地址；暂不支持 SOCKS5 和账号密码认证。"), size = 12.5.sp, maxLines = 3)
                    }
                }
            }
        }

        item { SectionHeader(t("文件")) }
        item {
            FieldCard(t("收到的文件放在"), null) {
                Text(v.inboxDir, color = Hero.Foreground, fontFamily = Hero.Mono, fontSize = 12.sp,
                    modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(Hero.Default).padding(10.dp))
            }
        }

        item { SectionHeader(t("关于")) }
        item {
            Group {
                InfoRow(t("版本"), v.appVersion ?: t("未知"))
                GroupDivider()
                InfoRow(t("本机公钥"), v.endpointId.take(10) + "…" + v.endpointId.takeLast(6), icon = Lucide.Copy) {
                    val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                    cm.setPrimaryClip(ClipData.newPlainText("P2PCLI", v.endpointId))
                    core.notices.tryEmit(t("公钥已复制"))
                }
                GroupDivider()
                InfoRow(t("GitHub 项目"), "", icon = Lucide.SquareArrowOutUpRight) { open(context, REPO_URL) }
                GroupDivider()
                InfoRow(t("反馈问题"), "", icon = Lucide.SquareArrowOutUpRight) { open(context, "$REPO_URL/issues") }
                GroupDivider()
                InfoRow(t("版本发布"), "", icon = Lucide.SquareArrowOutUpRight) { open(context, "$REPO_URL/releases") }
            }
        }
    }

    BottomBar {
        error?.let {
            Text(it, color = Hero.Danger, fontSize = 13.5.sp, modifier = Modifier.fillMaxWidth().padding(bottom = 10.dp)
                .clip(RoundedCornerShape(12.dp)).background(Hero.DangerSoft).padding(horizontal = 14.dp, vertical = 10.dp))
        }
        Text(t("保存后马上生效。改了代理或中继会重新连网络，开着的终端会断开，对方电脑上的会话还在。"),
            color = Hero.Muted, fontSize = 12.sp, modifier = Modifier.padding(bottom = 10.dp))
        HeroButton(if (saving) t("正在保存…") else t("保存"), { core.saveSettings(next) { error = it } }, Modifier.fillMaxWidth(),
            enabled = dirty && !saving, loading = saving)
    }
}

@Composable
private fun FieldCard(label: String?, hint: String?, content: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(Hero.Surface).padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        label?.let { Text(it, color = Hero.Foreground, fontSize = 14.sp, fontWeight = FontWeight.Medium) }
        content()
        hint?.let { SubText(it, size = 12.5.sp, maxLines = 3) }
    }
}

@Composable
private fun ChoiceRow(selected: Boolean, title: String, lines: List<String>, onClick: () -> Unit, badge: String? = null) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Radio(selected)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                RowTitle(title)
                badge?.let { Chip(it) }
            }
            lines.forEachIndexed { i, line -> SubText(line, mono = i > 0, size = if (i > 0) 12.sp else 13.sp, maxLines = 3) }
        }
    }
}

@Composable
private fun Radio(selected: Boolean) {
    Box(
        Modifier.size(20.dp).clip(RoundedCornerShape(10.dp)).background(if (selected) Hero.Accent else Hero.Default),
        contentAlignment = Alignment.Center,
    ) { if (selected) Box(Modifier.size(8.dp).clip(RoundedCornerShape(4.dp)).background(Color.White)) }
}

/** 开关：和桌面 HeroUI 的样子一样，蓝底白圆 */
@Composable
private fun HeroSwitch(on: Boolean) {
    val offset by animateDpAsState(if (on) 20.dp else 2.dp, label = "switch")
    Box(Modifier.size(44.dp, 26.dp).clip(RoundedCornerShape(13.dp)).background(if (on) Hero.Accent else Hero.Default)) {
        Box(Modifier.offset(x = offset, y = 3.dp).size(20.dp).clip(RoundedCornerShape(10.dp)).background(Color.White))
    }
}

@Composable
private fun InfoRow(title: String, value: String, icon: androidx.compose.ui.graphics.vector.ImageVector? = null, onClick: (() -> Unit)? = null) {
    Row(
        Modifier.fillMaxWidth().then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier).padding(horizontal = 14.dp, vertical = 15.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text(title, color = Hero.Foreground, fontSize = 15.sp, modifier = Modifier.weight(1f))
        if (value.isNotEmpty()) Text(value, color = Hero.Muted, fontSize = 13.sp, fontFamily = Hero.Mono)
        icon?.let { Icon(it, null, Modifier.size(16.dp), tint = Hero.Muted) }
    }
}

private fun open(context: Context, url: String) {
    context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
}

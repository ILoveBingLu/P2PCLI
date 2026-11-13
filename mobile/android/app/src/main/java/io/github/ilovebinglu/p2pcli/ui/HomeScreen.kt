package io.github.ilovebinglu.p2pcli.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LargeTopAppBar
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.ilovebinglu.p2pcli.ConnectPrompt
import io.github.ilovebinglu.p2pcli.Core
import io.github.ilovebinglu.p2pcli.terminal.RemoteTerminal
import uniffi.p2pcli_mobile.AgentState
import uniffi.p2pcli_mobile.DeviceView
import uniffi.p2pcli_mobile.ServerState
import uniffi.p2pcli_mobile.TransferState
import uniffi.p2pcli_mobile.TransferView

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(core: Core, @Suppress("UNUSED_PARAMETER") snackbar: SnackbarHostState) {
    val overview by core.overview.collectAsStateWithLifecycle()
    val conns by core.conns.collectAsStateWithLifecycle()
    val terms by core.terms.collectAsStateWithLifecycle()
    val sessions by core.sessions.collectAsStateWithLifecycle()
    val connecting by core.connecting.collectAsStateWithLifecycle()
    val transfers by core.transfers.collectAsStateWithLifecycle()
    val scroll = TopAppBarDefaults.exitUntilCollapsedScrollBehavior()
    val devices = overview?.devices.orEmpty()
    val nearby = overview?.nearby.orEmpty()

    Scaffold(
        modifier = Modifier.nestedScroll(scroll.nestedScrollConnection),
        topBar = {
            LargeTopAppBar(
                title = { Text("P2PCLI") },
                actions = {
                    IconButton(onClick = { core.scanNearby() }) { Icon(Icons.Default.Refresh, contentDescription = "刷新附近设备") }
                },
                scrollBehavior = scroll,
            )
        },
        floatingActionButton = {
            ExtendedFloatingActionButton(
                text = { Text("连接新设备") },
                icon = { Icon(Icons.Default.Add, contentDescription = null) },
                onClick = { core.prompt.value = ConnectPrompt() },
            )
        },
    ) { padding ->
        LazyColumn(
            Modifier.fillMaxSize(),
            contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = padding.calculateTopPadding(), bottom = padding.calculateBottomPadding() + 96.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            when (val server = overview?.server) {
                is ServerState.Offline, ServerState.None -> item {
                    Banner(if (server is ServerState.None) "还没配置牵线服务器，只能连同一个局域网里的设备" else "牵线服务器没连上，只能连同一个局域网里的设备")
                }
                else -> {}
            }
            val active = transfers.filter { it.state != TransferState.DONE && it.state != TransferState.FAILED }
            items(active, key = { "t-" + it.id }) { TransferCard(it) }

            if (terms.isNotEmpty()) {
                item { Section("正在使用") }
                items(terms, key = { "term-" + it.key }) { term ->
                    val list = sessions[term.endpointId]?.list.orEmpty()
                    val session = list.firstOrNull { it.id == term.sessionId }
                    val title = term.sessionId?.let { sessionTitles(list)[it] } ?: "新终端"
                    val device = devices.firstOrNull { it.endpointId == term.endpointId }
                    ListItem(
                        headlineContent = { Text(title) },
                        supportingContent = {
                            val state = when (term.state) {
                                RemoteTerminal.State.CLOSED -> " · 已断开"
                                else -> session?.agent?.let { " · " + agentText(it) } ?: ""
                            }
                            Text((device?.remark ?: device?.name ?: "") + state)
                        },
                        leadingContent = { PlatformBadge(device?.platform ?: "", connected = term.state != RemoteTerminal.State.CLOSED) },
                        trailingContent = { Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, contentDescription = null) },
                        modifier = Modifier.clickable { core.openTerminal(term.endpointId, term.sessionId) },
                    )
                }
            }

            if (devices.isNotEmpty()) {
                item { Section("我的设备") }
                items(devices, key = { "d-" + it.endpointId }) { d ->
                    DeviceRow(d, connected = conns.containsKey(d.endpointId), busy = connecting == d.endpointId,
                        liveSessions = sessions[d.endpointId]?.list, onClick = { core.openDevice(d) })
                }
            }
            item { Section("附近设备") }
            if (nearby.isEmpty()) {
                item { Text("同一个局域网里没找到别的设备", color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(8.dp)) }
            }
            items(nearby, key = { "n-" + it.endpointId }) { d ->
                DeviceRow(d, connected = conns.containsKey(d.endpointId), busy = connecting == d.endpointId,
                    liveSessions = null, onClick = { core.openDevice(d) })
            }
            if (devices.isEmpty() && overview != null) {
                item {
                    Column(Modifier.fillMaxWidth().padding(top = 32.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                        Text("连接你的电脑", style = MaterialTheme.typography.titleLarge)
                        Text(
                            "在电脑上打开 P2PCLI，点右下角“连接新设备”，输入它显示的设备码和配对密码。",
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(top = 8.dp, start = 16.dp, end = 16.dp),
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun DeviceRow(d: DeviceView, connected: Boolean, busy: Boolean, liveSessions: List<uniffi.p2pcli_mobile.SessionView>?, onClick: () -> Unit) {
    val (status, color) = presenceOf(d, connected)
    val sessions = liveSessions ?: d.sessions
    val waiting = sessions.count { it.agent?.state == AgentState.WAITING }
    ListItem(
        headlineContent = { Text(d.remark ?: d.name, fontWeight = FontWeight.Medium) },
        supportingContent = {
            Column {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(if (busy) "等对方同意…" else status, color = if (busy) MaterialTheme.colorScheme.primary else color ?: MaterialTheme.colorScheme.onSurfaceVariant)
                    if (sessions.isNotEmpty()) Text("${sessions.size} 个会话", color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                if (waiting > 0) Text("$waiting 个 AI 在等你确认", color = Warn)
            }
        },
        leadingContent = { PlatformBadge(d.platform, connected) },
        trailingContent = {
            if (busy) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
            else Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, contentDescription = null)
        },
        modifier = Modifier.clickable(enabled = !busy, onClick = onClick),
    )
}

@Composable
fun Section(title: String) {
    Text(
        title,
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(start = 8.dp, top = 16.dp, bottom = 4.dp),
    )
}

@Composable
fun Banner(text: String) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.tertiaryContainer)) {
        Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Icon(Icons.Default.Warning, contentDescription = null)
            Text(text)
        }
    }
}

@Composable
fun TransferCard(t: TransferView) {
    Card {
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(t.label, fontWeight = FontWeight.Medium)
            val who = if (t.outgoing) "发给 ${t.peerName}" else "来自 ${t.peerName}"
            val progress = if (t.total > 0uL) (t.done.toDouble() / t.total.toDouble()).toFloat().coerceIn(0f, 1f) else 0f
            Text(
                when (t.state) {
                    TransferState.WAITING -> "等 ${t.peerName} 同意"
                    TransferState.CONNECTING -> "正在连接 ${t.peerName}"
                    else -> "$who · ${(progress * 100).toInt()}%"
                },
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            LinearProgressIndicator(progress = { progress }, modifier = Modifier.fillMaxWidth())
        }
    }
}

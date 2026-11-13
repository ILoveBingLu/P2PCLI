package io.github.ilovebinglu.p2pcli.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SuggestionChip
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.ilovebinglu.p2pcli.Core
import io.github.ilovebinglu.p2pcli.terminal.RemoteTerminal
import kotlinx.coroutines.delay
import uniffi.p2pcli_mobile.SessionView
import uniffi.p2pcli_mobile.TransferState

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DeviceScreen(core: Core, endpointId: String, @Suppress("UNUSED_PARAMETER") snackbar: SnackbarHostState) {
    val conns by core.conns.collectAsStateWithLifecycle()
    val overview by core.overview.collectAsStateWithLifecycle()
    val sessionsMap by core.sessions.collectAsStateWithLifecycle()
    val terms by core.terms.collectAsStateWithLifecycle()
    val transfers by core.transfers.collectAsStateWithLifecycle()
    val conn = conns[endpointId]
    val device = overview?.devices?.firstOrNull { it.endpointId == endpointId } ?: overview?.nearby?.firstOrNull { it.endpointId == endpointId }
    val name = device?.remark ?: device?.name ?: conn?.name ?: "设备"
    val state = sessionsMap[endpointId]
    val list = state?.list.orEmpty().sortedByDescending { it.createdAt }
    val titles = sessionTitles(state?.list.orEmpty())
    val openIds = terms.filter { it.endpointId == endpointId && it.state != RemoteTerminal.State.CLOSED }.mapNotNull { it.sessionId }.toSet()
    var menu by remember { mutableStateOf(false) }
    var ending by remember { mutableStateOf<SessionView?>(null) }
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.GetMultipleContents()) { uris ->
        core.sendFiles(endpointId, uris)
    }

    // 会话列表和 AI 状态隔几秒刷新一次
    LaunchedEffect(endpointId, conn?.connId) {
        while (conn != null) {
            core.loadSessions(endpointId)
            delay(3000)
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(
                            if (conn != null) "已连接 · " + pathText(conn.path) else "没有连着",
                            style = MaterialTheme.typography.bodySmall,
                            color = if (conn != null) Ok else MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                },
                navigationIcon = { IconButton(onClick = { core.back() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "返回") } },
                actions = {
                    if (conn != null) {
                        IconButton(onClick = { pickFiles.launch("*/*") }) { Icon(Icons.Default.Share, contentDescription = "发送文件") }
                        IconButton(onClick = { menu = true }) { Icon(Icons.Default.MoreVert, contentDescription = "更多") }
                        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                            DropdownMenuItem(text = { Text("断开连接") }, onClick = { menu = false; core.disconnect(endpointId) })
                        }
                    }
                },
            )
        },
        floatingActionButton = {
            if (conn != null) {
                ExtendedFloatingActionButton(
                    text = { Text("新建终端") },
                    icon = { Icon(Icons.Default.Add, contentDescription = null) },
                    onClick = { core.openTerminal(endpointId, null) },
                )
            }
        },
    ) { padding ->
        if (conn == null) {
            Column(Modifier.fillMaxSize().padding(padding).padding(32.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
                Text("连接断开了", style = MaterialTheme.typography.titleLarge)
                Text("后台会话还在对方电脑上运行，重新连上就能接着用。", color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(vertical = 12.dp))
                device?.let { d -> Button(onClick = { core.openDevice(d) }) { Text("重新连接") } }
            }
            return@Scaffold
        }
        LazyColumn(
            Modifier.fillMaxSize(),
            contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = padding.calculateTopPadding(), bottom = padding.calculateBottomPadding() + 96.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            val active = transfers.filter { it.state != TransferState.DONE && it.state != TransferState.FAILED }
            items(active, key = { "t-" + it.id }) { TransferCard(it) }
            item { Section("会话") }
            state?.error?.let { err -> item { Text("读不到会话列表：$err", color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(8.dp)) } }
            if (state?.list == null && state?.error == null) {
                item { Text("正在读取会话…", color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(8.dp)) }
            } else if (list.isEmpty()) {
                item { Text("这台设备上还没有会话，点“新建终端”开始用。", color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(8.dp)) }
            }
            items(list, key = { it.id }) { s ->
                ListItem(
                    headlineContent = {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Text(titles[s.id] ?: "会话", fontWeight = FontWeight.Medium)
                            if (s.id in openIds) SuggestionChip(onClick = {}, label = { Text("已打开") })
                        }
                    },
                    supportingContent = {
                        Column {
                            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                s.agent?.let { a -> Text(agentText(a), color = agentColor(a) ?: MaterialTheme.colorScheme.onSurfaceVariant) }
                                    ?: s.command?.let { Text(it, fontFamily = FontFamily.Monospace) }
                                Text(shortPath(s.currentPath), fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                            Text(relativeTime(s.createdAt) + "创建", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    },
                    trailingContent = {
                        IconButton(onClick = { ending = s }) { Icon(Icons.Default.Delete, contentDescription = "结束会话") }
                    },
                    modifier = Modifier.fillMaxWidth().clickable { core.openTerminal(endpointId, s.id) },
                )
            }
        }
    }

    ending?.let { s ->
        AlertDialog(
            onDismissRequest = { ending = null },
            title = { Text("结束“${titles[s.id] ?: "会话"}”？") },
            text = { Text("里面运行的程序会被关掉，没保存的内容会丢失。") },
            confirmButton = { TextButton(onClick = { ending = null; core.endSession(endpointId, s.id) }) { Text("结束会话", color = MaterialTheme.colorScheme.error) } },
            dismissButton = { TextButton(onClick = { ending = null }) { Text("取消") } },
        )
    }
}

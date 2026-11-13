package io.github.ilovebinglu.p2pcli.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.ilovebinglu.p2pcli.ConnectPrompt
import io.github.ilovebinglu.p2pcli.Core
import io.github.ilovebinglu.p2pcli.Screen
import io.github.ilovebinglu.p2pcli.StartState

@Composable
fun Root(core: Core) {
    val start by core.start.collectAsStateWithLifecycle()
    val stack by core.stack.collectAsStateWithLifecycle()
    val prompt by core.prompt.collectAsStateWithLifecycle()
    val fileRequest by core.fileRequest.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(core) { core.notices.collect { snackbar.showSnackbar(it) } }

    // 系统返回键、返回手势：退一页；在首页时交给系统（退到桌面）
    BackHandler(enabled = stack.size > 1) { core.back() }

    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        when (val s = start) {
            StartState.Starting -> Centered {
                CircularProgressIndicator()
                Spacer(Modifier.height(16.dp))
                Text("正在启动…")
            }
            is StartState.Failed -> Centered {
                Text("启动失败", style = MaterialTheme.typography.titleLarge)
                Spacer(Modifier.height(8.dp))
                Text(s.message, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Spacer(Modifier.height(16.dp))
                Button(onClick = { core.start() }) { Text("重试") }
            }
            StartState.Ready -> when (val top = stack.last()) {
                Screen.Home -> HomeScreen(core, snackbar)
                is Screen.Device -> DeviceScreen(core, top.endpointId, snackbar)
                is Screen.Term -> TerminalScreen(core, top.key)
            }
        }
    }

    prompt?.let { ConnectDialog(core, it) }
    fileRequest?.let { req ->
        AlertDialog(
            onDismissRequest = { core.respondFiles(false) },
            title = { Text("${req.name} 想发文件给你") },
            text = { Text("${req.files} 个文件：${req.items.joinToString("、")}") },
            confirmButton = { TextButton(onClick = { core.respondFiles(true) }) { Text("接收") } },
            dismissButton = { TextButton(onClick = { core.respondFiles(false) }) { Text("拒绝") } },
        )
    }
    // 提示放在最上面，不管在哪一页
    Column(Modifier.fillMaxSize(), verticalArrangement = Arrangement.Bottom) {
        SnackbarHost(snackbar, Modifier.padding(bottom = 96.dp))
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
private fun ConnectDialog(core: Core, prompt: ConnectPrompt) {
    val connecting by core.connecting.collectAsStateWithLifecycle()
    val busy = connecting != null
    val knownDevice = prompt.endpointId != null
    var code by remember(prompt) { mutableStateOf(if (knownDevice) prompt.target else "") }
    var password by remember(prompt) { mutableStateOf("") }
    val digits = code.filter { it.isDigit() }
    val canSubmit = !busy && (knownDevice || digits.length == 9)
    AlertDialog(
        onDismissRequest = { if (!busy) core.prompt.value = null },
        title = { Text(if (knownDevice) "连接 ${prompt.name ?: "设备"}" else "连接新设备") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                if (!knownDevice) {
                    Text("在电脑上的 P2PCLI 里能看到设备码和配对密码。", color = MaterialTheme.colorScheme.onSurfaceVariant)
                    OutlinedTextField(
                        value = code,
                        onValueChange = { v -> code = v.filter { it.isDigit() || it == ' ' || it == '-' }.take(13) },
                        label = { Text("设备码（9 位数字）") },
                        singleLine = true,
                        enabled = !busy,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                OutlinedTextField(
                    value = password,
                    onValueChange = { password = it },
                    label = { Text("配对密码") },
                    singleLine = true,
                    enabled = !busy,
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                    modifier = Modifier.fillMaxWidth(),
                )
                prompt.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                if (busy) Text("等对方在电脑上点同意…", color = MaterialTheme.colorScheme.primary)
            }
        },
        confirmButton = {
            TextButton(enabled = canSubmit, onClick = { core.submitPrompt(if (knownDevice) prompt.target else digits, password) }) {
                Text("连接")
            }
        },
        dismissButton = {
            TextButton(enabled = !busy, onClick = { core.prompt.value = null }) { Text("取消") }
        },
    )
}

package io.github.ilovebinglu.p2pcli

import android.app.Application
import android.content.Context
import android.net.Uri
import android.net.wifi.WifiManager
import android.os.Build
import android.provider.OpenableColumns
import android.provider.Settings
import io.github.ilovebinglu.p2pcli.terminal.RemoteTerminal
import io.github.ilovebinglu.p2pcli.terminal.errorText
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.p2pcli_mobile.AppListener
import uniffi.p2pcli_mobile.Connected
import uniffi.p2pcli_mobile.DeviceView
import uniffi.p2pcli_mobile.FileRequestView
import uniffi.p2pcli_mobile.MobileConfig
import uniffi.p2pcli_mobile.Overview
import uniffi.p2pcli_mobile.P2p
import uniffi.p2pcli_mobile.P2pException
import uniffi.p2pcli_mobile.PairInfo
import uniffi.p2pcli_mobile.RejectReason
import uniffi.p2pcli_mobile.SessionView
import uniffi.p2pcli_mobile.TransferView
import java.io.File

/** 一层一层进的页面 */
sealed interface Screen {
    data object Home : Screen
    data class Device(val endpointId: String) : Screen
    data class Term(val key: Int) : Screen
}

sealed interface StartState {
    data object Starting : StartState
    data object Ready : StartState
    data class Failed(val message: String) : StartState
}

/** 一台设备的会话列表；list 为空是还没读到 */
data class SessionsState(val list: List<SessionView>? = null, val error: String? = null)

/** 连接面板：新连一台设备，或者连某台设备时要填密码 */
data class ConnectPrompt(
    /** 已知是哪台设备（从列表里点的）；手输设备码时是空 */
    val endpointId: String? = null,
    val name: String? = null,
    /** 已知的设备码或公钥；手输时用户自己填 */
    val target: String = "",
    val error: String? = null,
)

/**
 * 整个 App 的状态和操作，进程活着就一直在（转屏、切到后台再回来都不丢连接）。
 * Rust 那边的接口都在这里调；界面只读这里的状态、调这里的方法。
 */
class Core(private val app: Application) : AppListener {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    @Volatile
    private var p2p: P2p? = null

    val start = MutableStateFlow<StartState>(StartState.Starting)
    val overview = MutableStateFlow<Overview?>(null)
    val transfers = MutableStateFlow<List<TransferView>>(emptyList())
    /** 连着的设备：公钥 → 连接 */
    val conns = MutableStateFlow<Map<String, Connected>>(emptyMap())
    /** 各设备的会话列表：公钥 → 列表 */
    val sessions = MutableStateFlow<Map<String, SessionsState>>(emptyMap())
    val terms = MutableStateFlow<List<RemoteTerminal>>(emptyList())
    val stack = MutableStateFlow<List<Screen>>(listOf(Screen.Home))
    /** 正在连哪台设备（公钥）；手输设备码时是 "new" */
    val connecting = MutableStateFlow<String?>(null)
    val prompt = MutableStateFlow<ConnectPrompt?>(null)
    val fileRequest = MutableStateFlow<FileRequestView?>(null)
    /** 给人看的一句提示 */
    val notices = MutableSharedFlow<String>(extraBufferCapacity = 8)

    private var termSeq = 1
    private var multicast: WifiManager.MulticastLock? = null

    fun start() {
        start.value = StartState.Starting
        // 局域网发现靠组播；安卓默认把组播包过滤掉
        if (multicast == null) {
            val wifi = app.applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            multicast = wifi?.createMulticastLock("p2pcli")?.apply { setReferenceCounted(false); acquire() }
        }
        scope.launch {
            try {
                val config = MobileConfig(
                    dataDir = File(app.filesDir, "p2pcli").absolutePath,
                    inboxDir = File(app.getExternalFilesDir(null) ?: app.filesDir, "inbox").absolutePath,
                    deviceName = deviceName(),
                    server = null,
                    relay = null,
                    proxy = null,
                    appVersion = appVersion(),
                )
                // 启动要等网络起来，不能在主线程
                p2p = withContext(Dispatchers.IO) { P2p.start(config, this@Core) }
                start.value = StartState.Ready
                refreshOverview()
            } catch (e: Exception) {
                start.value = StartState.Failed(errorText(e))
            }
        }
    }

    // ---- 页面 ----

    val top: Screen get() = stack.value.last()

    fun push(screen: Screen) = stack.update { it + screen }
    fun back() = stack.update { if (it.size > 1) it.dropLast(1) else it }
    private fun replaceTop(screen: Screen) = stack.update { it.dropLast(1) + screen }
    private fun home() = stack.update { listOf(Screen.Home) }

    // ---- Rust 的回调（Rust 的线程）----

    override fun onOverviewChanged() {
        scope.launch { refreshOverview() }
    }

    override fun onDisconnected(connId: ULong, reason: String) {
        scope.launch {
            conns.update { all -> all.filterValues { it.connId != connId } }
        }
    }

    override fun onTransfersChanged(transfers: List<TransferView>) {
        val before = this.transfers.value.associateBy { it.id }
        this.transfers.value = transfers
        for (t in transfers) {
            val old = before[t.id] ?: continue
            if (old.state == t.state || !t.outgoing) continue
            when (t.state) {
                uniffi.p2pcli_mobile.TransferState.DONE -> notices.tryEmit("已发给 ${t.peerName}：${t.label}")
                uniffi.p2pcli_mobile.TransferState.FAILED -> notices.tryEmit("没发出去：${t.error ?: t.label}")
                else -> {}
            }
        }
    }

    override fun onFileRequest(request: FileRequestView) {
        fileRequest.value = request
    }

    override fun onFileRequestCancelled(id: ULong) {
        fileRequest.update { if (it?.id == id) null else it }
    }

    fun respondFiles(accept: Boolean) {
        val req = fileRequest.value ?: return
        fileRequest.value = null
        p2p?.respondFiles(req.id, accept)
    }

    suspend fun refreshOverview() {
        val p = p2p ?: return
        overview.value = withContext(Dispatchers.IO) { p.overview() }
    }

    fun scanNearby() {
        val p = p2p ?: return
        scope.launch {
            try {
                withContext(Dispatchers.IO) { p.scanNearby() }
                refreshOverview()
            } catch (e: Exception) {
                notices.tryEmit("扫描失败：${errorText(e)}")
            }
        }
    }

    // ---- 连接 ----

    /** 点了列表里的设备：连着就进会话页，没连着先连 */
    fun openDevice(device: DeviceView) {
        if (conns.value.containsKey(device.endpointId)) return push(Screen.Device(device.endpointId))
        // 局域网里在线就按公钥直接连，不经过牵线服务器
        val target = if (device.lanOnline || device.code == null) device.endpointId else device.code!!
        connect(target, null, device.endpointId, device.remark ?: device.name)
    }

    /** 连接面板里点“连接” */
    fun submitPrompt(target: String, password: String) {
        val p = prompt.value ?: return
        connect(target.ifBlank { p.target }, password, p.endpointId, p.name)
    }

    fun connectPair(pair: PairInfo) = connect(pair.code ?: pair.id, pair.password, pair.id, pair.name.ifBlank { null })

    private fun connect(target: String, password: String?, endpointId: String?, name: String?) {
        val p = p2p ?: return
        if (connecting.value != null) return
        connecting.value = endpointId ?: "new"
        scope.launch {
            try {
                val c = p.connect(target, password)
                conns.update { it + (c.endpointId to c) }
                prompt.value = null
                push(Screen.Device(c.endpointId))
                loadSessions(c.endpointId)
                refreshOverview()
            } catch (e: P2pException.Rejected) {
                if (e.reason == RejectReason.NEED_PASSWORD || e.reason == RejectReason.BAD_PASSWORD) {
                    // 要密码：弹出面板填密码；密码不对时说一声
                    prompt.value = ConnectPrompt(
                        endpointId = endpointId,
                        name = name,
                        target = target,
                        error = if (e.reason == RejectReason.BAD_PASSWORD) e.detail else null,
                    )
                } else {
                    showError(e.detail)
                }
            } catch (e: Exception) {
                showError(errorText(e))
            } finally {
                connecting.value = null
            }
        }
    }

    /** 面板开着时错误显示在面板里，不然弹一句提示 */
    private fun showError(message: String) {
        val p = prompt.value
        if (p != null) prompt.value = p.copy(error = message) else notices.tryEmit(message)
    }

    fun disconnect(endpointId: String) {
        val c = conns.value[endpointId] ?: return
        p2p?.disconnect(c.connId)
        conns.update { it - endpointId }
        terms.value.filter { it.endpointId == endpointId }.forEach { it.close() }
        terms.update { all -> all.filter { it.endpointId != endpointId } }
        home()
    }

    // ---- 会话 ----

    suspend fun loadSessions(endpointId: String) {
        val p = p2p ?: return
        val c = conns.value[endpointId] ?: return
        try {
            val list = p.listSessions(c.connId)
            sessions.update { it + (endpointId to SessionsState(list, null)) }
        } catch (e: Exception) {
            sessions.update { it + (endpointId to SessionsState(it[endpointId]?.list, errorText(e))) }
        }
    }

    /** 按钮点的刷新：不用等下一轮 */
    fun refreshSessions(endpointId: String) {
        scope.launch { loadSessions(endpointId) }
    }

    fun endSession(endpointId: String, sessionId: String) {
        val p = p2p ?: return
        val c = conns.value[endpointId] ?: return
        scope.launch {
            try {
                p.endSession(c.connId, sessionId)
                terms.value.filter { it.endpointId == endpointId && it.sessionId == sessionId }.forEach { closeTerminal(it.key) }
                loadSessions(endpointId)
                notices.tryEmit("会话已结束")
            } catch (e: Exception) {
                notices.tryEmit("结束会话失败：${errorText(e)}")
            }
        }
    }

    // ---- 终端 ----

    /** 打开一个会话；sessionId 为空是新建。已经开着就切过去 */
    fun openTerminal(endpointId: String, sessionId: String?) {
        val p = p2p ?: return
        val c = conns.value[endpointId] ?: return
        val existing = sessionId?.let { id ->
            terms.value.firstOrNull { it.endpointId == endpointId && it.sessionId == id && it.state != RemoteTerminal.State.CLOSED }
        }
        if (existing != null) return goTerminal(existing.key)
        val term = RemoteTerminal(app, termSeq++, endpointId, c.connId, sessionId, scope) { cols, rows, listener ->
            p.openTerminal(c.connId, sessionId, cols, rows, listener)
        }
        // 同一个会话之前断开的那个换掉
        terms.update { all ->
            all.filter { sessionId == null || it.endpointId != endpointId || it.sessionId != sessionId } + term
        }
        goTerminal(term.key)
    }

    private fun goTerminal(key: Int) {
        if (top is Screen.Term) replaceTop(Screen.Term(key)) else push(Screen.Term(key))
    }

    /** 关掉手机上的这个终端，会话在对方电脑上接着跑 */
    fun closeTerminal(key: Int) {
        val t = terms.value.firstOrNull { it.key == key } ?: return
        t.close()
        terms.update { all -> all.filter { it.key != key } }
        stack.update { all -> all.filter { it != Screen.Term(key) }.ifEmpty { listOf(Screen.Home) } }
    }

    /** 断开的终端重新连：连接还在就接回同一个会话，不然先重连设备 */
    fun reopen(term: RemoteTerminal) {
        if (conns.value.containsKey(term.endpointId)) {
            closeTerminal(term.key)
            openTerminal(term.endpointId, term.sessionId)
        } else {
            val d = overview.value?.devices?.firstOrNull { it.endpointId == term.endpointId }
            closeTerminal(term.key)
            if (d != null) openDevice(d) else notices.tryEmit("找不到这台设备，请回到首页重新连接")
        }
    }

    fun chooseOption(term: RemoteTerminal, approvalId: String, optionId: ULong) {
        val p = p2p ?: return
        val session = term.sessionId ?: return
        scope.launch {
            try {
                p.chooseAgentOption(term.connId, session, approvalId, optionId)
            } catch (e: Exception) {
                notices.tryEmit(errorText(e))
            } finally {
                loadSessions(term.endpointId)
            }
        }
    }

    // ---- 传文件 ----

    /** 系统选择框给的是 content:// 地址：先复制到缓存目录，再按路径发 */
    fun sendFiles(endpointId: String, uris: List<Uri>) {
        val p = p2p ?: return
        if (uris.isEmpty()) return
        scope.launch {
            try {
                val paths = withContext(Dispatchers.IO) { uris.mapIndexed { i, uri -> stage(uri, i) } }
                withContext(Dispatchers.IO) { p.sendFiles(endpointId, paths) }
                notices.tryEmit("开始发送 ${paths.size} 个文件")
            } catch (e: Exception) {
                notices.tryEmit("发不了：${errorText(e)}")
            }
        }
    }

    private fun stage(uri: Uri, index: Int): String {
        val resolver = app.contentResolver
        val name = resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null
        }?.substringAfterLast('/')?.takeIf { it.isNotBlank() && it != "." && it != ".." } ?: "file-$index"
        // 每个文件一个子目录：同名的各放各的，发过去还是原来的名字
        val dir = File(app.cacheDir, "outgoing/${System.currentTimeMillis()}-$index").apply { mkdirs() }
        val file = File(dir, name)
        resolver.openInputStream(uri)?.use { input -> file.outputStream().use { input.copyTo(it) } }
            ?: error("读不了这个文件")
        return file.absolutePath
    }

    // ---- 杂项 ----

    private fun deviceName(): String {
        val name = Settings.Global.getString(app.contentResolver, Settings.Global.DEVICE_NAME)
        return name?.takeIf { it.isNotBlank() } ?: "${Build.MANUFACTURER} ${Build.MODEL}"
    }

    private fun appVersion(): String? = try {
        app.packageManager.getPackageInfo(app.packageName, 0).versionName
    } catch (_: Exception) {
        null
    }
}

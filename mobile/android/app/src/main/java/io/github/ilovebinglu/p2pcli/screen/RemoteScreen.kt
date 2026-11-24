package io.github.ilovebinglu.p2pcli.screen

import android.content.Context
import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.github.ilovebinglu.p2pcli.t
import io.github.ilovebinglu.p2pcli.terminal.errorText
import io.github.ilovebinglu.p2pcli.tr
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import uniffi.p2pcli_mobile.ScreenCodec
import uniffi.p2pcli_mobile.ScreenCursorShape
import uniffi.p2pcli_mobile.ScreenInput
import uniffi.p2pcli_mobile.ScreenListener
import uniffi.p2pcli_mobile.ScreenMouseButton
import uniffi.p2pcli_mobile.ScreenQuality
import uniffi.p2pcli_mobile.ScreenStartedView
import uniffi.p2pcli_mobile.ScreenVideoConfig
import uniffi.p2pcli_mobile.ScreenView

/** 触屏怎么当鼠标用 */
enum class PointerMode {
    /** 触控板：手指滑动挪指针，点一下点在指针上。默认：手指粗，小按钮也点得准 */
    TRACKPAD,
    /** 直接点：手指点哪儿就点哪儿 */
    TOUCH,
}

/**
 * 手机上正在看的一路远程画面，和 iOS 的 RemoteScreen 一样。
 *
 * 对方电脑硬件编码的 H.264/H.265 从 Rust 原样送来，交给 [VideoDecoder]（系统硬件解码）直接画到 [ScreenCanvas] 上；
 * 手指的操作在 [ScreenCanvas] 里换成鼠标键盘事件，经这里发回去。
 */
class RemoteScreen(
    context: Context,
    val endpointId: String,
    val connId: ULong,
    val name: String,
    /** 对方的系统：Mac 上的 ⌘ 在 Windows 上是 Win 键 */
    val platform: String,
    private val scope: CoroutineScope,
    private val opener: suspend (ScreenQuality, List<ScreenCodec>, ScreenListener) -> ScreenView,
    /** 给人看的一句提示 */
    private val notify: (String) -> Unit,
) {
    sealed interface Phase {
        /** 等对方同意 */
        data object Opening : Phase
        data object Live : Phase
        /** 对方暂停发画面：relay 为真是连接走了中继 */
        data class Paused(val relay: Boolean) : Phase
        data class Closed(val reason: String?) : Phase
    }

    data class Stats(val bitrate: UInt, val fps: UInt)

    var phase by mutableStateOf<Phase>(Phase.Opening)
        private set
    var started by mutableStateOf<ScreenStartedView?>(null)
        private set
    var display by mutableStateOf("")
        private set
    var quality by mutableStateOf(ScreenQuality.BALANCED)
        private set
    var mode by mutableStateOf(PointerMode.TRACKPAD)
    var stats by mutableStateOf<Stats?>(null)
        private set
    /** 粘住的修饰键（ControlLeft、AltLeft、MetaLeft、ShiftLeft）：下一次按键、点击带上它们，之后松开 */
    val modifiers = mutableStateListOf<String>()
    /** 已经出过画面 */
    var hasVideo by mutableStateOf(false)
        private set
    /** 收起工具栏，只留一个小圆钮 */
    var toolbarCollapsed by mutableStateOf(false)

    private val main = Handler(Looper.getMainLooper())
    private var handle: ScreenView? = null
    /** 当前这一路的回调；Rust 的线程上也读，重新打开以后旧的那一路的回调就不认了 */
    @Volatile
    private var bridge: Bridge? = null

    val decoder = VideoDecoder(
        onNeedKeyframe = { main.post { handle?.requestKeyframe() } },
        onUnsupported = { w, h -> main.post { onUnsupported(w, h) } },
    )
    val canvas = ScreenCanvas(context, this)

    val isMac: Boolean get() = platform.lowercase().let { it.contains("mac") || it == "darwin" }
    val canControl: Boolean get() = started?.control ?: false

    fun open() {
        phase = Phase.Opening
        hasVideo = false
        val b = Bridge(this)
        bridge = b
        val q = quality
        scope.launch {
            try {
                val h = opener(q, VideoDecoder.supportedCodecs(), b)
                // 等的时候已经关掉了
                if (phase != Phase.Opening || bridge !== b) {
                    h.stop()
                    return@launch
                }
                handle = h
                val s = h.started()
                started = s
                display = s.display
                phase = Phase.Live
            } catch (e: Exception) {
                if (bridge !== b) return@launch
                val text = errorText(e)
                phase = Phase.Closed(
                    if (text.startsWith("relay:")) t("现在这条连接走的是中继，远程画面只走直连。终端照常能用；直连接通后再打开画面") else text,
                )
            }
        }
    }

    fun stop() {
        handle?.stop()
        handle = null
        bridge = null
        if (phase !is Phase.Closed) phase = Phase.Closed(null)
    }

    /** 整个页面关掉：解码线程也停 */
    fun dispose() {
        stop()
        decoder.release()
    }

    /** 回到前台：要一个关键帧，免得画面停在切出去之前 */
    fun resume() {
        decoder.reset()
    }

    fun switchDisplay(id: String) {
        handle?.switchDisplay(id)
    }

    fun changeQuality(q: ScreenQuality) {
        quality = q
        handle?.setQuality(q)
    }

    // ---- 输入 ----

    fun send(input: ScreenInput) {
        if (!canControl) return
        handle?.send(input)
    }

    /** 点一下：带上粘住的修饰键，点完松开 */
    fun click(button: ScreenMouseButton, x: Double, y: Double) {
        if (!canControl) return
        val mods = takeModifiers()
        for (m in mods) send(ScreenInput.Key(m, true))
        send(ScreenInput.Move(x, y))
        send(ScreenInput.Button(button, true, x, y))
        send(ScreenInput.Button(button, false, x, y))
        for (m in mods.reversed()) send(ScreenInput.Key(m, false))
    }

    /** 拖动开始：修饰键按住到拖完 */
    private var dragMods: List<String> = emptyList()

    fun dragBegan(x: Double, y: Double) {
        if (!canControl) return
        dragMods = takeModifiers()
        for (m in dragMods) send(ScreenInput.Key(m, true))
        send(ScreenInput.Move(x, y))
        send(ScreenInput.Button(ScreenMouseButton.LEFT, true, x, y))
    }

    fun dragEnded(x: Double, y: Double) {
        send(ScreenInput.Button(ScreenMouseButton.LEFT, false, x, y))
        for (m in dragMods.reversed()) send(ScreenInput.Key(m, false))
        dragMods = emptyList()
    }

    fun typeText(text: String) {
        if (!canControl || text.isEmpty()) return
        handle?.typeText(text, takeModifiers())
    }

    /** 按一个键；extra 是实体键盘上按着的修饰键 */
    fun tapKey(code: String, extra: List<String> = emptyList()) {
        if (!canControl) return
        handle?.tapKey(code, (takeModifiers() + extra).distinct())
    }

    fun toggleModifier(code: String) {
        if (!modifiers.remove(code)) modifiers.add(code)
    }

    private fun takeModifiers(): List<String> {
        val m = modifiers.toList()
        modifiers.clear()
        return m
    }

    // ---- Rust 的回调（已经切到主线程）----

    private fun onUnsupported(w: Int, h: Int) {
        if (quality == ScreenQuality.SHARP) {
            notify(t("这台手机解不了 {{w}}×{{h}} 的画面，已换成均衡画质", "w" to w, "h" to h))
            changeQuality(ScreenQuality.BALANCED)
        } else {
            stop()
            phase = Phase.Closed(t("这台手机解不了 {{w}}×{{h}} 的画面", "w" to w, "h" to h))
        }
    }

    private fun onPaused(relay: Boolean) {
        if (phase == Phase.Live) phase = Phase.Paused(relay)
    }

    private fun onResumed() {
        if (phase is Phase.Paused) phase = Phase.Live
    }

    private fun onClosed(reason: String?) {
        handle = null
        if (phase is Phase.Closed) return
        phase = Phase.Closed(reason?.let { tr(it) })
    }

    /** Rust 那边的回调：画面直接交给解码器（它有自己的线程），其他的切回主线程 */
    private class Bridge(private val owner: RemoteScreen) : ScreenListener {
        private val main = Handler(Looper.getMainLooper())
        @Volatile
        private var sawFrame = false

        private fun ui(f: RemoteScreen.() -> Unit) {
            main.post { if (owner.bridge === this) owner.f() }
        }

        override fun onConfig(config: ScreenVideoConfig) {
            if (owner.bridge !== this) return
            owner.decoder.configure(config)
            ui { canvas.setVideoSize(config.width.toInt(), config.height.toInt()) }
        }

        override fun onFrame(key: Boolean, data: ByteArray) {
            if (owner.bridge !== this) return
            owner.decoder.decode(key, data)
            if (!sawFrame) {
                sawFrame = true
                ui { hasVideo = true }
            }
        }

        override fun onCursor(x: Double, y: Double, visible: Boolean) = ui { canvas.remoteCursor(x, y, visible) }
        override fun onCursorShape(shape: ScreenCursorShape) = ui { canvas.setCursorShape(shape) }
        override fun onPaused(relay: Boolean) = ui { onPaused(relay) }
        override fun onResumed() = ui { onResumed() }
        override fun onSwitched(display: String) = ui { this.display = display }
        override fun onStats(bitrate: UInt, fps: UInt) = ui { stats = Stats(bitrate, fps) }
        override fun onClosed(reason: String?) = ui { onClosed(reason) }
    }
}

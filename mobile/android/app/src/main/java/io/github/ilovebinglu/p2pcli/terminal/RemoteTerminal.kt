package io.github.ilovebinglu.p2pcli.terminal

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.termux.terminal.TerminalEmulator
import com.termux.terminal.TerminalOutput
import com.termux.terminal.TerminalSession
import com.termux.terminal.TerminalSessionClient
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import uniffi.p2pcli_mobile.Terminal
import uniffi.p2pcli_mobile.TerminalListener

/**
 * 手机上开着的一个远程终端。
 *
 * 对方电脑上的输出（Rust 从网络收来）交给 Termux 的终端模拟器解析，画面由 [TerminalCanvasView] 画；
 * 键盘输入、模拟器要回给对方的应答（比如光标位置）经 Rust 发回去。
 * 终端第一次知道自己有多大（几列几行）时才真正打开，打开前不占连接。
 */
class RemoteTerminal(
    private val context: Context,
    val key: Int,
    val endpointId: String,
    val connId: ULong,
    sessionId: String?,
    private val scope: CoroutineScope,
    /** 真正打开：Core 调 Rust 的 open_terminal */
    private val opener: suspend (cols: UShort, rows: UShort, listener: TerminalListener) -> Terminal,
) : TerminalOutput(), TerminalListener {

    enum class State { OPENING, OPEN, CLOSED }

    /** 对方电脑上的会话编号；新建的终端打开以后才有 */
    var sessionId by mutableStateOf(sessionId)
        private set
    var state by mutableStateOf(State.OPENING)
        private set
    /** 结束、断开时给人看的原因 */
    var note by mutableStateOf<String?>(null)
        private set
    /** 程序退出（终端已结束），不是连接断了 */
    var exited by mutableStateOf(false)
        private set

    var emulator: TerminalEmulator? = null
        private set
    /** 正在显示它的 View；切到别的终端时是空 */
    var view: TerminalCanvasView? = null

    private var handle: Terminal? = null
    private val main = Handler(Looper.getMainLooper())

    /** View 量好了大小（主线程）：第一次就建模拟器、打开终端，之后改尺寸 */
    fun onSize(cols: Int, rows: Int, cellWidth: Int, cellHeight: Int) {
        val emu = emulator
        if (emu == null) {
            emulator = TerminalEmulator(this, cols, rows, cellWidth, cellHeight, TRANSCRIPT_ROWS, NoopClient)
            open(cols, rows)
        } else if (emu.mColumns != cols || emu.mRows != rows) {
            emu.resize(cols, rows, cellWidth, cellHeight)
            handle?.resize(cols.toUShort(), rows.toUShort())
        }
    }

    private fun open(cols: Int, rows: Int) {
        scope.launch {
            try {
                val t = opener(cols.toUShort(), rows.toUShort(), this@RemoteTerminal)
                handle = t
                sessionId = t.sessionId()
                state = State.OPEN
                // 打开的这段时间里屏幕转了、键盘弹了，尺寸可能已经变了
                emulator?.let { if (it.mColumns != cols || it.mRows != rows) t.resize(it.mColumns.toUShort(), it.mRows.toUShort()) }
            } catch (e: Exception) {
                state = State.CLOSED
                note = errorText(e)
            }
        }
    }

    /** 键盘、按键条、输入栏的内容原样发过去 */
    fun send(bytes: ByteArray) {
        handle?.write(bytes)
    }

    fun send(text: String) = send(text.toByteArray(Charsets.UTF_8))

    /** 当成粘贴发：对方程序开了整段粘贴（bracketed paste）就整段发，多行不会被当成几次回车 */
    fun paste(text: String) {
        emulator?.paste(text) ?: send(text)
    }

    /** 关掉手机上的这个终端；对方电脑上的会话接着跑 */
    fun close() {
        handle?.close()
        handle = null
    }

    // ---- Rust 送来的输出（Rust 的线程）----

    override fun onOutput(data: ByteArray) {
        main.post {
            emulator?.append(data, data.size)
            view?.onScreenUpdated()
        }
    }

    override fun onClosed(exitCode: UInt?, reason: String?) {
        main.post {
            handle = null
            exited = reason == null
            state = State.CLOSED
            note = reason ?: exitCode?.let { "终端已结束（退出码 $it）" } ?: "终端已结束"
        }
    }

    // ---- 终端模拟器要发回对方的内容（主线程）----

    override fun write(data: ByteArray, offset: Int, count: Int) {
        handle?.write(data.copyOfRange(offset, offset + count))
    }

    override fun titleChanged(oldTitle: String?, newTitle: String?) {}

    override fun onCopyTextToClipboard(text: String?) {
        if (text.isNullOrEmpty()) return
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        clipboard.setPrimaryClip(ClipData.newPlainText("terminal", text))
    }

    override fun onPasteTextFromClipboard() {
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        val text = clipboard.primaryClip?.getItemAt(0)?.coerceToText(context)?.toString()
        if (!text.isNullOrEmpty()) paste(text)
    }

    override fun onBell() {}

    override fun onColorsChanged() {
        view?.invalidate()
    }

    companion object {
        /** 往回能翻多少行 */
        private const val TRANSCRIPT_ROWS = 5000
    }
}

/** 模拟器要的回调，这里用不上（它们是给本机终端进程用的） */
private object NoopClient : TerminalSessionClient {
    override fun onTextChanged(changedSession: TerminalSession) {}
    override fun onTitleChanged(changedSession: TerminalSession) {}
    override fun onSessionFinished(finishedSession: TerminalSession) {}
    override fun onCopyTextToClipboard(session: TerminalSession, text: String?) {}
    override fun onPasteTextFromClipboard(session: TerminalSession?) {}
    override fun onBell(session: TerminalSession) {}
    override fun onColorsChanged(session: TerminalSession) {}
    override fun onTerminalCursorStateChange(state: Boolean) {}
    override fun getTerminalCursorStyle(): Int = TerminalEmulator.DEFAULT_TERMINAL_CURSOR_STYLE
    override fun logError(tag: String?, message: String?) {}
    override fun logWarn(tag: String?, message: String?) {}
    override fun logInfo(tag: String?, message: String?) {}
    override fun logDebug(tag: String?, message: String?) {}
    override fun logVerbose(tag: String?, message: String?) {}
    override fun logStackTraceWithMessage(tag: String?, message: String?, e: Exception?) {}
    override fun logStackTrace(tag: String?, e: Exception?) {}
}

/** Rust 那边的错误转成给人看的一句话 */
fun errorText(e: Throwable): String = when (e) {
    is uniffi.p2pcli_mobile.P2pException.Rejected -> e.message
    is uniffi.p2pcli_mobile.P2pException.Failed -> e.message
    is uniffi.p2pcli_mobile.P2pException.Disconnected -> "连接已断开"
    else -> e.message ?: e.toString()
}

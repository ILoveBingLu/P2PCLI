package io.github.ilovebinglu.p2pcli.terminal

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Typeface
import android.text.InputType
import android.util.TypedValue
import android.view.GestureDetector
import android.view.InputDevice
import android.view.KeyCharacterMap
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import com.termux.terminal.KeyHandler
import com.termux.terminal.TerminalEmulator
import com.termux.view.TerminalRenderer
import kotlin.math.abs
import kotlin.math.ceil
import kotlin.math.max

/**
 * 画远程终端、接键盘。照着 Termux 的 TerminalView 写：输入法、按键转换、滑动翻历史的做法和它一样，
 * 只是终端不是本机进程，而是 [RemoteTerminal]（网络另一头的会话）。
 *
 * 手势：点一下弹键盘；上下滑翻历史（对方开了鼠标模式时，比如 tmux，发滚轮给对方）；双指缩放改字号。
 */
@SuppressLint("ViewConstructor")
class TerminalCanvasView(context: Context, private val modifiers: Modifiers) : View(context) {

    /** 按键条上的 Ctrl、Alt：按一下只管下一个键 */
    interface Modifiers {
        fun takeCtrl(): Boolean
        fun takeAlt(): Boolean
    }

    var terminal: RemoteTerminal? = null
        set(value) {
            if (field === value) return
            field?.view = null
            field = value
            value?.view = this
            topRow = 0
            updateSize()
            invalidate()
        }

    private var textSizePx = sp(DEFAULT_TEXT_SP)
    private var renderer = TerminalRenderer(textSizePx, Typeface.MONOSPACE)
    /** 字的上沿到基线的距离：算一屏几行要用，TerminalRenderer 没公开 */
    private var ascent = ascentOf(textSizePx)
    /** 往回翻了几行：0 是最底下，负数是在看历史 */
    private var topRow = 0
    private var scrollRemainder = 0f

    init {
        isFocusable = true
        isFocusableInTouchMode = true
        setBackgroundColor(Color.BLACK)
    }

    private val gestures = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onSingleTapUp(e: MotionEvent): Boolean {
            showKeyboard()
            return true
        }

        override fun onScroll(e1: MotionEvent?, e2: MotionEvent, dx: Float, dy: Float): Boolean {
            if (scaling) return true
            val total = dy + scrollRemainder
            val rows = (total / renderer.fontLineSpacing).toInt()
            scrollRemainder = total - rows * renderer.fontLineSpacing
            if (rows != 0) doScroll(e2, rows)
            return true
        }
    })

    private var scaling = false
    private val scale = ScaleGestureDetector(context, object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
        private var accumulated = 1f
        override fun onScaleBegin(detector: ScaleGestureDetector): Boolean {
            scaling = true
            accumulated = 1f
            return true
        }

        override fun onScale(detector: ScaleGestureDetector): Boolean {
            accumulated *= detector.scaleFactor
            // 攒够一档再改，免得每一帧都重排
            if (abs(accumulated - 1f) > 0.08f) {
                val next = (textSizePx * accumulated).toInt().coerceIn(sp(MIN_TEXT_SP), sp(MAX_TEXT_SP))
                accumulated = 1f
                if (next != textSizePx) setTextSizePx(next)
            }
            return true
        }

        override fun onScaleEnd(detector: ScaleGestureDetector) {
            scaling = false
        }
    })

    fun setTextSizePx(px: Int) {
        textSizePx = px
        renderer = TerminalRenderer(px, Typeface.MONOSPACE)
        ascent = ascentOf(px)
        updateSize()
        invalidate()
    }

    fun showKeyboard() {
        requestFocus()
        val imm = context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
        imm.showSoftInput(this, InputMethodManager.SHOW_IMPLICIT)
    }

    // ---- 尺寸和绘制 ----

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) = updateSize()

    private fun updateSize() {
        val t = terminal ?: return
        if (width == 0 || height == 0) return
        val cols = max(4, (width / renderer.fontWidth).toInt())
        val rows = max(4, (height - (renderer.fontLineSpacing + ascent)) / renderer.fontLineSpacing)
        t.onSize(cols, rows, renderer.fontWidth.toInt(), renderer.fontLineSpacing)
        topRow = 0
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        val emu = terminal?.emulator
        if (emu == null) {
            canvas.drawColor(Color.BLACK)
            return
        }
        renderer.render(emu, canvas, topRow, -1, -1, -1, -1)
    }

    /** 有新输出：在看历史就停在原来的内容上，不然跟着到最底下 */
    fun onScreenUpdated() {
        val emu = terminal?.emulator ?: return
        val history = emu.screen.activeTranscriptRows
        if (topRow != 0) topRow = (topRow - emu.scrollCounter).coerceIn(-history, 0)
        emu.clearScrollCounter()
        invalidate()
    }

    // ---- 手势 ----

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (event.actionMasked == MotionEvent.ACTION_DOWN) scrollRemainder = 0f
        scale.onTouchEvent(event)
        gestures.onTouchEvent(event)
        return true
    }

    override fun onGenericMotionEvent(event: MotionEvent): Boolean {
        if (event.isFromSource(InputDevice.SOURCE_MOUSE) && event.action == MotionEvent.ACTION_SCROLL) {
            doScroll(event, if (event.getAxisValue(MotionEvent.AXIS_VSCROLL) > 0f) -3 else 3)
            return true
        }
        return super.onGenericMotionEvent(event)
    }

    /** 往下滑 rows 行（负数往上）。对方开了鼠标模式发滚轮；全屏程序（less、vim）发方向键；不然翻本地历史 */
    private fun doScroll(event: MotionEvent, rows: Int) {
        val emu = terminal?.emulator ?: return
        val up = rows < 0
        repeat(abs(rows)) {
            when {
                emu.isMouseTrackingActive -> {
                    val col = (event.x / renderer.fontWidth).toInt() + 1
                    val row = ((event.y - renderer.fontLineSpacing - ascent) / renderer.fontLineSpacing).toInt().coerceAtLeast(0) + 1
                    emu.sendMouseEvent(if (up) TerminalEmulator.MOUSE_WHEELUP_BUTTON else TerminalEmulator.MOUSE_WHEELDOWN_BUTTON, col, row, true)
                }
                emu.isAlternateBufferActive -> sendKeyCode(if (up) KeyEvent.KEYCODE_DPAD_UP else KeyEvent.KEYCODE_DPAD_DOWN, 0)
                else -> topRow = (topRow + if (up) -1 else 1).coerceIn(-emu.screen.activeTranscriptRows, 0)
            }
        }
        invalidate()
    }

    // ---- 输入法 ----

    override fun onCheckIsTextEditor() = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        // TYPE_NULL：输入法按一个键发一个字，不做联想和自动更正，和 Termux 一样
        outAttrs.inputType = InputType.TYPE_NULL
        outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN
        return object : BaseInputConnection(this, true) {
            override fun finishComposingText(): Boolean {
                super.finishComposingText()
                editable?.let { sendText(it); it.clear() }
                return true
            }

            override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
                super.commitText(text, newCursorPosition)
                editable?.let { sendText(it); it.clear() }
                return true
            }

            override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
                // 有的输入法删除时不发按键，只删字：换成退格键
                val del = KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DEL)
                repeat(beforeLength) { sendKeyEvent(del) }
                return super.deleteSurroundingText(beforeLength, afterLength)
            }
        }
    }

    /** 输入法给的一段字：逐个字符发，回车换成终端认的 \r，控制字符当成 Ctrl+字母 */
    private fun sendText(text: CharSequence) {
        var i = 0
        while (i < text.length) {
            var cp = Character.codePointAt(text, i)
            i += Character.charCount(cp)
            var ctrl = false
            if (cp <= 31 && cp != 27) {
                if (cp == '\n'.code) cp = '\r'.code
                ctrl = true
                cp = when (cp) {
                    31 -> '_'.code
                    30 -> '^'.code
                    29 -> ']'.code
                    28 -> '\\'.code
                    else -> cp + 96
                }
            }
            inputCodePoint(cp, ctrl, false)
        }
    }

    // ---- 实体键盘、输入法发的按键 ----

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        val t = terminal ?: return super.onKeyDown(keyCode, event)
        // 返回、音量这些交给系统
        if (event.isSystem && keyCode != KeyEvent.KEYCODE_ENTER) return super.onKeyDown(keyCode, event)
        if (event.action == KeyEvent.ACTION_MULTIPLE && keyCode == KeyEvent.KEYCODE_UNKNOWN) {
            event.characters?.let { t.send(it) }
            return true
        }
        val meta = event.metaState
        val ctrl = event.isCtrlPressed || modifiers.takeCtrl()
        val alt = (meta and KeyEvent.META_ALT_LEFT_ON) != 0 || modifiers.takeAlt()
        var keyMod = 0
        if (ctrl) keyMod = keyMod or KeyHandler.KEYMOD_CTRL
        if (event.isAltPressed || alt) keyMod = keyMod or KeyHandler.KEYMOD_ALT
        if (event.isShiftPressed) keyMod = keyMod or KeyHandler.KEYMOD_SHIFT
        if (event.isNumLockOn) keyMod = keyMod or KeyHandler.KEYMOD_NUM_LOCK
        if (!event.isFunctionPressed && sendKeyCode(keyCode, keyMod)) return true

        // 普通字符：Ctrl、左 Alt 自己处理，右 Alt 留给系统拼字符
        var clear = KeyEvent.META_CTRL_MASK
        if ((meta and KeyEvent.META_ALT_RIGHT_ON) == 0) clear = clear or KeyEvent.META_ALT_ON or KeyEvent.META_ALT_LEFT_ON
        val result = event.getUnicodeChar(meta and clear.inv())
        if (result == 0) return super.onKeyDown(keyCode, event)
        if (result and KeyCharacterMap.COMBINING_ACCENT != 0) return true
        inputCodePoint(result, ctrl, alt)
        return true
    }

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean {
        if (event.isSystem) return super.onKeyUp(keyCode, event)
        return true
    }

    /** 方向键、功能键这些：按终端现在的模式转成转义序列发出去 */
    fun sendKeyCode(keyCode: Int, keyMod: Int): Boolean {
        val t = terminal ?: return false
        val emu = t.emulator ?: return false
        val code = KeyHandler.getCode(keyCode, keyMod, emu.isCursorKeysApplicationMode, emu.isKeypadApplicationMode) ?: return false
        topRow = 0
        t.send(code)
        return true
    }

    /** 一个字符：带 Ctrl 换成控制字符，带 Alt 前面加 Esc */
    fun inputCodePoint(codePoint: Int, ctrlFromEvent: Boolean, altFromEvent: Boolean) {
        val t = terminal ?: return
        val ctrl = ctrlFromEvent || modifiers.takeCtrl()
        val alt = altFromEvent || modifiers.takeAlt()
        var cp = codePoint
        if (ctrl) {
            cp = when {
                cp in 'a'.code..'z'.code -> cp - 'a'.code + 1
                cp in 'A'.code..'Z'.code -> cp - 'A'.code + 1
                cp == ' '.code || cp == '2'.code -> 0
                cp == '['.code || cp == '3'.code -> 27
                cp == '\\'.code || cp == '4'.code -> 28
                cp == ']'.code || cp == '5'.code -> 29
                cp == '^'.code || cp == '6'.code -> 30
                cp == '_'.code || cp == '7'.code || cp == '/'.code -> 31
                cp == '8'.code -> 127
                else -> cp
            }
        }
        val bytes = String(Character.toChars(cp)).toByteArray(Charsets.UTF_8)
        topRow = 0
        t.send(if (alt) byteArrayOf(27) + bytes else bytes)
        invalidate()
    }

    private fun sp(value: Int) =
        TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, value.toFloat(), resources.displayMetrics).toInt()

    private fun ascentOf(px: Int): Int {
        val p = Paint().apply { typeface = Typeface.MONOSPACE; textSize = px.toFloat() }
        return ceil(p.ascent()).toInt()
    }

    companion object {
        const val DEFAULT_TEXT_SP = 13
        const val MIN_TEXT_SP = 8
        const val MAX_TEXT_SP = 28
    }
}

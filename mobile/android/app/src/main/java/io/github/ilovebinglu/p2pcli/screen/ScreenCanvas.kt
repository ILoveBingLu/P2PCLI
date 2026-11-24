package io.github.ilovebinglu.p2pcli.screen

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Matrix
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.SurfaceTexture
import android.text.InputType
import android.view.HapticFeedbackConstants
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.Surface
import android.view.TextureView
import android.view.VelocityTracker
import android.view.ViewConfiguration
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import android.widget.FrameLayout
import uniffi.p2pcli_mobile.ScreenCursorShape
import uniffi.p2pcli_mobile.ScreenInput
import uniffi.p2pcli_mobile.ScreenMouseButton
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min

/**
 * 画面、指针和手势，和 iOS 的 ScreenCanvas 一样。坐标都换成相对对方屏幕的 0~1 再发。
 *
 * - 触控板模式（默认）：一指滑动挪指针，点一下左键，两指点一下右键，长按后拖动是按住左键拖（选文字、拖窗口）。
 * - 直接点模式：点哪儿点哪儿，按住拖动就是拖，长按是右键。
 * - 两指上下左右拖是滚轮；两指捏合放大画面，放大后两指一起挪是平移，指针走到边上画面跟着走。
 *
 * 画面用 TextureView 显示：放大、平移只改一个变换矩阵，不用重建解码器。
 * 这个 View 自己就是输入框：系统键盘（包括中文输入法）打的字从 [onCreateInputConnection] 拿，选好字才发。
 */
@SuppressLint("ViewConstructor")
class ScreenCanvas(context: Context, private val screen: RemoteScreen) : FrameLayout(context), TextureView.SurfaceTextureListener {
    private val density = resources.displayMetrics.density
    private val texture = TextureView(context)
    private var surface: Surface? = null

    /** 画面的像素尺寸（解码配置里的） */
    private var videoW = 16f
    private var videoH = 10f
    /** 放大倍数和平移（相对居中摆放） */
    private var zoom = 1f
    private var panX = 0f
    private var panY = 0f
    /** 指针在对方屏幕上的位置（0~1） */
    private var cursorX = 0.5
    private var cursorY = 0.5
    private var cursorVisible = true
    private var cursorShape: ScreenCursorShape? = null
    private var cursorBitmap: Bitmap? = null
    private val cursorPaint = Paint(Paint.FILTER_BITMAP_FLAG)
    /** 最近一次本机挪指针的时间：这之后一小会儿不信对方报回来的位置，免得指针往回跳 */
    private var lastLocalMove = 0L

    init {
        setBackgroundColor(Color.BLACK)
        setWillNotDraw(false)
        isFocusable = true
        isFocusableInTouchMode = true
        texture.surfaceTextureListener = this
        texture.isOpaque = true
        addView(texture, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
    }

    // ---- 显示的地方 ----

    override fun onSurfaceTextureAvailable(st: SurfaceTexture, width: Int, height: Int) {
        val s = Surface(st)
        surface = s
        screen.decoder.setSurface(s)
        layoutContent()
    }

    override fun onSurfaceTextureSizeChanged(st: SurfaceTexture, width: Int, height: Int) = layoutContent()

    override fun onSurfaceTextureDestroyed(st: SurfaceTexture): Boolean {
        // 等解码器放开这个 Surface 再释放，不然它还往已经没了的地方画
        val s = surface
        surface = null
        screen.decoder.setSurface(null) {
            s?.release()
            st.release()
        }
        return false
    }

    override fun onSurfaceTextureUpdated(st: SurfaceTexture) {}

    // ---- 摆放 ----

    /** 画面按比例放进来，再按放大倍数和平移挪 */
    private fun contentRect(): RectF {
        val bw = width.toFloat()
        val bh = height.toFloat()
        if (bw <= 0 || bh <= 0 || videoW <= 0 || videoH <= 0) return RectF()
        val s = min(bw / videoW, bh / videoH) * zoom
        val w = videoW * s
        val h = videoH * s
        fun axis(size: Float, bound: Float, offset: Float): Float =
            if (size <= bound) (bound - size) / 2 else min(0f, max(bound - size, (bound - size) / 2 + offset))
        val x = axis(w, bw, panX)
        val y = axis(h, bh, panY)
        return RectF(x, y, x + w, y + h)
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        clampPan()
        layoutContent()
    }

    /** TextureView 铺满，用矩阵把画面摆到 contentRect 上 */
    private fun layoutContent() {
        val r = contentRect()
        val w = width.toFloat()
        val h = height.toFloat()
        if (w > 0 && h > 0 && r.width() > 0) {
            val m = Matrix()
            m.setScale(r.width() / w, r.height() / h)
            m.postTranslate(r.left, r.top)
            texture.setTransform(m)
        }
        invalidate()
    }

    /** 平移量收在画面能盖住的范围里（缩小到比屏幕小时居中） */
    private fun clampPan() {
        val r = contentRect()
        panX = if (r.width() <= width) 0f else r.left - (width - r.width()) / 2
        panY = if (r.height() <= height) 0f else r.top - (height - r.height()) / 2
    }

    fun setVideoSize(w: Int, h: Int) {
        videoW = w.toFloat()
        videoH = h.toFloat()
        clampPan()
        layoutContent()
    }

    private fun normX(x: Float): Double {
        val r = contentRect()
        return if (r.width() <= 0) 0.5 else ((x - r.left) / r.width()).toDouble().coerceIn(0.0, 1.0)
    }

    private fun normY(y: Float): Double {
        val r = contentRect()
        return if (r.height() <= 0) 0.5 else ((y - r.top) / r.height()).toDouble().coerceIn(0.0, 1.0)
    }

    // ---- 指针 ----

    fun setCursorShape(s: ScreenCursorShape) {
        val w = s.width.toInt()
        val h = s.height.toInt()
        if (w <= 0 || h <= 0 || s.rgba.size < w * h * 4) return
        // RGBA（不预乘）→ ARGB
        val px = IntArray(w * h)
        for (i in px.indices) {
            val o = i * 4
            val r = s.rgba[o].toInt() and 0xff
            val g = s.rgba[o + 1].toInt() and 0xff
            val b = s.rgba[o + 2].toInt() and 0xff
            val a = s.rgba[o + 3].toInt() and 0xff
            px[i] = (a shl 24) or (r shl 16) or (g shl 8) or b
        }
        cursorShape = s
        cursorBitmap = Bitmap.createBitmap(px, w, h, Bitmap.Config.ARGB_8888)
        invalidate()
    }

    fun remoteCursor(x: Double, y: Double, visible: Boolean) {
        cursorVisible = visible
        if (System.currentTimeMillis() - lastLocalMove > 400) {
            cursorX = x
            cursorY = y
        }
        invalidate()
    }

    /** 指针图案按对方屏幕上的大小缩放，太小了手机上看不清，至少 20dp 高 */
    override fun dispatchDraw(canvas: Canvas) {
        super.dispatchDraw(canvas)
        val s = cursorShape ?: return
        val bmp = cursorBitmap ?: return
        if (!cursorVisible) return
        val r = contentRect()
        val screenWidth = screen.started?.displays?.firstOrNull { it.id == screen.display }?.width?.toFloat() ?: videoW
        var scale = if (screenWidth > 0) r.width() / screenWidth else 1f
        scale = max(scale, 20 * density / max(1f, s.height.toFloat()))
        val px = r.left + cursorX.toFloat() * r.width()
        val py = r.top + cursorY.toFloat() * r.height()
        val left = px - s.hotX.toFloat() * scale
        val top = py - s.hotY.toFloat() * scale
        canvas.drawBitmap(bmp, null, RectF(left, top, left + bmp.width * scale, top + bmp.height * scale), cursorPaint)
    }

    /** 本机挪了指针：画上、发过去；放大时画面跟着指针走 */
    private fun moveCursor(x: Double, y: Double) {
        cursorX = x.coerceIn(0.0, 1.0)
        cursorY = y.coerceIn(0.0, 1.0)
        lastLocalMove = System.currentTimeMillis()
        screen.send(ScreenInput.Move(cursorX, cursorY))
        if (zoom > 1.01f) follow()
        invalidate()
    }

    /** 放大时指针靠近边上，画面往那边挪 */
    private fun follow() {
        val r = contentRect()
        val px = r.left + cursorX.toFloat() * r.width()
        val py = r.top + cursorY.toFloat() * r.height()
        val margin = 60 * density
        var dx = 0f
        var dy = 0f
        if (px < margin) dx = margin - px else if (px > width - margin) dx = width - margin - px
        if (py < margin) dy = margin - py else if (py > height - margin) dy = height - margin - py
        if (dx == 0f && dy == 0f) return
        panX += dx
        panY += dy
        clampPan()
        layoutContent()
    }

    // ---- 手势 ----

    private enum class TwoFinger { UNDECIDED, ZOOM, SCROLL }

    private val slop = ViewConfiguration.get(context).scaledTouchSlop.toFloat()
    private val longPressMs = 350L
    private val tapMs = ViewConfiguration.getTapTimeout().toLong() + 150
    private var velocity: VelocityTracker? = null
    private var downTime = 0L
    private var downX = 0f
    private var downY = 0f
    private var lastX = 0f
    private var lastY = 0f
    /** 这一下最多有几根手指 */
    private var maxPointers = 1
    private var moved = false
    private var longPressed = false
    private var dragging = false
    private var twoFinger = TwoFinger.UNDECIDED
    private var startSpan = 1f
    private var startZoom = 1f
    private var lastCx = 0f
    private var lastCy = 0f
    private var startCx = 0f
    private var startCy = 0f

    private val longPress = Runnable {
        if (maxPointers != 1 || moved) return@Runnable
        longPressed = true
        performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
        if (screen.mode == PointerMode.TOUCH) {
            // 直接点：长按是右键
            val x = normX(downX)
            val y = normY(downY)
            cursorX = x
            cursorY = y
            lastLocalMove = System.currentTimeMillis()
            invalidate()
            screen.click(ScreenMouseButton.RIGHT, x, y)
        } else {
            // 触控板：按住左键，接着挪手指就是拖
            dragging = true
            screen.dragBegan(cursorX, cursorY)
        }
    }

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(e: MotionEvent): Boolean {
        when (e.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                velocity?.recycle()
                velocity = VelocityTracker.obtain().apply { addMovement(e) }
                downTime = e.eventTime
                downX = e.x
                downY = e.y
                lastX = e.x
                lastY = e.y
                maxPointers = 1
                moved = false
                longPressed = false
                dragging = false
                twoFinger = TwoFinger.UNDECIDED
                postDelayed(longPress, longPressMs)
            }
            MotionEvent.ACTION_POINTER_DOWN -> {
                removeCallbacks(longPress)
                maxPointers = max(maxPointers, e.pointerCount)
                if (dragging) {
                    dragging = false
                    screen.dragEnded(cursorX, cursorY)
                }
                if (e.pointerCount == 2) {
                    startSpan = max(1f, span(e))
                    startZoom = zoom
                    startCx = centerX(e)
                    startCy = centerY(e)
                    lastCx = startCx
                    lastCy = startCy
                }
            }
            MotionEvent.ACTION_MOVE -> {
                velocity?.addMovement(e)
                if (maxPointers >= 2) {
                    if (e.pointerCount >= 2) twoFingerMove(e)
                } else {
                    oneFingerMove(e)
                }
            }
            MotionEvent.ACTION_UP -> {
                removeCallbacks(longPress)
                val quick = e.eventTime - downTime < tapMs
                when {
                    dragging -> {
                        val x = if (screen.mode == PointerMode.TOUCH) normX(e.x) else cursorX
                        val y = if (screen.mode == PointerMode.TOUCH) normY(e.y) else cursorY
                        screen.dragEnded(x, y)
                    }
                    maxPointers == 1 && !moved && !longPressed && quick -> tap(e.x, e.y)
                    maxPointers == 2 && twoFinger == TwoFinger.UNDECIDED && quick -> {
                        val x = if (screen.mode == PointerMode.TOUCH) normX(startCx) else cursorX
                        val y = if (screen.mode == PointerMode.TOUCH) normY(startCy) else cursorY
                        screen.click(ScreenMouseButton.RIGHT, x, y)
                    }
                }
                dragging = false
                velocity?.recycle()
                velocity = null
            }
            MotionEvent.ACTION_CANCEL -> {
                removeCallbacks(longPress)
                if (dragging) screen.dragEnded(cursorX, cursorY)
                dragging = false
                velocity?.recycle()
                velocity = null
            }
        }
        return true
    }

    private fun tap(x: Float, y: Float) {
        if (screen.mode == PointerMode.TOUCH) {
            cursorX = normX(x)
            cursorY = normY(y)
            lastLocalMove = System.currentTimeMillis()
            invalidate()
        }
        screen.click(ScreenMouseButton.LEFT, cursorX, cursorY)
    }

    private fun oneFingerMove(e: MotionEvent) {
        val dx = e.x - lastX
        val dy = e.y - lastY
        if (!moved) {
            if (hypot(e.x - downX, e.y - downY) < slop) return
            moved = true
            removeCallbacks(longPress)
            if (screen.mode == PointerMode.TOUCH && !longPressed) {
                // 直接点：从按下的地方开始拖
                dragging = true
                cursorX = normX(downX)
                cursorY = normY(downY)
                screen.dragBegan(cursorX, cursorY)
            }
        }
        lastX = e.x
        lastY = e.y
        if (screen.mode == PointerMode.TOUCH) {
            if (dragging) moveCursor(normX(e.x), normY(e.y))
            return
        }
        // 触控板：手指挪多少，指针挪多少（快的时候多挪一点）；按没放大时的画面算，放大以后挪得慢一点、更细
        val r = contentRect()
        if (r.width() <= 0) return
        var accel = 1f
        if (!dragging) {
            velocity?.computeCurrentVelocity(1000)
            val speed = hypot(velocity?.xVelocity ?: 0f, velocity?.yVelocity ?: 0f) / density
            accel = 1 + min(1.5f, speed / 1200)
        }
        val base = r.width() / zoom
        val k = accel * 1.4f / base / max(1f, zoom * 0.7f)
        moveCursor(cursorX + dx * k, cursorY + dy * k * (r.width() / r.height()))
    }

    private fun twoFingerMove(e: MotionEvent) {
        val cx = centerX(e)
        val cy = centerY(e)
        val sp = span(e)
        if (twoFinger == TwoFinger.UNDECIDED) {
            if (abs(sp / startSpan - 1) > 0.08f) twoFinger = TwoFinger.ZOOM
            else if (hypot(cx - startCx, cy - startCy) > 12 * density) twoFinger = TwoFinger.SCROLL
            else return
        }
        val dx = cx - lastCx
        val dy = cy - lastCy
        lastCx = cx
        lastCy = cy
        if (twoFinger == TwoFinger.ZOOM) {
            // 以两指中间为中心缩放，两指一起挪是平移
            val before = contentRect()
            val ax = (cx - dx - before.left) / max(1f, before.width())
            val ay = (cy - dy - before.top) / max(1f, before.height())
            zoom = (startZoom * sp / startSpan).coerceIn(1f, 5f)
            panX = 0f
            panY = 0f
            val r = contentRect()
            panX = cx - ax * r.width() - r.left
            panY = cy - ay * r.height() - r.top
            clampPan()
            layoutContent()
            return
        }
        // 自然滚动：手指往上推，内容往下滚
        val k = 2.5 / density
        val x = if (screen.mode == PointerMode.TOUCH) normX(cx) else cursorX
        val y = if (screen.mode == PointerMode.TOUCH) normY(cy) else cursorY
        screen.send(ScreenInput.Wheel(-dx * k, -dy * k, x, y))
    }

    private fun centerX(e: MotionEvent) = (e.getX(0) + e.getX(1)) / 2
    private fun centerY(e: MotionEvent) = (e.getY(0) + e.getY(1)) / 2
    private fun span(e: MotionEvent) = hypot(e.getX(0) - e.getX(1), e.getY(0) - e.getY(1))

    // ---- 键盘 ----

    private val imm get() = context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager

    fun showKeyboard() {
        requestFocus()
        imm.showSoftInput(this, InputMethodManager.SHOW_IMPLICIT)
    }

    fun hideKeyboard() {
        imm.hideSoftInputFromWindow(windowToken, 0)
    }

    override fun onCheckIsTextEditor() = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        // 普通文字框（中文输入法能用），不要联想、自动更正，也不要全屏的输入界面
        outAttrs.inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
        outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN or EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_ACTION_NONE
        return object : BaseInputConnection(this, true) {
            /** 选好的字发过去，框里清空；拼音还在组字（composing）时不发 */
            private fun flush() {
                val ed = editable ?: return
                if (getComposingSpanStart(ed) >= 0) return
                val text = ed.toString()
                ed.clear()
                if (text.isNotEmpty()) sendText(text)
            }

            override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
                super.commitText(text, newCursorPosition)
                flush()
                return true
            }

            override fun finishComposingText(): Boolean {
                super.finishComposingText()
                flush()
                return true
            }

            override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
                // 框里空着时按删除：发退格
                if (editable.isNullOrEmpty()) repeat(beforeLength) { screen.tapKey("Backspace") }
                return super.deleteSurroundingText(beforeLength, afterLength)
            }

            override fun sendKeyEvent(event: KeyEvent): Boolean {
                if (event.action == KeyEvent.ACTION_DOWN) onKeyDown(event.keyCode, event)
                return true
            }

            override fun performEditorAction(actionCode: Int): Boolean {
                screen.tapKey("Enter")
                return true
            }
        }
    }

    /** 输入法给的一段字：换行单独按回车，其余整段打过去 */
    private fun sendText(text: String) {
        val parts = text.split('\n')
        parts.forEachIndexed { i, part ->
            if (part.isNotEmpty()) screen.typeText(part)
            if (i < parts.size - 1) screen.tapKey("Enter")
        }
    }

    // ---- 实体键盘、输入法发的按键 ----

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        // 返回、音量这些交给系统
        if (event.isSystem && keyCode != KeyEvent.KEYCODE_ENTER) return super.onKeyDown(keyCode, event)
        val mods = buildList {
            if (event.isCtrlPressed) add("ControlLeft")
            if (event.isAltPressed) add("AltLeft")
            if (event.isMetaPressed) add("MetaLeft")
            if (event.isShiftPressed) add("ShiftLeft")
        }
        val code = KEY_CODES[keyCode]
        val ch = event.unicodeChar
        // 没按 Ctrl、Alt、⌘ 的可见字：当文字打，大小写、符号跟着手机这边的键盘布局
        if (!event.isCtrlPressed && !event.isAltPressed && !event.isMetaPressed && ch != 0 && !Character.isISOControl(ch)) {
            screen.typeText(String(Character.toChars(ch)))
            return true
        }
        if (code != null) {
            screen.tapKey(code, mods)
            return true
        }
        return super.onKeyDown(keyCode, event)
    }

    companion object {
        /** 安卓按键码 → 浏览器 KeyboardEvent.code */
        private val KEY_CODES: Map<Int, String> = buildMap {
            for (i in 0 until 26) put(KeyEvent.KEYCODE_A + i, "Key${'A' + i}")
            for (i in 0 until 10) put(KeyEvent.KEYCODE_0 + i, "Digit$i")
            for (i in 1..12) put(KeyEvent.KEYCODE_F1 + i - 1, "F$i")
            put(KeyEvent.KEYCODE_ENTER, "Enter")
            put(KeyEvent.KEYCODE_NUMPAD_ENTER, "Enter")
            put(KeyEvent.KEYCODE_DEL, "Backspace")
            put(KeyEvent.KEYCODE_FORWARD_DEL, "Delete")
            put(KeyEvent.KEYCODE_TAB, "Tab")
            put(KeyEvent.KEYCODE_ESCAPE, "Escape")
            put(KeyEvent.KEYCODE_SPACE, "Space")
            put(KeyEvent.KEYCODE_DPAD_LEFT, "ArrowLeft")
            put(KeyEvent.KEYCODE_DPAD_RIGHT, "ArrowRight")
            put(KeyEvent.KEYCODE_DPAD_UP, "ArrowUp")
            put(KeyEvent.KEYCODE_DPAD_DOWN, "ArrowDown")
            put(KeyEvent.KEYCODE_MOVE_HOME, "Home")
            put(KeyEvent.KEYCODE_MOVE_END, "End")
            put(KeyEvent.KEYCODE_PAGE_UP, "PageUp")
            put(KeyEvent.KEYCODE_PAGE_DOWN, "PageDown")
            put(KeyEvent.KEYCODE_MINUS, "Minus")
            put(KeyEvent.KEYCODE_EQUALS, "Equal")
            put(KeyEvent.KEYCODE_LEFT_BRACKET, "BracketLeft")
            put(KeyEvent.KEYCODE_RIGHT_BRACKET, "BracketRight")
            put(KeyEvent.KEYCODE_BACKSLASH, "Backslash")
            put(KeyEvent.KEYCODE_SEMICOLON, "Semicolon")
            put(KeyEvent.KEYCODE_APOSTROPHE, "Quote")
            put(KeyEvent.KEYCODE_COMMA, "Comma")
            put(KeyEvent.KEYCODE_PERIOD, "Period")
            put(KeyEvent.KEYCODE_SLASH, "Slash")
            put(KeyEvent.KEYCODE_GRAVE, "Backquote")
        }
    }
}

package io.github.ilovebinglu.p2pcli.screen

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.Process
import android.util.Log
import android.view.Surface
import uniffi.p2pcli_mobile.ScreenCodec
import uniffi.p2pcli_mobile.ScreenVideoConfig
import java.nio.ByteBuffer

/**
 * 系统硬件解码（MediaCodec），解出来直接画到 [Surface] 上，不经过我们的代码。
 *
 * 对方发来的是 AVCC（每个 NAL 前面几个字节的长度），MediaCodec 要 Annex-B（00 00 00 01 开头），喂之前换掉；
 * SPS、PPS（H.265 还有 VPS）从解码器配置记录（avcC、hvcC）里拿出来当 csd-0、csd-1。
 * 所有解码器的操作都在自己的线程上做，不占界面线程，也不卡 Rust 的线程。
 */
class VideoDecoder(
    /** 解码出错、刚换了显示的地方：要对方发一个关键帧（解码线程上调） */
    private val onNeedKeyframe: () -> Unit,
    /** 这台手机解不了这么大的画面、或者格式不支持（解码线程上调），参数是画面宽高 */
    private val onUnsupported: (Int, Int) -> Unit,
) {
    private val thread = HandlerThread("p2pcli-screen-decode", Process.THREAD_PRIORITY_DISPLAY).apply { start() }
    private val handler = Handler(thread.looper)

    // 下面这些只在解码线程上碰
    private var surface: Surface? = null
    private var config: ScreenVideoConfig? = null
    private var codec: MediaCodec? = null
    /** NAL 前面的长度占几个字节，从 avcC、hvcC 里读；一般是 4 */
    private var lengthSize = 4
    /** 下一帧要从关键帧开始 */
    private var needKey = true
    private val inputs = ArrayDeque<Int>()
    private val frames = ArrayDeque<Pair<Boolean, ByteArray>>()

    /** 显示的地方换了（TextureView 建好、销毁）。destroyed 回调里要等解码器放开旧的才能返回，done 在那之后调 */
    fun setSurface(s: Surface?, done: (() -> Unit)? = null) {
        handler.post {
            surface = s
            rebuild()
            done?.invoke()
        }
    }

    fun configure(c: ScreenVideoConfig) {
        handler.post {
            config = c
            rebuild()
        }
    }

    fun decode(key: Boolean, data: ByteArray) {
        handler.post {
            if (codec == null) return@post
            if (needKey && !key) return@post
            if (key) frames.clear()
            frames.addLast(key to data)
            // 解码跟不上：丢掉攒着的，从下一个关键帧接着来
            if (frames.size > 6) {
                frames.clear()
                needKey = true
                onNeedKeyframe()
                return@post
            }
            feed()
        }
    }

    /** 清掉重来，要一个关键帧（切回前台时） */
    fun reset() {
        handler.post {
            if (config != null && surface != null) rebuild()
        }
    }

    fun release() {
        handler.post {
            releaseCodec()
            thread.quitSafely()
        }
    }

    private fun releaseCodec() {
        codec?.let {
            try { it.stop() } catch (_: Exception) {}
            try { it.release() } catch (_: Exception) {}
        }
        codec = null
        inputs.clear()
        frames.clear()
        needKey = true
    }

    private fun rebuild() {
        releaseCodec()
        val s = surface ?: return
        val c = config ?: return
        val mime = if (c.codec == ScreenCodec.H265) MediaFormat.MIMETYPE_VIDEO_HEVC else MediaFormat.MIMETYPE_VIDEO_AVC
        val w = c.width.toInt()
        val h = c.height.toInt()
        val format = MediaFormat.createVideoFormat(mime, w, h)
        try {
            val csd = parameterSets(c)
            lengthSize = csd.lengthSize
            if (c.codec == ScreenCodec.H265) {
                format.setByteBuffer("csd-0", ByteBuffer.wrap(annexB(csd.vps + csd.sps + csd.pps)))
            } else {
                format.setByteBuffer("csd-0", ByteBuffer.wrap(annexB(csd.sps)))
                format.setByteBuffer("csd-1", ByteBuffer.wrap(annexB(csd.pps)))
            }
        } catch (e: Exception) {
            Log.w(TAG, "解码器配置记录读不了", e)
            return onUnsupported(w, h)
        }
        // 关键帧可能比默认的输入缓冲大
        format.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, maxOf(w * h, 1 shl 20))
        // 实时：不攒帧、解出来就送显
        format.setInteger(MediaFormat.KEY_PRIORITY, 0)
        if (Build.VERSION.SDK_INT >= 30) format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
        val mc = try {
            MediaCodec.createDecoderByType(mime)
        } catch (e: Exception) {
            Log.w(TAG, "建不了解码器", e)
            return onUnsupported(w, h)
        }
        // 高通的低延迟开关（安卓 11 以前的机器认这个，不认的会忽略）
        if (mc.name.startsWith("OMX.qcom") || mc.name.startsWith("c2.qti")) format.setInteger("vendor.qti-ext-dec-low-latency.enable", 1)
        mc.setCallback(object : MediaCodec.Callback() {
            override fun onInputBufferAvailable(codec: MediaCodec, index: Int) {
                if (codec !== this@VideoDecoder.codec) return
                inputs.addLast(index)
                feed()
            }

            override fun onOutputBufferAvailable(codec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
                if (codec !== this@VideoDecoder.codec) return
                try {
                    codec.releaseOutputBuffer(index, info.size > 0)
                } catch (_: Exception) {}
            }

            override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) {
                if (codec !== this@VideoDecoder.codec) return
                Log.w(TAG, "解码出错，重建解码器", e)
                rebuild()
                onNeedKeyframe()
            }

            override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {}
        }, handler)
        try {
            mc.configure(format, s, null, 0)
            mc.start()
        } catch (e: Exception) {
            Log.w(TAG, "解码器配置失败 ${w}x$h", e)
            try { mc.release() } catch (_: Exception) {}
            return onUnsupported(w, h)
        }
        codec = mc
        onNeedKeyframe()
    }

    private fun feed() {
        val mc = codec ?: return
        while (inputs.isNotEmpty() && frames.isNotEmpty()) {
            val index = inputs.removeFirst()
            val (key, data) = frames.removeFirst()
            try {
                val buf = mc.getInputBuffer(index) ?: continue
                buf.clear()
                val size = avccToAnnexB(data, buf)
                if (size < 0) {
                    // 放不下或者格式不对：这个缓冲留着下次用，从关键帧重来
                    inputs.addFirst(index)
                    frames.clear()
                    needKey = true
                    onNeedKeyframe()
                    return
                }
                mc.queueInputBuffer(index, 0, size, System.nanoTime() / 1000, if (key) MediaCodec.BUFFER_FLAG_KEY_FRAME else 0)
                needKey = false
            } catch (e: Exception) {
                Log.w(TAG, "喂帧失败", e)
                rebuild()
                return
            }
        }
    }

    /** 每个 NAL 的长度前缀换成 00 00 00 01，写进 out；放不下、长度不对返回 -1 */
    private fun avccToAnnexB(data: ByteArray, out: ByteBuffer): Int {
        var pos = 0
        val n = lengthSize
        while (pos + n <= data.size) {
            var len = 0
            for (i in 0 until n) len = (len shl 8) or (data[pos + i].toInt() and 0xff)
            pos += n
            if (len < 0 || pos + len > data.size) return -1
            if (out.remaining() < 4 + len) return -1
            out.put(START_CODE)
            out.put(data, pos, len)
            pos += len
        }
        return out.position()
    }

    private class ParameterSets(val lengthSize: Int, val vps: List<ByteArray>, val sps: List<ByteArray>, val pps: List<ByteArray>)

    private fun parameterSets(c: ScreenVideoConfig): ParameterSets {
        val b = c.description
        fun u8(i: Int) = b[i].toInt() and 0xff
        fun u16(i: Int) = (u8(i) shl 8) or u8(i + 1)
        fun nal(at: Int): Pair<ByteArray, Int> {
            val len = u16(at)
            return b.copyOfRange(at + 2, at + 2 + len) to at + 2 + len
        }
        if (c.codec == ScreenCodec.H264) {
            // avcC：版本、profile、兼容性、level、长度字节数、SPS 个数和内容、PPS 个数和内容
            val sps = mutableListOf<ByteArray>()
            val pps = mutableListOf<ByteArray>()
            var pos = 6
            repeat(u8(5) and 0x1f) { val (x, next) = nal(pos); sps += x; pos = next }
            val numPps = u8(pos++)
            repeat(numPps) { val (x, next) = nal(pos); pps += x; pos = next }
            require(sps.isNotEmpty() && pps.isNotEmpty()) { "没有 SPS、PPS" }
            return ParameterSets((u8(4) and 3) + 1, emptyList(), sps, pps)
        }
        // hvcC：前 22 字节是各种参数，第 21 字节低两位是长度字节数，然后是一组一组的 NAL（VPS 32、SPS 33、PPS 34）
        val vps = mutableListOf<ByteArray>()
        val sps = mutableListOf<ByteArray>()
        val pps = mutableListOf<ByteArray>()
        var pos = 23
        repeat(u8(22)) {
            val type = u8(pos) and 0x3f
            val count = u16(pos + 1)
            pos += 3
            repeat(count) {
                val (x, next) = nal(pos)
                pos = next
                when (type) {
                    32 -> vps += x
                    33 -> sps += x
                    34 -> pps += x
                }
            }
        }
        require(vps.isNotEmpty() && sps.isNotEmpty() && pps.isNotEmpty()) { "没有 VPS、SPS、PPS" }
        return ParameterSets((u8(21) and 3) + 1, vps, sps, pps)
    }

    private fun annexB(nals: List<ByteArray>): ByteArray {
        val out = java.io.ByteArrayOutputStream()
        for (n in nals) {
            out.write(START_CODE)
            out.write(n)
        }
        return out.toByteArray()
    }

    companion object {
        private const val TAG = "p2pcli.screen"
        private val START_CODE = byteArrayOf(0, 0, 0, 1)

        /** 这台手机能硬件解的格式：有硬件 H.265 解码器（安卓 10 起能可靠判断）就带上，对方会优先用它 */
        fun supportedCodecs(): List<ScreenCodec> {
            val list = mutableListOf(ScreenCodec.H264)
            if (Build.VERSION.SDK_INT >= 29) {
                val hevc = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.any { info ->
                    !info.isEncoder && info.isHardwareAccelerated &&
                        info.supportedTypes.any { it.equals(MediaFormat.MIMETYPE_VIDEO_HEVC, ignoreCase = true) } &&
                        hevcMain(info)
                }
                if (hevc) list.add(0, ScreenCodec.H265)
            }
            return list
        }

        private fun hevcMain(info: MediaCodecInfo): Boolean = try {
            info.getCapabilitiesForType(MediaFormat.MIMETYPE_VIDEO_HEVC).profileLevels.any {
                it.profile == MediaCodecInfo.CodecProfileLevel.HEVCProfileMain
            }
        } catch (_: Exception) {
            false
        }
    }
}

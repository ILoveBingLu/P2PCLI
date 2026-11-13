package io.github.ilovebinglu.p2pcli.ui

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.view.HapticFeedbackConstants
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.annotation.OptIn
import androidx.camera.core.CameraSelector
import androidx.camera.core.ExperimentalGetImage
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.keyframes
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.scale
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.min
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.google.mlkit.vision.barcode.BarcodeScannerOptions
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.common.InputImage
import io.github.ilovebinglu.p2pcli.Core
import io.github.ilovebinglu.p2pcli.t
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import uniffi.p2pcli_mobile.parsePairLink

private enum class Phase { SCANNING, FOUND, WRONG }

/**
 * 扫码页：扫电脑上 P2PCLI 标题栏手机图标里的二维码，里面有设备码和配对密码，扫完直接连。
 * 相机画面铺满，上面画取景框、扫描线；认出来框变绿收紧、弹出对勾、震一下；扫到别的码框变红抖一下，接着扫。
 * 系统返回键、左上角的关闭都只关这一页。
 */
@Composable
fun ScanScreen(core: Core) {
    val context = LocalContext.current
    var granted by remember { mutableStateOf(hasCamera(context)) }
    var asked by remember { mutableStateOf(false) }
    val request = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
        granted = ok
        asked = true
    }
    LaunchedEffect(Unit) { if (!granted) request.launch(Manifest.permission.CAMERA) }
    // 从系统设置里开了权限回来
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) { granted = hasCamera(context) }

    Box(Modifier.fillMaxSize().background(Color.Black)) {
        when {
            granted -> Scanner(core)
            asked -> Message(t("没有相机权限"), t("在系统设置里允许 P2PCLI 使用相机，或者输入设备码连接。")) {
                Pill(t("去设置"), primary = true) {
                    context.startActivity(
                        Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:" + context.packageName))
                            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                    )
                }
            }
        }
        // 顶上：关闭、标题
        Row(
            Modifier.fillMaxWidth().statusBarsPadding().padding(horizontal = 12.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                Modifier.size(44.dp).clip(RoundedCornerShape(22.dp)).background(Color.Black.copy(alpha = 0.45f)).clickable { core.closeScan() },
                contentAlignment = Alignment.Center,
            ) { Icon(Lucide.X, t("关闭扫码"), Modifier.size(22.dp), tint = Color.White) }
            Text(t("扫码连接"), color = Color.White, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center,
                modifier = Modifier.weight(1f))
            Spacer(Modifier.size(44.dp))
        }
        // 底下：改成手输设备码
        Box(Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(bottom = 28.dp)) {
            Pill(t("输入设备码"), icon = Lucide.KeyRound) { core.scanToManual() }
        }
    }
}

private fun hasCamera(context: Context) =
    ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED

@OptIn(ExperimentalGetImage::class)
@Composable
private fun Scanner(core: Core) {
    val context = LocalContext.current
    val owner = LocalLifecycleOwner.current
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    var phase by remember { mutableStateOf(Phase.SCANNING) }
    var cameraError by remember { mutableStateOf<String?>(null) }
    val preview = remember { PreviewView(context).apply { scaleType = PreviewView.ScaleType.FILL_CENTER } }

    // 认到一个码：是 P2PCLI 的就连，不是就提示一下接着扫
    val onCode by rememberUpdatedState { raw: String ->
        if (phase == Phase.SCANNING) {
            val pair = parsePairLink(raw)
            if (pair != null) {
                phase = Phase.FOUND
                view.performHapticFeedback(if (Build.VERSION.SDK_INT >= 30) HapticFeedbackConstants.CONFIRM else HapticFeedbackConstants.LONG_PRESS)
                scope.launch { delay(550); core.scanned(pair) }
            } else {
                phase = Phase.WRONG
                view.performHapticFeedback(if (Build.VERSION.SDK_INT >= 30) HapticFeedbackConstants.REJECT else HapticFeedbackConstants.LONG_PRESS)
                scope.launch { delay(1600); if (phase == Phase.WRONG) phase = Phase.SCANNING }
            }
        }
    }

    DisposableEffect(owner) {
        val scanner = BarcodeScanning.getClient(BarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build())
        val main = ContextCompat.getMainExecutor(context)
        val previewUse = Preview.Builder().build().also { it.setSurfaceProvider(preview.surfaceProvider) }
        val analysis = ImageAnalysis.Builder().setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST).build()
        analysis.setAnalyzer(main) { proxy ->
            val image = proxy.image
            if (image == null || phase != Phase.SCANNING) {
                proxy.close()
                return@setAnalyzer
            }
            scanner.process(InputImage.fromMediaImage(image, proxy.imageInfo.rotationDegrees))
                .addOnSuccessListener { codes -> codes.firstNotNullOfOrNull { it.rawValue }?.let { onCode(it) } }
                .addOnCompleteListener { proxy.close() }
        }
        var provider: ProcessCameraProvider? = null
        val future = ProcessCameraProvider.getInstance(context)
        future.addListener({
            try {
                provider = future.get().also {
                    it.unbindAll()
                    it.bindToLifecycle(owner, CameraSelector.DEFAULT_BACK_CAMERA, previewUse, analysis)
                }
            } catch (e: Exception) {
                cameraError = e.message ?: e.toString()
            }
        }, main)
        onDispose {
            analysis.clearAnalyzer()
            provider?.unbind(previewUse, analysis)
            scanner.close()
        }
    }

    AndroidView(factory = { preview }, modifier = Modifier.fillMaxSize())
    cameraError?.let {
        Message(t("打不开相机"), it) {}
        return
    }
    Viewfinder(phase)
}

/** 取景框：四周压暗，框里透亮，四个角、扫描线、对勾 */
@Composable
private fun Viewfinder(phase: Phase) {
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val size = min(maxWidth * 0.68f, 280.dp)
        val left = (maxWidth - size) / 2
        // 稍微偏上，看着居中
        val top = (maxHeight - size) / 2 - maxHeight * 0.05f
        val density = LocalDensity.current
        val radius = 22.dp

        // 抖一下：扫到的不是 P2PCLI 的码
        val shake = remember { Animatable(0f) }
        LaunchedEffect(phase) {
            if (phase == Phase.WRONG) shake.animateTo(0f, keyframes { durationMillis = 420; -8f at 80; 8f at 160; -8f at 240; 8f at 320; 0f at 420 })
        }
        val color by animateColorAsState(
            when (phase) {
                Phase.FOUND -> Hero.Success
                Phase.WRONG -> Hero.Danger
                Phase.SCANNING -> Hero.Accent
            },
            tween(250), label = "frame",
        )
        val inset by animateDpAsState(if (phase == Phase.FOUND) 10.dp else 0.dp, spring(dampingRatio = 0.55f, stiffness = 300f), label = "inset")

        // 压暗：整屏半透明黑，框的位置挖空
        Box(
            Modifier.fillMaxSize().graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen }.drawBehind {
                drawRect(Color.Black.copy(alpha = 0.6f))
                with(density) {
                    drawRoundRect(
                        Color.Black,
                        topLeft = Offset((left + shake.value.dp).toPx(), top.toPx()),
                        size = Size(size.toPx(), size.toPx()),
                        cornerRadius = CornerRadius(radius.toPx()),
                        blendMode = BlendMode.Clear,
                    )
                }
            },
        )

        Box(Modifier.offset(x = left + shake.value.dp, y = top).size(size)) {
            // 扫描线：从上往下扫，拖着一道光
            if (phase == Phase.SCANNING) {
                val sweep = rememberInfiniteTransition(label = "sweep")
                val y by sweep.animateFloat(-72f, size.value, infiniteRepeatable(tween(2200, easing = LinearEasing), RepeatMode.Restart), label = "y")
                val alpha = ((y + 72f) / 40f).coerceIn(0f, 1f) * ((size.value - y) / 40f).coerceIn(0f, 1f)
                Box(Modifier.fillMaxSize().clip(RoundedCornerShape(radius))) {
                    Box(
                        Modifier.fillMaxWidth().padding(horizontal = size * 0.08f).height(72.dp).offset(y = y.dp)
                            .graphicsLayer { this.alpha = alpha }
                            .background(Brush.verticalGradient(listOf(Color.Transparent, Hero.Accent.copy(alpha = 0.28f)))),
                    ) {
                        Box(Modifier.align(Alignment.BottomCenter).fillMaxWidth().height(2.dp).background(Hero.Accent))
                    }
                }
            }
            // 四个角：每个角是一段短直线、一段圆角弧、再一段短直线
            Canvas(Modifier.fillMaxSize()) {
                val r = radius.toPx()
                val e = 12.dp.toPx()
                val p = inset.toPx()
                val w = this.size.width
                val stroke = Stroke(width = 4.dp.toPx(), cap = StrokeCap.Round)
                val corners = listOf(
                    // 左上
                    Path().apply { moveTo(p, p + r + e); lineTo(p, p + r); arcTo(Rect(p, p, p + 2 * r, p + 2 * r), 180f, 90f, false); lineTo(p + r + e, p) },
                    // 右上
                    Path().apply { moveTo(w - p - r - e, p); lineTo(w - p - r, p); arcTo(Rect(w - p - 2 * r, p, w - p, p + 2 * r), 270f, 90f, false); lineTo(w - p, p + r + e) },
                    // 右下
                    Path().apply { moveTo(w - p, w - p - r - e); lineTo(w - p, w - p - r); arcTo(Rect(w - p - 2 * r, w - p - 2 * r, w - p, w - p), 0f, 90f, false); lineTo(w - p - r - e, w - p) },
                    // 左下
                    Path().apply { moveTo(p + r + e, w - p); lineTo(p + r, w - p); arcTo(Rect(p, w - p - 2 * r, p + 2 * r, w - p), 90f, 90f, false); lineTo(p, w - p - r - e) },
                )
                corners.forEach { drawPath(it, color, style = stroke) }
            }
            // 认出来了：中间弹出对勾
            val check by animateDpAsState(if (phase == Phase.FOUND) 64.dp else 0.dp, spring(dampingRatio = 0.5f, stiffness = 400f), label = "check")
            if (check > 1.dp) {
                Box(Modifier.align(Alignment.Center).size(check).clip(RoundedCornerShape(check / 2)).background(Hero.Success), contentAlignment = Alignment.Center) {
                    Icon(Lucide.Check, null, Modifier.size(check * 0.5f), tint = Color.White)
                }
            }
        }

        // 框下面的提示
        Box(Modifier.fillMaxWidth().offset(y = top + size + 28.dp), contentAlignment = Alignment.TopCenter) {
            Text(
                when (phase) {
                    Phase.FOUND -> t("已识别，正在连接…")
                    Phase.WRONG -> t("这不是 P2PCLI 的连接二维码")
                    Phase.SCANNING -> t("对准电脑上 P2PCLI 显示的二维码")
                },
                color = when (phase) {
                    Phase.FOUND -> Color(0xFF86EFAC)
                    Phase.WRONG -> Color(0xFFFCA5A5)
                    Phase.SCANNING -> Color.White
                },
                fontSize = 14.sp,
                textAlign = TextAlign.Center,
                modifier = Modifier.widthIn(max = 300.dp).clip(RoundedCornerShape(999.dp)).background(Color.Black.copy(alpha = 0.45f))
                    .padding(horizontal = 16.dp, vertical = 8.dp),
            )
        }
    }
}

/** 没有权限、相机打不开：压暗的整页，中间一段说明 */
@Composable
private fun Message(title: String, text: String, action: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.85f)).padding(horizontal = 32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(title, color = Color.White, fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
        Text(text, color = Color.White.copy(alpha = 0.7f), fontSize = 14.5.sp, textAlign = TextAlign.Center,
            modifier = Modifier.padding(top = 12.dp, bottom = 20.dp))
        action()
    }
}

/** 半透明的胶囊按钮 */
@Composable
private fun Pill(text: String, primary: Boolean = false, icon: androidx.compose.ui.graphics.vector.ImageVector? = null, onClick: () -> Unit) {
    Row(
        Modifier.height(48.dp).clip(RoundedCornerShape(24.dp)).background(if (primary) Hero.Accent else Color.Black.copy(alpha = 0.45f))
            .clickable(onClick = onClick).padding(horizontal = 22.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        icon?.let { Icon(it, null, Modifier.size(18.dp), tint = Color.White) }
        Text(text, color = Color.White, fontSize = 15.sp)
    }
}

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
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
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
 * 相机画面铺满，不另画取景框、扫描线；下面一行字提示状态：认出来变绿、震一下，扫到别的码变红，接着扫。
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
    Hint(phase)
}

/** 相机画面上的一行提示：放在“输入设备码”上面 */
@Composable
private fun Hint(phase: Phase) {
    Box(Modifier.fillMaxSize().navigationBarsPadding().padding(bottom = 96.dp), contentAlignment = Alignment.BottomCenter) {
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

package io.github.ilovebinglu.p2pcli

import android.os.Bundle
import android.graphics.Color
import android.os.Build
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import io.github.ilovebinglu.p2pcli.ui.HeroTheme
import io.github.ilovebinglu.p2pcli.ui.Root

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // 状态栏、导航栏完全透明，不加系统的半透明遮罩：安全区透出的就是页面本来的底色，界面只是让开它
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.dark(Color.TRANSPARENT),
            navigationBarStyle = SystemBarStyle.dark(Color.TRANSPARENT),
        )
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            window.isNavigationBarContrastEnforced = false
            window.isStatusBarContrastEnforced = false
        }
        val core = (application as P2pApplication).core
        setContent {
            HeroTheme { Root(core) }
        }
    }
}

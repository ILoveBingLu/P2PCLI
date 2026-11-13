package io.github.ilovebinglu.p2pcli

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import io.github.ilovebinglu.p2pcli.ui.HeroTheme
import io.github.ilovebinglu.p2pcli.ui.Root

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val core = (application as P2pApplication).core
        setContent {
            HeroTheme { Root(core) }
        }
    }
}

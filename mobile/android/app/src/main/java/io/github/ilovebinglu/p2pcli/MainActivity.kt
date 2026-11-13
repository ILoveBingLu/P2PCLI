package io.github.ilovebinglu.p2pcli

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import io.github.ilovebinglu.p2pcli.ui.P2pTheme
import io.github.ilovebinglu.p2pcli.ui.Root

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val core = (application as P2pApplication).core
        setContent {
            P2pTheme { Root(core) }
        }
    }
}

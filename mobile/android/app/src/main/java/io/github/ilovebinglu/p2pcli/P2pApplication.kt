package io.github.ilovebinglu.p2pcli

import android.app.Application

class P2pApplication : Application() {
    lateinit var core: Core
        private set

    override fun onCreate() {
        super.onCreate()
        core = Core(this)
        core.start()
    }
}

package io.github.ilovebinglu.p2pcli

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import org.json.JSONObject
import java.util.Locale

/**
 * 界面文字：和桌面端同一份翻译。键是中文原文，英文在桌面的 app/src/i18n/locales/en.json 里，
 * 打包时拷进 App（见 app/build.gradle.kts 的 copyI18n）。手机新加的文案也加在那两个文件里，
 * scripts/check-ui-i18n.mjs 会一起检查手机代码里用 t 和 tr 包起来的文字。
 *
 * 跟随系统时：中文环境用简体中文，其他环境用英文，和桌面一样。
 */
object I18n {
    enum class Language { SYSTEM, CHINESE, ENGLISH }

    /** 界面上选的语言；改了界面马上跟着变（读它的地方会重组） */
    var language by mutableStateOf(Language.SYSTEM)
        private set
    private var english: Map<String, String> = emptyMap()
    private const val PREFS = "p2pcli"
    private const val KEY = "language"

    fun init(context: Context) {
        english = try {
            val json = JSONObject(context.assets.open("i18n/en.json").bufferedReader().use { it.readText() })
            json.keys().asSequence().associateWith { json.getString(it) }
        } catch (_: Exception) {
            emptyMap()
        }
        val saved = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY, null)
        language = Language.entries.firstOrNull { it.name == saved } ?: Language.SYSTEM
    }

    fun setLanguage(context: Context, value: Language) {
        language = value
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(KEY, value.name).apply()
    }

    val isEnglish: Boolean
        get() = when (language) {
            Language.ENGLISH -> true
            Language.CHINESE -> false
            Language.SYSTEM -> !Locale.getDefault().language.startsWith("zh")
        }

    internal fun lookup(key: String, count: Long?): String? {
        if (!isEnglish) return null
        if (count == 1L) english["${key}_one"]?.let { return it }
        return english[key]
    }
}

/**
 * 界面文字。键是中文原文，{{name}} 这样的占位用后面的参数填：t(键, "name" to 设备名)。
 * 参数里有 count 时，英文按单复数取（键加 _one 的是单数），和桌面的 i18next 一样
 */
fun t(key: String, vararg args: Pair<String, Any?>): String {
    val count = args.firstOrNull { it.first == "count" }?.second?.let { (it as? Number)?.toLong() ?: it.toString().toLongOrNull() }
    var text = I18n.lookup(key, count) ?: key
    for ((name, value) in args) text = text.replace("{{$name}}", value.toString())
    return text
}

/** Rust 那边传来的中文提示：翻译里有就换成英文，没有就原样显示 */
fun tr(message: String): String = I18n.lookup(message, null) ?: message

import Foundation
import Observation

/// 界面语言：和桌面、安卓同一份翻译。键是中文原文，英文在桌面的 app/src/i18n/locales/en.json 里，
/// 打包时整个文件放进 App。手机新加的文案也加在那两个文件里（scripts/check-ui-i18n.mjs 会检查）。
/// 跟随系统时：中文环境用简体中文，其他环境用英文，和桌面一样。
@Observable
final class I18n {
    enum Language: String, CaseIterable { case system, chinese, english }

    static let shared = I18n()

    /// 界面上选的语言；改了界面马上跟着变（读它的视图会重画）
    var language: Language {
        didSet { UserDefaults.standard.set(language.rawValue, forKey: Self.key) }
    }

    @ObservationIgnored private let english: [String: String]
    private static let key = "language"

    private init() {
        if let url = Bundle.main.url(forResource: "en", withExtension: "json"),
           let data = try? Data(contentsOf: url),
           let dict = try? JSONSerialization.jsonObject(with: data) as? [String: String] {
            english = dict
        } else {
            english = [:]
        }
        language = Language(rawValue: UserDefaults.standard.string(forKey: Self.key) ?? "") ?? .system
    }

    var isEnglish: Bool {
        switch language {
        case .english: true
        case .chinese: false
        case .system: !(Locale.preferredLanguages.first ?? "en").hasPrefix("zh")
        }
    }

    func lookup(_ key: String, count: Int?) -> String? {
        guard isEnglish else { return nil }
        if count == 1, let one = english[key + "_one"] { return one }
        return english[key]
    }
}

/// 界面文字。键是中文原文，{{name}} 这样的占位用后面的参数填：t("连接 {{name}}", ["name": 设备名])。
/// 参数里有 count 时，英文按单复数取（键加 _one 的是单数），和桌面的 i18next 一样
func t(_ key: String, _ args: [String: Any] = [:]) -> String {
    let count = args["count"].flatMap { v -> Int? in
        if let n = v as? Int { return n }
        if let n = v as? UInt64 { return Int(n) }
        return Int("\(v)")
    }
    var text = I18n.shared.lookup(key, count: count) ?? key
    for (name, value) in args {
        text = text.replacingOccurrences(of: "{{\(name)}}", with: "\(value)")
    }
    return text
}

/// Rust 那边传来的中文提示：翻译里有就换成英文，没有就原样显示
func tr(_ message: String) -> String {
    I18n.shared.lookup(message, count: nil) ?? message
}

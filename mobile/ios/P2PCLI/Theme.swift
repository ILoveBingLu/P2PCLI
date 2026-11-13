import SwiftUI

/// 颜色：桌面端 HeroUI 深色主题的取值（和安卓一样）
enum Hero {
    static let background = Color(hex: 0x060607)
    static let surface = Color(hex: 0x18181B)
    static let surfaceSecondary = Color(hex: 0x232325)
    static let tile = Color(hex: 0x27272A)
    static let foreground = Color(hex: 0xFCFCFC)
    static let muted = Color(hex: 0x9F9FA9)
    static let accent = Color(hex: 0x0485F7)
    static let success = Color(hex: 0x17C964)
    static let warning = Color(hex: 0xF5A524)
    static let danger = Color(hex: 0xDB3B3E)
    static let terminal = Color(hex: 0x111114)
    static let mono = Font.system(.body, design: .monospaced)
}

extension Color {
    init(hex: UInt32) {
        self.init(.sRGB, red: Double((hex >> 16) & 0xFF) / 255, green: Double((hex >> 8) & 0xFF) / 255,
                  blue: Double(hex & 0xFF) / 255, opacity: 1)
    }
}

extension UIColor {
    convenience init(hex: UInt32) {
        self.init(red: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                  blue: CGFloat(hex & 0xFF) / 255, alpha: 1)
    }
}

/// lucide 图标（和桌面、安卓同一套）。资源由 tools/make_assets.py 生成，名字是 lucide 的原名
struct Icon: View {
    let name: String
    var size: CGFloat = 20

    init(_ name: String, size: CGFloat = 20) {
        self.name = name
        self.size = size
    }

    var body: some View {
        Image("lucide.\(name)").resizable().scaledToFit().frame(width: size, height: size)
    }
}

/// 工具栏、菜单里的项：文字和图标都给，系统按地方挑（iPhone Duo 竖排工具栏只放图标，溢出菜单放图标加文字）
func lucideLabel(_ title: String, _ icon: String) -> Label<Text, Image> {
    Label { Text(title) } icon: { Image("lucide.\(icon)") }
}

/// 方块底的图标：设备、会话前面那个
struct Tile<Content: View>: View {
    var size: CGFloat = 42
    var connected = false
    var dimmed = false
    @ViewBuilder var content: Content

    var body: some View {
        let radius: CGFloat = size > 60 ? 24 : 13
        ZStack(alignment: .bottomTrailing) {
            RoundedRectangle(cornerRadius: radius, style: .continuous)
                .fill(Hero.tile)
                .frame(width: size, height: size)
                .overlay { content }
                .overlay {
                    if dimmed {
                        RoundedRectangle(cornerRadius: radius, style: .continuous).fill(Hero.background.opacity(0.4))
                    }
                }
            if connected {
                let dot: CGFloat = size > 60 ? 18 : 12
                Circle().fill(Hero.success)
                    .frame(width: dot, height: dot)
                    .padding(size > 60 ? 3.5 : 2.5)
                    .background(Circle().fill(Hero.surface))
                    .offset(x: 3, y: 3)
                    .accessibilityHidden(true)
            }
        }
    }
}

/// 系统图标：苹果、Windows、Linux、安卓、鸿蒙，和桌面同一套
struct PlatformTile: View {
    let platform: String
    var connected = false
    var size: CGFloat = 42

    var body: some View {
        Tile(size: size, connected: connected) {
            if let name = Self.icon(platform) {
                Image(name).resizable().scaledToFit()
                    .frame(width: size > 60 ? 36 : 20, height: size > 60 ? 36 : 20)
                    .foregroundStyle(Hero.foreground)
                    .accessibilityLabel(Self.label(platform))
            } else {
                Text(platform.prefix(3)).font(.system(size: 12, weight: .semibold)).foregroundStyle(Hero.foreground)
            }
        }
    }

    static func icon(_ platform: String) -> String? {
        switch platform {
        case "macos", "ios": "platform.apple"
        case "windows": "platform.windows"
        case "linux": "platform.linux"
        case "android": "platform.android"
        case "harmonyos": "platform.harmonyos"
        default: nil
        }
    }

    static func label(_ platform: String) -> String {
        ["macos": "macOS", "ios": "iOS", "windows": "Windows", "linux": "Linux", "android": "Android",
         "harmonyos": "HarmonyOS"][platform] ?? platform
    }
}

/// 命令行 AI 的图标（和桌面同一套）；没收录的用机器人图标
struct AgentIcon: View {
    let kind: String
    var size: CGFloat = 20

    var body: some View {
        if UIImage(named: "agent.\(kind)") != nil {
            Image("agent.\(kind)").resizable().scaledToFit().frame(width: size, height: size)
        } else {
            Icon("bot", size: size).foregroundStyle(Hero.foreground)
        }
    }
}

struct Dot: View {
    let color: Color
    var size: CGFloat = 8
    var body: some View { Circle().fill(color).frame(width: size, height: size) }
}

/// 小标签：“已打开”“局域网”
struct Chip: View {
    let text: String
    var accent = false
    var body: some View {
        Text(text)
            .font(.system(size: 11, weight: .medium))
            .foregroundStyle(accent ? Hero.accent : Hero.muted)
            .padding(.horizontal, 7).padding(.vertical, 2)
            .background(Capsule().fill(accent ? Hero.accent.opacity(0.16) : Hero.tile))
    }
}

/// 液态玻璃：iOS 26 以上用系统的玻璃材质，旧系统用实心底色
extension View {
    @ViewBuilder
    func heroGlass(cornerRadius: CGFloat, tint: Color? = nil, interactive: Bool = false) -> some View {
        if #available(iOS 26.0, *) {
            self.glassEffect(Self.glass(tint: tint, interactive: interactive), in: .rect(cornerRadius: cornerRadius))
        } else {
            self.background(tint?.opacity(0.35) ?? Hero.tile, in: .rect(cornerRadius: cornerRadius))
        }
    }

    @ViewBuilder
    func heroCapsuleGlass(tint: Color? = nil, interactive: Bool = false) -> some View {
        if #available(iOS 26.0, *) {
            self.glassEffect(Self.glass(tint: tint, interactive: interactive), in: .capsule)
        } else {
            self.background(tint?.opacity(0.35) ?? Hero.surface, in: .capsule)
        }
    }

    @available(iOS 26.0, *)
    private static func glass(tint: Color?, interactive: Bool) -> Glass {
        var g = Glass.regular
        if let tint { g = g.tint(tint) }
        if interactive { g = g.interactive() }
        return g
    }

    /// 主要按钮：iOS 26 以上是带颜色的玻璃
    @ViewBuilder
    func heroProminent() -> some View {
        if #available(iOS 26.0, *) {
            self.buttonStyle(.glassProminent)
        } else {
            self.buttonStyle(.borderedProminent)
        }
    }

    @ViewBuilder
    func heroSecondary() -> some View {
        if #available(iOS 26.0, *) {
            self.buttonStyle(.glass)
        } else {
            self.buttonStyle(.bordered)
        }
    }
}

/// 一组玻璃控件放一起：iOS 26 以上靠得近时会融在一起
struct GlassGroup<Content: View>: View {
    var spacing: CGFloat = 6
    @ViewBuilder var content: Content

    var body: some View {
        if #available(iOS 26.0, *) {
            GlassEffectContainer(spacing: spacing) { content }
        } else {
            content
        }
    }
}

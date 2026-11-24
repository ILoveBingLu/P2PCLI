import SwiftUI
import UIKit

/// 远程控制页：铺满屏幕的对方画面，底下一条浮着的工具栏；弹出键盘时换成按键条（Esc、Tab、修饰键、方向键）
struct RemoteScreenView: View {
    @Environment(Core.self) private var core
    let screen: RemoteScreen

    var body: some View {
        ZStack {
            CanvasHost(canvas: screen.canvas)
                .ignoresSafeArea()
            status
            VStack {
                Spacer()
                if screen.keyboardUp {
                    ScreenKeyBar(screen: screen)
                } else if screen.toolbarCollapsed {
                    HStack {
                        Spacer()
                        Button { screen.toolbarCollapsed = false } label: {
                            Icon("ellipsis", size: 20).foregroundStyle(.white).frame(width: 44, height: 44)
                        }
                        .buttonStyle(.plain)
                        .heroCapsuleGlass(interactive: true)
                        .accessibilityLabel(t("显示工具栏"))
                    }
                    .padding(.horizontal, 16)
                    .padding(.bottom, 8)
                } else {
                    ScreenToolbar(screen: screen)
                        .padding(.bottom, 8)
                }
            }
        }
        .background(Color.black)
        .statusBarHidden(true)
        .persistentSystemOverlays(.hidden)
        .preferredColorScheme(.dark)
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillShowNotification)) { _ in
            screen.keyboardUp = true
        }
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillHideNotification)) { _ in
            screen.keyboardUp = false
        }
    }

    /// 画面上面的提示：等同意、暂停、结束
    @ViewBuilder private var status: some View {
        switch screen.phase {
        case .opening:
            card {
                ProgressView().tint(.white)
                Text(t("等待对方同意…")).font(.headline)
                Text(t("对方电脑上会弹窗问要不要让你看、能不能操作。")).font(.subheadline).foregroundStyle(Hero.muted)
                    .multilineTextAlignment(.center)
                Button(t("取消")) { core.closeScreen() }.heroProminent()
            }
        case .live:
            if !screen.hasVideo {
                ProgressView().tint(.white).controlSize(.large)
            } else if let note = screen.started?.controlNote, !(screen.started?.control ?? true) {
                VStack {
                    Text(t("只能看，不能操作：{{reason}}", ["reason": tr(note)]))
                        .font(.footnote).foregroundStyle(.white)
                        .padding(.horizontal, 14).padding(.vertical, 8)
                        .heroCapsuleGlass()
                        .padding(.top, 12)
                    Spacer()
                }
            }
        case .paused(let relay):
            card {
                Icon("circle-pause", size: 32)
                Text(relay ? t("现在这条连接走的是中继，画面暂停") : t("正在找直连的路，画面暂停")).font(.headline)
                Text(t("远程画面只走直连，直连接通后自动接着放。")).font(.subheadline).foregroundStyle(Hero.muted)
                    .multilineTextAlignment(.center)
            }
        case .closed(let reason):
            card {
                Icon("monitor", size: 32)
                Text(reason == nil ? t("远程画面已结束") : t("远程画面断开了")).font(.headline)
                if let reason { Text(reason).font(.subheadline).foregroundStyle(Hero.muted).multilineTextAlignment(.center) }
                HStack(spacing: 12) {
                    Button(t("关闭")) { core.closeScreen() }.buttonStyle(.bordered)
                    if core.conns[screen.endpointId] != nil {
                        Button(t("重新打开")) { screen.open() }.heroProminent()
                    }
                }
            }
        }
    }

    private func card<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        VStack(spacing: 12) { content() }
            .foregroundStyle(.white)
            .padding(24)
            .frame(maxWidth: 340)
            .background(Hero.surface.opacity(0.92), in: RoundedRectangle(cornerRadius: 22))
            .padding(24)
    }
}

/// 画面那块用 UIKit：显示层、指针、手势、藏起来的输入框
private struct CanvasHost: UIViewRepresentable {
    let canvas: ScreenCanvas
    func makeUIView(context: Context) -> ScreenCanvas { canvas }
    func updateUIView(_ uiView: ScreenCanvas, context: Context) {}
}

/// 平时的工具栏：关闭、键盘、操作方式、换屏幕和画质、收起
private struct ScreenToolbar: View {
    @Environment(Core.self) private var core
    let screen: RemoteScreen

    var body: some View {
        GlassGroup(spacing: 6) {
            HStack(spacing: 6) {
                button(t("关闭远程画面"), "x") { core.closeScreen() }
                if screen.canControl {
                    button(t("键盘"), "keyboard") { screen.canvas.showKeyboard() }
                    button(screen.mode == .trackpad ? t("触控板模式") : t("直接点模式"),
                           screen.mode == .trackpad ? "mouse-pointer-2" : "pointer") {
                        screen.mode = screen.mode == .trackpad ? .touch : .trackpad
                        core.notify(screen.mode == .trackpad
                                    ? t("触控板模式：手指滑动挪指针，点一下点在指针上")
                                    : t("直接点模式：手指点哪儿就点哪儿"))
                    }
                }
                Menu {
                    if let displays = screen.started?.displays, displays.count > 1 {
                        Section(t("屏幕")) {
                            ForEach(displays, id: \.id) { d in
                                Button {
                                    screen.switchDisplay(d.id)
                                } label: {
                                    if d.id == screen.display { Label(d.name, systemImage: "checkmark") } else { Text(d.name) }
                                }
                            }
                        }
                    }
                    Section(t("画质")) {
                        ForEach([ScreenQuality.sharp, .balanced, .smooth], id: \.self) { q in
                            Button { screen.setQuality(q) } label: {
                                if q == screen.quality { Label(qualityName(q), systemImage: "checkmark") } else { Text(qualityName(q)) }
                            }
                        }
                    }
                    if let s = screen.stats {
                        Section {
                            Text("\(s.fps) fps · \(String(format: "%.1f", Double(s.bitrate) / 1_000_000)) Mbps")
                        }
                    }
                    Section {
                        Button { screen.toolbarCollapsed = true } label: { Text(t("收起工具栏")) }
                    }
                } label: {
                    Icon("ellipsis", size: 20).foregroundStyle(.white).frame(width: 44, height: 44)
                }
                .heroCapsuleGlass(interactive: true)
                .accessibilityLabel(t("更多"))
            }
        }
    }

    private func button(_ label: String, _ icon: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Icon(icon, size: 20).foregroundStyle(.white).frame(width: 44, height: 44)
        }
        .buttonStyle(.plain)
        .heroCapsuleGlass(interactive: true)
        .accessibilityLabel(label)
    }
}

func qualityName(_ q: ScreenQuality) -> String {
    switch q {
    case .sharp: return t("清晰")
    case .balanced: return t("均衡")
    case .smooth: return t("流畅")
    }
}

/// 键盘弹着时的按键条：手机键盘上没有的键。修饰键按一下粘住，下一个键或点击带上它
private struct ScreenKeyBar: View {
    let screen: RemoteScreen

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            GlassGroup(spacing: 4) {
                HStack(spacing: 4) {
                    KeyButton(label: "Esc") { screen.tapKey("Escape") }
                    KeyButton(label: "Tab") { screen.tapKey("Tab") }
                    modifier("Ctrl", "ControlLeft")
                    modifier(screen.isMac ? "⌥" : "Alt", "AltLeft")
                    modifier(screen.isMac ? "⌘" : "Win", "MetaLeft")
                    modifier("⇧", "ShiftLeft")
                    KeyButton(label: "←", icon: "arrow-left") { screen.tapKey("ArrowLeft") }
                    KeyButton(label: "↑", icon: "arrow-up") { screen.tapKey("ArrowUp") }
                    KeyButton(label: "↓", icon: "arrow-down") { screen.tapKey("ArrowDown") }
                    KeyButton(label: "→", icon: "arrow-right") { screen.tapKey("ArrowRight") }
                    KeyButton(label: t("删除"), icon: "delete") { screen.tapKey("Delete") }
                    KeyButton(label: t("收起键盘"), icon: "keyboard-off") { screen.canvas.hideKeyboard() }
                }
            }
            .padding(.horizontal, 6)
        }
        .frame(height: 40)
        .padding(.vertical, 8)
    }

    private func modifier(_ label: String, _ code: String) -> some View {
        KeyButton(label: label, active: screen.modifiers.contains(code)) { screen.toggleModifier(code) }
    }
}

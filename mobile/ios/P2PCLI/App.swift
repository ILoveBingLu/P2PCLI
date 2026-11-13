import SwiftUI

@main
struct P2PCLIApp: App {
    @State private var core = Core.shared
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(core)
                .environment(I18n.shared)
                .preferredColorScheme(.dark)
                .tint(Hero.accent)
                .onAppear { if core.start == .starting { core.startCore() } }
        }
        .onChange(of: phase) { _, now in
            switch now {
            case .background: core.enteredBackground()
            case .active: core.becameActive()
            default: break
            }
        }
    }
}

/// 设备列表在左、设备和终端在右。
/// 用系统的分栏容器：窄屏（普通 iPhone、iPhone Duo 合上时的外屏）自动变成一层层推进去的页面，
/// 宽屏（iPhone Duo 展开、iPad、横屏）两栏并排，折叠时系统会让两栏避开折痕
struct RootView: View {
    @Environment(Core.self) private var core
    @State private var columns: NavigationSplitViewVisibility = .all

    var body: some View {
        @Bindable var core = core
        Group {
            switch core.start {
            case .starting:
                VStack(spacing: 20) {
                    Image("logo").resizable().frame(width: 68, height: 68)
                    ProgressView()
                }
            case .failed(let message):
                ContentUnavailableView {
                    Label { Text(t("启动失败")) } icon: { Icon("triangle-alert", size: 40) }
                } description: {
                    Text(message)
                } actions: {
                    Button(t("重试")) { core.startCore() }.heroProminent()
                }
            case .ready:
                NavigationSplitView(columnVisibility: $columns) {
                    HomeScreen()
                        .navigationSplitViewColumnWidth(min: 300, ideal: 340, max: 420)
                } detail: {
                    NavigationStack(path: $core.path) {
                        DetailRoot()
                            .navigationDestination(for: Route.self) { route in
                                switch route {
                                case .term(let key): TerminalScreen(key: key)
                                }
                            }
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .sheet(item: $core.prompt) { p in ConnectSheet(prompt: p) }
        .sheet(isPresented: $core.showSettings) { SettingsSheet() }
        .fullScreenCover(isPresented: $core.scanning) { ScanScreen() }
        .alert(core.fileRequest.map { t("{{name}} 想发文件给你", ["name": $0.name]) } ?? "",
               isPresented: Binding(get: { core.fileRequest != nil }, set: { if !$0 { core.respondFiles(false) } }),
               presenting: core.fileRequest) { _ in
            Button(t("拒绝"), role: .cancel) { core.respondFiles(false) }
            Button(t("接收")) { core.respondFiles(true) }
        } message: { r in
            Text(t("{{count}} 个文件：{{items}}", ["count": r.files, "items": r.items.joined(separator: t("、"))]))
        }
        .overlay(alignment: .top) { NoticeView() }
    }
}

/// 右边一栏的第一页：选中的设备或终端；什么都没选时给句提示
private struct DetailRoot: View {
    @Environment(Core.self) private var core

    var body: some View {
        switch core.selection {
        case .device(let id)?: DeviceScreen(endpointId: id)
        case .term(let key)?: TerminalScreen(key: key)
        case nil:
            ContentUnavailableView {
                Label { Text(t("选择一台设备")) } icon: { Icon("square-terminal", size: 40) }
            } description: {
                Text(t("从左边的列表里选一台设备，或者连接一台新设备。"))
            }
        }
    }
}

/// 一句提示：在顶上停一会儿自己消失
private struct NoticeView: View {
    @Environment(Core.self) private var core

    var body: some View {
        if let n = core.notice {
            Text(n.text)
                .font(.subheadline)
                .foregroundStyle(Hero.foreground)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 18).padding(.vertical, 11)
                .heroCapsuleGlass()
                .padding(.horizontal, 24)
                .padding(.top, 6)
                .transition(.move(edge: .top).combined(with: .opacity))
                .id(n.id)
                .onTapGesture { core.notice = nil }
                .accessibilityAddTraits(.isStaticText)
        }
    }
}

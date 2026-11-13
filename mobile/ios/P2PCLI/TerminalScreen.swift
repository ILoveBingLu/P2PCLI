import SwiftUI

/// 一个远程终端：上面是终端，下面是 AI 的确认选项、输入栏、按键条。
/// 只有 iPhone Duo 半折（像笔记本那样立着）时换一种排法：上半屏是终端和输入栏；折痕下面是按键条，
/// 再往下是键盘或 AI 面板（FoldDeck）。合上、全展开、别的设备都是平常的排法
struct TerminalScreen: View {
    @Environment(Core.self) private var core
    let key: Int
    @State private var composing = false
    @State private var draft = ""
    @State private var panel: Panel?
    /// 横着的折痕在页面里的位置；不是半折立着时为 nil
    @State private var fold: CGRect?
    /// 半折时下半屏自己选的模式；nil 跟着 AI 走
    @State private var deck: FoldDeck.Mode?
    @State private var keyboardUp = false
    @FocusState private var draftFocused: Bool

    enum Panel: String, Identifiable {
        case keys, switcher, dir
        var id: String { rawValue }
    }

    var body: some View {
        if let term = core.terms.first(where: { $0.key == key }) {
            content(term)
        } else {
            Color.clear
        }
    }

    private func content(_ term: RemoteTerminal) -> some View {
        let list = core.sessions[term.endpointId]?.list ?? []
        let agent = term.state == .open ? list.first { $0.id == term.sessionId }?.agent : nil
        let title = term.sessionId.flatMap { Format.sessionTitles(list)[$0] } ?? t("新终端")
        let live = term.state == .open
        let path = core.conns[term.endpointId]?.path
        var subtitle = core.deviceName(term.endpointId)
        if !live { subtitle += " · " + t("已断开") } else if let path { subtitle += " · " + Format.path(path) }

        // 终端控件始终是同一个位置上的同一个视图：半折和平常两种排法来回切时不会被拿下来重新放，
        // 否则它会丢掉键盘焦点，键盘收起
        return VStack(spacing: 0) {
            // 上半屏：终端；半折时输入栏也在这里，键盘在下半屏挡不到它
            VStack(spacing: 0) {
                terminalArea(term)
                if fold != nil && composing && live { composer(term, agent: agent) }
            }
            .frame(minHeight: 80, maxHeight: fold.map { max(80, $0.minY) } ?? .infinity)
            .layoutPriority(1)
            if let fold {
                // 半折立着：折痕那一条不放东西，下面是按键条，再往下是键盘或 AI 面板
                Color.clear.frame(height: fold.height)
                VStack(spacing: 0) { controls(term, agent: agent, live: live, folded: true) }
                if live {
                    FoldDeck(term: term, agent: agent, choice: $deck) { compose() }
                } else {
                    Spacer(minLength: 0)
                }
            }
        }
        .background {
            Color.clear
                .onGeometryChange(for: CGRect?.self) { Fold.horizontal($0) } action: { f in
                    // 平常排法时键盘会把页面顶短，这时量不到折痕不算，等键盘收了再量
                    if f != nil || !keyboardUp { fold = f }
                }
        }
        // 半折时整页（连同上面量折痕的那层）不跟着键盘缩：键盘从下半屏升起来盖住 AI 面板那块，
        // 终端和折痕的位置都不动
        .ignoresSafeArea(.keyboard, edges: fold != nil ? .bottom : [])
        .heroBottomBar {
            if fold == nil { VStack(spacing: 0) { controls(term, agent: agent, live: live, folded: false) } }
        }
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillShowNotification)) { _ in keyboardUp = true }
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillHideNotification)) { _ in keyboardUp = false }
        // 立起来时终端空闲就直接弹键盘；AI 跑起来自动换成 AI 面板，收起键盘
        .onChange(of: fold != nil && live) { _, ready in
            deck = nil
            if ready && agent == nil { term.focus() }
        }
        .onChange(of: agent != nil) { _, running in
            deck = nil
            if running && fold != nil && !draftFocused { term.unfocus() }
        }
        // 液态玻璃：栏不要自己的背景，颜色由下面的内容决定。终端的底色铺满整页（包括标题栏、按键条下面），
        // 玻璃按钮直接浮在终端上，不另垫一层
        .background(Hero.terminal.ignoresSafeArea())
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .heroSubtitle(subtitle)
        .heroToolbar {
            ToolbarItem(placement: .primaryAction) {
                Button { panel = .switcher } label: { lucideLabel(t("开着的终端"), "square-terminal") }
            }
            ToolbarItem(placement: .secondaryAction) {
                Button { core.openTerminal(term.endpointId, nil) } label: { lucideLabel(t("新建终端"), "plus") }
                    .disabled(core.conns[term.endpointId] == nil)
            }
            ToolbarItem(placement: .secondaryAction) {
                Button(role: .destructive) { core.closeTerminal(term.key) } label: { lucideLabel(t("关闭这个终端"), "x") }
            }
        }
        .sheet(item: $panel) { p in
            switch p {
            case .keys: KeysPanel(term: term).presentationDetents([.height(320)])
            case .switcher: SwitchPanel(current: term).presentationDetents([.medium, .large])
            case .dir: DirPickerSheet(term: term, session: sessions(term).first { $0.id == term.sessionId })
            }
        }
        // AI 状态隔几秒刷新一次
        .task(id: term.endpointId) {
            while !Task.isCancelled {
                await core.loadSessions(term.endpointId)
                try? await Task.sleep(for: .seconds(3))
            }
        }
    }

    private func terminalArea(_ term: RemoteTerminal) -> some View {
        ZStack {
            TerminalHost(term: term)
            switch term.state {
            case .opening:
                HStack(spacing: 10) {
                    ProgressView()
                    Text(t("正在打开终端…")).foregroundStyle(Hero.muted)
                }
            case .closed:
                ZStack {
                    Hero.terminal.opacity(0.82)
                    VStack(spacing: 10) {
                        Text(term.exited ? t("这个终端已经结束") : t("连接已断开")).font(.title3.weight(.semibold))
                        if let note = term.note { Text(note).font(.subheadline) }
                        Text(t("会话还在对方电脑上运行的话，重新连上就能接着用。")).font(.subheadline).foregroundStyle(Hero.muted)
                        Button { core.reopen(term) } label: {
                            Text(term.exited ? t("新建终端") : t("重新连接")).frame(maxWidth: .infinity)
                        }
                        .heroProminent()
                        .controlSize(.large)
                        Button(t("关闭")) { core.closeTerminal(term.key) }
                            .buttonStyle(.borderless)
                    }
                    .multilineTextAlignment(.center)
                    .padding(20)
                    .frame(maxWidth: 340)
                    .heroGlass(cornerRadius: 22)
                    .padding(20)
                }
            case .open:
                EmptyView()
            }
        }
        .background(Hero.terminal)
    }

    @ViewBuilder
    private func controls(_ term: RemoteTerminal, agent: AgentView?, live: Bool, folded: Bool) -> some View {
        // AI 等你确认：选项直接点，不用到终端里按方向键。半折时放在下半屏的 AI 面板里
        if !folded, let agent, agent.state == .waiting, let approval = agent.approval {
            ApprovalPanel(term: term, agent: agent, approval: approval)
        }
        if !folded && composing && live { composer(term, agent: agent) }
        if live {
            AgentBar(term: term, session: sessions(term).first { $0.id == term.sessionId }) { panel = .dir }
            KeyBar(term: term, composing: composing, keyboardUp: keyboardUp,
                   onCompose: { composing.toggle() },
                   onHideKeyboard: hideKeyboard,
                   onMore: { panel = .keys })
        }
    }

    /// 输入栏：打一段字（可以用输入法）一次发出去，发完自动回车
    private func composer(_ term: RemoteTerminal, agent: AgentView?) -> some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField(agent.map { t("发给 {{name}}", ["name": $0.name]) } ?? t("输入命令，发送后自动回车"),
                      text: $draft, axis: .vertical)
                .lineLimit(1...5)
                .focused($draftFocused)
                .submitLabel(.send)
                .onSubmit { send(term) }
                .padding(.horizontal, 14).padding(.vertical, 10)
                .heroGlass(cornerRadius: 21)
            Button { send(term) } label: {
                Icon("send-horizontal").foregroundStyle(.white).frame(width: 44, height: 44)
            }
            .background(Circle().fill(draft.isEmpty ? Hero.tile : Hero.accent))
            .disabled(draft.isEmpty)
            .accessibilityLabel(t("发送"))
        }
        .padding(.horizontal, 8).padding(.vertical, 8)
    }

    /// 收起键盘：不管现在是终端还是输入栏在打字
    private func hideKeyboard() {
        draftFocused = false
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
    }

    private func sessions(_ term: RemoteTerminal) -> [SessionView] {
        core.sessions[term.endpointId]?.list ?? []
    }

    /// 打开输入栏并把光标放进去（半折时 AI 面板上的“写指令”）
    private func compose() {
        composing = true
        Task {
            // 输入栏刚加进来、键盘焦点还在别处时，第一次给可能不生效，没拿到就再给
            for ms in [100, 300, 700] {
                try? await Task.sleep(for: .milliseconds(ms))
                if draftFocused { break }
                draftFocused = true
            }
        }
    }

    private func send(_ term: RemoteTerminal) {
        guard !draft.isEmpty else { return }
        term.paste(draft)
        draft = ""
        // 粘贴和回车分开发：有的程序（Claude Code 这些）一起收到会把回车当成粘贴内容
        Task {
            try? await Task.sleep(for: .milliseconds(80))
            term.send("\r")
        }
    }
}

/// 终端下面一排最常用的键，一屏放得下，不用左右滑；不常用的在“⋯”里。
/// 竖着的折痕穿过这一排时，键分成两组放在折痕两边
private struct KeyBar: View {
    @Environment(Core.self) private var core
    let term: RemoteTerminal
    let composing: Bool
    let keyboardUp: Bool
    let onCompose: () -> Void
    let onHideKeyboard: () -> Void
    let onMore: () -> Void

    var body: some View {
        GeometryReader { geo in
            let keys = items
            if let gap = Fold.vertical(geo) {
                let left = keys.filter { $0.order < 5 }
                let right = keys.filter { $0.order >= 5 }
                HStack(spacing: 0) {
                    row(left).frame(width: max(0, gap.lowerBound - 6))
                    Spacer(minLength: 0).frame(width: gap.upperBound - gap.lowerBound + 12)
                    row(right)
                }
            } else {
                row(keys)
            }
        }
        .frame(height: 40)
        .padding(.horizontal, 6)
        .padding(.vertical, 8)
    }

    private struct KeyItem: Identifiable {
        let order: Int
        let label: String
        var icon: String?
        var active = false
        let action: () -> Void
        var id: Int { order }
    }

    private var items: [KeyItem] {
        return [
            KeyItem(order: 0, label: t("输入栏"), icon: "keyboard", active: composing, action: onCompose),
            KeyItem(order: 1, label: "Esc") { term.send("\u{1b}") },
            KeyItem(order: 2, label: "Tab") { term.send("\t") },
            KeyItem(order: 3, label: "Ctrl", active: term.ctrl) { term.ctrl.toggle() },
            KeyItem(order: 4, label: "↑", icon: "arrow-up") { term.arrow("A") },
            KeyItem(order: 5, label: "↓", icon: "arrow-down") { term.arrow("B") },
            KeyItem(order: 6, label: "←", icon: "arrow-left") { term.arrow("D") },
            KeyItem(order: 7, label: "→", icon: "arrow-right") { term.arrow("C") },
            KeyItem(order: 8, label: t("粘贴"), icon: "clipboard-paste") { term.pasteClipboard() },
            KeyItem(order: 9, label: t("更多按键"), icon: "ellipsis", action: onMore),
        ]
        // 键盘弹着时最后多一个“收起键盘”：苹果的键盘很多时候没有收起的键
        + (keyboardUp ? [KeyItem(order: 10, label: t("收起键盘"), icon: "keyboard-off", action: onHideKeyboard)] : [])
    }

    private func row(_ keys: [KeyItem]) -> some View {
        GlassGroup(spacing: 4) {
            HStack(spacing: 4) {
                ForEach(keys) { k in
                    KeyButton(label: k.label, icon: k.icon, active: k.active, action: k.action)
                }
            }
        }
    }
}

/// 一个键：有图标画图标（读屏念 label），没有就写 label
struct KeyButton: View {
    let label: String
    var icon: String?
    var active = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Group {
                if let icon {
                    Icon(icon, size: 18)
                } else {
                    Text(label).font(.system(size: 13, weight: .medium, design: .monospaced)).lineLimit(1).minimumScaleFactor(0.7)
                }
            }
            .foregroundStyle(active ? Hero.accent : Hero.foreground)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .heroGlass(cornerRadius: 10, tint: active ? Hero.accent.opacity(0.35) : nil, interactive: true)
        .accessibilityLabel(label)
        .accessibilityAddTraits(active ? .isSelected : [])
    }
}

/// AI 弹出的确认框：选项直接点
struct ApprovalPanel: View {
    @Environment(Core.self) private var core
    let term: RemoteTerminal
    let agent: AgentView
    let approval: ApprovalView
    /// 放在按键条上面时四周留边；放进半折的 AI 面板时面板自己有边
    var inset = true

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
                Text(t("{{name}} · 等待确认", ["name": agent.name])).font(.caption).foregroundStyle(Hero.muted)
                    .padding(.horizontal, 4)
                Text(approval.context).font(.footnote).padding(.horizontal, 8).padding(.vertical, 4)
                ForEach(approval.options, id: \.id) { opt in
                    Button { core.chooseOption(term, approvalId: approval.id, optionId: opt.id) } label: {
                        HStack(spacing: 9) {
                            Text("\(opt.id + 1)").font(.caption)
                                .frame(width: 22, height: 22)
                                .background(RoundedRectangle(cornerRadius: 7).fill(Hero.surfaceSecondary))
                            Text(opt.label).font(.subheadline).frame(maxWidth: .infinity, alignment: .leading)
                            Icon("square-arrow-out-up-right", size: 15).foregroundStyle(Hero.muted)
                        }
                        .padding(8)
                        .frame(minHeight: 46)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
        .padding(10)
        .heroGlass(cornerRadius: 18)
        .padding(.horizontal, inset ? 8 : 0).padding(.top, inset ? 8 : 0)
    }
}

/// “⋯”里不常用的键：按一下就发，面板跟着关上，回到终端
private struct KeysPanel: View {
    @Environment(Core.self) private var core
    @Environment(\.dismiss) private var dismiss
    let term: RemoteTerminal

    private let keys: [(String, String)] = [
        ("⇧Tab", "\u{1b}[Z"), ("^C", "\u{03}"), ("^D", "\u{04}"), ("^Z", "\u{1a}"), ("^L", "\u{0c}"),
        ("^R", "\u{12}"), ("|", "|"), ("~", "~"), ("/", "/"), ("-", "-"),
    ]

    var body: some View {
        NavigationStack {
            VStack(spacing: 8) {
                let columns = Array(repeating: GridItem(.flexible(), spacing: 8), count: 5)
                GlassGroup(spacing: 8) {
                    LazyVGrid(columns: columns, spacing: 8) {
                        ForEach(keys, id: \.0) { k in
                            KeyButton(label: k.0) { dismiss(); term.send(k.1) }.frame(height: 46)
                        }
                    }
                    HStack(spacing: 8) {
                        KeyButton(label: "Alt", active: term.alt) { term.alt.toggle(); dismiss() }
                        // 终端里的撤销：bash、zsh、Claude Code 的输入框都是 Ctrl+_
                        KeyButton(label: t("撤销")) { dismiss(); term.send("\u{1f}") }
                        KeyButton(label: t("复制")) {
                            dismiss()
                            core.notify(term.copySelection() ? t("已复制") : t("要复制先长按终端里的文字，再拖动选择范围。"))
                        }
                    }
                    .frame(height: 46)
                }
                if !term.hasSelection {
                    Text(t("要复制先长按终端里的文字，再拖动选择范围。")).font(.footnote).foregroundStyle(Hero.muted)
                        .frame(maxWidth: .infinity, alignment: .leading).padding(.top, 4)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 16)
            .navigationTitle(t("更多按键"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button { dismiss() } label: { lucideLabel(t("关闭"), "x") }
                }
            }
        }
        .heroSheetVerticalBars()
    }
}

/// 开着的终端：点一个切过去
private struct SwitchPanel: View {
    @Environment(Core.self) private var core
    @Environment(\.dismiss) private var dismiss
    let current: RemoteTerminal

    var body: some View {
        NavigationStack {
            List {
                ForEach(core.terms) { other in
                    let l = core.sessions[other.endpointId]?.list ?? []
                    let a = other.state == .open ? l.first { $0.id == other.sessionId }?.agent : nil
                    Button {
                        dismiss()
                        core.openTerminal(other.endpointId, other.sessionId)
                    } label: {
                        HStack(spacing: 12) {
                            Tile(dimmed: other.state == .closed) {
                                if let a { AgentIcon(kind: a.kind) } else { Icon("square-terminal").foregroundStyle(Hero.foreground) }
                            }
                            VStack(alignment: .leading, spacing: 3) {
                                Text(other.sessionId.flatMap { Format.sessionTitles(l)[$0] } ?? t("新终端")).font(.body.weight(.medium))
                                Text(core.deviceName(other.endpointId) + (other.state == .closed ? " · " + t("已断开") : ""))
                                    .font(.footnote).foregroundStyle(Hero.muted)
                            }
                            Spacer(minLength: 0)
                            if let a { AgentStateLabel(agent: a) }
                            if other.key == current.key { Icon("check", size: 16).foregroundStyle(Hero.accent) }
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .listRowBackground(Hero.surface)
                    .swipeActions {
                        Button(role: .destructive) {
                            if core.terms.count == 1 { dismiss() }
                            core.closeTerminal(other.key)
                        } label: { lucideLabel(t("关闭"), "x") }
                    }
                }
                Section {
                    Button {
                        dismiss()
                        core.openTerminal(current.endpointId, nil)
                    } label: {
                        Label { Text(t("在 {{name}} 上新建终端", ["name": core.deviceName(current.endpointId)])) } icon: { Icon("plus", size: 18) }
                    }
                    .disabled(core.conns[current.endpointId] == nil)
                    .listRowBackground(Hero.surface)
                }
            }
            .navigationTitle(t("开着的终端"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button { dismiss() } label: { lucideLabel(t("关闭"), "x") }
                }
            }
        }
        .heroSheetVerticalBars()
    }
}

extension View {
    /// 导航栏标题下面的一行小字（iOS 26 以上）
    @ViewBuilder
    func heroSubtitle(_ text: String) -> some View {
        if #available(iOS 26.0, *) {
            self.navigationSubtitle(text)
        } else {
            self
        }
    }

    /// 弹出的小面板：iPhone Duo 合上时系统默认把面板的工具栏竖着放在侧边；
    /// 这些面板矮，工具栏只有一个关闭，横着放更顺手
    @ViewBuilder
    func heroSheetVerticalBars() -> some View {
        if #available(iOS 27.1, *) {
            self.toolbarVerticalBehavior(.disabled)
        } else {
            self
        }
    }
}

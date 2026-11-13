import SwiftUI

/// 首页（宽屏时是左边一栏）：开着的终端、连过的设备、局域网里的设备
struct HomeScreen: View {
    @Environment(Core.self) private var core

    var body: some View {
        let overview = core.overview
        let devices = overview?.devices ?? []
        let nearby = overview?.nearby ?? []
        let empty = overview != nil && devices.isEmpty && nearby.isEmpty
        let selection = Binding<Selection?>(get: { core.selection }, set: { core.select($0) })

        List(selection: selection) {
            TransfersSection()

            // 扫码连的设备列表里还没有：顶上放一张“正在连接”的卡片
            if let pending = core.connecting, !devices.contains(where: { $0.endpointId == pending }),
               !nearby.contains(where: { $0.endpointId == pending }) {
                Section {
                    HStack(spacing: 12) {
                        ProgressView().frame(width: 36, height: 36)
                            .background(Circle().fill(Hero.accent.opacity(0.16)))
                        VStack(alignment: .leading, spacing: 3) {
                            Text(t("正在连接 {{name}}", ["name": core.connectingName ?? t("电脑")])).font(.body.weight(.medium))
                            Text(t("等对方在电脑上点同意")).font(.footnote).foregroundStyle(Hero.muted)
                        }
                    }
                    .listRowBackground(Hero.surface)
                }
            }

            if let server = overview?.server, Self.serverDown(server) {
                Section {
                    Label {
                        Text(server == ServerState.none
                             ? t("还没配置牵线服务器，只能连同一个局域网里的设备")
                             : t("牵线服务器没连上，只能连同一个局域网里的设备"))
                            .font(.footnote)
                    } icon: { Icon("triangle-alert", size: 16) }
                    .foregroundStyle(Hero.warning)
                    .listRowBackground(Hero.warning.opacity(0.12))
                }
            }

            if empty {
                Section {
                    EmptyHome()
                        .listRowBackground(Color.clear)
                        .listRowInsets(EdgeInsets())
                }
            } else {
                if !core.terms.isEmpty {
                    Section(t("正在使用")) {
                        ForEach(core.terms) { term in
                            TermRow(term: term).tag(Selection.term(term.key))
                        }
                    }
                }
                if !devices.isEmpty {
                    Section(t("我的设备")) {
                        ForEach(devices, id: \.endpointId) { d in
                            DeviceRow(device: d, live: core.sessions[d.endpointId]?.list).tag(Selection.device(d.endpointId))
                        }
                    }
                }
                Section {
                    if nearby.isEmpty {
                        Text(t("同一个局域网里没找到别的设备")).font(.subheadline).foregroundStyle(Hero.muted)
                            .listRowBackground(Hero.surface)
                    } else {
                        ForEach(nearby, id: \.endpointId) { d in
                            DeviceRow(device: d, live: nil).tag(Selection.device(d.endpointId))
                        }
                    }
                } header: {
                    HStack {
                        Text(t("附近设备"))
                        Spacer()
                        Button { core.scanNearby() } label: { Icon("refresh-cw", size: 15) }
                            .accessibilityLabel(t("刷新附近设备"))
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .refreshable {
            core.scanNearby()
            await core.refreshOverview()
        }
        // 用系统原生的标题栏：内容滚上去时系统自己画渐变模糊（滚动边缘效果）
        .navigationTitle("P2PCLI")
        // logo 和名字就是标题：靠左和按钮放在同一行（系统的 inlineLarge），不另占一行
        .toolbarTitleDisplayMode(.inlineLarge)
        .heroBrandTitle()
        .heroToolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                Button { core.openScan() } label: { lucideLabel(t("扫码连接"), "scan-line") }
                Button { core.openSettings() } label: { lucideLabel(t("设置"), "settings") }
                    .accessibilityIdentifier("open-settings")
            }
            if !empty {
                ToolbarItem(placement: .bottomBar) {
                    Button { core.prompt = ConnectPrompt() } label: { lucideLabel(t("连接新设备"), "plus") }
                        .disabled(core.connecting != nil)
                        .accessibilityIdentifier("connect-new")
                }
            }
        }
    }
}

extension View {
    /// 大标题前面放软件的 logo（和 Mac 版同一个液态玻璃图标）。用系统给大标题留的位置（iOS 26 起），
    /// 还是系统的标题栏；系统不显示大标题的时候（比如 iPhone Duo 合上）就只显示文字标题
    @ViewBuilder
    func heroBrandTitle() -> some View {
        if #available(iOS 26.0, *) {
            self.toolbar {
                ToolbarItem(placement: .largeTitle) {
                    // 和按钮同一行：位置不够（iPad、iPhone Duo 展开时的侧栏）时字自动缩小，不压到按钮上
                    HStack(spacing: 9) {
                        Image("logo").resizable().frame(width: 32, height: 32).accessibilityHidden(true)
                        Text("P2PCLI").font(.title.bold()).lineLimit(1).minimumScaleFactor(0.6)
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityAddTraits(.isHeader)
                }
            }
        } else {
            self
        }
    }
}

extension HomeScreen {
    static func serverDown(_ s: ServerState) -> Bool {
        switch s {
        case .none, .offline: true
        case .connecting, .online: false
        }
    }
}

/// 还没有任何设备：教怎么连
private struct EmptyHome: View {
    @Environment(Core.self) private var core

    var body: some View {
        VStack(spacing: 14) {
            Text(t("连接你的电脑")).font(.title2.weight(.bold))
            Text(t("在电脑上打开 P2PCLI，点标题栏的手机图标，用这里扫它显示的二维码。"))
                .font(.subheadline).foregroundStyle(Hero.muted).multilineTextAlignment(.center)
            VStack(spacing: 10) {
                Button { core.openScan() } label: {
                    Label { Text(t("扫码连接")) } icon: { Icon("scan-line", size: 18) }.frame(maxWidth: .infinity)
                }
                .heroProminent()
                Button { core.prompt = ConnectPrompt() } label: {
                    Label { Text(t("输入设备码")) } icon: { Icon("key-round", size: 18) }.frame(maxWidth: .infinity)
                }
                .heroSecondary()
                .accessibilityIdentifier("connect-new")
                Button { core.scanNearby() } label: {
                    Label { Text(t("找找附近的设备")) } icon: { Icon("refresh-cw", size: 18) }
                }
                .buttonStyle(.borderless)
            }
            .controlSize(.large)
            .padding(.top, 8)
        }
        .frame(maxWidth: 420)
        .frame(maxWidth: .infinity)
        .padding(.vertical, 28)
    }
}

/// 开着的一个终端
private struct TermRow: View {
    @Environment(Core.self) private var core
    let term: RemoteTerminal

    var body: some View {
        let list = core.sessions[term.endpointId]?.list ?? []
        let agent = term.state == .open ? list.first { $0.id == term.sessionId }?.agent : nil
        HStack(spacing: 12) {
            Tile(dimmed: term.state == .closed) {
                if let agent { AgentIcon(kind: agent.kind) } else { Icon("square-terminal").foregroundStyle(Hero.foreground) }
            }
            VStack(alignment: .leading, spacing: 3) {
                Text(term.sessionId.flatMap { Format.sessionTitles(list)[$0] } ?? t("新终端"))
                    .font(.body.weight(.medium)).lineLimit(1)
                HStack(spacing: 8) {
                    Text(core.deviceName(term.endpointId) + (term.state == .closed ? " · " + t("已断开") : ""))
                        .font(.footnote).foregroundStyle(Hero.muted).lineLimit(1)
                    if let agent { AgentStateLabel(agent: agent) }
                }
            }
        }
        .padding(.vertical, 2)
        .listRowBackground(Hero.surface)
    }
}

/// 设备列表里的一行
private struct DeviceRow: View {
    @Environment(Core.self) private var core
    let device: DeviceView
    let live: [SessionView]?

    var body: some View {
        let connected = core.conns[device.endpointId] != nil
        let busy = core.connecting == device.endpointId
        let (status, color) = Format.presence(device, connected: connected)
        let sessions = live ?? device.sessions
        let waiting = sessions.filter { $0.agent?.state == .waiting }.count
        HStack(spacing: 12) {
            PlatformTile(platform: device.platform, connected: connected)
            VStack(alignment: .leading, spacing: 3) {
                Text(device.remark ?? device.name).font(.body.weight(.medium)).lineLimit(1)
                HStack(spacing: 8) {
                    if busy {
                        Text(t("等待对方同意…")).foregroundStyle(Hero.accent)
                    } else {
                        Text(status).foregroundStyle(color ?? Hero.muted)
                        if device.lanOnline && !connected && device.known { Chip(text: t("局域网")) }
                        if !sessions.isEmpty {
                            Text(t("{{count}} 个会话", ["count": sessions.count])).foregroundStyle(Hero.muted)
                        }
                    }
                }
                .font(.footnote)
                .lineLimit(1)
                if waiting > 0 && !busy {
                    Text(t("{{count}} 个 AI 在等你确认", ["count": waiting]))
                        .font(.footnote.weight(.medium)).foregroundStyle(Hero.warning)
                }
            }
            Spacer(minLength: 0)
            if busy { ProgressView() }
        }
        .padding(.vertical, 2)
        .listRowBackground(Hero.surface)
        .disabled(core.connecting != nil && !busy)
    }
}

/// 正在收发的文件和进度
struct TransfersSection: View {
    @Environment(Core.self) private var core

    var body: some View {
        let active = core.transfers.filter { $0.state != .done && $0.state != .failed }
        if !active.isEmpty {
            Section {
                ForEach(active, id: \.id) { x in
                    let progress = x.total > 0 ? min(1, Double(x.done) / Double(x.total)) : 0
                    HStack(spacing: 12) {
                        Icon(x.outgoing ? "arrow-up" : "arrow-down", size: 18)
                            .foregroundStyle(Hero.accent)
                            .frame(width: 36, height: 36)
                            .background(Circle().fill(Hero.accent.opacity(0.16)))
                        VStack(alignment: .leading, spacing: 4) {
                            Text(x.label).font(.body.weight(.medium)).lineLimit(1)
                            Text(Self.status(x, progress)).font(.footnote).foregroundStyle(Hero.muted)
                            ProgressView(value: progress).tint(Hero.accent)
                        }
                    }
                    .listRowBackground(Hero.surface)
                }
            }
        }
    }

    static func status(_ x: TransferView, _ progress: Double) -> String {
        switch x.state {
        case .waiting: return t("等 {{name}} 同意", ["name": x.peerName])
        case .connecting: return t("正在连接 {{name}}", ["name": x.peerName])
        default:
            let who = x.outgoing ? t("发给 {{name}}", ["name": x.peerName]) : t("来自 {{name}}", ["name": x.peerName])
            return "\(who) · \(Int(progress * 100))%"
        }
    }
}

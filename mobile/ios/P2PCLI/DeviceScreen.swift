import SwiftUI
import UniformTypeIdentifiers

/// 一台设备：它上面的后台会话，新建终端、发文件、断开
struct DeviceScreen: View {
    @Environment(Core.self) private var core
    let endpointId: String
    @State private var picking = false
    @State private var ending: SessionView?

    var body: some View {
        let conn = core.conns[endpointId]
        let device = core.device(endpointId)
        let name = device.map { $0.remark ?? $0.name } ?? conn?.name ?? t("设备")
        let platform = device?.platform ?? conn?.platform ?? ""
        let state = core.sessions[endpointId]
        let list = (state?.list ?? []).sorted { $0.createdAt > $1.createdAt }
        let titles = Format.sessionTitles(state?.list ?? [])
        let openIds = Set(core.terms.filter { $0.endpointId == endpointId && $0.state != .closed }.compactMap(\.sessionId))

        List {
            TransfersSection()
            Section {
                VStack(spacing: 10) {
                    PlatformTile(platform: platform, connected: conn != nil, size: 76)
                    if let conn {
                        Text(t("已连接") + " · " + Format.path(conn.path)).font(.footnote)
                            .foregroundStyle(conn.path.kind == .relay ? Hero.warning : Hero.success)
                    } else {
                        Text(t("没有连着")).font(.footnote).foregroundStyle(Hero.muted)
                    }
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 6)
                .listRowBackground(Color.clear)
            }

            if conn == nil {
                Section {
                    ContentUnavailableView {
                        Text(t("没有连着这台设备"))
                    } description: {
                        Text(t("连接断开了，后台会话还在对方电脑上运行，重新连上就能接着用。"))
                    } actions: {
                        let busy = core.connecting == endpointId
                        Button {
                            if let device { core.openDevice(device) }
                        } label: {
                            Label { Text(busy ? t("等待对方同意…") : t("重新连接")) } icon: { Icon("plug", size: 18) }
                        }
                        .heroProminent()
                        .disabled(device == nil || core.connecting != nil)
                    }
                    .listRowBackground(Color.clear)
                }
            } else {
                Section {
                    if let err = state?.error {
                        Text(t("读不到会话列表：{{error}}", ["error": tr(err)])).font(.footnote).foregroundStyle(Hero.danger)
                            .listRowBackground(Hero.surface)
                    }
                    if state?.list == nil && state?.error == nil {
                        HStack(spacing: 10) {
                            ProgressView()
                            Text(t("正在读取会话…")).foregroundStyle(Hero.muted)
                        }
                        .listRowBackground(Hero.surface)
                    } else if list.isEmpty {
                        Text(t("这台设备上还没有会话，新建一个终端开始用。")).font(.subheadline).foregroundStyle(Hero.muted)
                            .listRowBackground(Hero.surface)
                    }
                    ForEach(list, id: \.id) { s in
                        Button { core.openTerminal(endpointId, s.id) } label: {
                            SessionRow(session: s, title: titles[s.id] ?? t("会话"), open: openIds.contains(s.id))
                        }
                        .buttonStyle(.plain)
                        .listRowBackground(Hero.surface)
                        .swipeActions(edge: .trailing) {
                            Button(role: .destructive) { ending = s } label: { lucideLabel(t("结束会话"), "trash") }
                        }
                        .contextMenu {
                            Button { core.openTerminal(endpointId, s.id) } label: { lucideLabel(t("打开"), "square-arrow-out-up-right") }
                            Button(role: .destructive) { ending = s } label: { lucideLabel(t("结束会话"), "trash") }
                        }
                    }
                } header: {
                    Text(t("会话"))
                }

                Section {
                    Button(role: .destructive) { core.disconnect(endpointId) } label: {
                        Label { Text(t("断开连接")) } icon: { Icon("unplug", size: 18) }
                            .foregroundStyle(Hero.danger)
                            .frame(maxWidth: .infinity)
                    }
                    .listRowBackground(Hero.surface)
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle(name)
        .navigationBarTitleDisplayMode(.inline)
        .refreshable { await core.loadSessions(endpointId) }
        .toolbar {
            if conn != nil {
                // 竖排工具栏（iPhone Duo）上方是返回，接着是最常用的“新建终端”
                ToolbarItem(placement: .primaryAction) {
                    Button { core.openTerminal(endpointId, nil) } label: { lucideLabel(t("新建终端"), "plus") }
                        .accessibilityIdentifier("new-terminal")
                }
                ToolbarItem(placement: .secondaryAction) {
                    Button { picking = true } label: { lucideLabel(t("发送文件"), "file-up") }
                }
                ToolbarItem(placement: .secondaryAction) {
                    Button { Task { await core.loadSessions(endpointId) } } label: { lucideLabel(t("刷新会话"), "refresh-cw") }
                }
            }
        }
        .fileImporter(isPresented: $picking, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            if case .success(let urls) = result { core.sendFiles(endpointId, urls: urls) }
        }
        .confirmationDialog(ending.map { Format.sessionTitles(state?.list ?? [])[$0.id] ?? t("会话") } ?? "",
                            isPresented: Binding(get: { ending != nil }, set: { if !$0 { ending = nil } }),
                            titleVisibility: .visible, presenting: ending) { s in
            Button(t("结束会话"), role: .destructive) { core.endSession(endpointId, s.id) }
            Button(t("取消"), role: .cancel) {}
        } message: { _ in
            Text(t("结束后里面运行的程序会被关掉，没保存的内容会丢失。"))
        }
        // 会话列表和 AI 状态隔几秒刷新一次
        .task(id: conn?.connId) {
            while conn != nil && !Task.isCancelled {
                await core.loadSessions(endpointId)
                try? await Task.sleep(for: .seconds(3))
            }
        }
    }
}

private struct SessionRow: View {
    let session: SessionView
    let title: String
    let open: Bool

    var body: some View {
        HStack(spacing: 12) {
            Tile {
                if let a = session.agent { AgentIcon(kind: a.kind) } else { Icon("square-terminal").foregroundStyle(Hero.foreground) }
            }
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(title).font(.body.weight(.medium)).lineLimit(1)
                    if open { Chip(text: t("已打开"), accent: true) }
                }
                HStack(spacing: 8) {
                    if let a = session.agent {
                        AgentStateLabel(agent: a, size: 13)
                    } else if let c = session.command {
                        Text(c).font(.system(size: 12, design: .monospaced)).foregroundStyle(Hero.muted)
                    }
                    Text(Format.shortPath(session.currentPath)).font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(Hero.muted).lineLimit(1).truncationMode(.head)
                }
                Text(t("{{time}}创建", ["time": Format.relativeTime(session.createdAt)]))
                    .font(.caption).foregroundStyle(Hero.muted.opacity(0.8))
            }
            Spacer(minLength: 0)
            Icon("chevron-right", size: 16).foregroundStyle(Hero.muted.opacity(0.7))
        }
        .contentShape(Rectangle())
        .padding(.vertical, 2)
    }
}

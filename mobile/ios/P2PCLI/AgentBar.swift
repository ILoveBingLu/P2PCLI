import SwiftUI

/// 终端下面一行：最左边是当前工作目录（点开选目录），后面是对方电脑上装了的命令行 AI，点一下在这个会话里启动。
/// 和桌面一样，终端停在空闲的命令提示符、还没在跑 AI 时才能启动；跑着程序时目录只能看不能切。
/// 这个会话里在跑 AI 时整行不显示
struct AgentBar: View {
    @Environment(Core.self) private var core
    let term: RemoteTerminal
    let session: SessionView?
    let onPickDir: () -> Void
    @State private var launching: String?

    var body: some View {
        let state = core.agents[term.connId]
        let idle = session?.shellReady == true && session?.agent == nil
        let path = Format.shortPath(session?.currentPath)
        if session?.agent == nil { row(state: state, idle: idle, path: path) }
    }

    private func row(state: AgentsState?, idle: Bool, path: String) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            GlassGroup(spacing: 6) {
                HStack(spacing: 6) {
                    Button(action: onPickDir) {
                        HStack(spacing: 6) {
                            Icon("folder-open", size: 15).foregroundStyle(Hero.muted)
                            Text(path.isEmpty ? "~" : path)
                                .font(.system(size: 12.5, design: .monospaced))
                                .lineLimit(1).truncationMode(.head)
                                .frame(maxWidth: 180, alignment: .leading)
                        }
                        .padding(.horizontal, 12).frame(height: 34)
                    }
                    .buttonStyle(.plain)
                    .heroCapsuleGlass(interactive: true)
                    .accessibilityLabel(t("当前目录：{{path}}，点击选择目录", ["path": path.isEmpty ? "~" : path]))

                    if idle { agents(state) }
                }
                .padding(.horizontal, 8)
            }
        }
        .padding(.top, 8)
        .task(id: term.connId) { core.scanAgents(term.connId) }
    }

    @ViewBuilder
    private func agents(_ state: AgentsState?) -> some View {
        if state == nil || (state?.scanning == true && state?.list == nil) {
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text(t("正在检测 AI")).font(.system(size: 12.5)).foregroundStyle(Hero.muted)
            }
            .padding(.horizontal, 6)
        } else if let list = state?.list {
            if list.isEmpty {
                pill(t("未检测到 AI，点击重新检测")) {
                    Icon("refresh-cw", size: 15).foregroundStyle(Hero.muted)
                    Text(t("未检测到 AI，点击重新检测")).foregroundStyle(Hero.muted)
                } action: { core.scanAgents(term.connId, refresh: true) }
            } else {
                ForEach(list, id: \.kind) { a in
                    agentButton(a)
                }
                Button { core.scanAgents(term.connId, refresh: true) } label: {
                    Group {
                        if state?.scanning == true { ProgressView().controlSize(.small) } else { Icon("refresh-cw", size: 15) }
                    }
                    .foregroundStyle(Hero.muted)
                    .frame(width: 34, height: 34)
                }
                .buttonStyle(.plain)
                .disabled(launching != nil || state?.scanning == true)
                .accessibilityLabel(t("刷新已安装的 AI"))
            }
        } else {
            pill(t("AI 检测失败，重试")) {
                Icon("refresh-cw", size: 15)
                Text(t("检测失败 · 重试"))
            } action: { core.scanAgents(term.connId, refresh: true) }
        }
    }

    private func agentButton(_ a: InstalledAgentView) -> some View {
        Button { launch(a, bypass: false) } label: {
            HStack(spacing: 6) {
                if launching == a.kind { ProgressView().controlSize(.small) } else { AgentIcon(kind: a.kind, size: 16) }
                Text(a.name).font(.system(size: 12.5)).lineLimit(1)
            }
            .padding(.horizontal, 12).frame(height: 34)
        }
        .buttonStyle(.plain)
        .heroCapsuleGlass(interactive: true)
        .disabled(launching != nil)
        .accessibilityLabel(t("启动 {{name}}", ["name": a.name]))
        // Claude Code 长按可以选“允许之后切换全部放行”，和桌面的右键菜单一样
        .contextMenu {
            if a.kind == "claude" {
                Button { launch(a, bypass: false) } label: { lucideLabel(t("正常启动"), "play") }
                Button { launch(a, bypass: true) } label: { lucideLabel(t("启动并允许之后切换全部放行"), "play") }
            }
        }
    }

    private func pill<Content: View>(_ label: String, @ViewBuilder content: () -> Content, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 6) { content() }
                .font(.system(size: 12.5))
                .padding(.horizontal, 12).frame(height: 34)
        }
        .buttonStyle(.plain)
        .heroCapsuleGlass(interactive: true)
        .accessibilityLabel(label)
    }

    private func launch(_ a: InstalledAgentView, bypass: Bool) {
        guard launching == nil else { return }
        launching = a.kind
        Task {
            await core.launchAgent(term, kind: a.kind, allowBypass: bypass)
            launching = nil
        }
    }
}

/// 选工作目录：对方电脑上的目录列表，点子目录进去，点“切换到这里”让终端 cd 过去。
/// 终端跑着程序（包括 AI）时只能看，免得 cd 被当成输入发给程序
struct DirPickerSheet: View {
    @Environment(Core.self) private var core
    @Environment(\.dismiss) private var dismiss
    let term: RemoteTerminal
    let session: SessionView?
    @State private var listing: DirListingView?
    @State private var loading = true
    @State private var error: String?
    @State private var filter = ""
    @State private var changing = false

    var body: some View {
        let canChange = session?.shellReady == true && session?.agent == nil
        let dirs = (listing?.dirs ?? []).filter { filter.isEmpty || $0.localizedCaseInsensitiveContains(filter) }
        NavigationStack {
            List {
                Section {
                    Text(listing?.path ?? session?.currentPath ?? "").font(.system(size: 12.5, design: .monospaced))
                        .foregroundStyle(Hero.muted)
                    if let error {
                        Label { Text(tr(error)) } icon: { Icon("triangle-alert", size: 15) }.foregroundStyle(Hero.danger)
                    }
                    if !canChange {
                        Text(session?.agent.map { t("{{name}} 正在运行，退出后才能切换目录", ["name": $0.name]) }
                             ?? t("终端不在空闲提示符，完成当前输入或退出程序后才能切换"))
                            .font(.footnote).foregroundStyle(Hero.warning)
                    }
                }
                Section {
                    if loading && listing == nil {
                        ProgressView()
                    }
                    ForEach(dirs, id: \.self) { name in
                        Button { if let l = listing { browse(join(l.path, name)) } } label: {
                            Label { Text(name).lineLimit(1) } icon: { Icon("folder", size: 18).foregroundStyle(Hero.muted) }
                        }
                        .disabled(loading)
                    }
                } footer: {
                    if listing?.truncated == true {
                        Text(t("子目录太多，只列出了前 {{shown}} 个，可以输入名字筛选", ["shown": listing?.dirs.count ?? 0]))
                    }
                }
            }
            .searchable(text: $filter, prompt: t("筛选子目录"))
            .navigationTitle(t("选择工作目录"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button { dismiss() } label: { lucideLabel(t("关闭"), "x") }
                }
                ToolbarItem(placement: .secondaryAction) {
                    Button { if let parent = listing?.parent { browse(parent) } } label: { lucideLabel(t("上一级"), "arrow-up") }
                        .disabled(listing?.parent == nil || loading)
                }
                ToolbarItem(placement: .secondaryAction) {
                    Button { browse("") } label: { lucideLabel(t("用户目录"), "house") }
                        .disabled(loading)
                }
                ToolbarItem(placement: .bottomBar) {
                    Button {
                        guard let l = listing else { return }
                        changing = true
                        Task {
                            let err = await core.changeDir(term, path: l.path)
                            changing = false
                            if err == nil { dismiss() } else { error = err }
                        }
                    } label: {
                        if changing { ProgressView() } else { lucideLabel(t("切换到这里"), "corner-down-left") }
                    }
                    .disabled(!canChange || listing == nil || changing || loading)
                }
            }
            .task { browse(session?.currentPath ?? "") }
        }
        .presentationDetents([.medium, .large])
    }

    private func browse(_ path: String) {
        loading = true
        error = nil
        Task {
            do {
                let next = try await core.listDir(term.connId, path: path)
                if next.path != listing?.path { filter = "" }
                listing = next
            } catch {
                self.error = errorText(error)
            }
            loading = false
        }
    }

    /// 拼对方电脑上的路径：Windows 用反斜杠
    private func join(_ dir: String, _ name: String) -> String {
        let sep = dir.contains("\\") && !dir.contains("/") ? "\\" : "/"
        return dir.hasSuffix(sep) ? dir + name : dir + sep + name
    }
}

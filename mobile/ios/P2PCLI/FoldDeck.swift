import SwiftUI

/// iPhone Duo 像笔记本那样半折立着时，折痕下面按键条再往下的那块：
/// - 终端空闲：一大块“点这里打字”，点了弹系统键盘（中文输入法、听写都能用），终端留在折痕上面不被挡；
/// - 这个终端里在跑 AI：换成 AI 面板，等确认时直接点选项，平时是中断、切换模式、回车和写指令。
/// 跑 AI 时顶上可以在“AI 面板”和“键盘”之间切。键盘弹起来时这块被盖住，空间不够就不画
struct FoldDeck: View {
    enum Mode: Hashable { case agent, keyboard }

    let term: RemoteTerminal
    let agent: AgentView?
    /// 自己选的模式；nil 时跟着 AI：在跑就是 AI 面板，没跑就是键盘
    @Binding var choice: Mode?
    let onCompose: () -> Void

    private var mode: Mode { agent == nil ? .keyboard : (choice ?? .agent) }

    var body: some View {
        GeometryReader { geo in
            if geo.size.height >= 96 {
                VStack(spacing: 10) {
                    if let agent { header(agent) }
                    switch mode {
                    case .keyboard: typeHere
                    case .agent: if let agent { agentPanel(agent) }
                    }
                }
                .padding(.horizontal, 8)
                .padding(.top, 4)
                .padding(.bottom, 8)
                .frame(width: geo.size.width, height: geo.size.height, alignment: .top)
            }
        }
    }

    private func header(_ agent: AgentView) -> some View {
        HStack(spacing: 10) {
            AgentIcon(kind: agent.kind, size: 18)
            VStack(alignment: .leading, spacing: 2) {
                Text(agent.name).font(.subheadline.weight(.semibold)).lineLimit(1)
                AgentStateLabel(agent: agent)
            }
            Spacer(minLength: 8)
            Picker(t("下半屏用法"), selection: Binding(get: { mode }, set: { switchTo($0) })) {
                Text(t("AI 面板")).tag(Mode.agent)
                Text(t("键盘")).tag(Mode.keyboard)
            }
            .pickerStyle(.segmented)
            .fixedSize()
        }
        .padding(.horizontal, 6)
    }

    private func switchTo(_ m: Mode) {
        choice = m
        if m == .keyboard { term.focus() } else { term.unfocus() }
    }

    /// 键盘模式：一整块都能点，点了弹键盘
    private var typeHere: some View {
        Button { term.focus() } label: {
            VStack(spacing: 8) {
                Icon("keyboard", size: 30).foregroundStyle(Hero.foreground)
                Text(t("点这里打字")).font(.body.weight(.medium))
                Text(t("键盘会出现在下半屏，终端留在上面")).font(.footnote).foregroundStyle(Hero.muted)
                    .multilineTextAlignment(.center)
            }
            .padding(16)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .heroGlass(cornerRadius: 22, interactive: true)
        .accessibilityIdentifier("fold-type-here")
    }

    /// AI 面板：等确认时先放选项，下面是常用操作和写指令
    private func agentPanel(_ agent: AgentView) -> some View {
        ScrollView {
            VStack(spacing: 10) {
                if agent.state == .waiting, let approval = agent.approval {
                    ApprovalPanel(term: term, agent: agent, approval: approval, inset: false)
                }
                GlassGroup(spacing: 8) {
                    HStack(spacing: 8) {
                        action(t("中断"), "circle-pause", hint: "Esc") { term.send("\u{1b}") }
                        action(t("切换模式"), "repeat-2", hint: "⇧Tab") { term.send("\u{1b}[Z") }
                        action(t("回车"), "corner-down-left", hint: "⏎") { term.send("\r") }
                    }
                }
                Button(action: onCompose) {
                    HStack(spacing: 10) {
                        Icon("pencil-line", size: 18).foregroundStyle(Hero.muted)
                        Text(t("给 {{name}} 写指令", ["name": agent.name])).foregroundStyle(Hero.muted)
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 16)
                    .frame(height: 48)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .heroGlass(cornerRadius: 24, interactive: true)
                .accessibilityIdentifier("fold-compose")
            }
        }
        .scrollBounceBehavior(.basedOnSize)
        .scrollIndicators(.hidden)
    }

    private func action(_ label: String, _ icon: String, hint: String, _ run: @escaping () -> Void) -> some View {
        Button(action: run) {
            VStack(spacing: 6) {
                Icon(icon, size: 22).foregroundStyle(Hero.foreground)
                Text(label).font(.footnote.weight(.medium)).lineLimit(1).minimumScaleFactor(0.8)
                Text(hint).font(.system(size: 11, design: .monospaced)).foregroundStyle(Hero.muted)
            }
            .frame(maxWidth: .infinity)
            .frame(height: 84)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .heroGlass(cornerRadius: 18, interactive: true)
        .accessibilityLabel(label)
    }
}

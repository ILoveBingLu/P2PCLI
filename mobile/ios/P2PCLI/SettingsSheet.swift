import SwiftUI

private let repoURL = "https://github.com/ILoveBingLu/P2PCLI"

/// 设置：只放手机用得上的。手机只当连接的一方，桌面上给“被连的电脑”用的设置
/// （AI 启动、多机互动、受信设备、配对密码、局域网里显示本机、系统权限）这里都没有。
struct SettingsSheet: View {
    @Environment(Core.self) private var core

    var body: some View {
        NavigationStack {
            if let v = core.settings {
                SettingsForm(v: v)
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity).background(Hero.background)
            }
        }
        .interactiveDismissDisabled(core.savingSettings)
    }
}

private struct SettingsForm: View {
    @Environment(Core.self) private var core
    @Environment(I18n.self) private var i18n
    let v: SettingsView
    @State private var name: String
    @State private var server: String
    @State private var custom: Bool
    @State private var customServer: String
    @State private var relay: String
    @State private var proxyOn: Bool
    @State private var proxy: String
    @State private var error: String?

    init(v: SettingsView) {
        self.v = v
        let s = v.settings
        // 牵线服务器：选了预设就是它的地址；不在预设里的算“自定义”
        let initial = s.server ?? v.defaultServer
        let preset = v.serverPresets.contains { $0.address == initial }
        _name = State(initialValue: s.deviceName ?? "")
        _server = State(initialValue: preset ? initial : "")
        _custom = State(initialValue: !preset)
        _customServer = State(initialValue: preset ? "" : initial)
        _relay = State(initialValue: s.relay ?? "")
        _proxyOn = State(initialValue: s.proxy != nil)
        _proxy = State(initialValue: s.proxy ?? "")
    }

    private var next: MobileSettings {
        let chosen = custom ? customServer.trimmingCharacters(in: .whitespaces) : server
        func clean(_ s: String) -> String? {
            let x = s.trimmingCharacters(in: .whitespaces)
            return x.isEmpty ? nil : x
        }
        return MobileSettings(
            deviceName: clean(name),
            // 选的就是默认服务器时不记，以后默认换了跟着换
            server: clean(chosen).flatMap { $0 == v.defaultServer ? nil : $0 },
            relay: clean(relay),
            proxy: proxyOn ? clean(proxy) : nil
        )
    }

    var body: some View {
        @Bindable var i18n = i18n
        let saving = core.savingSettings
        Form {
            // 改了马上生效，不用点保存（和桌面一样）
            Section {
                Picker(selection: $i18n.language) {
                    Text(t("跟随系统")).tag(I18n.Language.system)
                    Text("简体中文").tag(I18n.Language.chinese)
                    Text("English").tag(I18n.Language.english)
                } label: {
                    Label { Text(t("界面语言")) } icon: { Icon("languages", size: 18) }
                }
            } footer: {
                Text(t("跟随系统时，中文环境用简体中文，其他环境用英文。"))
            }

            Section {
                TextField(v.defaultDeviceName, text: $name)
                    .onChange(of: name) { error = nil }
            } header: {
                Text(t("设备名称"))
            } footer: {
                Text(t("对方电脑上看到的名字，留空用手机型号。"))
            }

            Section(t("牵线服务器")) {
                ForEach(v.serverPresets, id: \.address) { p in
                    ChoiceRow(selected: !custom && server == p.address, title: p.label,
                              badge: p.address == v.defaultServer ? t("默认") : nil, lines: [p.note, p.address]) {
                        custom = false
                        server = p.address
                        error = nil
                    }
                }
                ChoiceRow(selected: custom, title: t("自定义"), badge: nil,
                          lines: [t("填自己部署的牵线服务器：公钥@IP或域名:端口 走 UDP，wss://域名 走 WebSocket。")]) {
                    custom = true
                    error = nil
                }
                if custom {
                    TextField(t("公钥@IP:端口 或 wss://域名"), text: $customServer)
                        .keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled()
                        .font(.system(.body, design: .monospaced))
                        .onChange(of: customServer) { error = nil }
                }
            }

            Section {
                TextField("https://relay.example.com", text: $relay)
                    .keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled()
                    .onChange(of: relay) { error = nil }
            } header: {
                Text(t("中继服务器"))
            } footer: {
                Text(t("直连失败时转发加密数据，多个地址用逗号分隔。留空用公共中继。"))
            }

            Section {
                Toggle(isOn: $proxyOn) {
                    VStack(alignment: .leading, spacing: 3) {
                        Text(t("使用 HTTP 代理"))
                        Text(t("用于 WebSocket 牵线和 HTTP(S) 中继连接，点对点直连不经过代理。"))
                            .font(.footnote).foregroundStyle(Hero.muted)
                    }
                }
                .onChange(of: proxyOn) { error = nil }
                if proxyOn {
                    TextField("http://127.0.0.1:7890", text: $proxy)
                        .keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled()
                        .onChange(of: proxy) { error = nil }
                }
            } header: {
                Text(t("代理"))
            } footer: {
                if proxyOn { Text(t("填 http:// 开头的地址；暂不支持 SOCKS5 和账号密码认证。")) }
            }

            Section(t("文件")) {
                VStack(alignment: .leading, spacing: 6) {
                    Text(t("收到的文件放在"))
                    Text(t("“文件” App › 我的 iPhone › P2PCLI › Inbox")).font(.footnote).foregroundStyle(Hero.muted)
                }
                if let url = URL(string: "shareddocuments://" + v.inboxDir) {
                    Button { UIApplication.shared.open(url) } label: {
                        Label { Text(t("在“文件”里打开")) } icon: { Icon("folder-open", size: 18) }
                    }
                }
            }

            Section(t("关于")) {
                LabeledContent(t("版本"), value: v.appVersion ?? t("未知"))
                Button {
                    UIPasteboard.general.string = v.endpointId
                    core.notify(t("公钥已复制"))
                } label: {
                    LabeledContent {
                        HStack(spacing: 6) {
                            Text(String(v.endpointId.prefix(10)) + "…" + String(v.endpointId.suffix(6))).font(.system(size: 13, design: .monospaced))
                            Icon("copy", size: 15)
                        }
                        .foregroundStyle(Hero.muted)
                    } label: {
                        Text(t("本机公钥")).foregroundStyle(Hero.foreground)
                    }
                }
                Link(destination: URL(string: repoURL)!) { linkRow(t("GitHub 项目")) }
                Link(destination: URL(string: repoURL + "/issues")!) { linkRow(t("反馈问题")) }
                Link(destination: URL(string: repoURL + "/releases")!) { linkRow(t("版本发布")) }
            }

            Section {
                if let error {
                    Label { Text(error) } icon: { Icon("triangle-alert", size: 16) }
                        .foregroundStyle(Hero.danger)
                }
            } footer: {
                Text(t("保存后马上生效。改了代理或中继会重新连网络，开着的终端会断开，对方电脑上的会话还在。"))
            }
        }
        .scrollContentBackground(.hidden)
        .background(Hero.background)
        .disabled(saving)
        .navigationTitle(t("设置"))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button { core.showSettings = false } label: { lucideLabel(t("关闭"), "x") }
                    .accessibilityIdentifier("close-settings")
            }
            ToolbarItem(placement: .confirmationAction) {
                if saving {
                    ProgressView()
                } else {
                    Button {
                        Task { error = await core.saveSettings(next) }
                    } label: { lucideLabel(t("保存"), "check") }
                    .disabled(next == v.settings)
                }
            }
        }
    }

    private func linkRow(_ title: String) -> some View {
        HStack {
            Text(title).foregroundStyle(Hero.foreground)
            Spacer()
            Icon("square-arrow-out-up-right", size: 16).foregroundStyle(Hero.muted)
        }
    }
}

private struct ChoiceRow: View {
    let selected: Bool
    let title: String
    let badge: String?
    let lines: [String]
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                Circle().fill(selected ? Hero.accent : Hero.tile).frame(width: 20, height: 20)
                    .overlay { if selected { Circle().fill(.white).frame(width: 8, height: 8) } }
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(title).foregroundStyle(Hero.foreground)
                        if let badge { Chip(text: badge) }
                    }
                    ForEach(Array(lines.enumerated()), id: \.offset) { i, line in
                        Text(line)
                            .font(i > 0 ? .system(size: 12, design: .monospaced) : .footnote)
                            .foregroundStyle(Hero.muted)
                    }
                }
                Spacer(minLength: 0)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

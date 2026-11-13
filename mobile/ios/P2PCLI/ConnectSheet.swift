import SwiftUI

/// 连接面板：新连一台设备（扫码或输入设备码），或者连某台设备时填配对密码
struct ConnectSheet: View {
    @Environment(Core.self) private var core
    let prompt: ConnectPrompt
    @State private var code = ""
    @State private var password = ""
    @FocusState private var focus: Field?

    enum Field { case code, password }

    var body: some View {
        let busy = core.connecting != nil
        let known = prompt.endpointId != nil
        let canSubmit = !busy && (known || code.count == 9)
        NavigationStack {
            Form {
                if !known {
                    Section {
                        Button { core.openScan() } label: {
                            Label { Text(t("扫码连接")) } icon: { Icon("scan-line", size: 18) }
                                .frame(maxWidth: .infinity)
                        }
                        .heroProminent()
                        .controlSize(.large)
                        .disabled(busy)
                        .listRowBackground(Color.clear)
                        .listRowInsets(EdgeInsets())
                    } footer: {
                        Text(t("在电脑上点标题栏的手机图标，会显示二维码")).frame(maxWidth: .infinity)
                    }
                    Section(t("或者输入设备码")) {
                        CodeField(code: $code, focused: $focus)
                            .disabled(busy)
                            .padding(.vertical, 6)
                            .listRowBackground(Hero.surface)
                    }
                }
                Section(t("配对密码")) {
                    // 配对密码不是账号密码：标成一次性验证码，系统不会弹“保存密码”
                    SecureField(t("输入对方的配对密码"), text: $password)
                        .textContentType(.oneTimeCode)
                        .submitLabel(.go)
                        .focused($focus, equals: .password)
                        .onSubmit { if canSubmit { submit(known) } }
                        .disabled(busy)
                        .accessibilityIdentifier("password-field")
                        .listRowBackground(Hero.surface)
                }
                if let error = prompt.error {
                    Section {
                        Label { Text(error) } icon: { Icon("triangle-alert", size: 16) }
                            .foregroundStyle(Hero.danger)
                            .listRowBackground(Hero.danger.opacity(0.12))
                    }
                }
                Section {
                    Button { submit(known) } label: {
                        HStack(spacing: 8) {
                            if busy { ProgressView() }
                            Text(busy ? t("等待对方同意…") : t("连接"))
                        }
                        .frame(maxWidth: .infinity)
                    }
                    .heroProminent()
                    .controlSize(.large)
                    .disabled(!canSubmit)
                    .accessibilityIdentifier("connect-submit")
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets())
                } footer: {
                    Label { Text(t("连接时对方电脑上会弹窗问要不要同意")) } icon: { Icon("lock-keyhole", size: 14) }
                        .frame(maxWidth: .infinity)
                }
            }
            .navigationTitle(known ? t("连接 {{name}}", ["name": prompt.name ?? t("设备")]) : t("连接新设备"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button { core.prompt = nil } label: { lucideLabel(t("取消"), "x") }
                        .disabled(busy)
                }
            }
            .onAppear { focus = known ? .password : .code }
        }
        .interactiveDismissDisabled(busy)
        .presentationDetents(known ? [.medium, .large] : [.large])
    }

    private func submit(_ known: Bool) {
        core.submitPrompt(target: known ? prompt.target : code, password: password)
    }
}

/// 设备码：9 位数字，三个一组
private struct CodeField: View {
    @Binding var code: String
    var focused: FocusState<ConnectSheet.Field?>.Binding

    var body: some View {
        ZStack {
            TextField("", text: $code)
                .keyboardType(.numberPad)
                .focused(focused, equals: .code)
                .foregroundStyle(.clear)
                .tint(.clear)
                .onChange(of: code) { _, v in
                    let digits = String(v.filter(\.isNumber).prefix(9))
                    if digits != v { code = digits }
                    if digits.count == 9 { focused.wrappedValue = .password }
                }
                .accessibilityLabel(t("设备码"))
                .accessibilityIdentifier("code-field")
            HStack(spacing: 6) {
                ForEach(0..<3, id: \.self) { group in
                    if group > 0 {
                        Capsule().fill(Hero.muted.opacity(0.6)).frame(width: 8, height: 2)
                    }
                    ForEach(group * 3..<group * 3 + 3, id: \.self) { i in
                        let chars = Array(code)
                        let current = i == chars.count && focused.wrappedValue == .code
                        Text(i < chars.count ? String(chars[i]) : "")
                            .font(.system(size: 20, weight: .medium, design: .monospaced))
                            .frame(maxWidth: .infinity, minHeight: 46)
                            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Hero.tile))
                            .overlay {
                                if current {
                                    RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Hero.accent, lineWidth: 1.5)
                                }
                            }
                    }
                }
            }
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        }
        .contentShape(Rectangle())
        .onTapGesture { focused.wrappedValue = .code }
    }
}

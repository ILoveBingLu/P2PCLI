import Foundation
import Observation
import SwiftTerm
import SwiftUI
import UIKit

/// 手机上开着的一个远程终端。
///
/// 对方电脑上的输出（Rust 从网络收来）交给 SwiftTerm 解析和显示；键盘输入、按键条、
/// 模拟器要回给对方的应答都经 Rust 发回去。终端第一次知道自己有多大（几列几行）时才真正打开。
/// 终端控件跟着这个对象走：切到别的页面再回来，屏幕内容和往回翻的记录都还在。
@MainActor
@Observable
final class RemoteTerminal: Identifiable {
    enum State { case opening, open, closed }

    let key: Int
    let endpointId: String
    let connId: UInt64
    /// 对方电脑上的会话编号；新建的终端打开以后才有
    private(set) var sessionId: String?
    private(set) var state: State = .opening
    /// 结束、断开时给人看的原因
    private(set) var note: String?
    /// 程序退出（终端已结束），不是连接断了
    private(set) var exited = false
    /// 按键条上的 Ctrl、Alt：按一下，下一个键带上它
    var ctrl = false
    var alt = false

    nonisolated var id: Int { key }

    @ObservationIgnored let view: TerminalView
    @ObservationIgnored private var handle: Terminal?
    @ObservationIgnored private var opened = false
    @ObservationIgnored private let opener: (UInt16, UInt16, TerminalListener) async throws -> Terminal
    @ObservationIgnored private var delegate: Delegate?
    @ObservationIgnored private var listener: Listener?
    @ObservationIgnored private var fontSize: CGFloat = 13

    init(key: Int, endpointId: String, connId: UInt64, sessionId: String?,
         opener: @escaping (UInt16, UInt16, TerminalListener) async throws -> Terminal) {
        self.key = key
        self.endpointId = endpointId
        self.connId = connId
        self.sessionId = sessionId
        self.opener = opener
        view = TerminalView(frame: CGRect(x: 0, y: 0, width: 320, height: 480),
                            font: UIFont.monospacedSystemFont(ofSize: 13, weight: .regular))
        let d = Delegate(owner: self)
        delegate = d
        listener = Listener(owner: self)
        view.terminalDelegate = d
        view.nativeBackgroundColor = UIColor(hex: 0x111114)
        view.nativeForegroundColor = UIColor(hex: 0xE4E4E7)
        view.caretColor = UIColor(hex: 0x0485F7)
        view.selectedTextBackgroundColor = UIColor(hex: 0x0485F7).withAlphaComponent(0.4)
        view.backgroundColor = UIColor(hex: 0x111114)
        view.keyboardAppearance = .dark
        // 自己的按键条在下面，不要 SwiftTerm 自带的那条
        view.inputAccessoryView = nil
        view.optionAsMetaKey = true
        view.accessibilityIdentifier = "terminal"
        let pinch = UIPinchGestureRecognizer(target: d, action: #selector(Delegate.pinch(_:)))
        view.addGestureRecognizer(pinch)
    }

    /// 控件量好了大小：第一次就打开终端，之后改尺寸
    fileprivate func onSize(cols: Int, rows: Int) {
        guard cols >= 2, rows >= 1 else { return }
        if !opened {
            opened = true
            open(cols: cols, rows: rows)
        } else {
            handle?.resize(cols: UInt16(cols), rows: UInt16(rows))
        }
    }

    /// 容器排好版时调：如果尺寸刚好和默认一样，SwiftTerm 不会通知，这里补一次
    fileprivate func laidOut() {
        guard !opened, view.bounds.width > 40, view.bounds.height > 40 else { return }
        let t = view.getTerminal()
        onSize(cols: t.cols, rows: t.rows)
    }

    private func open(cols: Int, rows: Int) {
        guard let listener else { return }
        Task {
            do {
                let h = try await opener(UInt16(cols), UInt16(rows), listener)
                handle = h
                sessionId = h.sessionId()
                state = .open
                // 打开的这段时间里屏幕转了、键盘弹了，尺寸可能已经变了
                let t = view.getTerminal()
                if t.cols != cols || t.rows != rows { h.resize(cols: UInt16(t.cols), rows: UInt16(t.rows)) }
            } catch {
                state = .closed
                note = errorText(error)
            }
        }
    }

    // MARK: 发给对方

    /// 按键条发的原始字节：走终端控件，和键盘打的一样
    func send(_ text: String) {
        view.send(Array(text.utf8))
    }

    func send(_ bytes: [UInt8]) {
        view.send(bytes)
    }

    /// 方向键：程序开了“应用光标模式”（vim、less 这些）时发的不一样
    func arrow(_ dir: Character) {
        let app = view.getTerminal().applicationCursor
        let seq: [UInt8] = switch dir {
        case "A": app ? EscapeSequences.moveUpApp : EscapeSequences.moveUpNormal
        case "B": app ? EscapeSequences.moveDownApp : EscapeSequences.moveDownNormal
        case "D": app ? EscapeSequences.moveLeftApp : EscapeSequences.moveLeftNormal
        default: app ? EscapeSequences.moveRightApp : EscapeSequences.moveRightNormal
        }
        view.send(seq)
    }

    /// 当成粘贴发：对方程序开了整段粘贴（bracketed paste）就整段发，多行不会被当成几次回车
    func paste(_ text: String) {
        let bracketed = view.getTerminal().bracketedPasteMode
        if bracketed { view.send(EscapeSequences.bracketedPasteStart) }
        view.send(Array(text.utf8))
        if bracketed { view.send(EscapeSequences.bracketedPasteEnd) }
    }

    func pasteClipboard() {
        if let s = UIPasteboard.general.string, !s.isEmpty { paste(s) }
    }

    var hasSelection: Bool { view.hasActiveSelection }

    /// 复制选中的文字；没选中返回 false
    func copySelection() -> Bool {
        guard view.hasActiveSelection else { return false }
        view.copy(nil)
        return true
    }

    func focus() { _ = view.becomeFirstResponder() }

    /// 收起键盘
    func unfocus() { _ = view.resignFirstResponder() }

    /// 关掉手机上的这个终端；对方电脑上的会话接着跑
    func close() {
        handle?.detach()
        handle = nil
    }

    // MARK: 从终端控件、Rust 来的

    /// 键盘打的字、终端的应答：带上按键条上按下的 Ctrl、Alt 再发出去
    fileprivate func outgoing(_ data: ArraySlice<UInt8>) {
        var bytes = Array(data)
        // 只对打的字生效：终端自己的应答（ESC 开头）不动
        let typed = !bytes.isEmpty && bytes.count <= 4 && bytes[0] != 0x1b
        if typed && ctrl {
            ctrl = false
            if bytes.count == 1 {
                let b = bytes[0]
                if (0x61...0x7a).contains(b) || (0x40...0x5f).contains(b) { bytes = [b & 0x1f] }
                else if b == 0x20 { bytes = [0] }
            }
        }
        if typed && alt {
            alt = false
            bytes.insert(0x1b, at: 0)
        }
        handle?.write(data: Data(bytes))
    }

    fileprivate func output(_ data: Data) {
        view.feed(byteArray: ArraySlice([UInt8](data)))
    }

    fileprivate func closed(exitCode: UInt32?, reason: String?) {
        handle = nil
        exited = reason == nil
        state = .closed
        if let reason { note = tr(reason) }
        else if let exitCode { note = t("终端视图已结束（退出码 {{code}}）", ["code": exitCode]) }
        else { note = t("终端视图已结束") }
    }

    fileprivate func zoom(_ scale: CGFloat) {
        let size = min(24, max(9, fontSize * scale))
        guard abs(size - view.font.pointSize) >= 0.5 else { return }
        view.font = UIFont.monospacedSystemFont(ofSize: size, weight: .regular)
    }

    fileprivate func zoomEnded() { fontSize = view.font.pointSize }

    /// SwiftTerm 的回调（主线程）
    private final class Delegate: NSObject, TerminalViewDelegate {
        weak var owner: RemoteTerminal?
        init(owner: RemoteTerminal) { self.owner = owner }

        private func on(_ f: @MainActor (RemoteTerminal) -> Void) {
            MainActor.assumeIsolated { if let o = owner { f(o) } }
        }

        func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) { on { $0.onSize(cols: newCols, rows: newRows) } }
        func setTerminalTitle(source: TerminalView, title: String) {}
        func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}
        func send(source: TerminalView, data: ArraySlice<UInt8>) { on { $0.outgoing(data) } }
        func scrolled(source: TerminalView, position: Double) {}
        func requestOpenLink(source: TerminalView, link: String, params: [String: String]) {
            guard let url = URL(string: link), ["http", "https"].contains(url.scheme?.lowercased() ?? "") else { return }
            MainActor.assumeIsolated { UIApplication.shared.open(url) }
        }
        func bell(source: TerminalView) {}
        func clipboardCopy(source: TerminalView, content: Data) {
            if let s = String(data: content, encoding: .utf8) { UIPasteboard.general.string = s }
        }
        func clipboardRead(source: TerminalView) -> Data? { nil }
        func iTermContent(source: TerminalView, content: ArraySlice<UInt8>) {}
        func rangeChanged(source: TerminalView, startY: Int, endY: Int) {}

        /// 两指捏合改字号
        @objc func pinch(_ g: UIPinchGestureRecognizer) {
            on { o in
                switch g.state {
                case .changed: o.zoom(g.scale)
                case .ended, .cancelled: o.zoomEnded()
                default: break
                }
            }
        }
    }

    /// Rust 送来的输出从 Rust 的线程来：按顺序切到主线程
    private final class Listener: TerminalListener, @unchecked Sendable {
        weak var owner: RemoteTerminal?
        init(owner: RemoteTerminal) { self.owner = owner }

        func onOutput(data: Data) {
            DispatchQueue.main.async { MainActor.assumeIsolated { self.owner?.output(data) } }
        }

        func onClosed(exitCode: UInt32?, reason: String?) {
            DispatchQueue.main.async { MainActor.assumeIsolated { self.owner?.closed(exitCode: exitCode, reason: reason) } }
        }
    }
}

/// 把终端控件放进 SwiftUI。控件属于 RemoteTerminal，切换终端时换掉里面那个
struct TerminalHost: UIViewRepresentable {
    let term: RemoteTerminal

    func makeUIView(context: Context) -> Container { Container() }

    func updateUIView(_ container: Container, context: Context) {
        container.show(term)
    }

    final class Container: UIView {
        private weak var term: RemoteTerminal?

        override init(frame: CGRect) {
            super.init(frame: frame)
            backgroundColor = UIColor(hex: 0x111114)
            clipsToBounds = true
        }

        required init?(coder: NSCoder) { fatalError() }

        @MainActor
        func show(_ term: RemoteTerminal) {
            guard self.term !== term else { return }
            self.term?.view.removeFromSuperview()
            self.term = term
            addSubview(term.view)
            setNeedsLayout()
        }

        override func layoutSubviews() {
            super.layoutSubviews()
            guard let term else { return }
            MainActor.assumeIsolated {
                term.view.frame = bounds.inset(by: UIEdgeInsets(top: 4, left: 6, bottom: 0, right: 2))
                term.laidOut()
            }
        }
    }
}

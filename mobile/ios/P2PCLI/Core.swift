import Foundation
import Observation
import SwiftUI
import UIKit

/// 左边列表（窄屏时是首页）选中的东西：右边（窄屏时推进去的那页）显示它
enum Selection: Hashable {
    case device(String)
    case term(Int)
}

/// 右边那一栏里一层层推进去的页面：从设备页开终端
enum Route: Hashable {
    case term(Int)
}

enum StartState: Equatable {
    case starting
    case ready
    case failed(String)
}

/// 一台设备的会话列表；list 为空是还没读到
struct SessionsState {
    var list: [SessionView]?
    var error: String?
}

/// 一条连接上检测到的命令行 AI；list 为空是还没检测完
struct AgentsState {
    var list: [InstalledAgentView]?
    var error: String?
    var scanning = false
}

/// 连接面板：新连一台设备，或者连某台设备时要填密码
struct ConnectPrompt: Identifiable, Equatable {
    let id = UUID()
    /// 已知是哪台设备（从列表里点的）；手输设备码时是空
    var endpointId: String?
    var name: String?
    /// 已知的设备码或公钥；手输时用户自己填
    var target = ""
    var error: String?
}

struct Notice: Identifiable, Equatable {
    let id = UUID()
    let text: String
}

/// 整个 App 的状态和操作。Rust 那边的接口都在这里调；界面只读这里的状态、调这里的方法。
@MainActor
@Observable
final class Core {
    static let shared = Core()

    @ObservationIgnored private var p2p: P2p?
    @ObservationIgnored private lazy var bridge = Bridge(core: self)
    @ObservationIgnored private var termSeq = 1
    @ObservationIgnored private var background: UIBackgroundTaskIdentifier = .invalid

    var start: StartState = .starting
    var overview: Overview?
    var transfers: [TransferView] = []
    /// 连着的设备：公钥 → 连接
    var conns: [String: Connected] = [:]
    /// 各设备的会话列表：公钥 → 列表
    var sessions: [String: SessionsState] = [:]
    /// 各连接上装了的 AI：连接编号 → 检测结果（被控端还有五分钟缓存）
    var agents: [UInt64: AgentsState] = [:]
    var terms: [RemoteTerminal] = []
    var selection: Selection?
    var path: [Route] = []
    /// 正在连哪台设备（公钥）；手输设备码时是 "new"
    var connecting: String?
    /// 正在连的设备叫什么：扫码连的新设备列表里还没有，首页顶上显示一张卡片
    var connectingName: String?
    var prompt: ConnectPrompt?
    var scanning = false
    var showSettings = false
    var fileRequest: FileRequestView?
    var settings: SettingsView?
    var savingSettings = false
    var notice: Notice?

    // MARK: 启动

    func startCore() {
        start = .starting
        let fm = FileManager.default
        let support = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("p2pcli")
        // 收到的文件放在“文件”App 里能看到的地方（Info.plist 开了 UIFileSharingEnabled）
        let inbox = fm.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("Inbox")
        try? fm.createDirectory(at: support, withIntermediateDirectories: true)
        try? fm.createDirectory(at: inbox, withIntermediateDirectories: true)
        let config = MobileConfig(
            dataDir: support.path,
            inboxDir: inbox.path,
            deviceName: UIDevice.current.name,
            server: nil,
            relay: nil,
            proxy: nil,
            appVersion: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String,
            platform: nil
        )
        let listener = bridge
        Task {
            do {
                // 启动要等网络起来，会阻塞一会儿：放到后台线程
                let p = try await Task.detached { try P2p.start(config: config, listener: listener) }.value
                p2p = p
                start = .ready
                await refreshOverview()
            } catch {
                start = .failed(errorText(error))
            }
        }
    }

    // MARK: 页面

    /// 列表里点了东西（或者窄屏时退回了首页）
    func select(_ value: Selection?) {
        switch value {
        case .device(let id)?:
            if conns[id] != nil {
                selection = value
                path = []
            } else if let d = device(id) {
                openDevice(d)
            }
        case .term?:
            selection = value
            path = []
        case nil:
            selection = nil
            path = []
        }
    }

    func device(_ id: String) -> DeviceView? {
        overview?.devices.first { $0.endpointId == id } ?? overview?.nearby.first { $0.endpointId == id }
    }

    func deviceName(_ id: String) -> String {
        if let d = device(id) { return d.remark ?? d.name }
        return conns[id]?.name ?? ""
    }

    func notify(_ text: String) {
        let n = Notice(text: text)
        notice = n
        Task {
            try? await Task.sleep(for: .seconds(2.6))
            if notice == n { notice = nil }
        }
    }

    // MARK: Rust 的回调（Bridge 已经切到主线程）

    fileprivate func onDisconnected(_ connId: UInt64) {
        conns = conns.filter { $0.value.connId != connId }
    }

    fileprivate func onTransfers(_ list: [TransferView]) {
        let before = Dictionary(uniqueKeysWithValues: transfers.map { ($0.id, $0) })
        transfers = list
        for x in list {
            guard let old = before[x.id], old.state != x.state, x.outgoing else { continue }
            if x.state == .done {
                notify(t("已发给 {{name}}：{{label}}", ["name": x.peerName, "label": x.label]))
            } else if x.state == .failed {
                notify(t("没发出去：{{error}}", ["error": x.error.map(tr) ?? x.label]))
            }
        }
    }

    fileprivate func onFileRequest(_ r: FileRequestView) { fileRequest = r }

    fileprivate func onFileRequestCancelled(_ id: UInt64) {
        if fileRequest?.id == id { fileRequest = nil }
    }

    func respondFiles(_ accept: Bool) {
        guard let r = fileRequest else { return }
        fileRequest = nil
        p2p?.respondFiles(id: r.id, accept: accept)
    }

    func refreshOverview() async {
        guard let p = p2p else { return }
        overview = await Task.detached { p.overview() }.value
    }

    func scanNearby() {
        guard let p = p2p else { return }
        Task {
            do {
                _ = try await p.scanNearby()
                await refreshOverview()
            } catch {
                notify(t("扫描失败：{{error}}", ["error": errorText(error)]))
            }
        }
    }

    // MARK: 连接

    /// 点了列表里的设备：连着就进会话页，没连着先连
    func openDevice(_ d: DeviceView) {
        if conns[d.endpointId] != nil {
            selection = .device(d.endpointId)
            path = []
            return
        }
        // 局域网里在线就按公钥直接连，不经过牵线服务器
        let target = d.lanOnline || d.code == nil ? d.endpointId : d.code!
        connect(target, password: nil, endpointId: d.endpointId, name: d.remark ?? d.name)
    }

    /// 连接面板里点“连接”
    func submitPrompt(target: String, password: String) {
        guard let p = prompt else { return }
        let t = target.trimmingCharacters(in: .whitespacesAndNewlines)
        connect(t.isEmpty ? p.target : t, password: password, endpointId: p.endpointId, name: p.name)
    }

    func connectPair(_ pair: PairInfo) {
        connect(pair.code ?? pair.id, password: pair.password, endpointId: pair.id, name: pair.name.isEmpty ? nil : pair.name)
    }

    /// 打开扫码页；从连接面板里点的先把面板关了
    func openScan() {
        guard connecting == nil else { return }
        prompt = nil
        scanning = true
    }

    /// 扫码页里点“输入设备码”：换成连接面板
    func scanToManual() {
        scanning = false
        prompt = ConnectPrompt()
    }

    /// 扫到了电脑的二维码：关掉扫码页，用里面的设备码和密码去连
    func scanned(_ pair: PairInfo) {
        scanning = false
        connectPair(pair)
    }

    private func connect(_ target: String, password: String?, endpointId: String?, name: String?) {
        guard let p = p2p, connecting == nil else { return }
        connecting = endpointId ?? "new"
        connectingName = name
        Task {
            defer {
                connecting = nil
                connectingName = nil
            }
            do {
                let c = try await p.connect(target: target, password: password)
                conns[c.endpointId] = c
                prompt = nil
                // 新设备要先进列表，列表的选中才对得上
                await refreshOverview()
                selection = .device(c.endpointId)
                path = []
                await loadSessions(c.endpointId)
            } catch P2pError.Rejected(let reason, let detail) {
                if reason == .needPassword || reason == .badPassword {
                    // 要密码：弹出面板填密码；密码不对时说一声
                    prompt = ConnectPrompt(endpointId: endpointId, name: name, target: target,
                                           error: reason == .badPassword ? tr(detail) : nil)
                } else {
                    showError(tr(detail))
                }
            } catch {
                showError(errorText(error))
            }
        }
    }

    /// 面板开着时错误显示在面板里，不然弹一句提示
    private func showError(_ message: String) {
        if prompt != nil { prompt?.error = message } else { notify(message) }
    }

    func disconnect(_ endpointId: String) {
        guard let c = conns[endpointId] else { return }
        p2p?.disconnect(connId: c.connId)
        conns[endpointId] = nil
        for x in terms where x.endpointId == endpointId { x.close() }
        terms.removeAll { $0.endpointId == endpointId }
        selection = nil
        path = []
    }

    // MARK: 会话

    func loadSessions(_ endpointId: String) async {
        guard let p = p2p, let c = conns[endpointId] else { return }
        do {
            let list = try await p.listSessions(connId: c.connId)
            sessions[endpointId] = SessionsState(list: list, error: nil)
        } catch {
            sessions[endpointId] = SessionsState(list: sessions[endpointId]?.list, error: errorText(error))
        }
    }

    func endSession(_ endpointId: String, _ sessionId: String) {
        guard let p = p2p, let c = conns[endpointId] else { return }
        Task {
            do {
                try await p.endSession(connId: c.connId, sessionId: sessionId)
                for x in terms where x.endpointId == endpointId && x.sessionId == sessionId { closeTerminal(x.key) }
                await loadSessions(endpointId)
                notify(t("会话已结束"))
            } catch {
                notify(t("结束会话失败：{{error}}", ["error": errorText(error)]))
            }
        }
    }

    // MARK: 终端

    /// 打开一个会话；sessionId 为空是新建。已经开着就切过去
    func openTerminal(_ endpointId: String, _ sessionId: String?) {
        guard let p = p2p, let c = conns[endpointId] else { return }
        if let id = sessionId,
           let existing = terms.first(where: { $0.endpointId == endpointId && $0.sessionId == id && $0.state != .closed }) {
            return goTerminal(existing.key)
        }
        let connId = c.connId
        let term = RemoteTerminal(key: termSeq, endpointId: endpointId, connId: connId, sessionId: sessionId) { cols, rows, listener in
            try await p.openTerminal(connId: connId, sessionId: sessionId, cols: cols, rows: rows, listener: listener)
        }
        termSeq += 1
        // 同一个会话之前断开的那个换掉
        let stale = terms.filter { sessionId != nil && $0.endpointId == endpointId && $0.sessionId == sessionId }
        for x in stale { forget(x.key) }
        terms.append(term)
        goTerminal(term.key)
    }

    /// 已经在终端页：原地换；在设备页：推进去；其他：直接显示
    private func goTerminal(_ key: Int) {
        if let last = path.last, case .term = last {
            path[path.count - 1] = .term(key)
        } else if path.isEmpty, case .term = selection {
            selection = .term(key)
        } else if case .device = selection {
            path.append(.term(key))
        } else {
            selection = .term(key)
            path = []
        }
    }

    /// 关掉手机上的这个终端，会话在对方电脑上接着跑
    func closeTerminal(_ key: Int) {
        guard let term = terms.first(where: { $0.key == key }) else { return }
        term.close()
        forget(key)
        if selection == .term(key) {
            selection = conns[term.endpointId] != nil ? .device(term.endpointId) : nil
        }
    }

    private func forget(_ key: Int) {
        terms.removeAll { $0.key == key }
        path.removeAll { $0 == .term(key) }
    }

    /// 断开的终端重新连：连接还在就接回同一个会话，不然先重连设备
    func reopen(_ term: RemoteTerminal) {
        if conns[term.endpointId] != nil {
            let (eid, sid) = (term.endpointId, term.sessionId)
            closeTerminal(term.key)
            openTerminal(eid, sid)
        } else {
            let d = device(term.endpointId)
            closeTerminal(term.key)
            if let d { openDevice(d) } else { notify(t("找不到这台设备，请回到首页重新连接")) }
        }
    }

    func chooseOption(_ term: RemoteTerminal, approvalId: String, optionId: UInt64) {
        guard let p = p2p, let session = term.sessionId else { return }
        Task {
            do {
                try await p.chooseAgentOption(connId: term.connId, sessionId: session, approvalId: approvalId, optionId: optionId)
            } catch {
                notify(errorText(error))
            }
            await loadSessions(term.endpointId)
        }
    }

    // MARK: 启动 AI、选工作目录

    /// 检测对方电脑上装了哪些命令行 AI；已经有结果时不重复检测，refresh 时强制重新检测
    func scanAgents(_ connId: UInt64, refresh: Bool = false) {
        guard let p = p2p else { return }
        let current = agents[connId]
        if current?.scanning == true || (!refresh && current?.list != nil) { return }
        agents[connId] = AgentsState(list: current?.list, error: nil, scanning: true)
        Task {
            do {
                let list = try await p.installedAgents(connId: connId, refresh: refresh)
                agents[connId] = AgentsState(list: list, error: nil, scanning: false)
            } catch {
                agents[connId] = AgentsState(list: current?.list, error: errorText(error), scanning: false)
                if refresh { notify(t("检测 AI 失败：{{e}}", ["e": errorText(error)])) }
            }
        }
    }

    /// 在终端的会话里启动一个 AI（会话要停在空闲的命令提示符）
    func launchAgent(_ term: RemoteTerminal, kind: String, allowBypass: Bool) async {
        guard let p = p2p, let session = term.sessionId else { return }
        do {
            try await p.launchAgent(connId: term.connId, sessionId: session, kind: kind, allowBypass: allowBypass)
        } catch {
            notify(t("启动失败：{{e}}", ["e": errorText(error)]))
        }
        await loadSessions(term.endpointId)
    }

    /// 对方电脑上某个目录的子目录；路径空着是用户目录
    func listDir(_ connId: UInt64, path: String) async throws -> DirListingView {
        guard let p = p2p else { throw P2pError.Disconnected }
        return try await p.listDir(connId: connId, path: path)
    }

    /// 让终端的会话切到这个目录；出错时返回原因
    func changeDir(_ term: RemoteTerminal, path: String) async -> String? {
        guard let p = p2p, let session = term.sessionId else { return t("连接已断开") }
        do {
            try await p.changeDir(connId: term.connId, sessionId: session, path: path)
            await loadSessions(term.endpointId)
            return nil
        } catch {
            return errorText(error)
        }
    }

    // MARK: 传文件

    /// 系统选文件给的地址要先申请访问：复制到临时目录，再按路径发
    func sendFiles(_ endpointId: String, urls: [URL]) {
        guard let p = p2p, !urls.isEmpty else { return }
        Task {
            do {
                let paths = try await Task.detached { try urls.enumerated().map { try Self.stage($0.element, $0.offset) } }.value
                _ = try await Task.detached { try p.sendFiles(target: endpointId, paths: paths) }.value
                notify(t("开始发送 {{count}} 个文件", ["count": paths.count]))
            } catch {
                notify(t("发不了：{{error}}", ["error": errorText(error)]))
            }
        }
    }

    private nonisolated static func stage(_ url: URL, _ index: Int) throws -> String {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        // 每个文件一个子目录：同名的各放各的，发过去还是原来的名字
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("outgoing/\(Int(Date().timeIntervalSince1970 * 1000))-\(index)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let name = url.lastPathComponent.isEmpty ? "file-\(index)" : url.lastPathComponent
        let dest = dir.appendingPathComponent(name)
        try FileManager.default.copyItem(at: url, to: dest)
        return dest.path
    }

    // MARK: 设置

    func openSettings() {
        settings = p2p?.settings()
        showSettings = true
    }

    /// 保存并马上生效；格式不对时返回原因
    func saveSettings(_ value: MobileSettings) async -> String? {
        guard let p = p2p, !savingSettings else { return nil }
        savingSettings = true
        defer { savingSettings = false }
        do {
            try await p.saveSettings(settings: value)
            settings = p.settings()
            notify(t("设置已保存"))
            showSettings = false
            await refreshOverview()
            return nil
        } catch {
            return errorText(error)
        }
    }

    // MARK: 切到后台

    /// 切到后台时向系统多要一点时间，刚切出去又切回来连接不会断
    func enteredBackground() {
        guard background == .invalid else { return }
        background = UIApplication.shared.beginBackgroundTask(withName: "p2pcli") { [weak self] in
            MainActor.assumeIsolated { self?.endBackground() }
        }
    }

    func becameActive() {
        endBackground()
        Task { await refreshOverview() }
    }

    private func endBackground() {
        if background != .invalid {
            UIApplication.shared.endBackgroundTask(background)
            background = .invalid
        }
    }
}

/// Rust 的回调从 Rust 的线程来：按顺序切到主线程再交给 Core
private final class Bridge: AppListener, @unchecked Sendable {
    weak var core: Core?

    init(core: Core) { self.core = core }

    private func main(_ f: @escaping @MainActor (Core) -> Void) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                if let core = self.core { f(core) }
            }
        }
    }

    func onOverviewChanged() { main { core in Task { await core.refreshOverview() } } }
    func onDisconnected(connId: UInt64, reason: String) { main { $0.onDisconnected(connId) } }
    func onTransfersChanged(transfers: [TransferView]) { main { $0.onTransfers(transfers) } }
    func onFileRequest(request: FileRequestView) { main { $0.onFileRequest(request) } }
    func onFileRequestCancelled(id: UInt64) { main { $0.onFileRequestCancelled(id) } }
}

import SwiftUI

/// 设备、会话、AI 状态显示成文字：和安卓的 Common.kt 一样
enum Format {
    static func path(_ path: PathView?) -> String {
        guard let path else { return t("连接中") }
        let kind: String
        switch path.kind {
        case .ipv6: kind = t("IPv6 直连")
        case .ipv4: kind = t("IPv4 直连")
        case .relay: kind = t("中继")
        case .local: kind = t("本机")
        case .unknown: return t("连接中")
        }
        if let rtt = path.rttMs { return "\(kind) · \(rtt) ms" }
        return kind
    }

    /// 设备一行的状态：文字和颜色
    static func presence(_ d: DeviceView, connected: Bool) -> (String, Color?) {
        if connected { return (t("已连接"), Hero.success) }
        switch d.presence {
        case .online:
            let rtt = d.path?.rttMs.map { " · \($0) ms" } ?? ""
            let text = (d.lanOnline ? t("局域网在线") : t("在线")) + rtt
            return (text, d.path?.kind == .relay ? Hero.warning : Hero.success)
        case .checking: return (t("检测中…"), nil)
        case .oldVersion: return (t("对方需要更新 P2PCLI"), Hero.warning)
        case .offline: return (t("离线"), nil)
        }
    }

    static func agentText(_ a: AgentView) -> String {
        if a.stalled { return t("可能卡住") }
        switch a.state {
        case .working: return t("干活中")
        case .waiting: return t("等你确认")
        case .idle: return t("空闲")
        }
    }

    static func agentColor(_ a: AgentView) -> Color? {
        if a.stalled || a.state == .waiting { return Hero.warning }
        if a.state == .working { return Hero.accent }
        return nil
    }

    /// 会话的名字：有备注用备注；跑着 AI 用 AI 的名字（同名的加编号）；不然按创建先后叫“会话 1、会话 2”
    static func sessionTitles(_ sessions: [SessionView]) -> [String: String] {
        let sorted = sessions.sorted { ($0.createdAt, $0.id) < ($1.createdAt, $1.id) }
        var numbers: [String: String] = [:]
        for (i, s) in sorted.enumerated() { numbers[s.id] = t("会话 {{value}}", ["value": i + 1]) }
        var agentCounts: [String: Int] = [:]
        for s in sessions { if let n = s.agent?.name { agentCounts[n, default: 0] += 1 } }
        var out: [String: String] = [:]
        for s in sessions {
            let number = numbers[s.id] ?? t("会话")
            if let remark = s.remark, !remark.trimmingCharacters(in: .whitespaces).isEmpty {
                out[s.id] = remark
            } else if let agent = s.agent {
                out[s.id] = (agentCounts[agent.name] ?? 0) > 1 ? "\(agent.name) · \(number)" : agent.name
            } else {
                out[s.id] = number
            }
        }
        return out
    }

    /// 刚刚、3 分钟前、2 小时前、5 天前
    static func relativeTime(_ secs: UInt64) -> String {
        let diff = Int(Date().timeIntervalSince1970) - Int(secs)
        if diff < 60 { return t("刚刚") }
        if diff < 3600 { return t("{{value}} 分钟前", ["value": diff / 60]) }
        if diff < 86400 { return t("{{value}} 小时前", ["value": diff / 3600]) }
        return t("{{value}} 天前", ["value": diff / 86400])
    }

    /// /Users/me/Projects/x → ~/Projects/x，太长只留最后两段
    static func shortPath(_ path: String?) -> String {
        guard let path, !path.trimmingCharacters(in: .whitespaces).isEmpty else { return "" }
        var home = path.replacingOccurrences(of: #"^/(Users|home)/[^/]+"#, with: "~", options: .regularExpression)
        home = home.replacingOccurrences(of: #"^[A-Za-z]:\\Users\\[^\\]+"#, with: "~", options: .regularExpression)
        let parts = home.split(whereSeparator: { $0 == "/" || $0 == "\\" }).map(String.init)
        return parts.count > 3 ? "…/" + parts.suffix(2).joined(separator: "/") : home
    }

    static func size(_ bytes: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
    }
}

/// Rust 那边的错误转成给人看的一句话
func errorText(_ e: Error) -> String {
    if let p = e as? P2pError {
        switch p {
        case .Rejected(_, let detail): return tr(detail)
        case .Failed(let detail): return tr(detail)
        case .Disconnected: return t("连接已断开")
        }
    }
    return e.localizedDescription
}

/// AI 的状态：点加字，干活中蓝、等确认黄
struct AgentStateLabel: View {
    let agent: AgentView
    var size: CGFloat = 12

    var body: some View {
        let color = Format.agentColor(agent) ?? Hero.muted
        HStack(spacing: 4) {
            Dot(color: color, size: 6)
            Text(Format.agentText(agent)).font(.system(size: size)).foregroundStyle(color)
        }
    }
}

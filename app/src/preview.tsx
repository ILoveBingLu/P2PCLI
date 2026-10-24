// 开发用：浏览器里假数据渲染界面，看排版。不进正式包。
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type { Overview, SettingsView, TerminalSession } from "./api";

const s = new URLSearchParams(location.search).get("s") ?? "empty";

const ov: Overview = {
  name: "我的 MacBook Pro",
  platform: "macos",
  endpoint_id: "459a2c0a8c093ad76c1b00d7ecfd6dce576f219cdc3c2907cdfc49b3b9ac9ca9",
  code: "246813579",
  password: "ankab7",
  server: { state: "online" },
  sessions:
    s === "busy"
      ? [{ session: 3, remote: "f9bb5fbc3f", name: "小王的 ThinkPad", platform: "windows", terminals: 1, since: 0 }]
      : [],
  recent: [
    { code: "123456789", endpoint_id: "x", name: "工作室 Mac mini", platform: "macos", last_used: 3 },
    { code: "987654321", endpoint_id: "y", name: "办公室 PC", platform: "windows", last_used: 2 },
  ],
  trusted: [{ endpoint_id: "f9bb5fbc3f", name: "工作室 Mac mini", platform: "macos", added_at: 0 }],
};

if (s === "unconfigured") {
  ov.server = { state: "none" };
  ov.code = null;
  ov.recent = [];
}
if (s === "offline") ov.server = { state: "offline", error: "无法连接牵线服务器" };
const previewSettings: SettingsView = {
  server: null, device_name: null, relay: null, proxy: null,
  effective_relay: null, effective_server: "wss://p2pcli.yscopy.cn",
  data_dir: "~/Library/Application Support/io.github.ilovebinglu.p2pcli",
};
let termSeq = 0;
const terminalSessions: TerminalSession[] = s === "sessions" ? [{ id: "a".repeat(32), created_at: Math.floor(Date.now() / 1000) - 3600, attached: false }] : [];
const attachments = new Map<number, { sessionId: string; status: any }>();

const sample = [
  "\x1b[1;32mme@studio\x1b[0m \x1b[1;34m~\x1b[0m % claude\r\n",
  "\x1b[38;5;208m╭──────────────────────────────────────────────╮\x1b[0m\r\n",
  "\x1b[38;5;208m│\x1b[0m \x1b[1m✻ Welcome to Claude Code!\x1b[0m                     \x1b[38;5;208m│\x1b[0m\r\n",
  "\x1b[38;5;208m│\x1b[0m   /help for help, /status for your setup     \x1b[38;5;208m│\x1b[0m\r\n",
  "\x1b[38;5;208m│\x1b[0m   cwd: ~/Projects/demo                       \x1b[38;5;208m│\x1b[0m\r\n",
  "\x1b[38;5;208m╰──────────────────────────────────────────────╯\x1b[0m\r\n\r\n",
  "\x1b[2m> \x1b[0m帮我看看这个仓库最近改了什么\r\n\r\n",
  "\x1b[1m⏺\x1b[0m 我先看一下最近的提交记录。\r\n",
  "\x1b[2m  ⎿  a1b2c3d feat: 初始版本\x1b[0m\r\n",
].join("");

mockWindows("main");
mockIPC(
  (cmd, args: any) => {
    switch (cmd) {
      case "overview":
        return ov;
      case "connect":
        if (s === "error") return Promise.reject({ reject: "bad_password", message: "临时密码不正确，请向对方获取最新密码。" });
        return new Promise((r) =>
          setTimeout(
            () =>
              r({
                conn_id: 1,
                code: "123456789",
                info: { endpoint_id: "x", name: "工作室 Mac mini", platform: "macos", user: "me" },
                path: { kind: s === "relay" ? "relay" : "ipv6", rtt_ms: s === "relay" ? 180 : 12, remote: "[240e::1]:7788" },
              }),
            600,
          ),
        );
      case "list_terminals":
        return terminalSessions.map((session) => ({ ...session }));
      case "end_terminal": {
        const index = terminalSessions.findIndex((session) => session.id === args.sessionId);
        if (index >= 0) terminalSessions.splice(index, 1);
        for (const [id, attachment] of attachments) {
          if (attachment.sessionId === args.sessionId) { attachment.status.onmessage({ type: "exited", code: 0 }); attachments.delete(id); }
        }
        return null;
      }
      case "open_terminal": {
        let session = terminalSessions.find((session) => session.id === args.sessionId);
        if (!session && args.sessionId) return Promise.reject("会话已结束");
        if (!session) { session = { id: crypto.randomUUID().replaceAll("-", ""), created_at: Math.floor(Date.now() / 1000), attached: true }; terminalSessions.push(session); }
        session.attached = true;
        const termId = ++termSeq;
        attachments.set(termId, { sessionId: session.id, status: args.status });
        ((window as any).__outs ??= []).push(args.output);
        setTimeout(() => args.output.onmessage(new TextEncoder().encode(sample).buffer), 100);
        return { term_id: termId, session_id: session.id };
      }
      case "term_close": {
        const attachment = attachments.get(args.termId);
        attachments.delete(args.termId);
        const session = terminalSessions.find((s) => s.id === attachment?.sessionId);
        if (session) session.attached = [...attachments.values()].some((a) => a.sessionId === session.id);
        return null;
      }
      case "refresh_password":
        ov.password = ov.password === "ankab7" ? "h9km2p" : "ankab7";
        return ov.password;
      case "forget_recent":
        ov.recent = ov.recent.filter((device) => device.code !== args.code);
        return null;
      case "remove_trusted":
        ov.trusted = ov.trusted.filter((device) => device.endpoint_id !== args.endpointId);
        return null;
      case "kick":
        ov.sessions = ov.sessions.filter((session) => session.session !== args.session);
        return true;
      case "path_info":
        return { kind: s === "relay" ? "relay" : "ipv6", rtt_ms: s === "relay" ? 180 : 12, remote: "[240e::1]:7788" };
      case "get_settings":
        return { ...previewSettings };
      case "save_settings":
        previewSettings.server = args.server?.trim() || null;
        previewSettings.device_name = args.deviceName?.trim() || null;
        previewSettings.relay = args.relay?.trim() || null;
        previewSettings.proxy = args.proxy?.trim() || null;
        ov.name = previewSettings.device_name ?? "我的 MacBook Pro";
        return null;
      case "restart":
        return null;
      case "selftest_config":
        return null;
      case "diagnose":
        if (s === "error") return Promise.reject("网络检测暂时不可用，请稍后重试。");
        return new Promise((r) =>
          setTimeout(
            () =>
              r({
                rating: "差",
                summary: "没有 IPv6，IPv4 是对称 NAT：对方网络好时还能直连，否则走中继，延迟会高一些",
                ipv6_addrs: [],
                ipv4_public: "203.0.113.25",
                nat: "symmetric",
                nat_source: "relay",
                stun_error: "小米 STUN 服务器连不上，可能是路由器或代理拦了 UDP 3478",
                relay: "https://relay.example.com/",
                relay_connected: true,
                relay_latency_ms: 38,
                notes: [
                  "没有公网 IPv6，只能走 IPv4 打洞或中继",
                  "IPv4 是对称 NAT（经中继测得）：每个目标外网端口都不同；对方是锥形 NAT 时多半还能打通，对方也是对称 NAT 就只能走中继",
                  "STUN 检测没成功：小米 STUN 服务器连不上，可能是路由器或代理拦了 UDP 3478",
                  "中继已连上：https://relay.example.com/，延迟 38 毫秒",
                  "牵线服务器在线",
                ],
              }),
            800,
          ),
        );
      default:
        return null;
    }
  },
  { shouldMockEvents: true },
);

await import("./main");
if (s === "request") {
  setTimeout(() => emit("host-request", { id: 9, remote: "f9bb5fbc3f0a1b2c3d4e5f60718293a4", name: "小王的 ThinkPad", platform: "windows" }), 300);
}

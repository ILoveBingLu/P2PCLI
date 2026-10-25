// 开发用：浏览器里假数据渲染界面，看排版。不进正式包。
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type { Overview, SettingsView, TerminalSession, Transfer } from "./api";

const s = new URLSearchParams(location.search).get("s") ?? "empty";

const ov: Overview = {
  name: "我的 MacBook Pro",
  platform: "macos",
  endpoint_id: "459a2c0a8c093ad76c1b00d7ecfd6dce576f219cdc3c2907cdfc49b3b9ac9ca9",
  code: "246813579",
  password: "ankab7",
  password_fixed: false,
  server: { state: "online" },
  sessions:
    s === "busy"
      ? [{ session: 3, remote: "f9bb5fbc3f", name: "小王的 ThinkPad", platform: "windows", terminals: 1, since: 0 }]
      : [],
  recent: [
    { code: "123456789", endpoint_id: "x", name: "工作室 Mac mini", platform: "macos", last_used: 3 },
    { code: "987654321", endpoint_id: "y", name: "办公室 PC", platform: "windows", last_used: 2 },
  ],
  // 工作室 Mac mini 也在连接历史里，会标「局域网在线」；连客厅那台要密码
  nearby: [
    { endpoint_id: "x", name: "工作室 Mac mini", platform: "macos", online: true },
    { endpoint_id: "c".repeat(64), name: "客厅的 Windows", platform: "windows", online: true },
  ],
  trusted: [{ endpoint_id: "f9bb5fbc3f", name: "工作室 Mac mini", platform: "macos", added_at: 0 }],
  // 后台探测：工作室 Mac mini 在线、配对过，会话是实时的；办公室 PC 离线，只有记住的会话
  devices: [
    {
      endpoint_id: "x", state: "online", paired: true,
      path: { kind: "ipv6", rtt_ms: 12, remote: "[240e::1]:7788" },
      sessions: [], sessions_live: true, sessions_at: Math.floor(Date.now() / 1000),
    },
    {
      endpoint_id: "y", state: "offline", paired: null, path: null,
      sessions: [{ id: "d".repeat(32), created_at: Math.floor(Date.now() / 1000) - 26 * 3600, attached: false }],
      sessions_live: false, sessions_at: Math.floor(Date.now() / 1000) - 2 * 3600,
    },
  ],
  paired: [{ endpoint_id: "zz", name: "小王的 ThinkPad", platform: "windows", added_at: 0 }],
  inbox_dir: "/Users/me/Downloads/P2PCLI",
};

if (s === "unconfigured") {
  ov.server = { state: "none" };
  ov.code = null;
  ov.recent = [];
}
// s=lan-empty：局域网里没发现设备
if (s === "unconfigured" || s === "lan-empty") ov.nearby = [];
if (s === "offline") ov.server = { state: "offline", error: "无法连接牵线服务器" };
const previewSettings: SettingsView = {
  server: null, device_name: null, relay: null, proxy: null,
  effective_relay: null, effective_server: "wss://p2pcli.aiqji.cn",
  default_server: "wss://p2pcli.aiqji.cn",
  lan_visible: true,
  inbox_dir: "/Users/me/Downloads/P2PCLI",
  server_presets: [
    { address: "wss://p2pcli.aiqji.cn", label: "国内服务器", note: "国内节点，在国内用连得最快" },
    { address: "wss://p2pcli.yscopy.cn", label: "Cloudflare", note: "海外节点，适合在国外用，国内访问有时不稳" },
  ],
};
// s=custom-server：设置里填过自定义牵线服务器
if (s === "custom-server") previewSettings.server = "b840307fdbdff2aa164308932575e8f7396b7fd539bd7cfd1927b2456b3fc8a8@rdv.example.com:7788";
let termSeq = 0;
let scanSeq = 0;
let pathSamples = 0;
// s=latency: probe and terminal deliberately disagree; connected views must share updates.
if (s === "latency") ov.devices[0].path!.rtt_ms = 99;
// s=signal: 连接历史里各档延迟都有一台，看信号格
if (s === "signal") {
  const at = Math.floor(Date.now() / 1000);
  const extra = [
    { id: "s1", name: "家里的 iMac", platform: "macos", kind: "ipv4", rtt: 99 },
    { id: "s2", name: "公司服务器", platform: "linux", kind: "relay", rtt: 180 },
    { id: "s3", name: "海外 VPS", platform: "linux", kind: "relay", rtt: 420 },
  ] as const;
  for (const [i, d] of extra.entries()) {
    ov.recent.push({ code: String(i + 1).repeat(9), endpoint_id: d.id, name: d.name, platform: d.platform, last_used: 1 - i });
    ov.devices.push({
      endpoint_id: d.id, state: "online", paired: true, path: { kind: d.kind, rtt_ms: d.rtt, remote: null },
      sessions: [], sessions_live: true, sessions_at: at,
    });
  }
}
let statusStartedAt = 0;
const now = Math.floor(Date.now() / 1000);
const terminalSessions: TerminalSession[] = s === "sessions" ? [
  { id: "a".repeat(32), created_at: now - 3 * 3600, attached: false },
  { id: "b".repeat(32), created_at: now - 40 * 60, attached: true },
  { id: "c".repeat(32), created_at: now - 90, attached: false },
] : [];
// 工作室 Mac mini 的后台探测结果用同一份会话，连上前后编号对得上
ov.devices[0].sessions = terminalSessions.map((session) => ({ ...session }));
const attachments = new Map<number, { sessionId: string; status: any }>();

// 传文件：s=transfers 一开始就有几条在传、传完、失败的；发文件时模拟进度
const transfers: Transfer[] = [];
const transferBase = { files: 1, error: null, dir: null, paths: [] as string[], started_at: now, finished_at: null as number | null };
if (s === "transfers") {
  transfers.push(
    { ...transferBase, id: "t1", direction: "send", peer: "x", peer_name: "工作室 Mac mini", label: "设计稿 等 3 项", files: 12, total: 420_000_000, done: 168_000_000, state: "running" },
    { ...transferBase, id: "t2", direction: "receive", peer: "y", peer_name: "办公室 PC", label: "报告.pdf", total: 2_400_000, done: 2_400_000, state: "done", finished_at: now + 3600, paths: ["/Users/me/Downloads/P2PCLI/办公室 PC/报告.pdf"] },
    { ...transferBase, id: "t3", direction: "send", peer: "y", peer_name: "办公室 PC", label: "素材.zip", total: 80_000_000, done: 0, state: "failed", finished_at: now + 3600, error: "30 秒内没连上对方，对方可能不在线" },
  );
}
const emitTransfers = () => void emit("transfers", transfers.map((t) => ({ ...t })));
function fakeSend(target: string, paths: string[]) {
  const id = `t${Date.now()}`;
  const name = paths[0]?.split("/").pop() ?? "文件";
  const peer = ov.recent.find((d) => d.endpoint_id === target)?.name ?? "设备";
  const t: Transfer = { ...transferBase, id, direction: "send", peer: target, peer_name: peer, label: paths.length > 1 ? `${name} 等 ${paths.length} 项` : name, total: 30_000_000, done: 0, state: "connecting", started_at: Math.floor(Date.now() / 1000) };
  transfers.unshift(t);
  let step = 0;
  const timer = setInterval(() => {
    step += 1;
    if (step === 1) t.state = "running";
    else t.done = Math.min(t.total, t.done + 6_000_000);
    if (t.done >= t.total) {
      t.state = "done";
      t.finished_at = Math.floor(Date.now() / 1000);
      t.paths = paths.map((p) => `/Users/me/Downloads/P2PCLI/我的 MacBook Pro/${p.split("/").pop()}`);
      clearInterval(timer);
    }
    emitTransfers();
  }, 500);
  emitTransfers();
  return id;
}

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
  async (cmd, args: any) => {
    switch (cmd) {
      case "overview":
        return ov;
      case "scan_nearby":
        return new Promise((resolve, reject) => setTimeout(() => {
          if (s === "lan-scan-error") return reject("局域网扫描启动失败");
          if (s === "lan-refresh") {
            scanSeq += 1;
            ov.nearby = ov.nearby.map((device, index) => ({ ...device, online: index !== 1 || scanSeq % 2 === 0 }));
            if (scanSeq === 1) ov.nearby.push({ endpoint_id: "d".repeat(64), name: "新发现的 Mac", platform: "macos", online: true });
          }
          resolve(ov.nearby);
        }, 3000));
      case "connect": {
        if (s === "error") return Promise.reject({ reject: "bad_password", message: "配对密码不正确，请向对方获取最新密码。" });
        const lan = ov.nearby.find((d) => d.endpoint_id === args.target);
        if (lan && lan.endpoint_id !== "x" && !args.password)
          return Promise.reject({ reject: "need_password", message: "需要输入配对密码" });
        return new Promise((r) =>
          setTimeout(
            () =>
              r({
                conn_id: 1,
                code: lan ? null : "123456789",
                info: { endpoint_id: lan?.endpoint_id ?? "x", name: lan?.name ?? "工作室 Mac mini", platform: lan?.platform ?? "macos", user: "me" },
                path: { kind: s === "relay" ? "relay" : "ipv6", rtt_ms: s === "relay" ? 180 : 12, remote: "[240e::1]:7788" },
              }),
            600,
          ),
        );
      }
      case "list_terminals": {
        // s=status-live: change metadata, fail briefly, then recover without a reload.
        const elapsed = statusStartedAt ? Date.now() - statusStartedAt : 0;
        if (s === "status-live" && elapsed >= 12000 && elapsed < 18000)
          return Promise.reject("模拟状态查询暂时失败");
        const changed = s === "status-live" && elapsed >= 6000;
        // 模拟网络往返，看得出刷新时列表有没有闪
        return new Promise((r) => setTimeout(() => r(terminalSessions.map((session) => ({
          ...session,
          details: changed
            ? { window_index: 1, window_name: "编辑器", window_count: 2, command: "vim", current_path: "/Users/demo/Projects/updated" }
            : { window_index: 0, window_name: "工作终端", window_count: 1, command: "zsh", current_path: "/Users/demo/Projects/P2PCLI" },
        }))), 300));
      }
      case "end_terminal": {
        await new Promise((r) => setTimeout(r, 500));
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
        if (!statusStartedAt) statusStartedAt = Date.now();
        attachments.set(termId, { sessionId: session.id, status: args.status });
        ((window as any).__outs ??= []).push(args.output);
        setTimeout(() => args.output.onmessage(new TextEncoder().encode(sample).buffer), 100);
        // Reproduce disconnected overlays alongside another mounted terminal tab.
        if (s === "disconnected" && termId === 1) {
          setTimeout(() => args.status.onmessage({ type: "closed", reason: "网络中断，连接超时" }), 1500);
        }
        return { term_id: termId, session_id: session.id };
      }
      case "term_close": {
        const attachment = attachments.get(args.termId);
        attachments.delete(args.termId);
        const session = terminalSessions.find((s) => s.id === attachment?.sessionId);
        if (session) session.attached = [...attachments.values()].some((a) => a.sessionId === session.id);
        return null;
      }
      case "plugin:app|version":
        return "0.1.0";
      // s=update：发布页上有新版本
      case "check_update":
        return s === "update"
          ? { version: "0.1.1", current_version: "0.1.0", notes: "局域网发现更稳定\n修复连接面板偶尔不弹出" }
          : null;
      case "install_update":
        return new Promise(() => {});
      case "set_password_fixed":
        ov.password_fixed = !!args.fixed;
        return null;
      case "refresh_password":
        ov.password = ov.password === "ankab7" ? "h9km2p" : "ankab7";
        return ov.password;
      case "forget_recent":
        ov.recent = ov.recent.filter((device) => device.code !== args.code);
        return null;
      case "refresh_devices":
        return null;
      case "remove_paired":
        ov.paired = ov.paired.filter((device) => device.endpoint_id !== args.endpointId);
        return null;
      case "remove_trusted":
        ov.trusted = ov.trusted.filter((device) => device.endpoint_id !== args.endpointId);
        return null;
      case "kick":
        ov.sessions = ov.sessions.filter((session) => session.session !== args.session);
        return true;
      case "path_info":
        if (s === "latency") return { kind: "ipv6", rtt_ms: ++pathSamples === 1 ? 20 : 35, remote: "[240e::1]:7788" };
        return { kind: s === "relay" ? "relay" : "ipv6", rtt_ms: s === "relay" ? 180 : 12, remote: "[240e::1]:7788" };
      case "get_settings":
        return { ...previewSettings };
      case "save_settings":
        previewSettings.server = args.server?.trim() || null;
        previewSettings.device_name = args.deviceName?.trim() || null;
        previewSettings.relay = args.relay?.trim() || null;
        previewSettings.proxy = args.proxy?.trim() || null;
        previewSettings.lan_visible = args.lanVisible ?? true;
        ov.name = previewSettings.device_name ?? "我的 MacBook Pro";
        return null;
      case "restart":
        return null;
      case "transfers":
        return transfers.map((t) => ({ ...t }));
      case "pick_paths":
        return args.folders ? ["/Users/me/Projects/demo"] : ["/Users/me/Desktop/截图 1.png", "/Users/me/Desktop/notes.md"];
      case "send_files":
        return fakeSend(args.target, args.paths);
      case "respond_files":
        return null;
      case "cli_status":
        return s === "cli-missing"
          ? { path: "/Users/me/.local/bin/p2pcli", installed: false, on_path: false, bundled: true }
          : { path: "/Users/me/.local/bin/p2pcli", installed: true, on_path: true, bundled: true };
      case "install_cli":
        return { path: "/Users/me/.local/bin/p2pcli", installed: true, on_path: true, bundled: true };
      case "term_input":
        ((window as any).__inputs ??= []).push(args.data);
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
// 测拖文件这类系统事件：页面里调 window.__emit("tauri://drag-drop", { paths, position })
(window as any).__emit = emit;
if (s === "file-request") {
  setTimeout(() => emit("file-request", { id: 11, remote: "f9bb5fbc3f0a1b2c3d4e5f60718293a4", name: "客厅的 Windows", platform: "windows", files: 3, total: 12_582_912, items: ["照片", "说明.txt"] }), 300);
}
if (s === "request") {
  setTimeout(() => emit("host-request", { id: 9, remote: "f9bb5fbc3f0a1b2c3d4e5f60718293a4", name: "小王的 ThinkPad", platform: "windows" }), 300);
}

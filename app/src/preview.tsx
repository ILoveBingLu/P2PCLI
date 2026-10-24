// 开发用：浏览器里假数据渲染界面，看排版。不进正式包。
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type { Overview } from "./api";

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
      case "open_terminal":
        ((window as any).__outs ??= []).push(args.output);
        setTimeout(() => args.output.onmessage(new TextEncoder().encode(sample).buffer), 100);
        return 7;
      case "path_info":
        return { kind: s === "relay" ? "relay" : "ipv6", rtt_ms: s === "relay" ? 180 : 12, remote: "[240e::1]:7788" };
      case "get_settings":
        return { server: null, device_name: null, relay: null, effective_relay: null, effective_server: "<服务器公钥>@203.0.113.10:7788", data_dir: "~/Library/Application Support/io.github.ilovebinglu.p2pcli" };
      case "selftest_config":
        return null;
      case "diagnose":
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

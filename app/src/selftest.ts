// 调试版自测：按 P2PCLI_SELFTEST 指向的配置，走真界面把连接、同意、跑命令走一遍。
// 正式版 selftest_config 永远返回空，这里什么都不做。
import { invoke } from "@tauri-apps/api/core";
import type { Terminal } from "@xterm/xterm";
import { api, type IncomingRequest, type Overview } from "./api";

interface HostCfg {
  role: "host";
  report: string;
  /** 自动点允许还是拒绝 */
  approve: boolean;
  trust?: boolean;
}

interface GuestCfg {
  role: "guest";
  report: string;
  target: string;
  password?: string;
  command: string;
  expect: string;
}

type Cfg = HostCfg | GuestCfg;

interface Hooks {
  terms: Terminal[];
  connect?: (target: string, password: string) => Promise<void>;
  onRequest?: (req: IncomingRequest) => void;
  respond?: (req: IncomingRequest, allow: boolean, trust: boolean) => void;
}

declare global {
  interface Window {
    __ptTest?: Hooks;
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function waitFor<T>(what: string, fn: () => T | undefined | null | false, ms: number): Promise<T> {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const v = fn();
    if (v) return v;
    await sleep(200);
  }
  throw new Error(`等 ${what} 超时`);
}

function screenText(t: Terminal): string {
  const b = t.buffer.active;
  const lines: string[] = [];
  for (let i = 0; i < b.length; i++) lines.push(b.getLine(i)?.translateToString(true) ?? "");
  return lines.join("\n");
}

export async function startSelftest() {
  const cfg = await invoke<Cfg | null>("selftest_config");
  if (!cfg) return;
  const hooks: Hooks = { terms: [] };
  window.__ptTest = hooks;
  const log: string[] = [];
  const started = Date.now();
  const note = (m: string) => log.push(`${((Date.now() - started) / 1000).toFixed(1)}s ${m}`);
  const report = (extra: Record<string, unknown>) =>
    invoke("selftest_report", { path: cfg.report, report: { role: cfg.role, log, ...extra } });

  try {
    const ov = await waitFor<Overview>("牵线服务器登记", () => lastOverview?.code ? lastOverview : null, 30000);
    note(`本机设备码 ${ov.code}，密码 ${ov.password}`);
    if (cfg.role === "host") {
      await report({ ready: true, code: ov.code, password: ov.password });
      hooks.onRequest = (req) => {
        note(`收到请求 ${req.name}（${req.platform}）`);
        setTimeout(() => {
          hooks.respond?.(req, cfg.approve, cfg.trust ?? false);
          note(cfg.approve ? "已点允许" : "已点拒绝");
          report({ ready: true, code: ov.code, password: ov.password, handled: true });
        }, 1500);
      };
      return;
    }
    await waitFor("界面就绪", () => hooks.connect, 10000);
    note(`连接 ${cfg.target}`);
    await hooks.connect!(cfg.target, cfg.password ?? "");
    const term = await waitFor("终端打开", () => hooks.terms[0], 30000);
    // 等提示符出来再输入
    await waitFor("提示符", () => screenText(term).trim().length > 0, 20000);
    note("终端已有输出");
    term.input(`${cfg.command}\r`, true);
    await waitFor(`输出里出现 ${cfg.expect}`, () => screenText(term).includes(cfg.expect), 30000);
    note("命令结果已显示");
    const ov2 = await api.overview();
    await report({ ok: true, screen: screenText(term).trimEnd().split("\n").slice(-15), recent: ov2.recent });
  } catch (e) {
    note(`失败：${e}`);
    const screen = hooks.terms[0] ? screenText(hooks.terms[0]).trimEnd().split("\n").slice(-15) : [];
    await report({ ok: false, error: String(e), screen });
  }
}

let lastOverview: Overview | null = null;
export function selftestSawOverview(ov: Overview) {
  lastOverview = ov;
}

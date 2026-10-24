// P2PCLI 牵线服务的 Cloudflare Workers 版（WebSocket）。
// 协议和 Rust 版完全一样，见 crates/core/src/rdv_ws.rs：
// - 连接时请求头 x-p2pcli-auth 带 `设备公钥.时间戳.签名`，签的是 `p2pcli-rdv-ws-v1|主机名|设备公钥|时间戳`
// - 设备发 {"id":1,"req":{...}}，回 {"id":1,"resp":{...}}，主动推送 {"push":{...}}
// 所有设备连到同一个 Durable Object，设备码存在它自带的 SQLite 里。终端数据不经过这里。

import { DurableObject } from "cloudflare:workers";

interface Env {
  RDV: DurableObjectNamespace<Rendezvous>;
}

const WS_PATH = "/v1/ws";
const AUTH_HEADER = "x-p2pcli-auth";
/** 鉴权时间戳允许的偏差（秒） */
const AUTH_MAX_SKEW = 300;
/** 每台设备每分钟最多查几次设备码（含请求发包），防被人挨个试 */
const LOOKUPS_PER_MINUTE = 20;
/** 转给被连方的地址最多几个 */
const MAX_PUNCH_ADDRS = 6;

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);
    if (url.pathname === WS_PATH) {
      if (req.headers.get("upgrade")?.toLowerCase() !== "websocket") {
        return new Response("这里只接 WebSocket\n", { status: 426 });
      }
      return env.RDV.get(env.RDV.idFromName("global")).fetch(req);
    }
    if (url.pathname === "/") {
      return new Response("P2PCLI 牵线服务\n", { headers: { "content-type": "text/plain; charset=utf-8" } });
    }
    return new Response("not found\n", { status: 404 });
  },
} satisfies ExportedHandler<Env>;

function hexToBytes(s: string): Uint8Array {
  const out = new Uint8Array(s.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(s.slice(i * 2, i * 2 + 2), 16);
  return out;
}

/** 验鉴权头，成功返回设备公钥（64 位小写十六进制） */
async function verifyAuth(header: string, host: string, now: number): Promise<string> {
  const parts = header.trim().split(".");
  if (parts.length !== 3) throw new Error("鉴权头格式不对");
  const [id, tsText, sig] = parts;
  if (!/^[0-9a-f]{64}$/.test(id)) throw new Error("设备公钥格式不对");
  if (!/^[0-9]+$/.test(tsText)) throw new Error("时间戳格式不对");
  const ts = Number(tsText);
  if (Math.abs(now - ts) > AUTH_MAX_SKEW) throw new Error("时间戳过期，检查两边的系统时间");
  if (!/^[0-9a-f]{128}$/.test(sig)) throw new Error("签名格式不对");
  const key = await crypto.subtle.importKey("raw", hexToBytes(id), { name: "Ed25519" }, false, ["verify"]);
  const msg = new TextEncoder().encode(`p2pcli-rdv-ws-v1|${host.toLowerCase()}|${id}|${ts}`);
  if (!(await crypto.subtle.verify({ name: "Ed25519" }, key, hexToBytes(sig), msg))) throw new Error("签名不对");
  return id;
}

/** 设备码统一成 9 位数字，用户输入时的空格、横杠都去掉 */
function normalizeCode(input: unknown): string | null {
  if (typeof input !== "string" || !/^[0-9\s-]+$/.test(input)) return null;
  const digits = input.replace(/\D/g, "");
  return digits.length === 9 ? digits : null;
}

/** 对方报的名字要显示在别人的弹窗里，去掉控制字符并限长 */
function clean(s: unknown, max: number): string {
  return (typeof s === "string" ? s : "").replace(/\p{Cc}/gu, "").slice(0, max);
}

/** 每条 WebSocket 上记的东西，Durable Object 休眠后也还在 */
interface Attachment {
  id: string;
  registered: boolean;
  addr: unknown;
  lookups: number[];
}

type Resp =
  | { t: "registered"; code: string }
  | { t: "ok" }
  | { t: "found"; endpoint_id: string; name: string; platform: string; online: boolean; addr: unknown }
  | { t: "not_found" }
  | { t: "error"; message: string };

export class Rendezvous extends DurableObject<Env> {
  private sql: SqlStorage;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.sql = ctx.storage.sql;
    this.sql.exec(`CREATE TABLE IF NOT EXISTS devices (
      endpoint_id TEXT PRIMARY KEY,
      code TEXT NOT NULL UNIQUE,
      name TEXT NOT NULL,
      platform TEXT NOT NULL,
      created_at INTEGER NOT NULL,
      last_seen INTEGER NOT NULL
    )`);
  }

  async fetch(req: Request): Promise<Response> {
    let id: string;
    try {
      id = await verifyAuth(req.headers.get(AUTH_HEADER) ?? "", new URL(req.url).host, Math.floor(Date.now() / 1000));
    } catch (e) {
      return new Response(`${(e as Error).message}\n`, { status: 401 });
    }
    const pair = new WebSocketPair();
    const [client, server] = Object.values(pair);
    // 用设备公钥打标签，之后按公钥找它的连接
    this.ctx.acceptWebSocket(server, [id]);
    server.serializeAttachment({ id, registered: false, addr: null, lookups: [] } satisfies Attachment);
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    if (typeof message !== "string") return;
    let frame: { id?: unknown; req?: Record<string, unknown> };
    try {
      frame = JSON.parse(message);
    } catch {
      return;
    }
    if (typeof frame.id !== "number" || !frame.req || typeof frame.req !== "object") return;
    let resp: Resp;
    try {
      resp = this.handle(ws, frame.req);
    } catch (e) {
      console.error("处理请求出错", e);
      resp = { t: "error", message: "服务器出错" };
    }
    ws.send(JSON.stringify({ id: frame.id, resp }));
  }

  async webSocketClose(ws: WebSocket, code: number, reason: string): Promise<void> {
    ws.close(code, reason);
  }

  /** 这台设备当前登记着的连接；没有就是不在线 */
  private online(id: string): WebSocket | undefined {
    return this.ctx.getWebSockets(id).find((w) => (w.deserializeAttachment() as Attachment).registered);
  }

  private allowLookup(ws: WebSocket, att: Attachment): boolean {
    const now = Date.now();
    att.lookups = att.lookups.filter((t) => now - t < 60_000);
    if (att.lookups.length >= LOOKUPS_PER_MINUTE) return false;
    att.lookups.push(now);
    ws.serializeAttachment(att);
    return true;
  }

  private handle(ws: WebSocket, req: Record<string, unknown>): Resp {
    const att = ws.deserializeAttachment() as Attachment;
    const now = Math.floor(Date.now() / 1000);
    switch (req.t) {
      case "register": {
        const name = clean(req.name, 48);
        const platform = clean(req.platform, 16);
        const row = this.sql.exec<{ code: string }>("SELECT code FROM devices WHERE endpoint_id = ?", att.id).toArray()[0];
        let code = row?.code;
        if (code) {
          this.sql.exec("UPDATE devices SET name = ?, platform = ?, last_seen = ? WHERE endpoint_id = ?", name, platform, now, att.id);
        } else {
          for (let i = 0; i < 20 && !code; i++) {
            const candidate = String(100_000_000 + Math.floor(Math.random() * 900_000_000));
            const r = this.sql.exec(
              "INSERT OR IGNORE INTO devices (endpoint_id, code, name, platform, created_at, last_seen) VALUES (?, ?, ?, ?, ?, ?)",
              att.id, candidate, name, platform, now, now,
            );
            if (r.rowsWritten === 1) code = candidate;
          }
          if (!code) return { t: "error", message: "发设备码失败" };
        }
        // 同一台设备重连了，旧的登记连接关掉；没登记的是临时查询用的，别动
        for (const other of this.ctx.getWebSockets(att.id)) {
          if (other !== ws && (other.deserializeAttachment() as Attachment).registered) {
            other.close(4000, "这台设备在别处重新登记了");
          }
        }
        att.registered = true;
        ws.serializeAttachment(att);
        return { t: "registered", code };
      }
      case "addr": {
        const addr = req.addr as { id?: unknown } | undefined;
        if (!addr || addr.id !== att.id) return { t: "error", message: "只能报告自己的地址" };
        if (!att.registered) return { t: "error", message: "请先登记" };
        att.addr = addr;
        ws.serializeAttachment(att);
        this.sql.exec("UPDATE devices SET last_seen = ? WHERE endpoint_id = ?", now, att.id);
        return { t: "ok" };
      }
      case "lookup": {
        if (!this.allowLookup(ws, att)) return { t: "error", message: "查询太频繁，请稍后再试" };
        const code = normalizeCode(req.code);
        if (!code) return { t: "not_found" };
        const row = this.sql
          .exec<{ endpoint_id: string; name: string; platform: string }>(
            "SELECT endpoint_id, name, platform FROM devices WHERE code = ?",
            code,
          )
          .toArray()[0];
        if (!row) return { t: "not_found" };
        const target = this.online(row.endpoint_id);
        const addr = target ? (target.deserializeAttachment() as Attachment).addr : null;
        return { t: "found", endpoint_id: row.endpoint_id, name: row.name, platform: row.platform, online: !!target, addr };
      }
      case "punch": {
        if (!this.allowLookup(ws, att)) return { t: "error", message: "查询太频繁，请稍后再试" };
        const code = normalizeCode(req.code);
        if (!code) return { t: "not_found" };
        const row = this.sql.exec<{ endpoint_id: string }>("SELECT endpoint_id FROM devices WHERE code = ?", code).toArray()[0];
        if (!row) return { t: "not_found" };
        const target = this.online(row.endpoint_id);
        if (!target) return { t: "error", message: "对方不在线" };
        // 只转 IPv6 地址，Rust 那边写成 "[地址]:端口"
        const addrs = (Array.isArray(req.addrs) ? req.addrs : [])
          .filter((a): a is string => typeof a === "string" && a.startsWith("["))
          .slice(0, MAX_PUNCH_ADDRS);
        try {
          target.send(JSON.stringify({ push: { t: "punch", from: att.id, addrs } }));
        } catch (e) {
          console.warn("转达发包请求失败", e);
        }
        return { t: "ok" };
      }
      default:
        return { t: "error", message: "看不懂的请求" };
    }
  }
}

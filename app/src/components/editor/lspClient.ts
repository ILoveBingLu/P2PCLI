// 和语言服务说话的 JSON-RPC：发请求、收回复、处理它反过来问的问题。不碰 Monaco，方便单独测。

/** 和 Monaco 的 CancellationToken 一样的形状 */
export interface CancelToken {
  isCancellationRequested: boolean;
  onCancellationRequested(listener: () => void): { dispose(): void };
}

interface Pending {
  resolve: (value: any) => void;
  reject: (error: Error) => void;
}

type RequestHandler = (params: any) => unknown;
type NotificationHandler = (params: any) => void;

/** 请求被取消或内容已过时：不算出错，当成没有结果 */
const QUIET_ERRORS = new Set([-32800, -32801, -32802]);

export class JsonRpc {
  private seq = 0;
  private pending = new Map<number, Pending>();
  private requests = new Map<string, RequestHandler>();
  private notifications = new Map<string, NotificationHandler>();
  private outbox: string[] = [];
  private flushing = false;
  closed = false;

  /** 一次发一批，等它完成再发下一批，保证顺序 */
  private send: (messages: string[]) => Promise<void>;

  constructor(send: (messages: string[]) => Promise<void>) {
    this.send = send;
  }

  onRequest(method: string, handler: RequestHandler) { this.requests.set(method, handler); }
  onNotification(method: string, handler: NotificationHandler) { this.notifications.set(method, handler); }

  request<T = any>(method: string, params?: unknown, token?: CancelToken): Promise<T | null> {
    if (this.closed) return Promise.reject(new Error("语言服务已经关了"));
    const id = ++this.seq;
    return new Promise<T | null>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.post({ jsonrpc: "2.0", id, method, params });
      if (!token) return;
      if (token.isCancellationRequested) return this.cancel(id);
      const sub = token.onCancellationRequested(() => this.cancel(id));
      const done = () => sub.dispose();
      this.pending.set(id, { resolve: (v) => { done(); resolve(v); }, reject: (e) => { done(); reject(e); } });
    });
  }

  notify(method: string, params?: unknown) {
    if (!this.closed) this.post({ jsonrpc: "2.0", method, params });
  }

  private cancel(id: number) {
    const pending = this.pending.get(id);
    if (!pending) return;
    this.pending.delete(id);
    this.notify("$/cancelRequest", { id });
    pending.resolve(null);
  }

  private post(message: object) {
    this.outbox.push(JSON.stringify(message));
    void this.flush();
  }

  private async flush() {
    if (this.flushing) return;
    this.flushing = true;
    try {
      while (this.outbox.length && !this.closed) {
        const batch = this.outbox;
        this.outbox = [];
        await this.send(batch).catch(() => {});
      }
    } finally {
      this.flushing = false;
    }
  }

  /** 语言服务发来的一条消息 */
  receive(text: string) {
    let message: any;
    try { message = JSON.parse(text); } catch { return; }
    if (message.method && message.id !== undefined) return void this.answer(message);
    if (message.method) return void this.notifications.get(message.method)?.(message.params);
    const pending = this.pending.get(message.id);
    if (!pending) return;
    this.pending.delete(message.id);
    if (message.error) {
      if (QUIET_ERRORS.has(message.error.code)) pending.resolve(null);
      else pending.reject(new Error(message.error.message ?? String(message.error.code)));
    } else {
      pending.resolve(message.result ?? null);
    }
  }

  private async answer(message: { id: number | string; method: string; params: unknown }) {
    const handler = this.requests.get(message.method);
    if (!handler) {
      return this.post({ jsonrpc: "2.0", id: message.id, error: { code: -32601, message: `Unhandled method ${message.method}` } });
    }
    try {
      this.post({ jsonrpc: "2.0", id: message.id, result: (await handler(message.params)) ?? null });
    } catch (e) {
      this.post({ jsonrpc: "2.0", id: message.id, error: { code: -32603, message: String(e) } });
    }
  }

  /** 语言服务没了：还在等回复的请求都失败 */
  close(reason: string) {
    if (this.closed) return;
    this.closed = true;
    this.outbox = [];
    for (const pending of this.pending.values()) pending.reject(new Error(reason));
    this.pending.clear();
  }
}

/** 对方电脑上的路径 → file: 地址。Windows 路径写成 file:///C:/a/b */
export function pathToUri(path: string): string {
  const windows = /^[A-Za-z]:[\\/]/.test(path);
  const parts = (windows ? path.replace(/\\/g, "/") : path).split("/");
  const encoded = parts.map((part, i) => (windows && i === 0 ? part : encodeURIComponent(part))).join("/");
  return windows ? "file:///" + encoded : "file://" + encoded;
}

/** file: 地址 → 对方电脑上的路径；不是 file: 地址时返回 null */
export function uriToPath(uri: string): string | null {
  if (!uri.startsWith("file://")) return null;
  let path = uri.slice("file://".length);
  // file://host/share 这种网络路径不管
  if (!path.startsWith("/")) return null;
  try { path = decodeURIComponent(path); } catch { return null; }
  if (/^\/[A-Za-z]:\//.test(path)) return path.slice(1).replace(/\//g, "\\");
  return path;
}

/** 比较路径用：Windows 不分大小写 */
export function pathKey(path: string): string {
  return /^[A-Za-z]:\\/.test(path) ? path.toLowerCase() : path;
}

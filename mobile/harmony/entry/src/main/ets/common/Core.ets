import { common } from '@kit.AbilityKit';
import { deviceInfo } from '@kit.BasicServicesKit';
import { promptAction } from '@kit.ArkUI';
import { webview } from '@kit.ArkWeb';
import { pasteboard } from '@kit.BasicServicesKit';
import { fileIo as fs } from '@kit.CoreFileKit';
import p2pcli from 'libp2pcli.so';
import { P2p, Terminal } from 'libp2pcli.so';
import {
  AppEvent, Connected, DeviceView, FileRequestView, MobileConfig, MobileSettings, Overview, PairInfo, parseError,
  SessionView, SettingsView, TransferView
} from './Model';
import { t, tr } from './I18n';

/** 一台设备的会话列表；list 为空是还没读到 */
export class SessionsState {
  list: SessionView[] | null = null;
  error: string | null = null;

  constructor(list: SessionView[] | null, error: string | null) {
    this.list = list;
    this.error = error;
  }
}

/** 连接面板：新连一台设备，或者连某台设备时要填密码 */
@ObservedV2
export class ConnectPrompt {
  /** 已知是哪台设备（从列表里点的）；手输设备码时是空 */
  endpointId: string | null = null;
  name: string | null = null;
  /** 已知的设备码或公钥；手输时用户自己填 */
  target: string = '';
  @Trace error: string | null = null;
}

/** 终端输出最多留多少给重新打开的终端页回放 */
const HISTORY_LIMIT = 512 * 1024;

/**
 * 手机上开着的一个远程终端。画面在终端页的 ArkWeb 里（xterm.js），离开终端页再回来时把攒下的输出重放一遍。
 * 终端第一次知道自己有多大（几列几行）时才真正打开
 */
@ObservedV2
export class TermModel {
  readonly key: number;
  readonly endpointId: string;
  readonly connId: number;
  @Trace sessionId: string | null;
  /** opening | open | closed */
  @Trace state: string = 'opening';
  @Trace note: string | null = null;
  /** 程序退出（终端已结束），不是连接断了 */
  @Trace exited: boolean = false;
  /** 按键条上的 Ctrl、Alt：按一下，下一个键带上它 */
  @Trace ctrl: boolean = false;
  @Trace alt: boolean = false;
  handle?: Terminal;
  port?: webview.WebMessagePort;
  private history: ArrayBuffer[] = [];
  private historySize: number = 0;
  private opening: boolean = false;
  private opener: (cols: number, rows: number, term: TermModel) => Promise<Terminal>;

  constructor(key: number, endpointId: string, connId: number, sessionId: string | null,
    opener: (cols: number, rows: number, term: TermModel) => Promise<Terminal>) {
    this.key = key;
    this.endpointId = endpointId;
    this.connId = connId;
    this.sessionId = sessionId;
    this.opener = opener;
  }

  /** 终端页的网页量好了大小：第一次就打开终端，之后改尺寸 */
  onSize(cols: number, rows: number): void {
    if (this.handle !== undefined) {
      this.handle.resize(cols, rows);
      return;
    }
    if (this.opening || this.state !== 'opening') {
      return;
    }
    this.opening = true;
    this.opener(cols, rows, this).then((h: Terminal) => {
      this.handle = h;
      this.sessionId = h.sessionId();
      this.state = 'open';
    }).catch((e: Error) => {
      this.state = 'closed';
      this.note = tr(parseError(e).detail);
    });
  }

  /** 网页换了（重新进终端页）：接上新的消息端口，把之前的输出重放进去 */
  attach(port: webview.WebMessagePort): void {
    this.port = port;
    this.history.forEach((b: ArrayBuffer) => port.postMessageEvent(b));
  }

  detachView(): void {
    this.port = undefined;
  }

  output(data: Uint8Array): void {
    const b = data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength);
    this.history.push(b);
    this.historySize += b.byteLength;
    while (this.historySize > HISTORY_LIMIT && this.history.length > 1) {
      this.historySize -= this.history[0].byteLength;
      this.history.shift();
    }
    if (this.port !== undefined) {
      this.port.postMessageEvent(b);
    }
  }

  closed(info: string): void {
    const v = JSON.parse(info) as ClosedInfo;
    this.handle = undefined;
    this.exited = v.reason === null || v.reason === undefined;
    this.state = 'closed';
    if (v.reason !== null && v.reason !== undefined) {
      this.note = tr(v.reason);
    } else if (v.exit_code !== null && v.exit_code !== undefined) {
      this.note = t('终端视图已结束（退出码 {{code}}）', { code: v.exit_code } as Record<string, string | number>);
    } else {
      this.note = t('终端视图已结束');
    }
  }

  /** 键盘打的字：带上按键条上按下的 Ctrl、Alt 再发出去 */
  input(d: string): void {
    let text = d;
    // 只对打的字生效：终端自己的应答（ESC 开头）不动
    const typed = text.length > 0 && text.length <= 4 && text.charCodeAt(0) !== 0x1b;
    if (typed && this.ctrl) {
      this.ctrl = false;
      if (text.length === 1) {
        const c = text.toUpperCase().charCodeAt(0);
        if (c >= 0x40 && c <= 0x5f) {
          text = String.fromCharCode(c & 0x1f);
        } else if (c === 0x20) {
          text = String.fromCharCode(0);
        }
      }
    }
    if (typed && this.alt) {
      this.alt = false;
      text = '\u001b' + text;
    }
    this.send(text);
  }

  send(text: string): void {
    if (this.handle !== undefined) {
      this.handle.writeText(text);
    }
  }

  /** 给网页发命令：粘贴、方向键、复制、聚焦 */
  command(cmd: TermCommand): void {
    if (this.port !== undefined) {
      this.port.postMessageEvent(JSON.stringify(cmd));
    }
  }

  close(): void {
    if (this.handle !== undefined) {
      this.handle.detach();
    }
    this.handle = undefined;
  }
}

interface ClosedInfo {
  exit_code?: number | null;
  reason?: string | null;
}

export interface TermCommand {
  cmd: string;
  text?: string;
  dir?: string;
  size?: number;
}

/** 整个 App 的状态和操作。Rust 那边的接口都在这里调；界面只读这里的状态、调这里的方法 */
@ObservedV2
export class Core {
  static readonly shared: Core = new Core();
  ctx?: common.UIAbilityContext;
  p2p?: P2p;
  /** 页面：首页是 Navigation 的主页，设备、终端、设置一层层推进去 */
  readonly stack: NavPathStack = new NavPathStack();
  /** starting | ready | failed */
  @Trace startState: string = 'starting';
  @Trace startError: string = '';
  @Trace overview: Overview | null = null;
  @Trace transfers: TransferView[] = [];
  /** 连着的设备：公钥 → 连接 */
  @Trace conns: Map<string, Connected> = new Map();
  /** 各设备的会话列表：公钥 → 列表 */
  @Trace sessions: Map<string, SessionsState> = new Map();
  @Trace terms: TermModel[] = [];
  /** 正在连哪台设备（公钥）；手输设备码时是 "new" */
  @Trace connecting: string | null = null;
  @Trace connectingName: string | null = null;
  @Trace prompt: ConnectPrompt | null = null;
  @Trace fileRequest: FileRequestView | null = null;
  @Trace settings: SettingsView | null = null;
  @Trace savingSettings: boolean = false;
  private termSeq: number = 1;

  // ---- 启动 ----

  startCore(ctx: common.UIAbilityContext): void {
    this.ctx = ctx;
    this.startState = 'starting';
    try {
      fs.mkdirSync(ctx.filesDir + '/p2pcli', true);
      fs.mkdirSync(ctx.filesDir + '/inbox', true);
    } catch (_) {
    }
    const config: MobileConfig = {
      data_dir: ctx.filesDir + '/p2pcli',
      inbox_dir: ctx.filesDir + '/inbox',
      device_name: deviceInfo.marketName,
      server: null,
      relay: null,
      proxy: null,
      app_version: null,
      // Rust 编译目标的系统名是 linux，这里明确告诉对方是鸿蒙
      platform: 'harmonyos'
    };
    p2pcli.start(JSON.stringify(config), (event: string) => this.onEvent(event)).then((p: P2p) => {
      this.p2p = p;
      this.startState = 'ready';
      this.refreshOverview();
    }).catch((e: Error) => {
      this.startState = 'failed';
      this.startError = tr(parseError(e).detail);
    });
  }

  notify(text: string): void {
    promptAction.showToast({ message: text, duration: 2500 });
  }

  device(id: string): DeviceView | undefined {
    const ov = this.overview;
    if (ov === null) {
      return undefined;
    }
    return ov.devices.find((d: DeviceView) => d.endpoint_id === id) ?? ov.nearby.find((d: DeviceView) => d.endpoint_id === id);
  }

  deviceName(id: string): string {
    const d = this.device(id);
    if (d !== undefined) {
      return d.remark ?? d.name;
    }
    return this.conns.get(id)?.name ?? '';
  }

  // ---- Rust 的回调（已经在主线程）----

  private onEvent(text: string): void {
    const ev = JSON.parse(text) as AppEvent;
    switch (ev.type) {
      case 'overview_changed':
        this.refreshOverview();
        break;
      case 'disconnected': {
        const next = new Map<string, Connected>();
        this.conns.forEach((c: Connected, id: string) => {
          if (c.conn_id !== ev.conn_id) {
            next.set(id, c);
          }
        });
        this.conns = next;
        break;
      }
      case 'transfers':
        this.onTransfers(ev.transfers ?? []);
        break;
      case 'file_request':
        this.fileRequest = ev.request ?? null;
        break;
      case 'file_request_cancelled':
        if (this.fileRequest !== null && this.fileRequest.id === ev.id) {
          this.fileRequest = null;
        }
        break;
      default:
        break;
    }
  }

  private onTransfers(list: TransferView[]): void {
    const before = new Map<string, TransferView>();
    this.transfers.forEach((x: TransferView) => before.set(x.id, x));
    this.transfers = list;
    list.forEach((x: TransferView) => {
      const old = before.get(x.id);
      if (old === undefined || old.state === x.state || !x.outgoing) {
        return;
      }
      if (x.state === 'done') {
        this.notify(t('已发给 {{name}}：{{label}}', { name: x.peer_name, label: x.label } as Record<string, string | number>));
      } else if (x.state === 'failed') {
        this.notify(t('没发出去：{{error}}', { error: x.error !== null ? tr(x.error) : x.label } as Record<string, string | number>));
      }
    });
  }

  respondFiles(accept: boolean): void {
    const r = this.fileRequest;
    if (r === null) {
      return;
    }
    this.fileRequest = null;
    this.p2p?.respondFiles(r.id, accept);
  }

  refreshOverview(): void {
    if (this.p2p !== undefined) {
      this.overview = JSON.parse(this.p2p.overview()) as Overview;
    }
  }

  scanNearby(): void {
    const p = this.p2p;
    if (p === undefined) {
      return;
    }
    p.scanNearby().then(() => this.refreshOverview()).catch((e: Error) => {
      this.notify(t('扫描失败：{{error}}', { error: tr(parseError(e).detail) } as Record<string, string | number>));
    });
  }

  // ---- 连接 ----

  /** 点了列表里的设备：连着就进会话页，没连着先连 */
  openDevice(d: DeviceView): void {
    if (this.conns.has(d.endpoint_id)) {
      this.showDevice(d.endpoint_id);
      return;
    }
    // 局域网里在线就按公钥直接连，不经过牵线服务器
    const target = d.lan_online || d.code === null ? d.endpoint_id : d.code;
    this.connect(target, null, d.endpoint_id, d.remark ?? d.name);
  }

  /** 设备页：宽屏时右边一栏直接换成它，不一层层叠 */
  showDevice(id: string): void {
    this.stack.clear(false);
    this.stack.pushPathByName('device', id);
  }

  newPrompt(): void {
    const p = new ConnectPrompt();
    this.prompt = p;
  }

  /** 连接面板里点“连接” */
  submitPrompt(target: string, password: string): void {
    const p = this.prompt;
    if (p === null) {
      return;
    }
    const tgt = target.trim();
    this.connect(tgt.length > 0 ? tgt : p.target, password, p.endpointId, p.name);
  }

  connectPair(pair: PairInfo): void {
    this.connect(pair.code ?? pair.id, pair.password, pair.id, pair.name.length > 0 ? pair.name : null);
  }

  private connect(target: string, password: string | null, endpointId: string | null, name: string | null): void {
    const p = this.p2p;
    if (p === undefined || this.connecting !== null) {
      return;
    }
    this.connecting = endpointId ?? 'new';
    this.connectingName = name;
    p.connect(target, password).then((text: string) => {
      const c = JSON.parse(text) as Connected;
      const next = new Map<string, Connected>(this.conns);
      next.set(c.endpoint_id, c);
      this.conns = next;
      this.prompt = null;
      this.refreshOverview();
      this.showDevice(c.endpoint_id);
      this.loadSessions(c.endpoint_id);
    }).catch((e: Error) => {
      const err = parseError(e);
      if (err.kind === 'rejected' && (err.reason === 'need_password' || err.reason === 'bad_password')) {
        // 要密码：弹出面板填密码；密码不对时说一声
        const prompt = new ConnectPrompt();
        prompt.endpointId = endpointId;
        prompt.name = name;
        prompt.target = target;
        prompt.error = err.reason === 'bad_password' ? tr(err.detail) : null;
        this.prompt = prompt;
      } else {
        this.showError(tr(err.detail));
      }
    }).finally(() => {
      this.connecting = null;
      this.connectingName = null;
    });
  }

  /** 面板开着时错误显示在面板里，不然弹一句提示 */
  private showError(message: string): void {
    if (this.prompt !== null) {
      this.prompt.error = message;
    } else {
      this.notify(message);
    }
  }

  disconnect(endpointId: string): void {
    const c = this.conns.get(endpointId);
    if (c === undefined) {
      return;
    }
    this.p2p?.disconnect(c.conn_id);
    const next = new Map<string, Connected>(this.conns);
    next.delete(endpointId);
    this.conns = next;
    this.terms.filter((x: TermModel) => x.endpointId === endpointId).forEach((x: TermModel) => x.close());
    this.terms = this.terms.filter((x: TermModel) => x.endpointId !== endpointId);
    this.stack.clear();
  }

  // ---- 会话 ----

  loadSessions(endpointId: string): Promise<void> {
    const p = this.p2p;
    const c = this.conns.get(endpointId);
    if (p === undefined || c === undefined) {
      return Promise.resolve();
    }
    return p.listSessions(c.conn_id).then((text: string) => {
      const next = new Map<string, SessionsState>(this.sessions);
      next.set(endpointId, new SessionsState(JSON.parse(text) as SessionView[], null));
      this.sessions = next;
    }).catch((e: Error) => {
      const next = new Map<string, SessionsState>(this.sessions);
      next.set(endpointId, new SessionsState(this.sessions.get(endpointId)?.list ?? null, tr(parseError(e).detail)));
      this.sessions = next;
    });
  }

  endSession(endpointId: string, sessionId: string): void {
    const p = this.p2p;
    const c = this.conns.get(endpointId);
    if (p === undefined || c === undefined) {
      return;
    }
    p.endSession(c.conn_id, sessionId).then(() => {
      this.terms.filter((x: TermModel) => x.endpointId === endpointId && x.sessionId === sessionId)
        .forEach((x: TermModel) => this.closeTerminal(x.key));
      this.loadSessions(endpointId);
      this.notify(t('会话已结束'));
    }).catch((e: Error) => {
      this.notify(t('结束会话失败：{{error}}', { error: tr(parseError(e).detail) } as Record<string, string | number>));
    });
  }

  // ---- 终端 ----

  term(key: number): TermModel | undefined {
    return this.terms.find((x: TermModel) => x.key === key);
  }

  /** 打开一个会话；sessionId 为空是新建。已经开着就切过去。fromDevice：从设备页点的，推进去，返回能回到设备页 */
  openTerminal(endpointId: string, sessionId: string | null, fromDevice: boolean): void {
    const p = this.p2p;
    const c = this.conns.get(endpointId);
    if (p === undefined || c === undefined) {
      return;
    }
    if (sessionId !== null) {
      const existing = this.terms.find((x: TermModel) => x.endpointId === endpointId && x.sessionId === sessionId && x.state !== 'closed');
      if (existing !== undefined) {
        this.goTerminal(existing.key, fromDevice);
        return;
      }
    }
    const connId = c.conn_id;
    const term = new TermModel(this.termSeq++, endpointId, connId, sessionId,
      (cols: number, rows: number, m: TermModel) => p.openTerminal(connId, sessionId, cols, rows,
        (data: Uint8Array) => m.output(data), (info: string) => m.closed(info)));
    // 同一个会话之前断开的那个换掉
    if (sessionId !== null) {
      this.terms.filter((x: TermModel) => x.endpointId === endpointId && x.sessionId === sessionId)
        .forEach((x: TermModel) => this.forget(x.key));
    }
    this.terms = this.terms.concat([term]);
    this.goTerminal(term.key, fromDevice);
  }

  private goTerminal(key: number, fromDevice: boolean): void {
    const names = this.stack.getAllPathName();
    if (names.length > 0 && names[names.length - 1] === 'terminal') {
      this.stack.replacePathByName('terminal', key, false);
    } else if (fromDevice) {
      this.stack.pushPathByName('terminal', key);
    } else {
      this.stack.clear(false);
      this.stack.pushPathByName('terminal', key);
    }
  }

  /** 关掉手机上的这个终端，会话在对方电脑上接着跑 */
  closeTerminal(key: number): void {
    const term = this.term(key);
    if (term === undefined) {
      return;
    }
    term.close();
    this.forget(key);
  }

  private forget(key: number): void {
    this.terms = this.terms.filter((x: TermModel) => x.key !== key);
    const names = this.stack.getAllPathName();
    for (let i = names.length - 1; i >= 0; i--) {
      if (names[i] === 'terminal' && this.stack.getParamByIndex(i) === key) {
        this.stack.removeByIndexes([i]);
      }
    }
  }

  /** 断开的终端重新连：连接还在就接回同一个会话，不然先重连设备 */
  reopen(term: TermModel): void {
    if (this.conns.has(term.endpointId)) {
      const eid = term.endpointId;
      const sid = term.sessionId;
      this.closeTerminal(term.key);
      this.openTerminal(eid, sid, false);
    } else {
      const d = this.device(term.endpointId);
      this.closeTerminal(term.key);
      if (d !== undefined) {
        this.openDevice(d);
      } else {
        this.notify(t('找不到这台设备，请回到首页重新连接'));
      }
    }
  }

  chooseOption(term: TermModel, approvalId: string, optionId: number): void {
    const p = this.p2p;
    if (p === undefined || term.sessionId === null) {
      return;
    }
    p.chooseAgentOption(term.connId, term.sessionId, approvalId, optionId).catch((e: Error) => {
      this.notify(tr(parseError(e).detail));
    }).finally(() => this.loadSessions(term.endpointId));
  }

  copyText(text: string): void {
    const data = pasteboard.createData(pasteboard.MIMETYPE_TEXT_PLAIN, text);
    pasteboard.getSystemPasteboard().setDataSync(data);
  }

  // ---- 传文件 ----

  /** 选文件给的是 file:// 地址：先复制到缓存目录，再按路径发 */
  sendFiles(endpointId: string, uris: string[]): void {
    const p = this.p2p;
    const ctx = this.ctx;
    if (p === undefined || ctx === undefined || uris.length === 0) {
      return;
    }
    try {
      const paths: string[] = [];
      uris.forEach((uri: string, i: number) => {
        // 每个文件一个子目录：同名的各放各的，发过去还是原来的名字
        const dir = `${ctx.cacheDir}/outgoing/${Date.now()}-${i}`;
        fs.mkdirSync(dir, true);
        let name = decodeURIComponent(uri.substring(uri.lastIndexOf('/') + 1));
        if (name.length === 0) {
          name = `file-${i}`;
        }
        const src = fs.openSync(uri, fs.OpenMode.READ_ONLY);
        fs.copyFileSync(src.fd, `${dir}/${name}`);
        fs.closeSync(src);
        paths.push(`${dir}/${name}`);
      });
      p.sendFiles(endpointId, paths);
      this.notify(t('开始发送 {{count}} 个文件', { count: paths.length } as Record<string, string | number>));
    } catch (e) {
      const err = e as Error;
      this.notify(t('发不了：{{error}}', { error: tr(parseError(err).detail) } as Record<string, string | number>));
    }
  }

  // ---- 设置 ----

  openSettings(): void {
    if (this.p2p !== undefined) {
      this.settings = JSON.parse(this.p2p.settings()) as SettingsView;
    }
    this.stack.clear(false);
    this.stack.pushPathByName('settings', null);
  }

  /** 保存并马上生效；格式不对时返回原因 */
  saveSettings(value: MobileSettings): Promise<string | null> {
    const p = this.p2p;
    if (p === undefined || this.savingSettings) {
      return Promise.resolve(null);
    }
    this.savingSettings = true;
    return p.saveSettings(JSON.stringify(value)).then((): string | null => {
      this.settings = JSON.parse(p.settings()) as SettingsView;
      this.notify(t('设置已保存'));
      this.stack.pop();
      this.refreshOverview();
      return null;
    }).catch((e: Error): string | null => tr(parseError(e).detail)).finally(() => {
      this.savingSettings = false;
    });
  }
}

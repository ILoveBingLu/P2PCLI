import { image } from '@kit.ImageKit';
import { util } from '@kit.ArkTS';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { P2p, ScreenSession } from 'libp2pcli.so';
import { parseError, ScreenEvent, ScreenInput, ScreenStartedView } from './Model';
import { t, tr } from './I18n';

/**
 * 手机上正在看的一路远程画面，和安卓、iOS 的 RemoteScreen 一样。
 *
 * 画面不经过这里：Rust 收到对方电脑硬件编码的 H.264/H.265，直接交给系统的硬件解码器，画到页面上 XComponent 的 Surface 里。
 * 这里管状态（等同意、在放、暂停、结束）、指针，把手指的操作换成键鼠事件发回去。
 */
@ObservedV2
export class RemoteScreenModel {
  readonly endpointId: string;
  readonly connId: number;
  readonly name: string;
  /** 对方的系统：Mac 上的 ⌘ 在 Windows 上是 Win 键 */
  readonly platform: string;
  /** opening（等对方同意）| live | paused | closed */
  @Trace phase: string = 'opening';
  /** 暂停时：连接走了中继（否则是还在找直连的路） */
  @Trace relay: boolean = false;
  /** 结束时的原因；空是正常关掉 */
  @Trace closedReason: string | null = null;
  @Trace started: ScreenStartedView | null = null;
  @Trace display: string = '';
  /** sharp | balanced | smooth */
  @Trace quality: string = 'balanced';
  /** trackpad（触控板：手指滑动挪指针，点一下点在指针上）| touch（直接点） */
  @Trace mode: string = 'trackpad';
  @Trace fps: number = -1;
  @Trace bitrate: number = 0;
  /** 粘住的修饰键（ControlLeft、AltLeft、MetaLeft、ShiftLeft）：下一次按键、点击带上它们，之后松开 */
  @Trace modifiers: string[] = [];
  /** 已经出过画面 */
  @Trace hasVideo: boolean = false;
  /** 收起工具栏，只留一个小圆钮 */
  @Trace toolbarCollapsed: boolean = false;
  /** 画面的像素尺寸 */
  @Trace videoW: number = 16;
  @Trace videoH: number = 10;
  /** 指针在对方屏幕上的位置（0~1） */
  @Trace cursorX: number = 0.5;
  @Trace cursorY: number = 0.5;
  @Trace cursorVisible: boolean = true;
  @Trace cursorImage: PixelMap | null = null;
  @Trace cursorW: number = 0;
  @Trace cursorH: number = 0;
  @Trace cursorHotX: number = 0;
  @Trace cursorHotY: number = 0;

  private p2p: P2p;
  private session: ScreenSession | null = null;
  private surfaceId: string | null = null;
  /** 第几次打开：重新打开以后，旧的那一路的回调就不认了 */
  private gen: number = 0;
  private dragMods: string[] = [];
  /** 最近一次本机挪指针的时间：这之后一小会儿不信对方报回来的位置，免得指针往回跳 */
  lastLocalMove: number = 0;
  private notify: (text: string) => void;

  constructor(p2p: P2p, endpointId: string, connId: number, name: string, platform: string, notify: (text: string) => void) {
    this.p2p = p2p;
    this.endpointId = endpointId;
    this.connId = connId;
    this.name = name;
    this.platform = platform;
    this.notify = notify;
  }

  get isMac(): boolean {
    const p = this.platform.toLowerCase();
    return p.includes('mac') || p === 'darwin';
  }

  get canControl(): boolean {
    return this.started?.control ?? false;
  }

  open(): void {
    const gen = ++this.gen;
    this.phase = 'opening';
    this.hasVideo = false;
    this.closedReason = null;
    this.p2p.openScreen(this.connId, this.quality, (text: string) => this.onEvent(gen, text)).then((s: ScreenSession) => {
      // 等的时候已经关掉了
      if (gen !== this.gen || this.phase !== 'opening') {
        s.stop();
        return;
      }
      this.session = s;
      const started = JSON.parse(s.started()) as ScreenStartedView;
      this.started = started;
      this.display = started.display;
      this.phase = 'live';
      if (this.surfaceId !== null) {
        s.setSurface(this.surfaceId);
      }
    }).catch((e: Error) => {
      if (gen !== this.gen) {
        return;
      }
      const detail = parseError(e).detail;
      this.phase = 'closed';
      this.closedReason = detail.startsWith('relay:')
        ? t('现在这条连接走的是中继，远程画面只走直连。终端照常能用；直连接通后再打开画面') : tr(detail);
    });
  }

  stop(): void {
    this.gen++;
    this.session?.send(JSON.stringify({ k: 'release_all' } as ScreenInput));
    this.session?.stop();
    this.session = null;
    if (this.phase !== 'closed') {
      this.phase = 'closed';
      this.closedReason = null;
    }
  }

  /** XComponent 的 Surface 建好、销毁 */
  setSurface(id: string | null): void {
    this.surfaceId = id;
    this.session?.setSurface(id);
  }

  /** 回到前台：解码器重来，要一个关键帧 */
  resume(): void {
    this.session?.resume();
  }

  switchDisplay(id: string): void {
    this.session?.switchDisplay(id);
  }

  setQuality(q: string): void {
    this.quality = q;
    this.session?.setQuality(q);
  }

  // ---- 输入 ----

  send(input: ScreenInput): void {
    if (!this.canControl) {
      return;
    }
    this.session?.send(JSON.stringify(input));
  }

  key(code: string, down: boolean): void {
    this.send({ k: 'key', code: code, down: down } as ScreenInput);
  }

  move(x: number, y: number): void {
    this.cursorX = Math.min(1, Math.max(0, x));
    this.cursorY = Math.min(1, Math.max(0, y));
    this.lastLocalMove = Date.now();
    this.send({ k: 'move', x: this.cursorX, y: this.cursorY } as ScreenInput);
  }

  /** 点一下：带上粘住的修饰键，点完松开 */
  click(button: string, x: number, y: number): void {
    if (!this.canControl) {
      return;
    }
    const mods = this.takeModifiers();
    mods.forEach((m: string) => this.key(m, true));
    this.send({ k: 'move', x: x, y: y } as ScreenInput);
    this.send({ k: 'button', button: button, down: true, x: x, y: y } as ScreenInput);
    this.send({ k: 'button', button: button, down: false, x: x, y: y } as ScreenInput);
    mods.reverse().forEach((m: string) => this.key(m, false));
  }

  /** 拖动开始：修饰键按住到拖完 */
  dragBegan(x: number, y: number): void {
    if (!this.canControl) {
      return;
    }
    this.dragMods = this.takeModifiers();
    this.dragMods.forEach((m: string) => this.key(m, true));
    this.send({ k: 'move', x: x, y: y } as ScreenInput);
    this.send({ k: 'button', button: 'left', down: true, x: x, y: y } as ScreenInput);
  }

  dragEnded(x: number, y: number): void {
    this.send({ k: 'button', button: 'left', down: false, x: x, y: y } as ScreenInput);
    this.dragMods.reverse().forEach((m: string) => this.key(m, false));
    this.dragMods = [];
  }

  wheel(dx: number, dy: number, x: number, y: number): void {
    this.send({ k: 'wheel', dx: dx, dy: dy, x: x, y: y } as ScreenInput);
  }

  typeText(text: string): void {
    if (!this.canControl || text.length === 0) {
      return;
    }
    this.session?.typeText(text, this.takeModifiers());
  }

  tapKey(code: string): void {
    if (!this.canControl) {
      return;
    }
    this.session?.tapKey(code, this.takeModifiers());
  }

  toggleModifier(code: string): void {
    if (this.modifiers.includes(code)) {
      this.modifiers = this.modifiers.filter((m: string) => m !== code);
    } else {
      this.modifiers = this.modifiers.concat([code]);
    }
  }

  private takeModifiers(): string[] {
    const m = this.modifiers;
    this.modifiers = [];
    return m;
  }

  // ---- Rust 的回调（已经在主线程）----

  private onEvent(gen: number, text: string): void {
    if (gen !== this.gen) {
      return;
    }
    const ev = JSON.parse(text) as ScreenEvent;
    switch (ev.type) {
      case 'config':
        this.videoW = ev.width ?? 16;
        this.videoH = ev.height ?? 10;
        break;
      case 'first_frame':
        this.hasVideo = true;
        break;
      case 'cursor':
        this.cursorVisible = ev.visible ?? true;
        if (Date.now() - this.lastLocalMove > 400) {
          this.cursorX = ev.x ?? this.cursorX;
          this.cursorY = ev.y ?? this.cursorY;
        }
        break;
      case 'cursor_shape':
        this.setCursorShape(ev);
        break;
      case 'paused':
        if (this.phase === 'live') {
          this.phase = 'paused';
          this.relay = ev.relay ?? false;
        }
        break;
      case 'resumed':
        if (this.phase === 'paused') {
          this.phase = 'live';
        }
        break;
      case 'switched':
        this.display = ev.display ?? this.display;
        break;
      case 'stats':
        this.fps = ev.fps ?? 0;
        this.bitrate = ev.bitrate ?? 0;
        break;
      case 'unsupported': {
        const args: [string, number][] = [['w', ev.width ?? 0], ['h', ev.height ?? 0]];
        if (this.quality === 'sharp') {
          this.notify(t('这台手机解不了 {{w}}×{{h}} 的画面，已换成均衡画质', ...args));
          this.setQuality('balanced');
        } else {
          this.stop();
          this.closedReason = t('这台手机解不了 {{w}}×{{h}} 的画面', ...args);
        }
        break;
      }
      case 'closed':
        this.session = null;
        if (this.phase !== 'closed') {
          this.phase = 'closed';
          this.closedReason = ev.reason ? tr(ev.reason) : null;
        }
        break;
      default:
        break;
    }
  }

  /** 指针图案：RGBA 换成 PixelMap */
  private setCursorShape(ev: ScreenEvent): void {
    const w = ev.width ?? 0;
    const h = ev.height ?? 0;
    if (w <= 0 || h <= 0 || ev.rgba === undefined) {
      return;
    }
    const bytes = new util.Base64Helper().decodeSync(ev.rgba);
    if (bytes.length < w * h * 4) {
      return;
    }
    const buf = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
    const opts: image.InitializationOptions = {
      size: { width: w, height: h },
      srcPixelFormat: image.PixelMapFormat.RGBA_8888,
      pixelFormat: image.PixelMapFormat.RGBA_8888,
      alphaType: image.AlphaType.UNPREMUL,
      editable: false
    };
    image.createPixelMap(buf, opts).then((pm: image.PixelMap) => {
      this.cursorImage = pm;
      this.cursorW = w;
      this.cursorH = h;
      this.cursorHotX = ev.hot_x ?? 0;
      this.cursorHotY = ev.hot_y ?? 0;
    }).catch((e: Error) => {
      hilog.warn(0x0001, 'p2pcli', 'cursor pixelmap failed: %{public}s', e.message);
    });
  }
}

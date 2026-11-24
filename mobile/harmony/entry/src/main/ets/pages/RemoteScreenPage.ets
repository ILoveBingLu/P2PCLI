import { window } from '@kit.ArkUI';
import { Core } from '../common/Core';
import { Hero } from '../common/Hero';
import { t, tr } from '../common/I18n';
import { ScreenDisplay } from '../common/Model';
import { RemoteScreenModel } from '../common/RemoteScreen';
import { GlassButton } from '../common/Glass';

/** 藏起来的输入框里一直放着这个字：框里看起来是空的，按删除时删掉它就知道是退格 */
const SENTINEL: string = '​';

class Rect {
  x: number = 0;
  y: number = 0;
  w: number = 0;
  h: number = 0;
}

class Point {
  x: number = 0;
  y: number = 0;

  constructor(x: number, y: number) {
    this.x = x;
    this.y = y;
  }
}

/** XComponent 的 Surface 建好、销毁时通知解码器（API 12 起的写法） */
class SurfaceController extends XComponentController {
  onCreated: (id: string) => void = () => {
  };
  onDestroyed: () => void = () => {
  };

  onSurfaceCreated(surfaceId: string): void {
    this.onCreated(surfaceId);
  }

  onSurfaceDestroyed(surfaceId: string): void {
    this.onDestroyed();
  }
}

/** 一个方向上：画面比屏幕小就居中，大就按平移量挪，但不露出边 */
function axis(size: number, bound: number, offset: number): number {
  if (size <= bound) {
    return (bound - size) / 2;
  }
  return Math.min(0, Math.max(bound - size, (bound - size) / 2 + offset));
}

class KeyItem {
  label: string;
  icon: Resource | null;
  /** 修饰键：按一下粘住 */
  modifier: string | null;
  action: () => void;

  constructor(label: string, icon: Resource | null, modifier: string | null, action: () => void) {
    this.label = label;
    this.icon = icon;
    this.modifier = modifier;
    this.action = action;
  }
}

/**
 * 远程控制页，和安卓、iOS 一样：铺满屏幕的对方画面，底下一条浮着的工具栏；弹出键盘时换成按键条。
 *
 * - 触控板模式（默认）：一指滑动挪指针，点一下左键，两指点一下右键，长按后拖动是按住左键拖。
 * - 直接点模式：点哪儿点哪儿，按住拖动就是拖，长按是右键。
 * - 两指上下左右拖是滚轮；两指捏合放大画面，放大后两指一起挪是平移，指针走到边上画面跟着走。
 *
 * 画面在 XComponent 里，Rust 直接把解码结果画上去；放大、平移只改 XComponent 的位置和大小。
 */
@ComponentV2
export struct RemoteScreenPage {
  @Param @Require screen: RemoteScreenModel;
  @Local core: Core = Core.shared;
  @Local w: number = 0;
  @Local h: number = 0;
  /** 放大倍数和平移（相对居中摆放） */
  @Local zoom: number = 1;
  @Local panX: number = 0;
  @Local panY: number = 0;
  @Local keyboardUp: boolean = false;
  @Local showMenu: boolean = false;
  @Local inputText: string = SENTINEL;
  private xc: SurfaceController = new SurfaceController();
  private inputCtl: TextInputController = new TextInputController();
  private win: window.Window | null = null;
  private onKeyboard: (h: number) => void = (h: number) => {
    this.keyboardUp = h > 0;
  };

  // 手势的状态
  private fingers: Map<number, Point> = new Map();
  private downTime: number = 0;
  private downX: number = 0;
  private downY: number = 0;
  private lastX: number = 0;
  private lastY: number = 0;
  private lastTime: number = 0;
  private speed: number = 0;
  private maxPointers: number = 1;
  private moved: boolean = false;
  private longPressed: boolean = false;
  private dragging: boolean = false;
  /** 两指这一下是 undecided | zoom | scroll */
  private twoFinger: string = 'undecided';
  private startSpan: number = 1;
  private startZoom: number = 1;
  private startCx: number = 0;
  private startCy: number = 0;
  private lastCx: number = 0;
  private lastCy: number = 0;
  private longTimer: number = -1;

  aboutToAppear(): void {
    this.xc.onCreated = (id: string) => this.screen.setSurface(id);
    this.xc.onDestroyed = () => this.screen.setSurface(null);
    // 全屏、不锁屏，看着键盘弹没弹出来
    const ctx = this.core.ctx;
    if (ctx !== undefined) {
      window.getLastWindow(ctx).then((w: window.Window) => {
        this.win = w;
        w.setSpecificSystemBarEnabled('status', false).catch(() => {
        });
        w.setSpecificSystemBarEnabled('navigationIndicator', false).catch(() => {
        });
        w.setWindowKeepScreenOn(true).catch(() => {
        });
        w.on('keyboardHeightChange', this.onKeyboard);
      }).catch(() => {
      });
    }
  }

  aboutToDisappear(): void {
    clearTimeout(this.longTimer);
    const w = this.win;
    if (w !== null) {
      w.setSpecificSystemBarEnabled('status', true).catch(() => {
      });
      w.setSpecificSystemBarEnabled('navigationIndicator', true).catch(() => {
      });
      w.setWindowKeepScreenOn(false).catch(() => {
      });
      w.off('keyboardHeightChange', this.onKeyboard);
    }
  }

  // ---- 摆放 ----

  /** 画面按比例放进来，再按放大倍数和平移挪 */
  contentRect(): Rect {
    const r = new Rect();
    const vw = this.screen.videoW;
    const vh = this.screen.videoH;
    if (this.w <= 0 || this.h <= 0 || vw <= 0 || vh <= 0) {
      return r;
    }
    const s = Math.min(this.w / vw, this.h / vh) * this.zoom;
    r.w = vw * s;
    r.h = vh * s;
    r.x = axis(r.w, this.w, this.panX);
    r.y = axis(r.h, this.h, this.panY);
    return r;
  }

  /** 平移量收在画面能盖住的范围里（缩小到比屏幕小时居中） */
  clampPan(): void {
    const r = this.contentRect();
    this.panX = r.w <= this.w ? 0 : r.x - (this.w - r.w) / 2;
    this.panY = r.h <= this.h ? 0 : r.y - (this.h - r.h) / 2;
  }

  normX(x: number): number {
    const r = this.contentRect();
    return r.w <= 0 ? 0.5 : Math.min(1, Math.max(0, (x - r.x) / r.w));
  }

  normY(y: number): number {
    const r = this.contentRect();
    return r.h <= 0 ? 0.5 : Math.min(1, Math.max(0, (y - r.y) / r.h));
  }

  /** 本机挪了指针：发过去；放大时指针靠近边上，画面往那边挪 */
  moveCursor(x: number, y: number): void {
    this.screen.move(x, y);
    if (this.zoom <= 1.01) {
      return;
    }
    const r = this.contentRect();
    const px = r.x + this.screen.cursorX * r.w;
    const py = r.y + this.screen.cursorY * r.h;
    const margin = 60;
    let dx = 0;
    let dy = 0;
    if (px < margin) {
      dx = margin - px;
    } else if (px > this.w - margin) {
      dx = this.w - margin - px;
    }
    if (py < margin) {
      dy = margin - py;
    } else if (py > this.h - margin) {
      dy = this.h - margin - py;
    }
    if (dx !== 0 || dy !== 0) {
      this.panX += dx;
      this.panY += dy;
      this.clampPan();
    }
  }

  // ---- 手势 ----

  onTouchEvent(e: TouchEvent): void {
    const ch = e.changedTouches.length > 0 ? e.changedTouches[0] : e.touches[0];
    if (ch === undefined) {
      return;
    }
    switch (e.type) {
      case TouchType.Down:
        this.fingers.set(ch.id, new Point(ch.x, ch.y));
        if (this.fingers.size === 1) {
          this.downTime = e.timestamp;
          this.downX = ch.x;
          this.downY = ch.y;
          this.lastX = ch.x;
          this.lastY = ch.y;
          this.lastTime = e.timestamp;
          this.speed = 0;
          this.maxPointers = 1;
          this.moved = false;
          this.longPressed = false;
          this.dragging = false;
          this.twoFinger = 'undecided';
          clearTimeout(this.longTimer);
          this.longTimer = setTimeout(() => this.onLongPress(), 350);
        } else {
          clearTimeout(this.longTimer);
          this.maxPointers = Math.max(this.maxPointers, this.fingers.size);
          if (this.dragging) {
            this.dragging = false;
            this.screen.dragEnded(this.screen.cursorX, this.screen.cursorY);
          }
          if (this.fingers.size === 2) {
            const c = this.center();
            this.startSpan = Math.max(1, this.span());
            this.startZoom = this.zoom;
            this.startCx = c.x;
            this.startCy = c.y;
            this.lastCx = c.x;
            this.lastCy = c.y;
          }
        }
        break;
      case TouchType.Move:
        e.touches.forEach((p: TouchObject) => {
          if (this.fingers.has(p.id)) {
            this.fingers.set(p.id, new Point(p.x, p.y));
          }
        });
        if (this.maxPointers >= 2) {
          if (this.fingers.size >= 2) {
            this.twoFingerMove();
          }
        } else {
          const p = this.fingers.values().next().value as Point;
          this.oneFingerMove(p.x, p.y, e.timestamp);
        }
        break;
      case TouchType.Up:
        this.fingers.delete(ch.id);
        if (this.fingers.size === 0) {
          this.touchEnded(ch.x, ch.y, e.timestamp);
        }
        break;
      case TouchType.Cancel:
        this.fingers.clear();
        clearTimeout(this.longTimer);
        if (this.dragging) {
          this.screen.dragEnded(this.screen.cursorX, this.screen.cursorY);
        }
        this.dragging = false;
        break;
      default:
        break;
    }
  }

  center(): Point {
    const ps = Array.from(this.fingers.values());
    return new Point((ps[0].x + ps[1].x) / 2, (ps[0].y + ps[1].y) / 2);
  }

  span(): number {
    const ps = Array.from(this.fingers.values());
    return Math.hypot(ps[0].x - ps[1].x, ps[0].y - ps[1].y);
  }

  onLongPress(): void {
    if (this.maxPointers !== 1 || this.moved || this.fingers.size !== 1) {
      return;
    }
    this.longPressed = true;
    if (this.screen.mode === 'touch') {
      // 直接点：长按是右键
      const x = this.normX(this.downX);
      const y = this.normY(this.downY);
      this.screen.cursorX = x;
      this.screen.cursorY = y;
      this.screen.lastLocalMove = Date.now();
      this.screen.click('right', x, y);
    } else {
      // 触控板：按住左键，接着挪手指就是拖
      this.dragging = true;
      this.screen.dragBegan(this.screen.cursorX, this.screen.cursorY);
    }
  }

  oneFingerMove(x: number, y: number, time: number): void {
    const dx = x - this.lastX;
    const dy = y - this.lastY;
    const dt = (time - this.lastTime) / 1e9;
    if (dt > 0) {
      this.speed = Math.hypot(dx, dy) / dt;
    }
    this.lastTime = time;
    if (!this.moved) {
      if (Math.hypot(x - this.downX, y - this.downY) < 8) {
        return;
      }
      this.moved = true;
      clearTimeout(this.longTimer);
      if (this.screen.mode === 'touch' && !this.longPressed) {
        // 直接点：从按下的地方开始拖
        this.dragging = true;
        const sx = this.normX(this.downX);
        const sy = this.normY(this.downY);
        this.screen.cursorX = sx;
        this.screen.cursorY = sy;
        this.screen.dragBegan(sx, sy);
      }
    }
    this.lastX = x;
    this.lastY = y;
    if (this.screen.mode === 'touch') {
      if (this.dragging) {
        this.moveCursor(this.normX(x), this.normY(y));
      }
      return;
    }
    // 触控板：手指挪多少，指针挪多少（快的时候多挪一点）；按没放大时的画面算，放大以后挪得慢一点、更细
    const r = this.contentRect();
    if (r.w <= 0) {
      return;
    }
    const accel = this.dragging ? 1 : 1 + Math.min(1.5, this.speed / 1200);
    const k = accel * 1.4 / (r.w / this.zoom) / Math.max(1, this.zoom * 0.7);
    this.moveCursor(this.screen.cursorX + dx * k, this.screen.cursorY + dy * k * (r.w / r.h));
  }

  twoFingerMove(): void {
    const c = this.center();
    const sp = this.span();
    if (this.twoFinger === 'undecided') {
      if (Math.abs(sp / this.startSpan - 1) > 0.08) {
        this.twoFinger = 'zoom';
      } else if (Math.hypot(c.x - this.startCx, c.y - this.startCy) > 12) {
        this.twoFinger = 'scroll';
      } else {
        return;
      }
    }
    const dx = c.x - this.lastCx;
    const dy = c.y - this.lastCy;
    this.lastCx = c.x;
    this.lastCy = c.y;
    if (this.twoFinger === 'zoom') {
      // 以两指中间为中心缩放，两指一起挪是平移
      const before = this.contentRect();
      const ax = (c.x - dx - before.x) / Math.max(1, before.w);
      const ay = (c.y - dy - before.y) / Math.max(1, before.h);
      this.zoom = Math.min(5, Math.max(1, this.startZoom * sp / this.startSpan));
      this.panX = 0;
      this.panY = 0;
      const r = this.contentRect();
      this.panX = c.x - ax * r.w - r.x;
      this.panY = c.y - ay * r.h - r.y;
      this.clampPan();
      return;
    }
    // 自然滚动：手指往上推，内容往下滚
    const k = 2.5;
    const x = this.screen.mode === 'touch' ? this.normX(c.x) : this.screen.cursorX;
    const y = this.screen.mode === 'touch' ? this.normY(c.y) : this.screen.cursorY;
    this.screen.wheel(-dx * k, -dy * k, x, y);
  }

  touchEnded(x: number, y: number, time: number): void {
    clearTimeout(this.longTimer);
    const quick = (time - this.downTime) / 1e6 < 300;
    const touch = this.screen.mode === 'touch';
    if (this.dragging) {
      this.screen.dragEnded(touch ? this.normX(x) : this.screen.cursorX, touch ? this.normY(y) : this.screen.cursorY);
    } else if (this.maxPointers === 1 && !this.moved && !this.longPressed && quick) {
      if (touch) {
        this.screen.cursorX = this.normX(x);
        this.screen.cursorY = this.normY(y);
        this.screen.lastLocalMove = Date.now();
      }
      this.screen.click('left', this.screen.cursorX, this.screen.cursorY);
    } else if (this.maxPointers === 2 && this.twoFinger === 'undecided' && quick) {
      this.screen.click('right', touch ? this.normX(this.startCx) : this.screen.cursorX,
        touch ? this.normY(this.startCy) : this.screen.cursorY);
    }
    this.dragging = false;
  }

  // ---- 键盘 ----

  showKeyboard(): void {
    this.inputText = SENTINEL;
    focusControl.requestFocus('remote-screen-keys');
  }

  hideKeyboard(): void {
    this.getUIContext().getFocusController().clearFocus();
  }

  /** 输入法打的字：拼音还在组字时不发，选好字才发；框里的字删光了是退格 */
  onInput(value: string, preview?: PreviewText): void {
    if (preview !== undefined && preview.value.length > 0) {
      return;
    }
    if (value === SENTINEL) {
      return;
    }
    if (value.length === 0) {
      this.screen.tapKey('Backspace');
    } else {
      const typed = value.split(SENTINEL).join('');
      const parts = typed.split('\n');
      parts.forEach((part: string, i: number) => {
        if (part.length > 0) {
          this.screen.typeText(part);
        }
        if (i < parts.length - 1) {
          this.screen.tapKey('Enter');
        }
      });
    }
    this.inputText = SENTINEL;
    setTimeout(() => this.inputCtl.caretPosition(1), 0);
  }

  keys(): KeyItem[] {
    const s = this.screen;
    return [
      new KeyItem('Esc', null, null, () => s.tapKey('Escape')),
      new KeyItem('Tab', null, null, () => s.tapKey('Tab')),
      new KeyItem('Ctrl', null, 'ControlLeft', () => s.toggleModifier('ControlLeft')),
      new KeyItem(s.isMac ? '⌥' : 'Alt', null, 'AltLeft', () => s.toggleModifier('AltLeft')),
      new KeyItem(s.isMac ? '⌘' : 'Win', null, 'MetaLeft', () => s.toggleModifier('MetaLeft')),
      new KeyItem('⇧', null, 'ShiftLeft', () => s.toggleModifier('ShiftLeft')),
      new KeyItem('←', $r('app.media.lucide_arrow_left'), null, () => s.tapKey('ArrowLeft')),
      new KeyItem('↑', $r('app.media.lucide_arrow_up'), null, () => s.tapKey('ArrowUp')),
      new KeyItem('↓', $r('app.media.lucide_arrow_down'), null, () => s.tapKey('ArrowDown')),
      new KeyItem('→', $r('app.media.lucide_arrow_right'), null, () => s.tapKey('ArrowRight')),
      new KeyItem(t('删除'), $r('app.media.lucide_delete'), null, () => s.tapKey('Delete')),
      new KeyItem(t('收起键盘'), $r('app.media.lucide_keyboard_off'), null, () => this.hideKeyboard())
    ];
  }

  // ---- 界面 ----

  build() {
    Stack({ alignContent: Alignment.TopStart }) {
      XComponent({ id: 'remote-screen', type: XComponentType.SURFACE, controller: this.xc })
        .position({ x: this.contentRect().x, y: this.contentRect().y })
        .width(this.contentRect().w)
        .height(this.contentRect().h)
        .hitTestBehavior(HitTestMode.None)

      if (this.screen.cursorImage !== null && this.screen.cursorVisible) {
        this.cursor()
      }

      // 手势层：盖在画面上面
      Column()
        .width('100%')
        .height('100%')
        .onTouch((e: TouchEvent) => this.onTouchEvent(e))

      // 藏起来的输入框：系统键盘（包括中文输入法）打的字从这里拿
      TextInput({ text: this.inputText!!, controller: this.inputCtl })
        .id('remote-screen-keys')
        .width(1)
        .height(1)
        .opacity(0)
        .position({ x: 0, y: 0 })
        .focusable(true)
        .enterKeyType(EnterKeyType.Send)
        .onChange((value: string, preview?: PreviewText) => this.onInput(value, preview))
        .onSubmit((key: EnterKeyType, event?: SubmitEvent) => {
          this.screen.tapKey('Enter');
          event?.keepEditableState();
        })

      this.status()

      Column() {
        if (this.keyboardUp && this.screen.canControl) {
          this.keyBar()
        } else if (this.screen.toolbarCollapsed) {
          Row() {
            this.roundButton(t('显示工具栏'), $r('app.media.lucide_ellipsis'), () => {
              this.screen.toolbarCollapsed = false;
            })
          }.width('100%').justifyContent(FlexAlign.End).padding({ right: 16, bottom: 16 })
        } else {
          this.toolbar()
        }
      }
      .width('100%')
      .height('100%')
      .justifyContent(FlexAlign.End)
      .hitTestBehavior(HitTestMode.Transparent)
    }
    .width('100%')
    .height('100%')
    .clip(true)
    .backgroundColor(Color.Black)
    .expandSafeArea([SafeAreaType.SYSTEM], [SafeAreaEdge.TOP, SafeAreaEdge.BOTTOM])
    .onAreaChange((_: Area, a: Area) => {
      this.w = a.width as number;
      this.h = a.height as number;
      this.clampPan();
    })
    .bindSheet(this.showMenu!!, this.menu(), {
      height: SheetSize.FIT_CONTENT,
      backgroundColor: Hero.surface,
      title: { title: this.screen.name }
    })
  }

  /** 指针图案按对方屏幕上的大小缩放，太小了手机上看不清，至少 20 高 */
  @Builder
  cursor() {
    Image(this.screen.cursorImage)
      .width(this.cursorScale() * this.screen.cursorW)
      .height(this.cursorScale() * this.screen.cursorH)
      .position({
        x: this.contentRect().x + this.screen.cursorX * this.contentRect().w - this.screen.cursorHotX * this.cursorScale(),
        y: this.contentRect().y + this.screen.cursorY * this.contentRect().h - this.screen.cursorHotY * this.cursorScale()
      })
      .interpolation(ImageInterpolation.High)
      .hitTestBehavior(HitTestMode.None)
  }

  cursorScale(): number {
    const d = this.screen.started?.displays.find((x: ScreenDisplay) => x.id === this.screen.display);
    const screenWidth = d?.width ?? this.screen.videoW;
    const r = this.contentRect();
    const scale = screenWidth > 0 ? r.w / screenWidth : 1;
    return Math.max(scale, 20 / Math.max(1, this.screen.cursorH));
  }

  /** 画面上面的提示：等同意、暂停、结束 */
  @Builder
  status() {
    if (this.screen.phase === 'opening') {
      this.card()
    } else if (this.screen.phase === 'live') {
      if (!this.screen.hasVideo) {
        Column() {
          LoadingProgress().width(36).height(36).color(Color.White)
        }.width('100%').height('100%').justifyContent(FlexAlign.Center).hitTestBehavior(HitTestMode.None)
      } else if (!this.screen.canControl && this.screen.started?.control_note) {
        Column() {
          Text(t('只能看，不能操作：{{reason}}', ['reason', tr(this.screen.started?.control_note ?? '')]))
            .fontSize(13).fontColor(Color.White)
            .padding({ left: 14, right: 14, top: 8, bottom: 8 })
            .backgroundColor('#99000000').borderRadius(999)
        }.width('100%').padding({ top: 12, left: 24, right: 24 }).hitTestBehavior(HitTestMode.None)
      }
    } else {
      this.card()
    }
  }

  @Builder
  card() {
    Column() {
      Column({ space: 12 }) {
        if (this.screen.phase === 'opening') {
          LoadingProgress().width(28).height(28).color(Color.White)
          Text(t('等待对方同意…')).fontSize(17).fontWeight(FontWeight.Medium).fontColor(Color.White)
          Text(t('对方电脑上会弹窗问要不要让你看、能不能操作。')).fontSize(14).fontColor(Hero.muted).textAlign(TextAlign.Center)
          Button(t('取消')).width('100%').height(46).backgroundColor(Hero.accent).onClick(() => this.core.closeScreen())
        } else if (this.screen.phase === 'paused') {
          Image($r('app.media.lucide_circle_pause')).width(32).height(32)
          Text(this.screen.relay ? t('现在这条连接走的是中继，画面暂停') : t('正在找直连的路，画面暂停'))
            .fontSize(17).fontWeight(FontWeight.Medium).fontColor(Color.White).textAlign(TextAlign.Center)
          Text(t('远程画面只走直连，直连接通后自动接着放。')).fontSize(14).fontColor(Hero.muted).textAlign(TextAlign.Center)
        } else {
          Image($r('app.media.lucide_monitor')).width(32).height(32)
          Text(this.screen.closedReason === null ? t('远程画面已结束') : t('远程画面断开了'))
            .fontSize(17).fontWeight(FontWeight.Medium).fontColor(Color.White)
          if (this.screen.closedReason !== null) {
            Text(this.screen.closedReason).fontSize(14).fontColor(Hero.muted).textAlign(TextAlign.Center)
          }
          Row({ space: 12 }) {
            Button(t('关闭')).layoutWeight(1).height(46).backgroundColor(Hero.tile).fontColor(Hero.foreground)
              .onClick(() => this.core.closeScreen())
            if (this.core.conns.has(this.screen.endpointId)) {
              Button(t('重新打开')).layoutWeight(1).height(46).backgroundColor(Hero.accent).onClick(() => this.screen.open())
            }
          }.width('100%')
        }
      }
      .width('100%')
      .constraintSize({ maxWidth: 340 })
      .padding(24)
      .backgroundColor('#EB18181B')
      .borderRadius(22)
    }
    .width('100%').height('100%').justifyContent(FlexAlign.Center).padding(24)
  }

  /** 平时的工具栏：关闭、键盘、操作方式、更多（换屏幕、画质、收起） */
  @Builder
  toolbar() {
    Row({ space: 8 }) {
      this.roundButton(t('关闭远程画面'), $r('app.media.lucide_x'), () => this.core.closeScreen())
      if (this.screen.canControl) {
        this.roundButton(t('键盘'), $r('app.media.lucide_keyboard'), () => this.showKeyboard())
        this.roundButton(this.screen.mode === 'trackpad' ? t('触控板模式') : t('直接点模式'),
          this.screen.mode === 'trackpad' ? $r('app.media.lucide_mouse_pointer_2') : $r('app.media.lucide_pointer'), () => {
            this.screen.mode = this.screen.mode === 'trackpad' ? 'touch' : 'trackpad';
            this.core.notify(this.screen.mode === 'trackpad' ? t('触控板模式：手指滑动挪指针，点一下点在指针上')
              : t('直接点模式：手指点哪儿就点哪儿'));
          })
      }
      this.roundButton(t('更多'), $r('app.media.lucide_ellipsis'), () => {
        this.showMenu = true;
      })
    }
    .padding({ bottom: 28 })
  }

  @Builder
  roundButton(label: string, icon: Resource, action: () => void) {
    Button({ type: ButtonType.Circle, stateEffect: true }) {
      Image(icon).width(21).height(21)
    }
    .width(48).height(48)
    .accessibilityText(label)
    .onClick(action)
    .attributeModifier(new GlassButton('#8C000000'))
  }

  /** 键盘弹着时的按键条：手机键盘上没有的键。修饰键按一下粘住，下一个键或点击带上它 */
  @Builder
  keyBar() {
    Scroll() {
      Row({ space: 4 }) {
        ForEach(this.keys(), (k: KeyItem) => {
          this.keyButton(k, k.modifier !== null && this.screen.modifiers.includes(k.modifier))
        }, (k: KeyItem) => k.label + (k.modifier !== null && this.screen.modifiers.includes(k.modifier)))
      }
      .padding({ left: 6, right: 6, top: 8, bottom: 8 })
    }
    .scrollable(ScrollDirection.Horizontal)
    .scrollBar(BarState.Off)
    .width('100%')
    .backgroundColor(Hero.background)
  }

  @Builder
  keyButton(k: KeyItem, active: boolean) {
    Button({ type: ButtonType.Normal, stateEffect: true }) {
      if (k.icon !== null) {
        Image(k.icon).width(18).height(18)
      } else {
        Text(k.label).fontSize(13).fontWeight(FontWeight.Medium).fontFamily('monospace').maxLines(1)
          .fontColor(active ? Hero.accent : Hero.foreground)
      }
    }
    .constraintSize({ minWidth: 46 }).height(40).borderRadius(10).padding({ left: 10, right: 10 })
    .accessibilityText(k.label)
    .onClick(k.action)
    .attributeModifier(new GlassButton(active ? '#380485F7' : Hero.tile, active ? '#590485F7' : undefined))
  }

  /** 更多：换屏幕、画质、收起工具栏 */
  @Builder
  menu() {
    Column({ space: 2 }) {
      if ((this.screen.started?.displays.length ?? 0) > 1) {
        Text(t('屏幕')).fontSize(12.5).fontColor(Hero.muted).width('100%').padding({ left: 10, top: 6, bottom: 2 })
        ForEach(this.screen.started?.displays ?? [], (d: ScreenDisplay) => {
          this.menuRow(d.name, null, d.id === this.screen.display, () => {
            this.showMenu = false;
            this.screen.switchDisplay(d.id);
          })
        }, (d: ScreenDisplay) => d.id)
      }
      Text(t('画质')).fontSize(12.5).fontColor(Hero.muted).width('100%').padding({ left: 10, top: 6, bottom: 2 })
      this.menuRow(t('清晰'), t('原始分辨率，看小字'), this.screen.quality === 'sharp', () => this.pickQuality('sharp'))
      this.menuRow(t('均衡'), t('最高 1080p'), this.screen.quality === 'balanced', () => this.pickQuality('balanced'))
      this.menuRow(t('流畅'), t('720p、30 帧，网差时用'), this.screen.quality === 'smooth', () => this.pickQuality('smooth'))
      if (this.screen.fps >= 0) {
        Text(`${this.screen.fps} fps · ${(this.screen.bitrate / 1000000).toFixed(1)} Mbps`)
          .fontSize(12.5).fontColor(Hero.muted).fontFamily('monospace').width('100%').padding({ left: 10, top: 6, bottom: 6 })
      }
      this.menuRow(t('收起工具栏'), null, false, () => {
        this.showMenu = false;
        this.screen.toolbarCollapsed = true;
      })
    }
    .padding({ left: 16, right: 16, bottom: 24 })
  }

  pickQuality(q: string): void {
    this.showMenu = false;
    this.screen.setQuality(q);
  }

  @Builder
  menuRow(label: string, note: string | null, checked: boolean, action: () => void) {
    Row({ space: 14 }) {
      Column({ space: 2 }) {
        Text(label).fontSize(15.5).fontColor(Hero.foreground)
        if (note !== null) {
          Text(note).fontSize(12.5).fontColor(Hero.muted)
        }
      }.alignItems(HorizontalAlign.Start).layoutWeight(1)
      if (checked) {
        Image($r('app.media.lucide_check_accent')).width(20).height(20)
      }
    }
    .width('100%').constraintSize({ minHeight: 52 }).padding(10).borderRadius(14)
    .onClick(action)
  }
}

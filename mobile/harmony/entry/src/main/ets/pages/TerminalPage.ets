import { webview } from '@kit.ArkWeb';
import { pasteboard } from '@kit.BasicServicesKit';
import { Core, TermCommand, TermModel } from '../common/Core';
import { Hero } from '../common/Hero';
import { t } from '../common/I18n';
import { AgentView, ApprovalChoice, SessionView } from '../common/Model';
import { pathText, sessionTitles } from '../common/Format';
import { AgentStateLabel, SessionTile } from '../common/Widgets';
import { AgentBar, DirPicker } from './AgentBar';

/** 网页（终端）发回来的消息 */
interface WebMsg {
  t: string;
  d?: string;
  cols?: number;
  rows?: number;
}

/** 按键条上的一个键 */
class KeyItem {
  label: string;
  icon: Resource | null;
  action: () => void;

  constructor(label: string, icon: Resource | null, action: () => void) {
    this.label = label;
    this.icon = icon;
    this.action = action;
  }
}

/** 一个远程终端：上面是终端（ArkWeb 里的 xterm.js），下面是 AI 的确认选项、输入栏、按键条 */
@ComponentV2
export struct TerminalPage {
  @Param @Require termKey: number;
  @Local core: Core = Core.shared;
  @Local composing: boolean = false;
  @Local draft: string = '';
  @Local showKeys: boolean = false;
  @Local showSwitch: boolean = false;
  @Local showDir: boolean = false;
  private controller: webview.WebviewController = new webview.WebviewController();
  private ports: webview.WebMessagePort[] = [];
  private timer: number = -1;

  aboutToAppear(): void {
    const term = this.term();
    if (term !== undefined) {
      // AI 状态隔几秒刷新一次
      this.core.loadSessions(term.endpointId);
      this.timer = setInterval(() => this.core.loadSessions(term.endpointId), 3000);
    }
  }

  aboutToDisappear(): void {
    clearInterval(this.timer);
    this.term()?.detachView();
    this.ports.forEach((p: webview.WebMessagePort) => p.close());
  }

  term(): TermModel | undefined {
    return this.core.term(this.termKey);
  }

  session(): SessionView | null {
    const term = this.term();
    return term !== undefined ? (this.sessions().find((s: SessionView) => s.id === term.sessionId) ?? null) : null;
  }

  sessions(): SessionView[] {
    const term = this.term();
    return term !== undefined ? (this.core.sessions.get(term.endpointId)?.list ?? []) : [];
  }

  agent(): AgentView | null {
    const term = this.term();
    if (term === undefined || term.state !== 'open') {
      return null;
    }
    return this.sessions().find((s: SessionView) => s.id === term.sessionId)?.agent ?? null;
  }

  title(): string {
    const sid = this.term()?.sessionId ?? null;
    return sid !== null ? (sessionTitles(this.sessions()).get(sid) ?? t('新终端')) : t('新终端');
  }

  subtitle(): string {
    const term = this.term();
    if (term === undefined) {
      return '';
    }
    const name = this.core.deviceName(term.endpointId);
    if (term.state !== 'open') {
      return name + ' · ' + t('已断开');
    }
    const conn = this.core.conns.get(term.endpointId);
    return conn !== undefined ? `${name} · ${pathText(conn.path)}` : name;
  }

  /** 网页加载完：建消息端口，一头交给网页，一头留着收发 */
  connectWeb(): void {
    const term = this.term();
    if (term === undefined) {
      return;
    }
    this.ports = this.controller.createWebMessagePorts();
    this.ports[0].onMessageEvent((m: webview.WebMessage) => this.onWeb(m as string));
    this.controller.postMessage('init', [this.ports[1]], '*');
    term.attach(this.ports[0]);
  }

  onWeb(text: string): void {
    const term = this.term();
    if (term === undefined) {
      return;
    }
    const msg = JSON.parse(text) as WebMsg;
    if (msg.t === 'in' && msg.d !== undefined) {
      term.input(msg.d);
    } else if (msg.t === 'size' && msg.cols !== undefined && msg.rows !== undefined) {
      term.onSize(msg.cols, msg.rows);
    } else if (msg.t === 'copy') {
      if (msg.d !== undefined && msg.d.length > 0) {
        this.core.copyText(msg.d);
        this.core.notify(t('已复制'));
      } else {
        this.core.notify(t('要复制先长按终端里的文字，再拖动选择范围。'));
      }
    }
  }

  send(): void {
    const term = this.term();
    if (term === undefined || this.draft.length === 0) {
      return;
    }
    term.command({ cmd: 'paste', text: this.draft } as TermCommand);
    this.draft = '';
    // 粘贴和回车分开发：有的程序（Claude Code 这些）一起收到会把回车当成粘贴内容
    setTimeout(() => term.send('\r'), 80);
  }

  keys(term: TermModel): KeyItem[] {
    return [
      new KeyItem(t('输入栏'), $r(this.composing ? 'app.media.lucide_keyboard_accent' : 'app.media.lucide_keyboard'), () => {
        this.composing = !this.composing;
      }),
      new KeyItem('Esc', null, () => term.send('\u001b')),
      new KeyItem('Tab', null, () => term.send('\t')),
      new KeyItem('Ctrl', null, () => {
        term.ctrl = !term.ctrl;
      }),
      new KeyItem('↑', $r('app.media.lucide_arrow_up'), () => term.command({ cmd: 'arrow', dir: 'A' } as TermCommand)),
      new KeyItem('↓', $r('app.media.lucide_arrow_down'), () => term.command({ cmd: 'arrow', dir: 'B' } as TermCommand)),
      new KeyItem('←', $r('app.media.lucide_arrow_left'), () => term.command({ cmd: 'arrow', dir: 'D' } as TermCommand)),
      new KeyItem('→', $r('app.media.lucide_arrow_right'), () => term.command({ cmd: 'arrow', dir: 'C' } as TermCommand))
    ];
  }

  build() {
    NavDestination() {
      if (this.term() !== undefined) {
        Column() {
          Stack() {
            Web({ src: $rawfile('terminal/index.html'), controller: this.controller })
              .javaScriptAccess(true)
              .domStorageAccess(false)
              .fileAccess(false)
              .backgroundColor(Hero.terminal)
              .overScrollMode(OverScrollMode.NEVER)
              .onPageEnd(() => this.connectWeb())
            if (this.term()?.state === 'opening') {
              Row({ space: 10 }) {
                LoadingProgress().width(20).height(20).color(Hero.muted)
                Text(t('正在打开终端…')).fontSize(14).fontColor(Hero.muted)
              }
            } else if (this.term()?.state === 'closed') {
              this.closedCard()
            }
          }
          .layoutWeight(1)
          .width('100%')
          .backgroundColor(Hero.terminal)
          // 三个面板各挂一个组件：同一个组件挂两个 bindSheet，后挂的会把前一个顶掉
          .bindSheet(this.showDir!!, this.dirSheet(), {
            height: SheetSize.LARGE,
            backgroundColor: Hero.surface,
            title: { title: t('选择工作目录') }
          })

          this.approvalPanel()
          if (this.composing && this.term()?.state === 'open') {
            this.composeBar()
          }
          if (this.term()?.state === 'open') {
            AgentBar({ term: this.term() as TermModel, session: this.session(), onPickDir: () => {
              this.showDir = true;
            } })
            this.keyBar()
          }
        }
        .width('100%')
        .height('100%')
        // 两个面板挂在不同的组件上：同一个组件挂两个 bindSheet，后挂的会把前一个顶掉
        .bindSheet(this.showKeys, this.keysPanel(), {
          height: SheetSize.FIT_CONTENT,
          backgroundColor: Hero.surface,
          title: { title: t('更多按键') },
          onDisappear: () => {
            this.showKeys = false;
          }
        })
      }
    }
    .title(this.title())
    .menus([
      { value: t('开着的终端'), icon: $r('app.media.menu_square_terminal'), action: () => {
        this.showSwitch = true;
      } },
      { value: t('新建终端'), icon: $r('app.media.menu_plus'), action: () => {
        const term = this.term();
        if (term !== undefined) {
          this.core.openTerminal(term.endpointId, null, true);
        }
      } },
      { value: t('关闭这个终端'), icon: $r('app.media.menu_x'), action: () => {
        this.core.closeTerminal(this.termKey);
      } }
    ])
    .backgroundColor(Hero.terminal)
    .bindSheet(this.showSwitch, this.switchPanel(), {
      height: SheetSize.MEDIUM,
      backgroundColor: Hero.surface,
      title: { title: t('开着的终端'), subtitle: this.subtitle() },
      onDisappear: () => {
        this.showSwitch = false;
      }
    })
  }

  @Builder
  dirSheet() {
    if (this.term() !== undefined) {
      DirPicker({ term: this.term() as TermModel, session: this.session(), onDone: () => {
        this.showDir = false;
      } })
    }
  }

  @Builder
  closedCard() {
    Column({ space: 10 }) {
      Text(this.term()?.exited ? t('这个终端已经结束') : t('连接已断开')).fontSize(18).fontWeight(FontWeight.Medium)
        .fontColor(Hero.foreground)
      if (this.term()?.note) {
        Text(this.term()?.note ?? '').fontSize(14).fontColor(Hero.foreground).textAlign(TextAlign.Center)
      }
      Text(t('会话还在对方电脑上运行的话，重新连上就能接着用。')).fontSize(14).fontColor(Hero.muted).textAlign(TextAlign.Center)
      Button(this.term()?.exited ? t('新建终端') : t('重新连接')).width('100%').height(46).backgroundColor(Hero.accent)
        .onClick(() => {
          const term = this.term();
          if (term !== undefined) {
            this.core.reopen(term);
          }
        })
      Button(t('关闭')).width('100%').height(42).backgroundColor(Color.Transparent).fontColor(Hero.accent)
        .onClick(() => this.core.closeTerminal(this.termKey))
    }
    .padding(20).width('86%').constraintSize({ maxWidth: 340 }).borderRadius(22).backgroundColor(Hero.surface)
  }

  /** AI 等你确认：选项直接点，不用到终端里按方向键 */
  @Builder
  approvalPanel() {
    if (this.agent() !== null && this.agent()?.state === 'waiting' && this.agent()?.approval) {
      Column({ space: 6 }) {
        Text(t('{{name}} · 等待确认', ['name', this.agent()?.name ?? ''])).fontSize(12).fontColor(Hero.muted)
        Text(this.agent()?.approval?.context ?? '').fontSize(13).fontColor(Hero.foreground)
        ForEach(this.agent()?.approval?.options ?? [], (opt: ApprovalChoice) => {
          Row({ space: 9 }) {
            Text(`${opt.id + 1}`).fontSize(12).fontColor(Hero.foreground).width(22).height(22).textAlign(TextAlign.Center)
              .borderRadius(7).backgroundColor(Hero.surfaceSecondary)
            Text(opt.label).fontSize(14).fontColor(Hero.foreground).layoutWeight(1)
            Image($r('app.media.lucide_square_arrow_out_up_right_muted')).width(15).height(15)
          }
          .width('100%').constraintSize({ minHeight: 46 }).padding(8)
          .onClick(() => {
            const term = this.term();
            const approval = this.agent()?.approval;
            if (term !== undefined && approval) {
              this.core.chooseOption(term, approval.id, opt.id);
            }
          })
        }, (opt: ApprovalChoice) => `${opt.id}`)
      }
      .alignItems(HorizontalAlign.Start).padding(10).margin({ left: 8, right: 8, top: 8 }).borderRadius(16)
      .backgroundColor(Hero.surface)
    }
  }

  @Builder
  composeBar() {
    Row({ space: 8 }) {
      TextArea({ placeholder: this.agent() !== null ? t('发给 {{name}}', ['name', this.agent()?.name ?? ''])
        : t('输入命令，发送后自动回车'), text: this.draft })
        .layoutWeight(1).constraintSize({ maxHeight: 120 }).backgroundColor(Hero.surface).borderRadius(21)
        .fontColor(Hero.foreground)
        .onChange((v: string) => {
          this.draft = v;
        })
      Button() {
        Image($r('app.media.lucide_send_horizontal')).width(20).height(20)
      }
      .width(44).height(44).backgroundColor(this.draft.length > 0 ? Hero.accent : Hero.tile)
      .enabled(this.draft.length > 0)
      .accessibilityText(t('发送'))
      .onClick(() => this.send())
    }
    .alignItems(VerticalAlign.Bottom).padding({ left: 8, right: 8, top: 8 })
  }

  /** 终端下面一排最常用的键，一屏放得下，不用左右滑；不常用的在“⋯”里 */
  @Builder
  keyBar() {
    Row({ space: 4 }) {
      ForEach(this.keys(this.term() as TermModel), (k: KeyItem) => {
        this.keyButton(k.label, k.icon, k.label === 'Ctrl' && (this.term()?.ctrl ?? false), k.action)
      }, (k: KeyItem) => k.label)
      // 粘贴用系统的粘贴控件：读剪贴板不用申请权限
      Stack() {
        PasteButton({ icon: PasteIconStyle.LINES, buttonType: ButtonType.Normal })
          .iconColor(Hero.foreground).iconSize(18).backgroundColor(Hero.tile).borderRadius(10)
          .width('100%').height(40).padding(0)
          .onClick((event: ClickEvent, result: PasteButtonOnClickResult) => {
            if (result === PasteButtonOnClickResult.SUCCESS) {
              const data = pasteboard.getSystemPasteboard().getDataSync();
              const text = data.getPrimaryText();
              if (text) {
                this.term()?.command({ cmd: 'paste', text: text } as TermCommand);
              }
            }
          })
      }
      .layoutWeight(1).height(40)
      this.keyButton(t('更多按键'), $r('app.media.lucide_ellipsis'), false, () => {
        this.showKeys = true;
      })
    }
    .width('100%').padding({ left: 6, right: 6, top: 8, bottom: 8 })
  }

  @Builder
  keyButton(label: string, icon: Resource | null, active: boolean, action: () => void) {
    Stack() {
      if (icon !== null) {
        Image(icon).width(18).height(18)
      } else {
        Text(label).fontSize(13).fontWeight(FontWeight.Medium).fontFamily('monospace').maxLines(1)
          .fontColor(active ? Hero.accent : Hero.foreground)
      }
    }
    .layoutWeight(1).height(40).borderRadius(10)
    .backgroundColor(active ? '#380485F7' : Hero.tile)
    .accessibilityText(label)
    .onClick(action)
  }

  /** “⋯”里不常用的键：按一下就发，面板跟着关上，回到终端 */
  @Builder
  keysPanel() {
    Column({ space: 8 }) {
      Grid() {
        ForEach([['⇧Tab', '\u001b[Z'], ['^C', '\u0003'], ['^D', '\u0004'], ['^Z', '\u001a'], ['^L', '\u000c'],
          ['^R', '\u0012'], ['|', '|'], ['~', '~'], ['/', '/'], ['-', '-']], (k: string[]) => {
          GridItem() {
            this.keyButton(k[0], null, false, () => {
              this.showKeys = false;
              this.term()?.send(k[1]);
            })
          }
        }, (k: string[]) => k[0])
      }
      .columnsTemplate('1fr 1fr 1fr 1fr 1fr').rowsGap(8).columnsGap(8).height(88)
      Row({ space: 8 }) {
        this.keyButton('Alt', null, this.term()?.alt ?? false, () => {
          const term = this.term();
          if (term !== undefined) {
            term.alt = !term.alt;
          }
          this.showKeys = false;
        })
        // 终端里的撤销：bash、zsh、Claude Code 的输入框都是 Ctrl+_
        this.keyButton(t('撤销'), null, false, () => {
          this.showKeys = false;
          this.term()?.send('\u001f');
        })
        this.keyButton(t('复制'), null, false, () => {
          this.showKeys = false;
          this.term()?.command({ cmd: 'copy' } as TermCommand);
        })
      }
    }
    .padding({ left: 16, right: 16, bottom: 24 })
  }

  @Builder
  switchPanel() {
    List() {
      ForEach(this.core.terms, (other: TermModel) => {
        ListItem() {
          Row({ space: 12 }) {
            SessionTile({ agent: null, dimmed: other.state === 'closed' })
            Column({ space: 3 }) {
              Text(other.sessionId !== null ? (sessionTitles(this.core.sessions.get(other.endpointId)?.list ?? []).get(other.sessionId) ?? t('新终端'))
                : t('新终端')).fontSize(16).fontColor(Hero.foreground)
              Text(this.core.deviceName(other.endpointId) + (other.state === 'closed' ? ' · ' + t('已断开') : ''))
                .fontSize(13).fontColor(Hero.muted)
            }.alignItems(HorizontalAlign.Start).layoutWeight(1)
            if (other.key === this.termKey) {
              Image($r('app.media.lucide_check_accent')).width(16).height(16)
            }
            Image($r('app.media.lucide_x_muted')).width(20).height(20).accessibilityText(t('关闭'))
              .onClick(() => {
                if (this.core.terms.length === 1) {
                  this.showSwitch = false;
                }
                this.core.closeTerminal(other.key);
              })
          }.width('100%').padding(10)
        }
        .onClick(() => {
          this.showSwitch = false;
          this.core.openTerminal(other.endpointId, other.sessionId, false);
        })
      }, (other: TermModel) => `${other.key}`)
    }
    .padding({ left: 12, right: 12 })
  }
}

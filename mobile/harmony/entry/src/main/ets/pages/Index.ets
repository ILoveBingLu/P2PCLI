import { common } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { scanBarcode, scanCore } from '@kit.ScanKit';
import p2pcli from 'libp2pcli.so';
import { ConnectPrompt, Core, SessionsState, TermModel } from '../common/Core';
import { Hero } from '../common/Hero';
import { I18n, t } from '../common/I18n';
import { AgentView, DeviceView, PairInfo, SessionView, TransferView } from '../common/Model';
import { presenceOf, sessionTitles } from '../common/Format';
import { AgentStateLabel, Chip, PlatformTile, sectionHeader, SessionTile } from '../common/Widgets';
import { DevicePage } from './DevicePage';
import { TerminalPage } from './TerminalPage';
import { SettingsPage } from './SettingsPage';

/**
 * 首页：开着的终端、连过的设备、局域网里的设备。用系统的 Navigation：
 * 窄屏一层层推进去，宽屏（折叠屏展开、平板）左边一直是设备列表，右边是设备、终端或设置
 */
@Entry
@ComponentV2
struct Index {
  @Local core: Core = Core.shared;
  @Local i18n: I18n = I18n.shared;
  /** 面板开关：跟着 core.prompt、core.fileRequest 走，用户下滑关掉时反过来清掉它们 */
  @Local showConnect: boolean = false;
  @Local showFileRequest: boolean = false;

  @Monitor('core.prompt')
  onPrompt(): void {
    this.showConnect = this.core.prompt !== null;
  }

  @Monitor('core.fileRequest')
  onFileRequest(): void {
    this.showFileRequest = this.core.fileRequest !== null;
  }

  aboutToAppear(): void {
    hilog.info(0x0001, 'p2pcli', 'index: appear');
    try {
      const ctx = AppStorage.get<common.UIAbilityContext>('context') as common.UIAbilityContext;
      I18n.shared.init(ctx);
      this.core.startCore(ctx);
    } catch (e) {
      const err = e as Error;
      hilog.error(0x0001, 'p2pcli', 'index: start threw %{public}s %{public}s', err.message, err.stack ?? '');
      this.core.startState = 'failed';
      this.core.startError = err.message;
    }
  }

  @Builder
  pageMap(name: string, param: Object) {
    if (name === 'device') {
      DevicePage({ endpointId: param as string })
    } else if (name === 'terminal') {
      TerminalPage({ termKey: param as number })
    } else if (name === 'settings') {
      SettingsPage()
    }
  }

  /** 标题：软件的 logo 和名字（logo 和 Mac、iOS 同一个液态玻璃图标） */
  @Builder
  brandTitle() {
    Row({ space: 10 }) {
      Image($r('app.media.logo')).width(32).height(32)
      Text('P2PCLI').fontSize(24).fontWeight(FontWeight.Bold).fontColor(Hero.foreground)
    }
    .height('100%')
    .padding({ left: 16 })
  }

  scan(): void {
    const ctx = this.core.ctx;
    if (ctx === undefined || this.core.connecting !== null) {
      return;
    }
    this.core.prompt = null;
    const options: scanBarcode.ScanOptions = { scanTypes: [scanCore.ScanType.QR_CODE], enableMultiMode: false, enableAlbum: true };
    scanBarcode.startScanForResult(ctx, options).then((r: scanBarcode.ScanResult) => {
      const text = p2pcli.parsePairLink(r.originalValue);
      if (text === null) {
        this.core.notify(t('这不是 P2PCLI 的连接二维码'));
        return;
      }
      this.core.connectPair(JSON.parse(text) as PairInfo);
    }).catch(() => {
      // 用户返回了，什么都不做
    });
  }

  build() {
    if (this.core.startState === 'ready') {
      // 两个面板挂在不同的组件上：同一个组件挂两个 bindSheet，后挂的会把前一个顶掉
      Stack() {
      Navigation(this.core.stack) {
        this.home()
      }
      .title(this.brandTitle())
      .titleMode(NavigationTitleMode.Mini)
      // 首页是最上层：不要返回箭头
      .hideBackButton(true)
      .menus([
        { value: t('扫码连接'), icon: $r('app.media.menu_scan_line'), action: () => this.scan() },
        { value: t('设置'), icon: $r('app.media.menu_settings'), action: () => this.core.openSettings() }
      ])
      .toolbarConfiguration([
        { value: t('连接新设备'), icon: $r('app.media.menu_plus'), action: () => this.core.newPrompt() }
      ], {
        // 系统的半透明模糊底，列表从它下面透出来，不是一条实心的黑底
        backgroundColor: Color.Transparent,
        backgroundBlurStyle: BlurStyle.COMPONENT_THICK
      })
      .mode(NavigationMode.Auto)
      .navBarWidth(360)
      .navBarWidthRange([300, 420])
      .minContentWidth(360)
      .navDestination(this.pageMap)
      .backgroundColor(Hero.background)
      .expandSafeArea([SafeAreaType.SYSTEM], [SafeAreaEdge.TOP, SafeAreaEdge.BOTTOM])
      .bindSheet(this.showConnect!!, this.connectSheet(), {
        height: SheetSize.LARGE,
        backgroundColor: Hero.surface,
        title: { title: this.core.prompt?.endpointId ? t('连接 {{name}}', ['name', this.core.prompt?.name ?? t('设备')]) : t('连接新设备') },
        onDisappear: () => {
          this.core.prompt = null;
        }
      })
      }
      .width('100%')
      .height('100%')
      .bindSheet(this.showFileRequest!!, this.fileRequestSheet(), {
        height: SheetSize.FIT_CONTENT,
        backgroundColor: Hero.surface,
        title: { title: t('{{name}} 想发文件给你', ['name', this.core.fileRequest?.name ?? '']) },
        onDisappear: () => {
          if (this.core.fileRequest !== null) {
            this.core.respondFiles(false);
          }
        }
      })
    } else if (this.core.startState === 'failed') {
      Column({ space: 14 }) {
        Image($r('app.media.lucide_triangle_alert_muted')).width(40).height(40)
        Text(t('启动失败')).fontSize(22).fontWeight(FontWeight.Bold).fontColor(Hero.foreground)
        Text(this.core.startError).fontSize(14).fontColor(Hero.muted).textAlign(TextAlign.Center)
        Button(t('重试')).backgroundColor(Hero.accent).onClick(() => {
          if (this.core.ctx !== undefined) {
            this.core.startCore(this.core.ctx);
          }
        })
      }
      .width('100%').height('100%').justifyContent(FlexAlign.Center).padding(32).backgroundColor(Hero.background)
    } else {
      Column({ space: 20 }) {
        Image($r('app.media.logo')).width(68).height(68)
        LoadingProgress().width(28).height(28).color(Hero.muted)
      }
      .width('100%').height('100%').justifyContent(FlexAlign.Center).backgroundColor(Hero.background)
    }
  }

  @Builder
  home() {
    List() {
      ListItem() {
        TransfersCard({ transfers: this.core.transfers })
      }

      // 扫码连的设备列表里还没有：顶上放一张“正在连接”的卡片
      if (this.core.connecting !== null && this.core.device(this.core.connecting) === undefined) {
        ListItem() {
          Row({ space: 12 }) {
            LoadingProgress().width(24).height(24).color(Hero.accent)
            Column({ space: 3 }) {
              Text(t('正在连接 {{name}}', ['name', this.core.connectingName ?? t('电脑')]))
                .fontSize(16).fontWeight(FontWeight.Medium).fontColor(Hero.foreground)
              Text(t('等对方在电脑上点同意')).fontSize(13).fontColor(Hero.muted)
            }.alignItems(HorizontalAlign.Start)
          }
          .width('100%').padding(14).borderRadius(16).backgroundColor(Hero.surface).margin({ top: 10 })
        }
      }

      if (this.core.overview !== null && (this.core.overview.server.state === 'none' || this.core.overview.server.state === 'offline')) {
        ListItem() {
          Row({ space: 8 }) {
            Image($r('app.media.lucide_triangle_alert_warning')).width(16).height(16)
            Text(this.core.overview.server.state === 'none'
              ? t('还没配置牵线服务器，只能连同一个局域网里的设备')
              : t('牵线服务器没连上，只能连同一个局域网里的设备')).fontSize(13).fontColor(Hero.warning).layoutWeight(1)
          }
          .width('100%').padding(12).borderRadius(14).backgroundColor('#1FF5A524').margin({ top: 10 })
        }
      }

      if (this.core.overview !== null && this.core.overview.devices.length === 0 && this.core.overview.nearby.length === 0) {
        ListItem() {
          this.emptyHome()
        }
      } else {
        if (this.core.terms.length > 0) {
          ListItem() {
            sectionHeader(t('正在使用'))
          }
          ListItemGroup() {
            ForEach(this.core.terms, (term: TermModel) => {
              ListItem() {
                TermRow({ term: term })
              }
              .onClick(() => this.core.openTerminal(term.endpointId, term.sessionId, false))
            }, (term: TermModel) => `${term.key}`)
          }
          // 卡片：标题在外面，组和组之间靠标题上方的留白隔开（和安卓、iOS 一样）
          .divider({ strokeWidth: 0.5, color: '#28282C', startMargin: 66, endMargin: 12 })
          .backgroundColor(Hero.surface).borderRadius(16).clip(true)
        }
        if (this.core.overview !== null && this.core.overview.devices.length > 0) {
          ListItem() {
            sectionHeader(t('我的设备'))
          }
          ListItemGroup() {
            ForEach(this.core.overview.devices, (d: DeviceView) => {
              ListItem() {
                DeviceRow({ device: d, live: this.core.sessions.get(d.endpoint_id) ?? null })
              }
              .onClick(() => this.core.openDevice(d))
            }, (d: DeviceView) => d.endpoint_id)
          }
          // 卡片：标题在外面，组和组之间靠标题上方的留白隔开（和安卓、iOS 一样）
          .divider({ strokeWidth: 0.5, color: '#28282C', startMargin: 66, endMargin: 12 })
          .backgroundColor(Hero.surface).borderRadius(16).clip(true)
        }
        ListItem() {
          this.nearbyHeader()
        }
        ListItemGroup() {
          if (this.core.overview === null || this.core.overview.nearby.length === 0) {
            ListItem() {
              Text(t('同一个局域网里没找到别的设备')).fontSize(14).fontColor(Hero.muted).padding(14).width('100%')
            }
          } else {
            ForEach(this.core.overview.nearby, (d: DeviceView) => {
              ListItem() {
                DeviceRow({ device: d, live: null })
              }
              .onClick(() => this.core.openDevice(d))
            }, (d: DeviceView) => d.endpoint_id)
          }
        }
        // 卡片：标题在外面，组和组之间靠标题上方的留白隔开（和安卓、iOS 一样）
        .divider({ strokeWidth: 0.5, color: '#28282C', startMargin: 66, endMargin: 12 })
          .backgroundColor(Hero.surface).borderRadius(16).clip(true)
      }
    }
    .width('100%')
    .height('100%')
    .padding({ left: 16, right: 16 })
    .scrollBar(BarState.Off)
    .edgeEffect(EdgeEffect.Spring)
  }

  @Builder
  nearbyHeader() {
    Row() {
      Text(t('附近设备')).fontSize(13).fontColor(Hero.muted).fontWeight(FontWeight.Medium).layoutWeight(1)
      Image($r('app.media.lucide_refresh_cw_accent')).width(16).height(16)
        .accessibilityText(t('刷新附近设备'))
        .onClick(() => this.core.scanNearby())
    }
    .width('100%').padding({ left: 4, right: 8, top: 18, bottom: 8 })
  }

  @Builder
  emptyHome() {
    Column({ space: 12 }) {
      Image($r('app.media.logo')).width(64).height(64)
      Text(t('连接你的电脑')).fontSize(22).fontWeight(FontWeight.Bold).fontColor(Hero.foreground)
      Text(t('在电脑上打开 P2PCLI，点标题栏的手机图标，用这里扫它显示的二维码。'))
        .fontSize(14).fontColor(Hero.muted).textAlign(TextAlign.Center)
      Button() {
        Row({ space: 8 }) {
          Image($r('app.media.lucide_scan_line')).width(18).height(18)
          Text(t('扫码连接')).fontSize(16).fontColor(Color.White)
        }
      }
      .width('100%').height(48).backgroundColor(Hero.accent).margin({ top: 8 })
      .onClick(() => this.scan())
      Button() {
        Row({ space: 8 }) {
          Image($r('app.media.lucide_key_round')).width(18).height(18)
          Text(t('输入设备码')).fontSize(16).fontColor(Hero.foreground)
        }
      }
      .width('100%').height(48).backgroundColor(Hero.tile)
      .onClick(() => this.core.newPrompt())
      Button() {
        Row({ space: 8 }) {
          Image($r('app.media.lucide_refresh_cw_accent')).width(18).height(18)
          Text(t('找找附近的设备')).fontSize(15).fontColor(Hero.accent)
        }
      }
      .backgroundColor(Color.Transparent)
      .onClick(() => this.core.scanNearby())
    }
    .width('100%').padding({ top: 48, left: 8, right: 8 }).constraintSize({ maxWidth: 420 })
  }

  @Builder
  connectSheet() {
    if (this.core.prompt !== null) {
      ConnectSheet({ prompt: this.core.prompt, onScan: () => this.scan() })
    }
  }

  @Builder
  fileRequestSheet() {
    Column({ space: 14 }) {
      if (this.core.fileRequest !== null) {
        Text(t('{{count}} 个文件：{{items}}', ['count', this.core.fileRequest.files], ['items', this.core.fileRequest.items.join(t('、'))])).fontSize(14).fontColor(Hero.muted)
      }
      Button(t('接收')).width('100%').height(46).backgroundColor(Hero.accent).onClick(() => this.core.respondFiles(true))
      Button(t('拒绝')).width('100%').height(46).backgroundColor(Hero.tile).fontColor(Hero.foreground)
        .onClick(() => this.core.respondFiles(false))
    }
    .padding({ left: 20, right: 20, bottom: 24 })
  }
}

/** 正在收发的文件和进度 */
@ComponentV2
struct TransfersCard {
  @Param transfers: TransferView[] = [];

  build() {
    Column({ space: 8 }) {
      ForEach(this.transfers.filter((x: TransferView) => x.state !== 'done' && x.state !== 'failed'), (x: TransferView) => {
        Row({ space: 12 }) {
          Image(x.outgoing ? $r('app.media.lucide_arrow_up_accent') : $r('app.media.lucide_arrow_down_accent'))
            .width(18).height(18).padding(9).borderRadius(18).backgroundColor(Hero.accentSoft)
          Column({ space: 4 }) {
            Text(x.label).fontSize(15).fontColor(Hero.foreground).maxLines(1).textOverflow({ overflow: TextOverflow.Ellipsis })
            Text(this.status(x)).fontSize(12).fontColor(Hero.muted)
            Progress({ value: x.total > 0 ? x.done / x.total * 100 : 0, total: 100, type: ProgressType.Linear })
              .color(Hero.accent).width('100%')
          }.alignItems(HorizontalAlign.Start).layoutWeight(1)
        }
        .width('100%').padding(14).borderRadius(16).backgroundColor(Hero.surface).margin({ top: 10 })
      }, (x: TransferView) => x.id)
    }
  }

  status(x: TransferView): string {
    if (x.state === 'waiting') {
      return t('等 {{name}} 同意', ['name', x.peer_name]);
    }
    if (x.state === 'connecting') {
      return t('正在连接 {{name}}', ['name', x.peer_name]);
    }
    const who = x.outgoing ? t('发给 {{name}}', ['name', x.peer_name])
      : t('来自 {{name}}', ['name', x.peer_name]);
    const pct = x.total > 0 ? Math.floor(x.done / x.total * 100) : 0;
    return `${who} · ${pct}%`;
  }
}

/** 开着的一个终端 */
@ComponentV2
struct TermRow {
  @Param @Require term: TermModel;
  @Local core: Core = Core.shared;

  build() {
    Row({ space: 12 }) {
      SessionTile({ agent: this.agent(), dimmed: this.term.state === 'closed' })
      Column({ space: 3 }) {
        Text(this.title()).fontSize(16).fontWeight(FontWeight.Medium).fontColor(Hero.foreground).maxLines(1)
        Row({ space: 8 }) {
          Text(this.core.deviceName(this.term.endpointId) + (this.term.state === 'closed' ? ' · ' + t('已断开') : ''))
            .fontSize(13).fontColor(Hero.muted).maxLines(1)
          AgentStateLabel({ agent: this.agent() })
        }
      }.alignItems(HorizontalAlign.Start).layoutWeight(1)
    }
    .width('100%').padding({ left: 12, right: 12, top: 10, bottom: 10 })
  }

  list(): SessionView[] {
    return this.core.sessions.get(this.term.endpointId)?.list ?? [];
  }

  agent(): AgentView | null {
    if (this.term.state !== 'open') {
      return null;
    }
    const s = this.list().find((x: SessionView) => x.id === this.term.sessionId);
    return s?.agent ?? null;
  }

  title(): string {
    const sid = this.term.sessionId;
    return sid !== null ? (sessionTitles(this.list()).get(sid) ?? t('新终端')) : t('新终端');
  }
}

/** 设备列表里的一行 */
@ComponentV2
struct DeviceRow {
  @Param @Require device: DeviceView;
  @Param live: SessionsState | null = null;
  @Local core: Core = Core.shared;

  build() {
    Row({ space: 12 }) {
      PlatformTile({ platform: this.device.platform, connected: this.core.conns.has(this.device.endpoint_id) })
      Column({ space: 3 }) {
        Text(this.device.remark ?? this.device.name).fontSize(16).fontWeight(FontWeight.Medium).fontColor(Hero.foreground).maxLines(1)
        Row({ space: 8 }) {
          if (this.core.connecting === this.device.endpoint_id) {
            Text(t('等待对方同意…')).fontSize(13).fontColor(Hero.accent)
          } else {
            Text(presenceOf(this.device, this.core.conns.has(this.device.endpoint_id)).text).fontSize(13)
              .fontColor(presenceOf(this.device, this.core.conns.has(this.device.endpoint_id)).color)
            if (this.device.lan_online && !this.core.conns.has(this.device.endpoint_id) && this.device.known) {
              Chip({ text: t('局域网') })
            }
            if (this.sessions().length > 0) {
              Text(t('{{count}} 个会话', ['count', this.sessions().length])).fontSize(13).fontColor(Hero.muted)
            }
          }
        }
        if (this.waiting() > 0) {
          Text(t('{{count}} 个 AI 在等你确认', ['count', this.waiting()]))
            .fontSize(13).fontWeight(FontWeight.Medium).fontColor(Hero.warning)
        }
      }.alignItems(HorizontalAlign.Start).layoutWeight(1)
      if (this.core.connecting === this.device.endpoint_id) {
        LoadingProgress().width(22).height(22).color(Hero.accent)
      } else {
        Image($r('app.media.lucide_chevron_right_muted')).width(18).height(18)
      }
    }
    .width('100%').padding({ left: 12, right: 12, top: 10, bottom: 10 })
    .opacity(this.core.connecting !== null && this.core.connecting !== this.device.endpoint_id ? 0.5 : 1)
  }

  sessions(): SessionView[] {
    return this.live?.list ?? this.device.sessions;
  }

  waiting(): number {
    return this.sessions().filter((s: SessionView) => s.agent !== null && s.agent.state === 'waiting').length;
  }
}

/** 连接面板：扫码，或者输入设备码和配对密码 */
@ComponentV2
struct ConnectSheet {
  @Param @Require prompt: ConnectPrompt;
  @Event onScan: () => void = () => {};
  @Local core: Core = Core.shared;
  @Local code: string = '';
  @Local password: string = '';

  build() {
    Column({ space: 16 }) {
      if (this.prompt.endpointId === null) {
        Button() {
          Row({ space: 8 }) {
            Image($r('app.media.lucide_scan_line')).width(18).height(18)
            Text(t('扫码连接')).fontSize(16).fontColor(Color.White)
          }
        }
        .width('100%').height(48).backgroundColor(Hero.accent).enabled(this.core.connecting === null)
        .onClick(() => this.onScan())
        Text(t('在电脑上点标题栏的手机图标，会显示二维码')).fontSize(12).fontColor(Hero.muted)
        Text(t('或者输入设备码')).fontSize(14).fontWeight(FontWeight.Medium).fontColor(Hero.foreground).width('100%')
        TextInput({ placeholder: '123 456 789', text: this.code })
          .type(InputType.Number).maxLength(9).fontSize(22).fontColor(Hero.foreground)
          .height(52).backgroundColor(Hero.tile).borderRadius(12)
          .enabled(this.core.connecting === null)
          .onChange((v: string) => {
            this.code = v.replace(new RegExp('[^0-9]', 'g'), '');
          })
      }
      Text(t('配对密码')).fontSize(14).fontWeight(FontWeight.Medium).fontColor(Hero.foreground).width('100%')
      TextInput({ placeholder: t('输入对方的配对密码'), text: this.password })
        .type(InputType.Password).height(48).backgroundColor(Hero.tile).borderRadius(12).fontColor(Hero.foreground)
        .enterKeyType(EnterKeyType.Go)
        .enabled(this.core.connecting === null)
        .onChange((v: string) => {
          this.password = v;
        })
        .onSubmit(() => this.submit())
      if (this.prompt.error !== null) {
        Text(this.prompt.error).fontSize(14).fontColor(Hero.danger).width('100%')
          .padding({ left: 14, right: 14, top: 10, bottom: 10 }).borderRadius(12).backgroundColor(Hero.dangerSoft)
      }
      Button() {
        Row({ space: 8 }) {
          if (this.core.connecting !== null) {
            LoadingProgress().width(20).height(20).color(Color.White)
          }
          Text(this.core.connecting !== null ? t('等待对方同意…') : t('连接')).fontSize(16).fontColor(Color.White)
        }
      }
      .width('100%').height(48).backgroundColor(Hero.accent)
      .enabled(this.core.connecting === null && (this.prompt.endpointId !== null || this.code.length === 9))
      .onClick(() => this.submit())
      Row({ space: 6 }) {
        Image($r('app.media.lucide_lock_keyhole_muted')).width(14).height(14)
        Text(t('连接时对方电脑上会弹窗问要不要同意')).fontSize(12).fontColor(Hero.muted)
      }
    }
    .padding({ left: 20, right: 20, top: 8, bottom: 24 })
  }

  submit(): void {
    this.core.submitPrompt(this.prompt.endpointId !== null ? this.prompt.target : this.code, this.password);
  }
}

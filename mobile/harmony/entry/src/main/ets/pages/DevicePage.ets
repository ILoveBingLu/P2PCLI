import { picker } from '@kit.CoreFileKit';
import { Core, SessionsState, TermModel } from '../common/Core';
import { Hero } from '../common/Hero';
import { t, tr } from '../common/I18n';
import { Connected, DeviceView, SessionView } from '../common/Model';
import { pathText, relativeTime, sessionTitles, shortPath } from '../common/Format';
import { AgentStateLabel, Chip, PlatformTile, sectionHeader, SessionTile } from '../common/Widgets';

/** 一台设备：它上面的后台会话，新建终端、发文件、断开 */
@ComponentV2
export struct DevicePage {
  @Param @Require endpointId: string;
  @Local core: Core = Core.shared;
  @Local ending: SessionView | null = null;
  private timer: number = -1;

  aboutToAppear(): void {
    // 会话列表和 AI 状态隔几秒刷新一次
    this.core.loadSessions(this.endpointId);
    this.timer = setInterval(() => this.core.loadSessions(this.endpointId), 3000);
  }

  aboutToDisappear(): void {
    clearInterval(this.timer);
  }

  conn(): Connected | undefined {
    return this.core.conns.get(this.endpointId);
  }

  device(): DeviceView | undefined {
    return this.core.device(this.endpointId);
  }

  state(): SessionsState | undefined {
    return this.core.sessions.get(this.endpointId);
  }

  list(): SessionView[] {
    return (this.state()?.list ?? []).slice().sort((a: SessionView, b: SessionView) => b.created_at - a.created_at);
  }

  pickFiles(): void {
    const ctx = this.core.ctx;
    if (ctx === undefined) {
      return;
    }
    const options = new picker.DocumentSelectOptions();
    options.maxSelectNumber = 20;
    new picker.DocumentViewPicker(ctx).select(options).then((uris: string[]) => {
      this.core.sendFiles(this.endpointId, uris);
    }).catch(() => {
    });
  }

  build() {
    NavDestination() {
      List() {
        ListItem() {
          Column({ space: 10 }) {
            PlatformTile({ platform: this.device()?.platform ?? this.conn()?.platform ?? '', connected: this.conn() !== undefined, size: 76 })
            if (this.conn() !== undefined) {
              Text(t('已连接') + ' · ' + pathText(this.conn()?.path ?? null)).fontSize(13)
                .fontColor(this.conn()?.path.kind === 'relay' ? Hero.warning : Hero.success)
            } else {
              Text(t('没有连着')).fontSize(13).fontColor(Hero.muted)
            }
          }.width('100%').padding({ top: 12, bottom: 6 })
        }

        if (this.conn() === undefined) {
          ListItem() {
            Column({ space: 12 }) {
              Text(t('没有连着这台设备')).fontSize(18).fontWeight(FontWeight.Medium).fontColor(Hero.foreground)
              Text(t('连接断开了，后台会话还在对方电脑上运行，重新连上就能接着用。')).fontSize(14).fontColor(Hero.muted)
                .textAlign(TextAlign.Center)
              Button(this.core.connecting === this.endpointId ? t('等待对方同意…') : t('重新连接'))
                .width('100%').height(46).backgroundColor(Hero.accent)
                .enabled(this.device() !== undefined && this.core.connecting === null)
                .onClick(() => {
                  const d = this.device();
                  if (d !== undefined) {
                    this.core.openDevice(d);
                  }
                })
            }.padding(20).width('100%')
          }
        } else {
          ListItemGroup({ header: sectionHeader(t('会话')), style: ListItemGroupStyle.CARD }) {
            if (this.state()?.error) {
              ListItem() {
                Text(t('读不到会话列表：{{error}}', { error: tr(this.state()?.error ?? '') } as Record<string, string | number>))
                  .fontSize(13).fontColor(Hero.danger).padding(14)
              }
            }
            if (this.state() === undefined || (this.state()?.list === null && this.state()?.error === null)) {
              ListItem() {
                Row({ space: 10 }) {
                  LoadingProgress().width(20).height(20).color(Hero.muted)
                  Text(t('正在读取会话…')).fontSize(14).fontColor(Hero.muted)
                }.padding(14)
              }
            } else if (this.list().length === 0) {
              ListItem() {
                Text(t('这台设备上还没有会话，新建一个终端开始用。')).fontSize(14).fontColor(Hero.muted).padding(14)
              }
            }
            ForEach(this.list(), (s: SessionView) => {
              ListItem() {
                SessionRow({ session: s, title: sessionTitles(this.state()?.list ?? []).get(s.id) ?? t('会话'),
                  open: this.core.terms.some((x: TermModel) => x.endpointId === this.endpointId && x.sessionId === s.id && x.state !== 'closed') })
              }
              .onClick(() => this.core.openTerminal(this.endpointId, s.id, true))
              .swipeAction({ end: this.endButton(s) })
            }, (s: SessionView) => s.id + (s.agent?.state ?? '') + (s.current_path ?? ''))
          }

          ListItem() {
            Button() {
              Row({ space: 8 }) {
                Image($r('app.media.lucide_unplug_danger')).width(18).height(18)
                Text(t('断开连接')).fontSize(15).fontColor(Hero.danger)
              }
            }
            .width('100%').height(48).backgroundColor(Hero.surface).margin({ top: 24 })
            .onClick(() => this.core.disconnect(this.endpointId))
          }
        }
      }
      .width('100%').height('100%').padding({ left: 16, right: 16 }).scrollBar(BarState.Off)
    }
    .title(this.device()?.remark ?? this.device()?.name ?? this.conn()?.name ?? t('设备'))
    .menus(this.conn() !== undefined ? [
      { value: t('新建终端'), icon: $r('app.media.lucide_plus_accent'), action: () => this.core.openTerminal(this.endpointId, null, true) },
      { value: t('发送文件'), icon: $r('app.media.lucide_file_up_accent'), action: () => this.pickFiles() },
      { value: t('刷新会话'), icon: $r('app.media.lucide_refresh_cw_accent'), action: () => this.core.loadSessions(this.endpointId) }
    ] : [])
    .backgroundColor(Hero.background)
    .bindSheet(this.ending !== null, this.endSheet(), {
      height: SheetSize.FIT_CONTENT,
      backgroundColor: Hero.surface,
      title: { title: this.ending !== null ? (sessionTitles(this.state()?.list ?? []).get(this.ending.id) ?? t('会话')) : '' },
      onDisappear: () => {
        this.ending = null;
      }
    })
  }

  @Builder
  endButton(s: SessionView) {
    Row() {
      Image($r('app.media.lucide_trash')).width(20).height(20)
    }
    .width(72).height('100%').justifyContent(FlexAlign.Center).backgroundColor(Hero.danger)
    .onClick(() => {
      this.ending = s;
    })
  }

  @Builder
  endSheet() {
    Column({ space: 12 }) {
      Text(t('结束后里面运行的程序会被关掉，没保存的内容会丢失。')).fontSize(14).fontColor(Hero.muted).textAlign(TextAlign.Center)
      Button(t('结束会话')).width('100%').height(46).backgroundColor(Hero.danger).onClick(() => {
        const s = this.ending;
        this.ending = null;
        if (s !== null) {
          this.core.endSession(this.endpointId, s.id);
        }
      })
      Button(t('取消')).width('100%').height(46).backgroundColor(Hero.tile).fontColor(Hero.foreground).onClick(() => {
        this.ending = null;
      })
    }.padding({ left: 20, right: 20, bottom: 24 })
  }
}

@ComponentV2
struct SessionRow {
  @Param @Require session: SessionView;
  @Param title: string = '';
  @Param open: boolean = false;

  build() {
    Row({ space: 12 }) {
      SessionTile({ agent: this.session.agent })
      Column({ space: 3 }) {
        Row({ space: 6 }) {
          Text(this.title).fontSize(16).fontWeight(FontWeight.Medium).fontColor(Hero.foreground).maxLines(1)
          if (this.open) {
            Chip({ text: t('已打开'), accent: true })
          }
        }
        Row({ space: 8 }) {
          if (this.session.agent !== null) {
            AgentStateLabel({ agent: this.session.agent, size: 13 })
          } else if (this.session.command !== null) {
            Text(this.session.command).fontSize(12).fontFamily('monospace').fontColor(Hero.muted)
          }
          Text(shortPath(this.session.current_path)).fontSize(12).fontFamily('monospace').fontColor(Hero.muted).maxLines(1)
        }
        Text(t('{{time}}创建', { time: relativeTime(this.session.created_at) } as Record<string, string | number>))
          .fontSize(12).fontColor(Hero.muted).opacity(0.8)
      }.alignItems(HorizontalAlign.Start).layoutWeight(1)
      Image($r('app.media.lucide_chevron_right_muted')).width(16).height(16)
    }
    .width('100%').padding({ left: 12, right: 12, top: 10, bottom: 10 })
  }
}

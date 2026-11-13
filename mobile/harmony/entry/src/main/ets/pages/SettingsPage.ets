import { common, Want } from '@kit.AbilityKit';
import { Core } from '../common/Core';
import { Hero } from '../common/Hero';
import { I18n, t } from '../common/I18n';
import { MobileSettings, ServerPreset, SettingsView } from '../common/Model';
import { sectionHeader } from '../common/Widgets';

const REPO_URL = 'https://github.com/ILoveBingLu/P2PCLI';

/**
 * 设置：只放手机用得上的。手机只当连接的一方，桌面上给“被连的电脑”用的设置
 * （AI 启动、多机互动、受信设备、配对密码、局域网里显示本机、系统权限）这里都没有。
 */
@ComponentV2
export struct SettingsPage {
  @Local core: Core = Core.shared;
  @Local i18n: I18n = I18n.shared;
  @Local name: string = '';
  @Local server: string = '';
  @Local custom: boolean = false;
  @Local customServer: string = '';
  @Local relay: string = '';
  @Local proxyOn: boolean = false;
  @Local proxy: string = '';
  @Local error: string | null = null;

  aboutToAppear(): void {
    const v = this.core.settings;
    if (v === null) {
      return;
    }
    const s = v.settings;
    // 牵线服务器：选了预设就是它的地址；不在预设里的算“自定义”
    const initial = s.server ?? v.default_server;
    const preset = v.server_presets.some((p: ServerPreset) => p.address === initial);
    this.name = s.device_name ?? '';
    this.server = preset ? initial : '';
    this.custom = !preset;
    this.customServer = preset ? '' : initial;
    this.relay = s.relay ?? '';
    this.proxyOn = s.proxy !== null;
    this.proxy = s.proxy ?? '';
  }

  next(v: SettingsView): MobileSettings {
    const clean = (x: string): string | null => x.trim().length > 0 ? x.trim() : null;
    const chosen = clean(this.custom ? this.customServer : this.server);
    return {
      device_name: clean(this.name),
      // 选的就是默认服务器时不记，以后默认换了跟着换
      server: chosen === v.default_server ? null : chosen,
      relay: clean(this.relay),
      proxy: this.proxyOn ? clean(this.proxy) : null
    };
  }

  dirty(v: SettingsView): boolean {
    return JSON.stringify(this.next(v)) !== JSON.stringify(v.settings);
  }

  open(url: string): void {
    const ctx = this.core.ctx as common.UIAbilityContext;
    const want: Want = { action: 'ohos.want.action.viewData', entities: ['entity.system.browsable'], uri: url };
    ctx.startAbility(want).catch(() => {
    });
  }

  build() {
    NavDestination() {
      if (this.core.settings !== null) {
        List() {
          ListItemGroup({ header: sectionHeader(t('界面语言')), style: ListItemGroupStyle.CARD }) {
            ForEach([['system', t('跟随系统')], ['chinese', '简体中文'], ['english', 'English']], (o: string[]) => {
              ListItem() {
                this.choice(this.i18n.language === o[0], o[1],
                  o[0] === 'system' ? [t('跟随系统时，中文环境用简体中文，其他环境用英文。')] : [], null, () => this.i18n.setLanguage(o[0]))
              }
            }, (o: string[]) => o[0])
          }

          ListItemGroup({ header: sectionHeader(t('设备名称')), style: ListItemGroupStyle.CARD }) {
            ListItem() {
              Column({ space: 8 }) {
                TextInput({ placeholder: this.core.settings?.default_device_name ?? '', text: this.name })
                  .backgroundColor(Hero.tile).fontColor(Hero.foreground)
                  .onChange((v: string) => {
                    this.name = v;
                    this.error = null;
                  })
                Text(t('对方电脑上看到的名字，留空用手机型号。')).fontSize(12).fontColor(Hero.muted)
              }.alignItems(HorizontalAlign.Start).padding(14)
            }
          }

          ListItemGroup({ header: sectionHeader(t('牵线服务器')), style: ListItemGroupStyle.CARD }) {
            ForEach(this.core.settings?.server_presets ?? [], (p: ServerPreset) => {
              ListItem() {
                this.choice(!this.custom && this.server === p.address, p.label, [p.note, p.address],
                  p.address === this.core.settings?.default_server ? t('默认') : null, () => {
                    this.custom = false;
                    this.server = p.address;
                    this.error = null;
                  })
              }
            }, (p: ServerPreset) => p.address)
            ListItem() {
              this.choice(this.custom, t('自定义'), [t('填自己部署的牵线服务器：公钥@IP或域名:端口 走 UDP，wss://域名 走 WebSocket。')], null,
                () => {
                  this.custom = true;
                  this.error = null;
                })
            }
            if (this.custom) {
              ListItem() {
                TextInput({ placeholder: t('公钥@IP:端口 或 wss://域名'), text: this.customServer })
                  .backgroundColor(Hero.tile).fontColor(Hero.foreground).fontFamily('monospace')
                  .onChange((v: string) => {
                    this.customServer = v;
                    this.error = null;
                  }).margin({ left: 14, right: 14, bottom: 14 })
              }
            }
          }

          ListItemGroup({ header: sectionHeader(t('中继服务器')), style: ListItemGroupStyle.CARD }) {
            ListItem() {
              Column({ space: 8 }) {
                TextInput({ placeholder: 'https://relay.example.com', text: this.relay })
                  .backgroundColor(Hero.tile).fontColor(Hero.foreground).type(InputType.URL)
                  .onChange((v: string) => {
                    this.relay = v;
                    this.error = null;
                  })
                Text(t('直连失败时转发加密数据，多个地址用逗号分隔。留空用公共中继。')).fontSize(12).fontColor(Hero.muted)
              }.alignItems(HorizontalAlign.Start).padding(14)
            }
          }

          ListItemGroup({ header: sectionHeader(t('代理')), style: ListItemGroupStyle.CARD }) {
            ListItem() {
              Row({ space: 12 }) {
                Column({ space: 3 }) {
                  Text(t('使用 HTTP 代理')).fontSize(16).fontColor(Hero.foreground)
                  Text(t('用于 WebSocket 牵线和 HTTP(S) 中继连接，点对点直连不经过代理。')).fontSize(12).fontColor(Hero.muted)
                }.alignItems(HorizontalAlign.Start).layoutWeight(1)
                Toggle({ type: ToggleType.Switch, isOn: this.proxyOn }).selectedColor(Hero.accent)
                  .onChange((on: boolean) => {
                    this.proxyOn = on;
                    this.error = null;
                  })
              }.padding(14)
            }
            if (this.proxyOn) {
              ListItem() {
                Column({ space: 8 }) {
                  TextInput({ placeholder: 'http://127.0.0.1:7890', text: this.proxy })
                    .backgroundColor(Hero.tile).fontColor(Hero.foreground).type(InputType.URL)
                    .onChange((v: string) => {
                      this.proxy = v;
                      this.error = null;
                    })
                  Text(t('填 http:// 开头的地址；暂不支持 SOCKS5 和账号密码认证。')).fontSize(12).fontColor(Hero.muted)
                }.alignItems(HorizontalAlign.Start).padding({ left: 14, right: 14, bottom: 14 })
              }
            }
          }

          ListItemGroup({ header: sectionHeader(t('关于')), style: ListItemGroupStyle.CARD }) {
            ListItem() {
              this.infoRow(t('本机公钥'), (this.core.settings?.endpoint_id ?? '').substring(0, 10) + '…', $r('app.media.lucide_copy_muted'),
                () => {
                  this.core.copyText(this.core.settings?.endpoint_id ?? '');
                  this.core.notify(t('公钥已复制'));
                })
            }
            ListItem() {
              this.infoRow(t('GitHub 项目'), '', $r('app.media.lucide_square_arrow_out_up_right_muted'), () => this.open(REPO_URL))
            }
            ListItem() {
              this.infoRow(t('反馈问题'), '', $r('app.media.lucide_square_arrow_out_up_right_muted'), () => this.open(REPO_URL + '/issues'))
            }
            ListItem() {
              this.infoRow(t('版本发布'), '', $r('app.media.lucide_square_arrow_out_up_right_muted'), () => this.open(REPO_URL + '/releases'))
            }
          }

          ListItem() {
            Column({ space: 10 }) {
              if (this.error !== null) {
                Text(this.error).fontSize(14).fontColor(Hero.danger).width('100%')
                  .padding({ left: 14, right: 14, top: 10, bottom: 10 }).borderRadius(12).backgroundColor(Hero.dangerSoft)
              }
              Text(t('保存后马上生效。改了代理或中继会重新连网络，开着的终端会断开，对方电脑上的会话还在。')).fontSize(12).fontColor(Hero.muted)
            }.padding({ top: 16, bottom: 32 })
          }
        }
        .width('100%').height('100%').padding({ left: 16, right: 16 }).scrollBar(BarState.Off)
      }
    }
    .title(t('设置'))
    .menus([
      { value: t('保存'), icon: $r('app.media.menu_check'), isEnabled: !this.core.savingSettings, action: () => {
        const v = this.core.settings;
        if (v !== null && this.dirty(v)) {
          this.core.saveSettings(this.next(v)).then((e: string | null) => {
            this.error = e;
          });
        }
      } }
    ])
    .backgroundColor(Hero.background)
  }

  @Builder
  choice(selected: boolean, title: string, lines: string[], badge: string | null, action: () => void) {
    Row({ space: 12 }) {
      Stack() {
        if (selected) {
          Circle().width(8).height(8).fill(Color.White)
        }
      }
      .width(20).height(20).borderRadius(10).backgroundColor(selected ? Hero.accent : Hero.tile)
      Column({ space: 3 }) {
        Row({ space: 6 }) {
          Text(title).fontSize(16).fontColor(Hero.foreground)
          if (badge !== null) {
            Text(badge).fontSize(11).fontColor(Hero.muted).padding({ left: 7, right: 7, top: 2, bottom: 2 })
              .borderRadius(8).backgroundColor(Hero.tile)
          }
        }
        ForEach(lines, (line: string, i: number) => {
          Text(line).fontSize(i > 0 ? 12 : 13).fontFamily(i > 0 ? 'monospace' : undefined).fontColor(Hero.muted)
        }, (line: string) => line)
      }.alignItems(HorizontalAlign.Start).layoutWeight(1)
    }
    .width('100%').padding(14)
    .onClick(action)
  }

  @Builder
  infoRow(title: string, value: string, icon: Resource, action: () => void) {
    Row({ space: 10 }) {
      Text(title).fontSize(16).fontColor(Hero.foreground).layoutWeight(1)
      if (value.length > 0) {
        Text(value).fontSize(13).fontFamily('monospace').fontColor(Hero.muted)
      }
      Image(icon).width(16).height(16)
    }
    .width('100%').padding({ left: 14, right: 14, top: 15, bottom: 15 })
    .onClick(action)
  }
}

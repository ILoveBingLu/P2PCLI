import { AgentsState, Core, TermModel } from '../common/Core';
import { agentIcon, Hero } from '../common/Hero';
import { t, tr } from '../common/I18n';
import { DirListingView, InstalledAgentView, parseError, SessionView } from '../common/Model';
import { shortPath } from '../common/Format';
import { GlassButton, sheetBackground } from '../common/Glass';

/**
 * 终端下面一行：最左边是当前工作目录（点开选目录），后面是对方电脑上装了的命令行 AI，点一下在这个会话里启动。
 * 和桌面一样，终端停在空闲的命令提示符、还没在跑 AI 时才能启动；跑着程序时目录只能看不能切。
 * 这个会话里在跑 AI 时整行不显示
 */
@ComponentV2
export struct AgentBar {
  @Param @Require term: TermModel;
  @Param session: SessionView | null = null;
  @Event onPickDir: () => void = () => {};
  @Local core: Core = Core.shared;
  @Local launching: string | null = null;
  @Local menuFor: InstalledAgentView | null = null;
  @Local showMenu: boolean = false;

  aboutToAppear(): void {
    this.core.scanAgents(this.term.connId, false);
  }

  state(): AgentsState | undefined {
    return this.core.agents.get(this.term.connId);
  }

  idle(): boolean {
    return this.session !== null && this.session.shell_ready && this.session.agent === null;
  }

  launch(a: InstalledAgentView, bypass: boolean): void {
    if (this.launching !== null) {
      return;
    }
    this.launching = a.kind;
    this.core.launchAgent(this.term, a.kind, bypass).finally(() => {
      this.launching = null;
    });
  }

  build() {
    if (this.session === null || this.session.agent === null) {
      Scroll() {
        Row({ space: 6 }) {
          this.pill(t('当前目录：{{path}}，点击选择目录', ['path', this.path()]), $r('app.media.lucide_folder_open_muted'), this.path(), true,
            () => this.onPickDir())
          if (this.idle()) {
            if (this.state() === undefined || (this.state()?.scanning && this.state()?.list === null)) {
              Row({ space: 6 }) {
                LoadingProgress().width(16).height(16).color(Hero.muted)
                Text(t('正在检测 AI')).fontSize(12.5).fontColor(Hero.muted)
              }.padding({ left: 6, right: 6 })
            } else if (this.state()?.list === null) {
              this.pill(t('AI 检测失败，重试'), $r('app.media.lucide_refresh_cw'), t('检测失败 · 重试'), false,
                () => this.core.scanAgents(this.term.connId, true))
            } else if ((this.state()?.list ?? []).length === 0) {
              this.pill(t('未检测到 AI，点击重新检测'), $r('app.media.lucide_refresh_cw_muted'), t('未检测到 AI，点击重新检测'), false,
                () => this.core.scanAgents(this.term.connId, true))
            } else {
              ForEach(this.state()?.list ?? [], (a: InstalledAgentView) => {
                Button({ type: ButtonType.Capsule, stateEffect: true }) {
                  Row({ space: 6 }) {
                    if (this.launching === a.kind) {
                      LoadingProgress().width(16).height(16).color(Hero.accent)
                    } else {
                      Image(agentIcon(a.kind)).width(16).height(16)
                    }
                    Text(a.name).fontSize(12.5).fontColor(Hero.foreground).maxLines(1)
                  }
                }
                .height(34).padding({ left: 12, right: 12 })
                .attributeModifier(new GlassButton())
                .opacity(this.launching !== null && this.launching !== a.kind ? 0.5 : 1)
                .accessibilityText(t('启动 {{name}}', ['name', a.name]))
                .onClick(() => this.launch(a, false))
                // Claude Code 长按可以选“允许之后切换全部放行”，和桌面的右键菜单一样
                .gesture(LongPressGesture().onAction(() => {
                  if (a.kind === 'claude') {
                    this.menuFor = a;
                    this.showMenu = true;
                  }
                }))
              }, (a: InstalledAgentView) => a.kind)
              Stack() {
                if (this.state()?.scanning) {
                  LoadingProgress().width(16).height(16).color(Hero.muted)
                } else {
                  Image($r('app.media.lucide_refresh_cw_muted')).width(15).height(15)
                }
              }
              .width(34).height(34)
              .accessibilityText(t('刷新已安装的 AI'))
              .onClick(() => this.core.scanAgents(this.term.connId, true))
            }
          }
        }
        .padding({ left: 8, right: 8 })
      }
      .scrollable(ScrollDirection.Horizontal)
      .scrollBar(BarState.Off)
      .width('100%')
      .padding({ top: 8 })
      .bindSheet(this.showMenu!!, this.menuSheet(), {
        height: SheetSize.FIT_CONTENT,
        backgroundColor: sheetBackground(),
        title: { title: this.menuFor?.name ?? '' },
        onDisappear: () => {
          this.menuFor = null;
        }
      })
    }
  }

  path(): string {
    const p = shortPath(this.session?.current_path ?? null);
    return p.length > 0 ? p : '~';
  }

  @Builder
  pill(label: string, icon: Resource, text: string, mono: boolean, action: () => void) {
    Button({ type: ButtonType.Capsule, stateEffect: true }) {
      Row({ space: 6 }) {
        Image(icon).width(15).height(15)
        Text(text).fontSize(12.5).fontColor(Hero.foreground).fontFamily(mono ? 'monospace' : undefined).maxLines(1)
          .textOverflow({ overflow: TextOverflow.Ellipsis }).constraintSize({ maxWidth: 180 })
      }
    }
    .height(34).padding({ left: 12, right: 12 })
    .accessibilityText(label)
    .onClick(action)
    .attributeModifier(new GlassButton())
  }

  @Builder
  menuSheet() {
    Column({ space: 4 }) {
      ForEach([[t('正常启动'), 'normal'], [t('启动并允许之后切换全部放行'), 'bypass']], (o: string[]) => {
        Row({ space: 12 }) {
          Image($r('app.media.lucide_play')).width(20).height(20)
          Text(o[0]).fontSize(16).fontColor(Hero.foreground).layoutWeight(1)
        }
        .width('100%').padding(14)
        .onClick(() => {
          const a = this.menuFor;
          this.showMenu = false;
          if (a !== null) {
            this.launch(a, o[1] === 'bypass');
          }
        })
      }, (o: string[]) => o[1])
    }
    .padding({ left: 12, right: 12, bottom: 24 })
  }
}

/**
 * 选工作目录：对方电脑上的目录列表，点子目录进去，点“切换到这里”让终端 cd 过去。
 * 终端跑着程序（包括 AI）时只能看，免得 cd 被当成输入发给程序
 */
@ComponentV2
export struct DirPicker {
  @Param @Require term: TermModel;
  @Param session: SessionView | null = null;
  @Event onDone: () => void = () => {};
  @Local core: Core = Core.shared;
  @Local listing: DirListingView | null = null;
  @Local loading: boolean = true;
  @Local error: string | null = null;
  @Local filter: string = '';
  @Local changing: boolean = false;

  aboutToAppear(): void {
    this.browse(this.session?.current_path ?? '');
  }

  canChange(): boolean {
    return this.session !== null && this.session.shell_ready && this.session.agent === null;
  }

  browse(path: string): void {
    this.loading = true;
    this.error = null;
    this.core.listDir(this.term.connId, path).then((next: DirListingView) => {
      if (this.listing === null || next.path !== this.listing.path) {
        this.filter = '';
      }
      this.listing = next;
    }).catch((e: Error) => {
      this.error = tr(parseError(e).detail);
    }).finally(() => {
      this.loading = false;
    });
  }

  dirs(): string[] {
    const f = this.filter.trim().toLowerCase();
    return (this.listing?.dirs ?? []).filter((d: string) => f.length === 0 || d.toLowerCase().indexOf(f) >= 0);
  }

  /** 拼对方电脑上的路径：Windows 用反斜杠 */
  join(dir: string, name: string): string {
    const sep = dir.indexOf('\\') >= 0 && dir.indexOf('/') < 0 ? '\\' : '/';
    return dir.endsWith(sep) ? dir + name : dir + sep + name;
  }

  build() {
    Column({ space: 10 }) {
      Text(this.listing?.path ?? this.session?.current_path ?? '').fontSize(12.5).fontFamily('monospace').fontColor(Hero.muted)
        .width('100%')
      Row({ space: 8 }) {
        Button() {
          Row({ space: 6 }) {
            Image($r('app.media.lucide_arrow_up')).width(16).height(16)
            Text(t('上一级')).fontSize(14).fontColor(Hero.foreground)
          }
        }
        .layoutWeight(1).height(40).backgroundColor(Hero.tile)
        .enabled(this.listing?.parent !== null && this.listing?.parent !== undefined && !this.loading)
        .onClick(() => {
          const parent = this.listing?.parent;
          if (parent) {
            this.browse(parent);
          }
        })
        Button() {
          Row({ space: 6 }) {
            Image($r('app.media.lucide_house')).width(16).height(16)
            Text(t('用户目录')).fontSize(14).fontColor(Hero.foreground)
          }
        }
        .layoutWeight(1).height(40).backgroundColor(Hero.tile).enabled(!this.loading)
        .onClick(() => this.browse(''))
      }
      TextInput({ placeholder: t('筛选子目录'), text: this.filter })
        .height(40).backgroundColor(Hero.tile).fontColor(Hero.foreground)
        .onChange((v: string) => {
          this.filter = v;
        })
      if (this.error !== null) {
        Text(this.error).fontSize(13).fontColor(Hero.danger).width('100%')
      }
      List() {
        ForEach(this.dirs(), (name: string) => {
          ListItem() {
            Row({ space: 10 }) {
              Image($r('app.media.lucide_folder_muted')).width(18).height(18)
              Text(name).fontSize(14).fontColor(Hero.foreground).maxLines(1).textOverflow({ overflow: TextOverflow.Ellipsis })
                .layoutWeight(1)
            }
            .width('100%').padding({ left: 8, right: 8, top: 11, bottom: 11 })
            .onClick(() => {
              if (this.listing !== null && !this.loading) {
                this.browse(this.join(this.listing.path, name));
              }
            })
          }
        }, (name: string) => name)
      }
      .height(300).width('100%')
      if (this.listing?.truncated) {
        Text(t('子目录太多，只列出了前 {{shown}} 个，可以输入名字筛选', ['shown', this.listing?.dirs.length ?? 0]))
          .fontSize(12).fontColor(Hero.muted).width('100%')
      }
      if (!this.canChange()) {
        Text(this.session?.agent ? t('{{name}} 正在运行，退出后才能切换目录', ['name', this.session?.agent?.name ?? ''])
          : t('终端不在空闲提示符，完成当前输入或退出程序后才能切换')).fontSize(12.5).fontColor(Hero.warning).width('100%')
      }
      Button() {
        Row({ space: 8 }) {
          if (this.changing) {
            LoadingProgress().width(18).height(18).color(Color.White)
          } else {
            Image($r('app.media.lucide_corner_down_left')).width(18).height(18)
          }
          Text(t('切换到这里')).fontSize(16).fontColor(Color.White)
        }
      }
      .width('100%').height(46).backgroundColor(Hero.accent)
      .enabled(this.canChange() && this.listing !== null && !this.changing && !this.loading)
      .onClick(() => {
        const l = this.listing;
        if (l === null) {
          return;
        }
        this.changing = true;
        this.core.changeDir(this.term, l.path).then((err: string | null) => {
          this.changing = false;
          if (err === null) {
            this.onDone();
          } else {
            this.error = err;
          }
        });
      })
    }
    .padding({ left: 16, right: 16, bottom: 24 })
  }
}

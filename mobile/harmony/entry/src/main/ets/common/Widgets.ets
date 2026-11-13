import { AgentView } from './Model';
import { agentIcon, Hero, platformIcon } from './Hero';
import { agentColor, agentText } from './Format';

/** 系统图标的方块：苹果、Windows、Linux、安卓、鸿蒙；连着时右下角一个绿点 */
@ComponentV2
export struct PlatformTile {
  @Param platform: string = '';
  @Param connected: boolean = false;
  @Param size: number = 42;

  build() {
    Stack({ alignContent: Alignment.BottomEnd }) {
      Stack() {
        if (platformIcon(this.platform) !== null) {
          Image(platformIcon(this.platform)).width(this.size > 60 ? 36 : 20).height(this.size > 60 ? 36 : 20)
        } else {
          Text(this.platform.substring(0, 3)).fontSize(12).fontWeight(FontWeight.Medium).fontColor(Hero.foreground)
        }
      }
      .width(this.size).height(this.size).borderRadius(this.size > 60 ? 24 : 13).backgroundColor(Hero.tile)
      if (this.connected) {
        Circle().width(this.size > 60 ? 12 : 8).height(this.size > 60 ? 12 : 8).fill(Hero.success)
          .border({ width: this.size > 60 ? 3.5 : 2.5, color: Hero.surface })
          .offset({ x: 3, y: 3 })
      }
    }
  }
}

/** 会话、终端前面的方块：跑着 AI 显示 AI 的图标，不然是终端图标 */
@ComponentV2
export struct SessionTile {
  @Param agent: AgentView | null = null;
  @Param dimmed: boolean = false;

  build() {
    Stack() {
      Image(this.agent !== null ? agentIcon(this.agent.kind) : $r('app.media.lucide_square_terminal')).width(20).height(20)
    }
    .width(42).height(42).borderRadius(13).backgroundColor(Hero.tile).opacity(this.dimmed ? 0.5 : 1)
  }
}

/** AI 的状态：点加字，干活中蓝、等确认黄 */
@ComponentV2
export struct AgentStateLabel {
  @Param agent: AgentView | null = null;
  @Param size: number = 12;

  build() {
    if (this.agent !== null) {
      Row({ space: 4 }) {
        Circle().width(6).height(6).fill(agentColor(this.agent))
        Text(agentText(this.agent)).fontSize(this.size).fontColor(agentColor(this.agent))
      }
    }
  }
}

/** 小标签：“已打开”“局域网” */
@ComponentV2
export struct Chip {
  @Param text: string = '';
  @Param accent: boolean = false;

  build() {
    Text(this.text).fontSize(11).fontWeight(FontWeight.Medium)
      .fontColor(this.accent ? Hero.accent : Hero.muted)
      .padding({ left: 7, right: 7, top: 2, bottom: 2 })
      .borderRadius(8)
      .backgroundColor(this.accent ? Hero.accentSoft : Hero.tile)
  }
}

/** 分组标题 */
@Builder
export function sectionHeader(title: string) {
  Text(title).fontSize(13).fontColor(Hero.muted).fontWeight(FontWeight.Medium)
    .padding({ left: 4, top: 18, bottom: 8 })
}

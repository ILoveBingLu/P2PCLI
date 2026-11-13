// 颜色：桌面端 HeroUI 深色主题的取值（和安卓、iOS 一样）
export class Hero {
  static readonly background: string = '#060607';
  static readonly surface: string = '#18181B';
  static readonly surfaceSecondary: string = '#232325';
  static readonly tile: string = '#27272A';
  static readonly foreground: string = '#FCFCFC';
  static readonly muted: string = '#9F9FA9';
  static readonly accent: string = '#0485F7';
  static readonly accentSoft: string = '#1A0485F7';
  static readonly success: string = '#17C964';
  static readonly warning: string = '#F5A524';
  static readonly danger: string = '#DB3B3E';
  static readonly dangerSoft: string = '#1FDB3B3E';
  static readonly terminal: string = '#111114';
}

/** 系统图标：苹果、Windows、Linux、安卓、鸿蒙，和桌面同一套 */
export function platformIcon(platform: string): Resource | null {
  switch (platform) {
    case 'macos':
    case 'ios':
      return $r('app.media.platform_apple');
    case 'windows':
      return $r('app.media.platform_windows');
    case 'linux':
      return $r('app.media.platform_linux');
    case 'android':
      return $r('app.media.platform_android');
    case 'harmonyos':
      return $r('app.media.platform_harmonyos');
    default:
      return null;
  }
}

const AGENTS: string[] = ['amp', 'claude', 'codebuddy', 'codex', 'copilot', 'cursor', 'gemini', 'goose', 'kimi', 'kiro',
  'opencode', 'qoder', 'qwen', 'trae', 'zcode'];

/** 命令行 AI 的图标；没收录的用机器人图标 */
export function agentIcon(kind: string): Resource {
  if (AGENTS.indexOf(kind) >= 0) {
    return $r(`app.media.agent_${kind}`);
  }
  return $r('app.media.lucide_bot');
}

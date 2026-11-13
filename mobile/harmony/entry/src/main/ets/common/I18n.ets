import { common } from '@kit.AbilityKit';
import { i18n } from '@kit.LocalizationKit';
import { preferences } from '@kit.ArkData';
import { util } from '@kit.ArkTS';

/**
 * 界面文字：和桌面、安卓、iOS 同一份翻译。键是中文原文，英文在桌面的 app/src/i18n/locales/en.json 里，
 * build-rust.ps1 把它拷进 rawfile/i18n。跟随系统时：中文环境用简体中文，其他环境用英文，和桌面一样。
 */
@ObservedV2
export class I18n {
  /** 实例在类定义之后才创建（见 Core.shared 的说明） */
  static get shared(): I18n {
    return sharedI18n;
  }
  /** system | chinese | english；改了界面马上跟着变 */
  @Trace language: string = 'system';
  private english: Record<string, string> = {};
  private prefs?: preferences.Preferences;

  init(ctx: common.UIAbilityContext): void {
    try {
      const raw = ctx.resourceManager.getRawFileContentSync('i18n/en.json');
      const text = new util.TextDecoder('utf-8').decodeToString(raw);
      this.english = JSON.parse(text) as Record<string, string>;
    } catch (_) {
      this.english = {};
    }
    try {
      this.prefs = preferences.getPreferencesSync(ctx, { name: 'p2pcli' } as preferences.Options);
      this.language = this.prefs.getSync('language', 'system') as string;
    } catch (_) {
      this.language = 'system';
    }
  }

  setLanguage(value: string): void {
    this.language = value;
    if (this.prefs) {
      this.prefs.putSync('language', value);
      this.prefs.flush();
    }
  }

  get isEnglish(): boolean {
    if (this.language === 'english') {
      return true;
    }
    if (this.language === 'chinese') {
      return false;
    }
    return !i18n.System.getSystemLanguage().startsWith('zh');
  }

  lookup(key: string, count?: number): string | undefined {
    if (!this.isEnglish) {
      return undefined;
    }
    if (count === 1) {
      const one = this.english[key + '_one'];
      if (one !== undefined) {
        return one;
      }
    }
    return this.english[key];
  }
}

const sharedI18n: I18n = new I18n();

/**
 * 界面文字。键是中文原文，{{name}} 这样的占位用后面的键值对填：t('连接 {{name}}', ['name', 设备名])。
 * 参数里有 count 时，英文按单复数取（键加 _one 的是单数），和桌面的 i18next 一样
 */
export function t(key: string, ...args: [string, string | number][]): string {
  // 读一下语言：界面跟着它刷新
  const lang = I18n.shared.language;
  let count: number | undefined = undefined;
  args.forEach((a: [string, string | number]) => {
    if (a[0] === 'count') {
      count = Number(a[1]);
    }
  });
  let text = I18n.shared.lookup(key, count) ?? key;
  args.forEach((a: [string, string | number]) => {
    text = text.split(`{{${a[0]}}}`).join(String(a[1]));
  });
  return lang.length >= 0 ? text : key;
}

/** Rust 那边传来的中文提示：翻译里有就换成英文，没有就原样显示 */
export function tr(message: string): string {
  return I18n.shared.lookup(message) ?? message;
}

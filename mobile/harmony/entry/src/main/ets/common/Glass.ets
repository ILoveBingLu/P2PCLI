import { uiMaterial } from '@kit.ArkUI';
import { deviceInfo } from '@kit.BasicServicesKit';
import { Hero } from './Hero';

/**
 * 系统的沉浸光感材质（HarmonyOS 7 / API 26 起）：毛玻璃加随视角流动的光，和 iOS 的液态玻璃对应。
 *
 * 应用级开关在 module.json5 的 metadata `ohos.arkui.UIMaterial.state = enable`：系统的面板、弹窗、Toast 自动用上。
 * 自己的控件里，页面内容区只有按钮、选择类组件（Button、Toggle…）挂材质才生效，所以按键、胶囊都用 Button 承载；
 * 标题栏用 Navigation 的 title 选项挂。
 * 旧系统（App 最低支持 API 12）、设备不支持时不挂材质，用原来的纯色底
 */
let supported: boolean | null = null;

export function glassSupported(): boolean {
  if (supported === null) {
    supported = false;
    try {
      if (deviceInfo.sdkApiVersion >= 26) {
        supported = uiMaterial.isImmersiveMaterialSupported();
      }
    } catch (e) {
      supported = false;
    }
  }
  return supported;
}

/** 一块材质；不支持时为 undefined。tint 是叠在材质上的半透明颜色（必须带透明度），interactive 是按下时的形变和光感反馈 */
export function glass(interactive: boolean, tint?: string): uiMaterial.Material | undefined {
  if (!glassSupported()) {
    return undefined;
  }
  return new uiMaterial.ImmersiveMaterial({
    style: uiMaterial.ImmersiveStyle.THIN,
    materialColor: tint ?? Color.Transparent,
    applyShadow: false,
    interactive: interactive,
    lightEffect: interactive ? {} : null
  });
}

/** 标题栏的材质：滚上去的内容从它下面透出来 */
export function barGlass(): uiMaterial.Material | undefined {
  if (!glassSupported()) {
    return undefined;
  }
  return new uiMaterial.ImmersiveMaterial({ style: uiMaterial.ImmersiveStyle.REGULAR, applyShadow: false });
}

/** 弹出面板的底：支持材质时透明（让系统的材质透出来），不支持时用原来的面板色 */
export function sheetBackground(): ResourceColor {
  return glassSupported() ? Color.Transparent : Hero.surface;
}

/**
 * 玻璃按钮：挂在 Button 上。支持时底色透明、挂材质；不支持时用 fallback 纯色。
 * 材质要放在背景色之后设置，所以用 attributeModifier 放在属性链最后
 */
export class GlassButton implements AttributeModifier<ButtonAttribute> {
  private fallback: ResourceColor;
  private tint: string | undefined;
  private interactive: boolean;

  constructor(fallback: ResourceColor = Hero.tile, tint?: string, interactive: boolean = true) {
    this.fallback = fallback;
    this.tint = tint;
    this.interactive = interactive;
  }

  applyNormalAttribute(instance: ButtonAttribute): void {
    const m = glass(this.interactive, this.tint);
    if (m !== undefined) {
      instance.backgroundColor(Color.Transparent);
      instance.systemMaterial(m);
    } else {
      instance.backgroundColor(this.fallback);
    }
  }
}

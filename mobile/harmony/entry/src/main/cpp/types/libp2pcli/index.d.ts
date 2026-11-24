// Rust 库（crates/harmony-ffi）导出给 ArkTS 的接口。数据都是 JSON 字符串，结构见 ets/common/Model.ets。
// 会等网络的方法返回 Promise；出错时 Error.message 也是 JSON：{"kind":"rejected","reason":"need_password","detail":"…"}

/** 启动：读身份、连牵线服务器、开始局域网发现。config 是 MobileConfig 的 JSON；onEvent 收后台变化（AppEvent 的 JSON） */
export const start: (config: string, onEvent: (event: string) => void) => Promise<P2p>;
/** 扫出来的不是 P2PCLI 的二维码时返回 null；是的话返回 PairInfo 的 JSON */
export const parsePairLink: (text: string) => string | null;

export class P2p {
  overview(): string;
  scanNearby(): Promise<string>;
  connect(target: string, password: string | null): Promise<string>;
  disconnect(connId: number): void;
  listSessions(connId: number): Promise<string>;
  endSession(connId: number, sessionId: string): Promise<void>;
  chooseAgentOption(connId: number, sessionId: string, approvalId: string, optionId: number): Promise<void>;
  installedAgents(connId: number, refresh: boolean): Promise<string>;
  launchAgent(connId: number, sessionId: string, kind: string, allowBypass: boolean): Promise<void>;
  listDir(connId: number, path: string): Promise<string>;
  changeDir(connId: number, sessionId: string, path: string): Promise<void>;
  openTerminal(connId: number, sessionId: string | null, cols: number, rows: number,
    onOutput: (data: Uint8Array) => void, onClosed: (info: string) => void): Promise<Terminal>;
  sendFiles(target: string, paths: string[]): string;
  transfers(): string;
  respondFiles(id: number, accept: boolean): void;
  setDeviceRemark(endpointId: string, remark: string): void;
  forgetDevice(endpointId: string): void;
  refreshDevices(): void;
  settings(): string;
  saveSettings(settings: string): Promise<void>;
  shutdown(): void;
  /** 对方能不能远程控制（新版本的 macOS、Windows） */
  supportsScreen(connId: number): boolean;
  /** 开始看对方的画面，对方点了同意才返回。quality：sharp | balanced | smooth。
   *  onEvent 收状态（ScreenEvent 的 JSON）；画面不经过 ArkTS，Rust 直接解码画到 setSurface 给的 XComponent 上 */
  openScreen(connId: number, quality: string, onEvent: (event: string) => void): Promise<ScreenSession>;
}

/** 一路正在看的远程画面 */
export class ScreenSession {
  /** ScreenStartedView 的 JSON */
  started(): string;
  /** XComponent 的 Surface 建好了（surfaceId）或者没了（null） */
  setSurface(surfaceId: string | null): void;
  /** 键鼠事件：ScreenInput 的 JSON，比如 {"k":"move","x":0.5,"y":0.5} */
  send(input: string): void;
  typeText(text: string, modifiers: string[]): void;
  tapKey(code: string, modifiers: string[]): void;
  switchDisplay(display: string): void;
  setQuality(quality: string): void;
  /** 切回前台：解码器重来，要一个关键帧 */
  resume(): void;
  stop(): void;
}

export class Terminal {
  sessionId(): string;
  write(data: Uint8Array): void;
  writeText(text: string): void;
  resize(cols: number, rows: number): void;
  detach(): void;
}

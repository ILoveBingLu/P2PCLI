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
}

export class Terminal {
  sessionId(): string;
  write(data: Uint8Array): void;
  writeText(text: string): void;
  resize(cols: number, rows: number): void;
  detach(): void;
}

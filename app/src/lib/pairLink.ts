/**
 * 桌面端标题栏的手机图标里显示的二维码内容，手机扫了直接连：
 * p2pcli://pair?code=设备码&pw=配对密码&id=公钥&name=设备名
 * 设备码要牵线服务器连着才有；没有时手机按公钥连（同一个局域网里能找到）。
 */
export interface PairInfo {
  /** 9 位设备码；牵线服务器没连上时没有 */
  code: string | null;
  password: string;
  /** 设备公钥，设备码不在时用它连 */
  id: string;
  name: string;
}

const SCHEME = "p2pcli:";
const HOST = "pair";

export function buildPairLink(info: PairInfo): string {
  const params = new URLSearchParams();
  if (info.code) params.set("code", info.code);
  params.set("pw", info.password);
  params.set("id", info.id);
  params.set("name", info.name);
  return `${SCHEME}//${HOST}?${params.toString()}`;
}

/** 扫出来的不是 P2PCLI 的二维码时返回 null */
export function parsePairLink(text: string): PairInfo | null {
  let url: URL;
  try {
    url = new URL(text.trim());
  } catch {
    return null;
  }
  if (url.protocol !== SCHEME || (url.hostname || url.pathname.replace(/^\/+/, "")) !== HOST) return null;
  const p = url.searchParams;
  const code = (p.get("code") ?? "").replace(/\D/g, "");
  const id = (p.get("id") ?? "").trim();
  const password = p.get("pw") ?? "";
  if (code.length !== 9 && !/^[0-9a-f]{64}$/i.test(id)) return null;
  return { code: code.length === 9 ? code : null, password, id, name: p.get("name") ?? "" };
}

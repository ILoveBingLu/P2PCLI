// 借网：把本机代理借给一台设备。设备右键菜单和端口转发面板都用它
import { api, type ReverseView } from "../api";
import { toast } from "@/components/ui/toast";
import { t, tr } from "@/i18n";

/** 没在设置里填代理时，先试这个（Clash 这类代理软件常用的端口） */
const FALLBACK_PROXY = "127.0.0.1:7890";

/** 把本机代理借给这条连接的对方：用设置里的代理，没填就试常用端口。成功、失败都给提示 */
export async function lendProxy(connId: number, deviceName: string): Promise<ReverseView | null> {
  try {
    const info = await api.reverseInfo(connId);
    if (!info.supported) {
      toast.error(t("对方的 P2PCLI 版本较旧，不支持反向端口转发，请更新对方的 P2PCLI"));
      return null;
    }
    const view = await api.reverseForward(connId, info.proxy ?? FALLBACK_PROXY, "http");
    toast.success(t("已把本机代理借给 {{name}}：对方电脑上新开的终端走 127.0.0.1:{{port}}", { name: deviceName, port: view.remote_port }));
    // 直连优先，中继只是兜底：走中继时说一声会慢
    const path = await api.pathInfo(connId).catch(() => null);
    if (path?.kind === "relay") toast.notice(t("现在走的是中继，转发能用但会比较慢；直连接通后会自动切过去"));
    return view;
  } catch (e) {
    toast.error(t("借网没成功：{{error}}。本机代理不在常用端口的话，在「设置 → 代理设置」里填代理地址，或者在「端口转发」里手动填", { error: tr(String(e)) }));
    return null;
  }
}

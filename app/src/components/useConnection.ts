import { useRef, useState } from "react";
import { api, type ConnectFail, type NearbyDevice, type PathInfo, type RemoteInfo } from "../api";
import { toast } from "@/components/ui/toast";
import { t } from "@/i18n";
import { isMobile } from "@/lib/platform";
import { parsePairLink } from "@/lib/pairLink";

interface Options {
  onConnected: (connId: number, info: RemoteInfo, code: string | null, path: PathInfo) => void;
  onOpenChange: (open: boolean) => void;
}

export function useConnection({ onConnected, onOpenChange }: Options) {
  const [target, setTarget] = useState("");
  const [password, setPassword] = useState("");
  /** 互相信任：连上以后同时允许对方控制本机；每次连接都要重新勾，不会一直开着 */
  const [mutual, setMutual] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** 正在连的局域网设备：按公钥连，要密码时面板里只填密码 */
  const [lanTarget, setLanTarget] = useState<NearbyDevice | null>(null);
  const pwRef = useRef<HTMLInputElement>(null);
  const connecting = useRef(false);

  const doConnect = async (target: string, pw: string, lan: NearbyDevice | null = null) => {
    if (connecting.current || !target.trim()) return;
    connecting.current = true;
    setBusy(true);
    setError(null);
    setLanTarget(lan);
    try {
      const c = await api.connect(target.trim(), pw || null, mutual);
      onConnected(c.conn_id, c.info, c.code, c.path);
      if (mutual && !c.mutual) toast.error(t("已连上，但没能记下互相信任，对方还不能直接控制本机"));
      if (c.mutual) toast.success(t("已和 {{name}} 互相信任，它以后可以直接连本机", { name: c.info.name }));
      setPassword("");
      setMutual(false);
      setLanTarget(null);
      onOpenChange(false);
    } catch (e) {
      const f = e as ConnectFail;
      setError(f.message ?? String(e));
      onOpenChange(true);
      if (f.reject === "need_password" || f.reject === "bad_password") {
        if (!lan) setTarget(target.replace(/\D/g, ""));
        window.setTimeout(() => pwRef.current?.focus(), 0);
      }
    } finally {
      connecting.current = false;
      setBusy(false);
    }
  };

  if (window.__ptTest) window.__ptTest.connect = doConnect;

  /** 局域网设备按公钥连；受信设备直接连上，否则面板弹出来要密码 */
  const connectLan = (d: NearbyDevice) => doConnect(d.endpoint_id, "", d);

  /** 手机：扫桌面端标题栏手机图标里的二维码，设备码和密码都在里面，扫完直接连 */
  const scanAndConnect = async () => {
    if (!isMobile || connecting.current) return;
    let content: string;
    try {
      // 扫码插件只有手机端有，用到时才加载
      const scanner = await import("@tauri-apps/plugin-barcode-scanner");
      let permission = await scanner.checkPermissions();
      if (permission !== "granted") permission = await scanner.requestPermissions();
      if (permission !== "granted") {
        toast.error(t("没有相机权限：去系统设置里允许 P2PCLI 使用相机，或者输入设备码连接"));
        return;
      }
      content = (await scanner.scan({ windowed: false, formats: [scanner.Format.QRCode] })).content;
    } catch (e) {
      // 按返回键取消扫码也会走到这里，不算出错
      if (!/cancel/i.test(String(e))) toast.error(t("扫码失败：{{error}}", { error: String(e) }));
      return;
    }
    const pair = parsePairLink(content);
    if (!pair) {
      toast.error(t("这不是 P2PCLI 的连接二维码"));
      return;
    }
    setLanTarget(null);
    setTarget(pair.code ?? "");
    setPassword(pair.password);
    await doConnect(pair.code ?? pair.id, pair.password);
  };

  return { target, setTarget, password, setPassword, mutual, setMutual, busy, error, setError, pwRef, doConnect, lanTarget, setLanTarget, connectLan, scanAndConnect };
}

import { useRef, useState } from "react";
import { api, type ConnectFail, type NearbyDevice, type PathInfo, type RemoteInfo } from "../api";
import { toast } from "@/components/ui/toast";

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

  const doConnect = async (t: string, pw: string, lan: NearbyDevice | null = null) => {
    if (connecting.current || !t.trim()) return;
    connecting.current = true;
    setBusy(true);
    setError(null);
    setLanTarget(lan);
    try {
      const c = await api.connect(t.trim(), pw || null, mutual);
      onConnected(c.conn_id, c.info, c.code, c.path);
      if (mutual && !c.mutual) toast.error("已连上，但没能记下互相信任，对方还不能直接控制本机");
      if (c.mutual) toast.success(`已和 ${c.info.name} 互相信任，它以后可以直接连本机`);
      setPassword("");
      setMutual(false);
      setLanTarget(null);
      onOpenChange(false);
    } catch (e) {
      const f = e as ConnectFail;
      setError(f.message ?? String(e));
      onOpenChange(true);
      if (f.reject === "need_password" || f.reject === "bad_password") {
        if (!lan) setTarget(t.replace(/\D/g, ""));
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

  return { target, setTarget, password, setPassword, mutual, setMutual, busy, error, setError, pwRef, doConnect, lanTarget, setLanTarget, connectLan };
}

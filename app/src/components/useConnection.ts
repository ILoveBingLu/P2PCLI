import { useRef, useState } from "react";
import { api, type ConnectFail, type PathInfo, type RemoteInfo } from "../api";

interface Options {
  onConnected: (connId: number, info: RemoteInfo, code: string | null, path: PathInfo) => void;
  onOpenChange: (open: boolean) => void;
}

export function useConnection({ onConnected, onOpenChange }: Options) {
  const [target, setTarget] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pwRef = useRef<HTMLInputElement>(null);
  const connecting = useRef(false);

  const doConnect = async (t: string, pw: string) => {
    if (connecting.current || !t.trim()) return;
    connecting.current = true;
    setBusy(true);
    setError(null);
    try {
      const c = await api.connect(t.trim(), pw || null);
      onConnected(c.conn_id, c.info, c.code, c.path);
      setPassword("");
      onOpenChange(false);
    } catch (e) {
      const f = e as ConnectFail;
      setError(f.message ?? String(e));
      onOpenChange(true);
      if (f.reject === "need_password" || f.reject === "bad_password") {
        setTarget(t.replace(/\D/g, ""));
        window.setTimeout(() => pwRef.current?.focus(), 0);
      }
    } finally {
      connecting.current = false;
      setBusy(false);
    }
  };

  if (window.__ptTest) window.__ptTest.connect = doConnect;

  return { target, setTarget, password, setPassword, busy, error, pwRef, doConnect };
}

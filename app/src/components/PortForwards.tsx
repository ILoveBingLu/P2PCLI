// 端口转发：把对方电脑上跑着的服务（开发服务器、接口、数据库后台）接到本机，用本机浏览器打开
import { useEffect, useState } from "react";
import { Popover } from "@heroui/react";
import { Cable, Copy, ExternalLink, LoaderCircle, RefreshCw, Square } from "lucide-react";
import { api, type ForwardView, type ListeningPort } from "../api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { toast } from "@/components/ui/toast";
import { writeClipboardText } from "@/lib/clipboard";
import { t, tr } from "@/i18n";

export function PortForwards({ connId, deviceName, forwards, onChanged }: {
  connId: number;
  deviceName: string;
  /** 这台设备正在转发的端口 */
  forwards: ForwardView[];
  onChanged: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [ports, setPorts] = useState<ListeningPort[] | null>(null);
  const [scanError, setScanError] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);
  const [busy, setBusy] = useState<number | null>(null);
  const [manual, setManual] = useState("");

  const scan = async () => {
    setScanning(true);
    setScanError(null);
    try {
      setPorts(await api.listPorts(connId));
    } catch (e) {
      setScanError(tr(String(e)));
    } finally {
      setScanning(false);
    }
  };

  useEffect(() => {
    if (!open) return;
    void scan();
    onChanged();
    // 开着时刷新连接数
    const timer = window.setInterval(onChanged, 2000);
    return () => window.clearInterval(timer);
  }, [open, connId]); // eslint-disable-line react-hooks/exhaustive-deps

  const start = async (port: number, openAfter: boolean) => {
    setBusy(port);
    try {
      const forward = await api.forwardPort(connId, port);
      onChanged();
      if (forward.local_port !== port) toast.notice(t("本机的 {{port}} 端口被占用了，改用 {{local}}", { port, local: forward.local_port }));
      if (openAfter) await api.openForward(forward.id);
    } catch (e) {
      toast.error(tr(String(e)));
    } finally {
      setBusy(null);
    }
  };

  const submitManual = () => {
    const port = Number(manual.trim());
    if (!Number.isInteger(port) || port < 1 || port > 65535) return void toast.error(t("端口号是 1 到 65535 之间的数字"));
    setManual("");
    void start(port, false);
  };

  const forwarded = new Map(forwards.map((f) => [f.remote_port, f]));
  const found = (ports ?? []).filter((p) => !forwarded.has(p.port));

  return (
    <Popover isOpen={open} onOpenChange={setOpen}>
      <Button variant="ghost" size="icon-sm" className="terminal-ports-toggle" data-active={forwards.length > 0 || undefined}
        aria-label={t("端口转发")} title={forwards.length ? t("端口转发（{{count}} 个正在转发）", { count: forwards.length }) : t("端口转发：在本机打开对方电脑上的网页服务")}>
        <Cable />
        {forwards.length > 0 && <span className="terminal-ports-count">{forwards.length}</span>}
      </Button>
      <Popover.Content placement="bottom end" offset={8} className="w-[24rem] max-w-[calc(100vw-24px)]">
        <Popover.Dialog aria-label={t("端口转发")} className="ports-panel">
          <div className="ports-head">
            <Popover.Heading className="ports-title">{t("端口转发")}</Popover.Heading>
            <span className="ports-device truncate">{deviceName}</span>
          </div>
          <p className="ports-note">{t("把对方电脑上 localhost 的端口接到本机，用本机浏览器打开对方跑着的开发服务器。只在本机能访问，经 P2P 加密传输。")}</p>

          {forwards.length > 0 && <section className="ports-section">
            <h3>{t("正在转发")}</h3>
            {forwards.map((f) => (
              <div key={f.id} className="ports-row">
                <span className="ports-port">{f.remote_port}</span>
                <span className="ports-arrow">→</span>
                <button type="button" className="ports-link truncate" title={t("在浏览器里打开")}
                  onClick={() => void api.openForward(f.id).catch((e) => toast.error(tr(String(e))))}>localhost:{f.local_port}</button>
                <span className="ports-meta">{f.connections ? t("{{count}} 个连接", { count: f.connections }) : ""}</span>
                <Button size="icon-xs" variant="ghost" aria-label={t("在浏览器里打开")} title={t("在浏览器里打开")}
                  onPress={() => void api.openForward(f.id).catch((e) => toast.error(tr(String(e))))}><ExternalLink /></Button>
                <Button size="icon-xs" variant="ghost" aria-label={t("复制地址")} title={t("复制地址")}
                  onPress={() => void writeClipboardText(`http://localhost:${f.local_port}/`).then(() => toast.success(t("已复制"))).catch((e) => toast.error(String(e)))}><Copy /></Button>
                <Button size="icon-xs" variant="ghost" aria-label={t("停止转发")} title={t("停止转发")}
                  onPress={() => void api.stopForward(f.id).then(onChanged)}><Square /></Button>
              </div>
            ))}
          </section>}

          <section className="ports-section">
            <h3>
              {t("对方电脑上的服务")}
              <Button size="icon-xs" variant="ghost" aria-label={t("重新查找")} title={t("重新查找")} isDisabled={scanning} onPress={() => void scan()}>
                {scanning ? <LoaderCircle className="animate-spin" /> : <RefreshCw />}
              </Button>
            </h3>
            {scanError && <p className="ports-note is-error">{scanError}</p>}
            {!scanError && ports && found.length === 0 && <p className="ports-note">{t("没有发现别的正在监听的端口。")}</p>}
            {!ports && !scanError && <p className="ports-note"><LoaderCircle className="inline size-3 animate-spin" /> {t("正在查找…")}</p>}
            {found.map((p) => (
              <div key={p.port} className="ports-row">
                <span className="ports-port">{p.port}</span>
                <span className="ports-meta truncate">{p.process ?? ""}</span>
                <Button size="xs" variant="outline" isDisabled={busy != null} onPress={() => void start(p.port, true)}>
                  {busy === p.port ? <LoaderCircle className="animate-spin" /> : <ExternalLink />}{t("转发并打开")}
                </Button>
              </div>
            ))}
          </section>

          <form className="ports-manual" onSubmit={(e) => { e.preventDefault(); submitManual(); }}>
            <Input value={manual} inputMode="numeric" placeholder={t("端口号，比如 3000")} aria-label={t("要转发的端口")}
              onChange={(e) => setManual(e.target.value.replace(/\D/g, "").slice(0, 5))} />
            <Button type="submit" size="sm" isDisabled={!manual || busy != null}>{t("转发")}</Button>
          </form>
        </Popover.Dialog>
      </Popover.Content>
    </Popover>
  );
}

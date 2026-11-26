// 端口转发：把对方电脑上跑着的服务（开发服务器、接口、数据库后台）接到本机，用本机浏览器打开；
// 反过来，把本机的端口接到对方电脑上（反向转发），借网给没有代理的对方也是它
import { useEffect, useState, type ReactNode } from "react";
import { Popover } from "@heroui/react";
import { Cable, Copy, ExternalLink, Globe, LoaderCircle, RefreshCw, Square } from "lucide-react";
import { api, pathLabel, type ForwardView, type ListeningPort, type PathInfo, type ProxyKind, type ReverseView } from "../api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { toast } from "@/components/ui/toast";
import { writeClipboardText } from "@/lib/clipboard";
import { t, tr } from "@/i18n";

/** 用户填的本机地址：只写端口就是 127.0.0.1 上的 */
function localTarget(text: string): string | null {
  const v = text.trim().replace(/^[a-z0-9]+:\/\//i, "").replace(/\/+$/, "");
  if (/^\d{1,5}$/.test(v)) return Number(v) >= 1 && Number(v) <= 65535 ? `127.0.0.1:${v}` : null;
  const m = /^(\[[^\]]+\]|[^:\s]+):(\d{1,5})$/.exec(v);
  return m && Number(m[2]) >= 1 && Number(m[2]) <= 65535 ? v : null;
}

/** 代理地址里的种类：写了 socks5:// 就是 SOCKS5，别的按 HTTP */
function proxyKind(text: string): ProxyKind {
  return /^socks5h?:\/\//i.test(text.trim()) ? "socks5" : "http";
}

type PanelProps = {
  connId: number;
  deviceName: string;
  /** 这台设备正在转发的端口 */
  forwards: ForwardView[];
  /** 这台设备上正在进行的反向转发（含借网） */
  reverses: ReverseView[];
  onChanged: () => void;
};

/** 终端旁边的端口转发按钮：点开是面板 */
export function PortForwards(props: PanelProps) {
  const { forwards, reverses } = props;
  const [open, setOpen] = useState(false);
  return (
    <Popover isOpen={open} onOpenChange={setOpen}>
      <Button variant="ghost" size="icon-sm" className="terminal-ports-toggle terminal-dock-button" data-active={forwards.length + reverses.length > 0 || undefined}
        aria-label={t("端口转发")} title={forwards.length ? t("端口转发（{{count}} 个正在转发）", { count: forwards.length }) : t("端口转发：在本机打开对方电脑上的网页服务")}>
        <Cable />
        {forwards.length + reverses.length > 0 && <span className="terminal-ports-count">{forwards.length + reverses.length}</span>}
      </Button>
      <Popover.Content placement="left" offset={10} className="w-[24rem] max-w-[calc(100vw-24px)]">
        <Popover.Dialog aria-label={t("端口转发")} className="ports-panel">
          <PortForwardsPanel {...props} active={open} heading={<Popover.Heading className="ports-title">{t("端口转发")}</Popover.Heading>} />
        </Popover.Dialog>
      </Popover.Content>
    </Popover>
  );
}

/** 不开终端标签也能用：从设备的右键菜单打开的端口转发、反向转发、借网 */
export function PortForwardsDialog({ open, onOpenChange, ...props }: PanelProps & { open: boolean; onOpenChange: (open: boolean) => void }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="ports-panel ports-dialog">
        <PortForwardsPanel {...props} active={open} heading={<DialogTitle className="ports-title">{t("端口转发")}</DialogTitle>} />
      </DialogContent>
    </Dialog>
  );
}

/** 端口转发面板的内容：正向转发、对方电脑上的服务、反向转发、借网。active 为假时不扫端口、不刷新 */
function PortForwardsPanel({ active, heading, connId, deviceName, forwards, reverses, onChanged }: PanelProps & { active: boolean; heading: ReactNode }) {
  const [ports, setPorts] = useState<ListeningPort[] | null>(null);
  const [scanError, setScanError] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);
  const [busy, setBusy] = useState<number | null>(null);
  const [manual, setManual] = useState("");
  // 反向转发：对方支不支持、设置里的代理（借网默认用它）
  const [reverseInfo, setReverseInfo] = useState<{ supported: boolean; proxy: string | null } | null>(null);
  const [proxyText, setProxyText] = useState("");
  const [reverseLocal, setReverseLocal] = useState("");
  const [reverseRemote, setReverseRemote] = useState("");
  const [reverseBusy, setReverseBusy] = useState(false);
  // 线路：直连优先，中继只兜底。走中继时转发和借网能用但会卡，要说清楚
  const [path, setPath] = useState<PathInfo | null>(null);
  const onRelay = path?.kind === "relay";

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
    if (!active) return;
    let stop = false;
    const tick = () => void api.pathInfo(connId).then((p) => { if (!stop) setPath(p); }).catch(() => {});
    tick();
    const timer = window.setInterval(tick, 2000);
    return () => { stop = true; window.clearInterval(timer); };
  }, [active, connId]);

  /** 开始转发以后：还在中继上就提醒一句（开始前后端已经等过几秒直连） */
  const warnIfRelay = () => void api.pathInfo(connId).then((p) => {
    setPath(p);
    if (p.kind === "relay") toast.notice(t("现在走的是中继，转发能用但会比较慢；直连接通后会自动切过去"));
  }).catch(() => {});

  useEffect(() => {
    if (!active) return;
    void api.reverseInfo(connId).then((info) => {
      setReverseInfo(info);
      setProxyText((old) => old || (info.proxy ? `http://${info.proxy}` : "http://127.0.0.1:7890"));
    }).catch(() => setReverseInfo(null));
    void scan();
    onChanged();
    // 开着时刷新连接数
    const timer = window.setInterval(onChanged, 2000);
    return () => window.clearInterval(timer);
  }, [active, connId]); // eslint-disable-line react-hooks/exhaustive-deps

  const start = async (port: number, openAfter: boolean) => {
    setBusy(port);
    try {
      const forward = await api.forwardPort(connId, port);
      onChanged();
      warnIfRelay();
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

  const lending = reverses.find((r) => r.proxy);
  const startReverse = async (target: string | null, proxy: ProxyKind | null, remotePort?: number) => {
    if (!target) return void toast.error(t("本机地址写端口号，或者 主机:端口"));
    setReverseBusy(true);
    try {
      const view = await api.reverseForward(connId, target, proxy, remotePort);
      onChanged();
      warnIfRelay();
      if (proxy) toast.success(t("已把本机代理借给对方：对方电脑上新开的终端走 127.0.0.1:{{port}}", { port: view.remote_port }));
      return true;
    } catch (e) {
      toast.error(tr(String(e)));
      return false;
    } finally {
      setReverseBusy(false);
    }
  };
  const submitReverse = async () => {
    const remote = reverseRemote.trim() ? Number(reverseRemote.trim()) : undefined;
    if (remote !== undefined && (!Number.isInteger(remote) || remote < 1 || remote > 65535)) return void toast.error(t("端口号是 1 到 65535 之间的数字"));
    if (await startReverse(localTarget(reverseLocal), null, remote)) { setReverseLocal(""); setReverseRemote(""); }
  };

  const forwarded = new Map(forwards.map((f) => [f.remote_port, f]));
  const found = (ports ?? []).filter((p) => !forwarded.has(p.port));

  return (
    <>
          <div className="ports-head">
            {heading}
            <span className="ports-device truncate">{deviceName}</span>
          </div>
          <p className="ports-note">{t("把对方电脑上 localhost 的端口接到本机，用本机浏览器打开对方跑着的开发服务器。只在本机能访问，经 P2P 加密传输。")}</p>
          {path && <p className={`ports-note${onRelay ? " is-warn" : ""}`}>
            {t("线路：{{path}}", { path: pathLabel(path) })}
            {onRelay ? ` · ${t("中继只是兜底，转发和借网能用但会比较慢；直连接通后自动切过去")}` : ""}
          </p>}

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

          <section className="ports-section">
            <h3>{t("反向转发（本机 → 对方）")}</h3>
            {reverseInfo && !reverseInfo.supported ? (
              <p className="ports-note">{t("对方的 P2PCLI 版本较旧，不支持反向端口转发，请更新对方的 P2PCLI")}</p>
            ) : (
              <>
                <p className="ports-note">{t("把本机的端口接到对方电脑的 127.0.0.1 上，对方的程序连那个端口就连到本机。对方电脑上别的程序也能连到这个端口。")}</p>
                {reverses.map((r) => (
                  <div key={r.id} className="ports-row">
                    {r.proxy ? <Globe /> : null}
                    <span className="ports-port">{r.remote_port}</span>
                    <span className="ports-arrow">←</span>
                    <span className="ports-meta truncate" title={r.target}>
                      {r.proxy ? t("本机代理 {{target}}", { target: r.target }) : t("本机 {{target}}", { target: r.target })}
                      {r.connections ? ` · ${t("{{count}} 个连接", { count: r.connections })}` : ""}
                    </span>
                    <Button size="icon-xs" variant="ghost" aria-label={r.proxy ? t("停止借网") : t("停止转发")} title={r.proxy ? t("停止借网") : t("停止转发")}
                      onPress={() => void api.stopReverse(r.id).then(onChanged)}><Square /></Button>
                  </div>
                ))}
                <form className="ports-manual" onSubmit={(e) => { e.preventDefault(); void submitReverse(); }}>
                  <Input value={reverseLocal} placeholder={t("本机端口，比如 8080")} aria-label={t("本机要接过去的端口")}
                    onChange={(e) => setReverseLocal(e.target.value.slice(0, 64))} />
                  <Input value={reverseRemote} inputMode="numeric" placeholder={t("对方端口（可不填）")} aria-label={t("对方电脑上的端口")}
                    onChange={(e) => setReverseRemote(e.target.value.replace(/\D/g, "").slice(0, 5))} />
                  <Button type="submit" size="sm" isDisabled={!reverseLocal || reverseBusy}>{t("转发")}</Button>
                </form>
              </>
            )}
          </section>

          {reverseInfo?.supported && (
            <section className="ports-section">
              <h3>{t("把本机代理借给对方")}</h3>
              <p className="ports-note">{t("对方没有代理、上不了外网时用：之后在对方电脑上新开的终端、远程执行的命令走本机的代理。已经开着的终端不受影响，不改对方系统的代理设置。")}</p>
              {lending ? (
                <div className="ports-row">
                  <Globe />
                  <span className="ports-meta truncate">{t("正在借给对方：{{target}}", { target: lending.target })}</span>
                  <Button size="xs" variant="outline" onPress={() => void api.stopReverse(lending.id).then(onChanged)}><Square />{t("停止借网")}</Button>
                </div>
              ) : (
                <form className="ports-manual" onSubmit={(e) => { e.preventDefault(); void startReverse(localTarget(proxyText), proxyKind(proxyText)); }}>
                  <Input value={proxyText} placeholder="http://127.0.0.1:7890" aria-label={t("本机代理地址")}
                    onChange={(e) => setProxyText(e.target.value.slice(0, 128))} />
                  <Button type="submit" size="sm" isDisabled={!proxyText || reverseBusy}>
                    {reverseBusy ? <LoaderCircle className="animate-spin" /> : <Globe />}{t("借给对方")}
                  </Button>
                </form>
              )}
            </section>
          )}
    </>
  );
}

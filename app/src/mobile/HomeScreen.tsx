import type { ReactNode } from "react";
import { ChevronRight, LoaderCircle, Plus, RefreshCw, ScanLine, Settings, TerminalSquare, TriangleAlert } from "lucide-react";
import logo from "@/assets/logo.svg";
import type { AgentInfo, NearbyDevice, Overview, PathInfo, RecentDevice, TerminalSessionSnapshot } from "../api";
import { Button } from "@/components/ui/button";
import { PlatformIcon } from "../components/PlatformIcon";
import { AgentIcon, AgentStateLabel } from "../components/Agent";
import { useRemarks } from "../components/Remarks";
import { presence, waitingCount, type Conn, type Term } from "./model";
import { t } from "@/i18n";

export interface TermRow {
  term: Term;
  title: string;
  device: string;
  agent: AgentInfo | null;
}

export function HomeScreen({ ov, conns, terms, liveSessions, paths, connecting, scanning, transfers, onDevice, onNearby, onTerminal, onConnectNew, onScan, onScanNearby, onSettings }: {
  ov: Overview | null;
  conns: Record<string, Conn>;
  terms: TermRow[];
  liveSessions: Record<number, TerminalSessionSnapshot>;
  paths: Record<number, PathInfo>;
  /** 正在连哪台设备 */
  connecting: string | null;
  scanning: boolean;
  /** 正在收发的文件 */
  transfers: ReactNode;
  onDevice: (device: RecentDevice) => void;
  onNearby: (device: NearbyDevice) => void;
  onTerminal: (key: number) => void;
  onConnectNew: () => void;
  onScan: () => void;
  onScanNearby: () => void;
  onSettings: () => void;
}) {
  const remarks = useRemarks();
  const seen = new Set<string>();
  const devices = (ov?.recent ?? []).filter((d) => !seen.has(d.endpoint_id) && !!seen.add(d.endpoint_id));
  const nearby = (ov?.nearby ?? []).filter((d) => !seen.has(d.endpoint_id));
  const lan = new Set((ov?.nearby ?? []).filter((d) => d.online).map((d) => d.endpoint_id));
  const statuses = new Map((ov?.devices ?? []).map((d) => [d.endpoint_id, d]));
  const label = (d: { endpoint_id: string; name: string }) => remarks.device(d.endpoint_id) || d.name;
  const empty = !!ov && !devices.length && !nearby.length;

  return (
    <div className="m-screen">
      <header className="m-home-header">
        <img className="m-home-logo" src={logo} alt="" draggable={false} />
        <h1>P2PCLI</h1>
        <Button variant="ghost" size="icon" className="m-icon-button" aria-label={t("扫码连接")} onPress={onScan}>
          <ScanLine />
        </Button>
        <Button variant="ghost" size="icon" className="m-icon-button" aria-label={t("设置")} disabled={!ov} onPress={onSettings}>
          <Settings />
        </Button>
      </header>
      <div className="m-scroll">
        {transfers}
        {ov && ov.server.state !== "online" && ov.server.state !== "connecting" && (
          <button type="button" className="m-banner" onClick={onSettings}>
            <TriangleAlert aria-hidden="true" />
            <span>
              {ov.server.state === "none"
                ? t("还没配置牵线服务器，只能连同一个局域网里的设备")
                : t("牵线服务器没连上，只能连同一个局域网里的设备")}
            </span>
            <ChevronRight aria-hidden="true" />
          </button>
        )}

        {empty ? (
          <div className="m-empty">
            <img src={logo} alt="" draggable={false} />
            <h2>{t("连接你的电脑")}</h2>
            <p>{t("在电脑上打开 P2PCLI，点标题栏的手机图标，用这里扫它显示的二维码。")}</p>
            <div className="m-empty-actions">
              <Button size="lg" className="w-full" onPress={onScan}>
                <ScanLine data-icon="inline-start" />
                {t("扫码连接")}
              </Button>
              <Button size="lg" variant="secondary" className="w-full" onPress={onConnectNew}>
                {t("输入设备码")}
              </Button>
              <Button variant="ghost" className="w-full text-muted-foreground" disabled={scanning} onPress={onScanNearby}>
                {scanning ? <LoaderCircle data-icon="inline-start" className="animate-spin" /> : <RefreshCw data-icon="inline-start" />}
                {t("找找附近的设备")}
              </Button>
            </div>
          </div>
        ) : (
          <>
            {terms.length > 0 && (
              <section className="m-section" aria-labelledby="m-open-terms">
                <h2 id="m-open-terms" className="m-section-title">{t("正在使用")}</h2>
                <div className="m-group">
                  {terms.map(({ term, title, device, agent }) => (
                    <button key={term.key} type="button" className="m-row" onClick={() => onTerminal(term.key)}>
                      <span className="m-row-icon" data-tone={term.state === "closed" || term.state === "exited" ? "off" : undefined}>
                        {agent ? <AgentIcon kind={agent.kind} className="size-5" /> : <TerminalSquare />}
                      </span>
                      <span className="m-row-text">
                        <span className="m-row-title">{title}</span>
                        <span className="m-row-sub">
                          {device}
                          {term.state === "closed" ? ` · ${t("已断开")}` : term.state === "exited" ? ` · ${t("已结束")}` : ""}
                        </span>
                      </span>
                      {agent && term.state === "open" && <AgentStateLabel agent={agent} className="m-row-trailing-label" />}
                      <ChevronRight className="m-row-chevron" aria-hidden="true" />
                    </button>
                  ))}
                </div>
              </section>
            )}

            {devices.length > 0 && (
              <section className="m-section" aria-labelledby="m-devices">
                <h2 id="m-devices" className="m-section-title">{t("我的设备")}</h2>
                <div className="m-group">
                  {devices.map((device) => {
                    const conn = conns[device.endpoint_id];
                    const status = statuses.get(device.endpoint_id);
                    const p = presence(status, !!conn, conn ? paths[conn.connId] : undefined);
                    const sessions = conn ? liveSessions[conn.connId]?.sessions : status?.sessions_live ? status.sessions : null;
                    const waiting = waitingCount(sessions);
                    const busy = connecting === device.endpoint_id;
                    return (
                      <button key={device.endpoint_id} type="button" className="m-row" disabled={!!connecting && !busy}
                        onClick={() => onDevice(device)}>
                        <span className="m-device-tile" data-connected={conn ? "" : undefined}>
                          <PlatformIcon platform={device.platform} decorative />
                        </span>
                        <span className="m-row-text">
                          <span className="m-row-title">{label(device)}</span>
                          <span className="m-row-sub">
                            {busy ? (
                              <span className="m-presence" data-tone="busy">{t("等待对方同意…")}</span>
                            ) : (
                              <span className="m-presence" data-tone={p.tone}>{p.text}</span>
                            )}
                            {!busy && lan.has(device.endpoint_id) && !conn && <span className="m-chip">{t("局域网")}</span>}
                            {!busy && sessions && sessions.length > 0 && (
                              <span>{t("{{count}} 个会话", { count: sessions.length })}</span>
                            )}
                          </span>
                          {waiting > 0 && !busy && (
                            <span className="m-row-alert">{t("{{count}} 个 AI 在等你确认", { count: waiting })}</span>
                          )}
                        </span>
                        {busy ? <LoaderCircle className="m-row-chevron animate-spin" aria-hidden="true" /> : <ChevronRight className="m-row-chevron" aria-hidden="true" />}
                      </button>
                    );
                  })}
                </div>
              </section>
            )}

            <section className="m-section" aria-labelledby="m-nearby">
              <div className="m-section-head">
                <h2 id="m-nearby" className="m-section-title">{t("附近设备")}</h2>
                <Button variant="ghost" size="icon-sm" className="m-section-action" aria-label={t("刷新附近设备")} disabled={scanning} onPress={onScanNearby}>
                  <RefreshCw className={scanning ? "animate-spin" : undefined} />
                </Button>
              </div>
              {nearby.length ? (
                <div className="m-group">
                  {nearby.map((device) => {
                    const busy = connecting === device.endpoint_id;
                    return (
                      <button key={device.endpoint_id} type="button" className="m-row" disabled={!device.online || (!!connecting && !busy)}
                        onClick={() => onNearby(device)}>
                        <span className="m-device-tile" data-connected={conns[device.endpoint_id] ? "" : undefined}>
                          <PlatformIcon platform={device.platform} decorative />
                        </span>
                        <span className="m-row-text">
                          <span className="m-row-title">{label(device)}</span>
                          <span className="m-row-sub">
                            {busy
                              ? <span className="m-presence" data-tone="busy">{t("等待对方同意…")}</span>
                              : <span className="m-presence" data-tone={device.online ? "ok" : "off"}>{device.online ? t("同一个局域网") : t("离线")}</span>}
                          </span>
                        </span>
                        {busy ? <LoaderCircle className="m-row-chevron animate-spin" aria-hidden="true" /> : <ChevronRight className="m-row-chevron" aria-hidden="true" />}
                      </button>
                    );
                  })}
                </div>
              ) : (
                <p className="m-section-empty">{scanning ? t("正在找…") : t("同一个局域网里没找到别的设备")}</p>
              )}
            </section>
          </>
        )}
      </div>
      {!empty && (
        <div className="m-bottom-bar">
          <Button size="lg" className="w-full" onPress={onConnectNew}>
            <Plus data-icon="inline-start" />
            {t("连接新设备")}
          </Button>
        </div>
      )}
    </div>
  );
}

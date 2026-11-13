import { useState, type ReactNode } from "react";
import { ChevronLeft, EllipsisVertical, FileUp, LoaderCircle, Plug, Plus, RefreshCw, SquareArrowOutUpRight, TerminalSquare, Trash2, Unplug } from "lucide-react";
import type { DeviceStatus, PathInfo, TerminalSession, TerminalSessionSnapshot } from "../api";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { PlatformIcon } from "../components/PlatformIcon";
import { AgentIcon, AgentStateLabel } from "../components/Agent";
import { relativeTime, sessionTitles } from "../components/sessionText";
import { presence, shortPath, type Conn, type Term } from "./model";
import { Sheet, SheetAction } from "./Sheet";
import { t } from "@/i18n";

export function DeviceScreen({ name, platform, conn, status, snapshot, path, terms, busy, transfers, onBack, onConnect, onOpen, onEnd, onSendFiles, onDisconnect, onRefresh, sheet }: {
  name: string;
  platform: string;
  /** 没连着（比如刚断开）时为空 */
  conn: Conn | undefined;
  status: DeviceStatus | undefined;
  snapshot: TerminalSessionSnapshot | undefined;
  path: PathInfo | undefined;
  /** 这台设备上手机开着的终端 */
  terms: Term[];
  busy: boolean;
  /** 正在收发的文件 */
  transfers: ReactNode;
  onBack: () => void;
  onConnect: () => void;
  /** 打开会话；不带 id 是新建 */
  onOpen: (sessionId?: string) => void;
  onEnd: (sessionId: string) => Promise<void>;
  onSendFiles: () => void;
  onDisconnect: () => void;
  onRefresh: () => void;
  /** 开、关会话操作面板时要登记到返回键 */
  sheet: { open: (close: () => void) => void; close: () => void };
}) {
  const [menuFor, setMenuFor] = useState<TerminalSession | null>(null);
  const [confirmEnd, setConfirmEnd] = useState(false);
  const [ending, setEnding] = useState(false);
  const p = presence(status, !!conn, path);
  const sessions = conn ? snapshot?.sessions ?? null : null;
  const titles = sessionTitles(sessions ?? []);
  const ordered = [...(sessions ?? [])].sort((a, b) => (b.created_at ?? 0) - (a.created_at ?? 0));
  const openIds = new Set(terms.filter((term) => term.state === "open" || term.state === "opening").map((term) => term.sessionId));
  const closeMenu = () => {
    setMenuFor(null);
    setConfirmEnd(false);
    sheet.close();
  };

  return (
    <div className="m-screen">
      <header className="m-nav">
        <Button variant="ghost" size="icon" className="m-icon-button" aria-label={t("返回")} onPress={onBack}>
          <ChevronLeft />
        </Button>
        <div className="m-nav-title">
          <span className="m-nav-name">{name}</span>
          <span className="m-presence" data-tone={p.tone}>{p.text}</span>
        </div>
        {conn && (
          <Button variant="ghost" size="icon" className="m-icon-button" aria-label={t("发送文件")} onPress={onSendFiles}>
            <FileUp />
          </Button>
        )}
      </header>
      <div className="m-scroll">
        {transfers}
        <div className="m-device-hero">
          <span className="m-device-tile m-device-tile-lg" data-connected={conn ? "" : undefined}>
            <PlatformIcon platform={platform} decorative />
          </span>
        </div>

        {!conn ? (
          <div className="m-empty m-empty-compact">
            <h2>{t("没有连着这台设备")}</h2>
            <p>{t("连接断开了，后台会话还在对方电脑上运行，重新连上就能接着用。")}</p>
            <div className="m-empty-actions">
              <Button size="lg" className="w-full" disabled={busy} onPress={onConnect}>
                {busy ? <LoaderCircle data-icon="inline-start" className="animate-spin" /> : <Plug data-icon="inline-start" />}
                {busy ? t("等待对方同意…") : t("重新连接")}
              </Button>
            </div>
          </div>
        ) : (
          <section className="m-section" aria-labelledby="m-sessions">
            <div className="m-section-head">
              <h2 id="m-sessions" className="m-section-title">{t("会话")}</h2>
              <Button variant="ghost" size="icon-sm" className="m-section-action" aria-label={t("刷新会话")} onPress={onRefresh}>
                <RefreshCw />
              </Button>
            </div>
            {snapshot?.error && (
              <Alert variant="destructive" className="mb-3">
                <AlertDescription>{t("读不到会话列表：{{error}}", { error: snapshot.error })}</AlertDescription>
              </Alert>
            )}
            {sessions === null && !snapshot?.error ? (
              <p className="m-section-empty"><LoaderCircle className="inline size-4 animate-spin" /> {t("正在读取会话…")}</p>
            ) : ordered.length === 0 ? (
              <p className="m-section-empty">{t("这台设备上还没有会话，新建一个终端开始用。")}</p>
            ) : (
              <div className="m-group">
                {ordered.map((session) => {
                  const agent = session.agent ?? null;
                  const where = shortPath(session.details?.current_path);
                  const command = session.details?.command;
                  return (
                    <div key={session.id} className="m-row m-row-split">
                      <button type="button" className="m-row-main" onClick={() => onOpen(session.id)}>
                        <span className="m-row-icon">
                          {agent ? <AgentIcon kind={agent.kind} className="size-5" /> : <TerminalSquare />}
                        </span>
                        <span className="m-row-text">
                          <span className="m-row-title">
                            {titles.get(session.id)}
                            {openIds.has(session.id) && <span className="m-chip m-chip-accent">{t("已打开")}</span>}
                          </span>
                          <span className="m-row-sub">
                            {agent ? <AgentStateLabel agent={agent} /> : command && <span className="m-mono">{command}</span>}
                            {where && <span className="m-mono m-truncate">{where}</span>}
                          </span>
                          <span className="m-row-meta">{t("{{time}}创建", { time: relativeTime(session.created_at) })}</span>
                        </span>
                      </button>
                      <Button variant="ghost" size="icon" className="m-icon-button" aria-label={t("会话操作")}
                        onPress={() => { setMenuFor(session); sheet.open(() => { setMenuFor(null); setConfirmEnd(false); }); }}>
                        <EllipsisVertical />
                      </Button>
                    </div>
                  );
                })}
              </div>
            )}
            <button type="button" className="m-text-danger" onClick={onDisconnect}>
              <Unplug aria-hidden="true" />
              {t("断开连接")}
            </button>
          </section>
        )}
      </div>
      {conn && (
        <div className="m-bottom-bar">
          <Button size="lg" className="w-full" onPress={() => onOpen()}>
            <Plus data-icon="inline-start" />
            {t("新建终端")}
          </Button>
        </div>
      )}

      <Sheet open={!!menuFor} title={menuFor ? titles.get(menuFor.id) : undefined} onClose={closeMenu}>
        {menuFor && !confirmEnd && (
          <div className="m-sheet-actions">
            <SheetAction icon={<SquareArrowOutUpRight />} label={t("打开")} onPress={() => { const id = menuFor.id; closeMenu(); onOpen(id); }} />
            <SheetAction icon={<Trash2 />} label={t("结束会话")} description={t("关掉对方电脑上的这个会话和里面运行的程序")} danger
              onPress={() => setConfirmEnd(true)} />
          </div>
        )}
        {menuFor && confirmEnd && (
          <div className="m-confirm">
            <p>{t("结束后里面运行的程序会被关掉，没保存的内容会丢失。")}</p>
            <Button size="lg" variant="destructive" className="w-full" disabled={ending} onPress={() => {
              setEnding(true);
              void onEnd(menuFor.id).then(closeMenu, () => {}).finally(() => setEnding(false));
            }}>
              {ending ? <LoaderCircle data-icon="inline-start" className="animate-spin" /> : <Trash2 data-icon="inline-start" />}
              {t("结束会话")}
            </Button>
            <Button size="lg" variant="ghost" className="w-full" disabled={ending} onPress={() => setConfirmEnd(false)}>{t("取消")}</Button>
          </div>
        )}
      </Sheet>
    </div>
  );
}

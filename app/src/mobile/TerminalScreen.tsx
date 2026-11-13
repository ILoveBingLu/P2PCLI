import { useRef, useState, type MutableRefObject } from "react";
import { ChevronLeft, Ellipsis, FileUp, Keyboard, LoaderCircle, Plus, SendHorizontal, TerminalSquare, X } from "lucide-react";
import type { AgentInfo, PathInfo, TermStatus } from "../api";
import { TerminalView, type TerminalHandle } from "../TerminalView";
import { Button } from "@/components/ui/button";
import { AgentApproval } from "../components/AgentApproval";
import { AgentIcon, AgentStateLabel } from "../components/Agent";
import { MobileKeyBar } from "./MobileKeyBar";
import { Sheet, SheetAction } from "./Sheet";
import type { Term } from "./model";
import { t } from "@/i18n";

export interface TermMeta {
  title: string;
  device: string;
  platform: string;
  agent: AgentInfo | null;
  path?: PathInfo;
}

/**
 * 终端页：所有开着的终端都挂着（切走不断），只显示当前这个。
 * 顶上返回、切换终端、更多操作；下面是 AI 的确认选项、输入栏和一排按键。
 */
export function TerminalScreen({ visible, activeKey, terms, meta, handles, onBack, onSwitch, onClose, onNew, onSendFiles, onReady, onStatus, onReconnect, onChoose, sheet }: {
  visible: boolean;
  activeKey: number | null;
  terms: Term[];
  meta: (term: Term) => TermMeta;
  handles: MutableRefObject<Map<number, TerminalHandle>>;
  onBack: () => void;
  onSwitch: (key: number) => void;
  /** 关掉手机上的这个终端，会话在对方电脑上接着跑 */
  onClose: (key: number) => void;
  /** 在这个终端所在的设备上新建一个 */
  onNew: (term: Term) => void;
  onSendFiles: (term: Term) => void;
  onReady: (key: number, termId: number, sessionId: string) => void;
  onStatus: (key: number, status: TermStatus) => void;
  onReconnect: (term: Term) => void;
  onChoose: (term: Term, approvalId: string, optionId: number) => Promise<void>;
  sheet: { open: (close: () => void) => void; close: () => void };
}) {
  const active = terms.find((term) => term.key === activeKey) ?? null;
  const info = active ? meta(active) : null;
  const [panel, setPanel] = useState<"switch" | "more" | null>(null);
  const [composing, setComposing] = useState(false);
  const [draft, setDraft] = useState("");
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const handle = () => (active ? handles.current.get(active.key) : undefined);
  const showPanel = (next: "switch" | "more") => {
    setPanel(next);
    sheet.open(() => setPanel(null));
  };
  const hidePanel = () => {
    setPanel(null);
    sheet.close();
  };
  /** 输入栏：用系统输入法打字，整段发过去再按回车；AI 和 shell 都适用 */
  const send = () => {
    const h = handle();
    if (!h || !draft) return;
    h.paste(draft);
    window.setTimeout(() => h.enter(), 80);
    setDraft("");
    inputRef.current?.focus();
  };
  const approval = active?.state === "open" && info?.agent?.state === "waiting" ? info.agent.approval ?? null : null;
  const live = active?.state === "open";

  return (
    <div className="m-term-screen" hidden={!visible}>
      <header className="m-nav m-term-nav">
        <Button variant="ghost" size="icon" className="m-icon-button" aria-label={t("返回")} onPress={onBack}>
          <ChevronLeft />
        </Button>
        <div className="m-nav-title">
          <span className="m-nav-name">
            {info?.agent && <AgentIcon kind={info.agent.kind} className="size-4" />}
            {info?.title}
          </span>
          <span className="m-nav-sub">
            <span className="m-dot" data-tone={!live ? "off" : info?.path?.kind === "relay" ? "relay" : "ok"} aria-hidden="true" />
            {info?.device}
            {info?.agent && live && <AgentStateLabel agent={info.agent} />}
          </span>
        </div>
        <Button variant="ghost" size="icon" className="m-icon-button m-term-count" aria-label={t("切换终端，共 {{count}} 个", { count: terms.length })}
          onPress={() => showPanel("switch")}>
          <span>{terms.length}</span>
        </Button>
        <Button variant="ghost" size="icon" className="m-icon-button" aria-label={t("更多")} onPress={() => showPanel("more")}>
          <Ellipsis />
        </Button>
      </header>

      <div className="m-term-stack">
        {terms.map((term) => (
          <div key={term.key} className="m-term-panel" hidden={term.key !== activeKey}>
            <TerminalView
              connId={term.connId}
              sessionId={term.sessionId}
              tmuxWheel={meta(term).platform !== "windows"}
              active={visible && term.key === activeKey}
              onReady={(termId, sessionId) => onReady(term.key, termId, sessionId)}
              onStatus={(status) => onStatus(term.key, status)}
              onHandle={(h) => {
                if (h) handles.current.set(term.key, h);
                else handles.current.delete(term.key);
              }}
            />
            {term.state === "opening" && (
              <div className="m-term-overlay" role="status">
                <LoaderCircle className="animate-spin" />
                {t("正在打开终端…")}
              </div>
            )}
            {(term.state === "closed" || term.state === "exited") && (
              <div className="m-term-overlay m-term-overlay-card">
                <div className="m-term-ended">
                  <h2>{term.state === "exited" ? t("这个终端已经结束") : t("连接已断开")}</h2>
                  {term.note && <p>{term.note}</p>}
                  <p className="m-muted">{t("会话还在对方电脑上运行的话，重新连上就能接着用。")}</p>
                  {term.state === "closed" ? (
                    <Button size="lg" className="w-full" onPress={() => onReconnect(term)}>{t("重新连接")}</Button>
                  ) : (
                    <Button size="lg" className="w-full" onPress={() => onNew(term)}>
                      <Plus data-icon="inline-start" />
                      {t("新建终端")}
                    </Button>
                  )}
                  <Button size="lg" variant="ghost" className="w-full" onPress={() => onClose(term.key)}>{t("关闭")}</Button>
                </div>
              </div>
            )}
          </div>
        ))}
      </div>

      {approval && info?.agent && active && (
        <div className="m-approval">
          <AgentApproval key={approval.id} approval={approval} name={info.agent.name}
            onChoose={(approvalId, optionId) => onChoose(active, approvalId, optionId)}
            onTerminal={() => handle()?.focus()} />
        </div>
      )}

      {live && composing && (
        <form className="m-compose" onSubmit={(e) => { e.preventDefault(); send(); }}>
          <textarea
            ref={inputRef}
            className="m-compose-input"
            rows={1}
            value={draft}
            placeholder={info?.agent ? t("发给 {{name}}", { name: info.agent.name }) : t("输入命令，发送后自动回车")}
            enterKeyHint="send"
            autoFocus
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              // 输入法还在拼字时的回车是选字，不发送
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                send();
              }
            }}
          />
          <Button type="submit" size="icon" className="m-compose-send" aria-label={t("发送")} disabled={!draft}>
            <SendHorizontal />
          </Button>
        </form>
      )}

      {live && (
        <MobileKeyBar
          onKeys={(data) => handle()?.keys(data)}
          leading={
            <button type="button" className="m-key m-key-toggle" data-active={composing ? "" : undefined}
              aria-label={composing ? t("收起输入栏") : t("打开输入栏")} aria-pressed={composing}
              onPointerDown={(e) => {
                e.preventDefault();
                setComposing((v) => !v);
              }}>
              <Keyboard />
            </button>
          }
        />
      )}

      <Sheet open={panel === "switch"} title={t("开着的终端")} onClose={hidePanel}>
        <div className="m-sheet-list">
          {terms.map((term) => {
            const m = meta(term);
            return (
              <div key={term.key} className="m-switch-row" data-active={term.key === activeKey ? "" : undefined}>
                <button type="button" className="m-switch-main" onClick={() => { hidePanel(); onSwitch(term.key); }}>
                  <span className="m-row-icon">{m.agent ? <AgentIcon kind={m.agent.kind} className="size-5" /> : <TerminalSquare />}</span>
                  <span className="m-row-text">
                    <span className="m-row-title">{m.title}</span>
                    <span className="m-row-sub">
                      {m.device}
                      {term.state === "closed" ? ` · ${t("已断开")}` : term.state === "exited" ? ` · ${t("已结束")}` : ""}
                    </span>
                  </span>
                  {m.agent && term.state === "open" && <AgentStateLabel agent={m.agent} />}
                </button>
                <Button variant="ghost" size="icon" className="m-icon-button" aria-label={t("关闭 {{name}}", { name: m.title })}
                  onPress={() => {
                    if (terms.length === 1) hidePanel();
                    onClose(term.key);
                  }}>
                  <X />
                </Button>
              </div>
            );
          })}
        </div>
        {active && (
          <div className="m-sheet-actions">
            <SheetAction icon={<Plus />} label={t("在 {{name}} 上新建终端", { name: info?.device ?? "" })}
              onPress={() => { hidePanel(); onNew(active); }} />
          </div>
        )}
      </Sheet>

      <Sheet open={panel === "more"} title={info?.title} onClose={hidePanel}>
        {active && (
          <div className="m-sheet-actions">
            <SheetAction icon={<FileUp />} label={t("发送文件到 {{name}}", { name: info?.device ?? "" })}
              description={t("放在对方的下载文件夹里")} disabled={!live}
              onPress={() => { hidePanel(); onSendFiles(active); }} />
            <SheetAction icon={<Plus />} label={t("新建终端")} onPress={() => { hidePanel(); onNew(active); }} />
            <SheetAction icon={<X />} label={t("关闭这个终端")} description={t("会话在对方电脑上接着运行，之后还能打开")}
              onPress={() => { hidePanel(); onClose(active.key); }} />
          </div>
        )}
      </Sheet>
    </div>
  );
}

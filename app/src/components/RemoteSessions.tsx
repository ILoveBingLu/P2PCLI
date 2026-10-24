import { useCallback, useEffect, useState } from "react";
import { ArrowLeft, LoaderCircle, Plus, RefreshCw, Terminal, Trash2 } from "lucide-react";
import { api, type RemoteInfo, type TerminalSession } from "../api";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { AlertDialog, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";

export interface SessionTarget { connId: number; info: RemoteInfo; code: string | null }

export function RemoteSessions({ target, onOpen, onBack, onEnded }: {
  target: SessionTarget;
  onOpen: (id?: string) => void;
  onBack: () => void;
  onEnded: (id: string) => void;
}) {
  const [sessions, setSessions] = useState<TerminalSession[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [ending, setEnding] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try { setSessions(await api.listTerminals(target.connId)); }
    catch (e) { setError(String(e)); }
    finally { setLoading(false); }
  }, [target.connId]);
  useEffect(() => { void load(); }, [load]);
  const end = async () => {
    if (!ending || busy) return;
    setBusy(true);
    try {
      await api.endTerminal(target.connId, ending);
      onEnded(ending);
      setEnding(null);
      await load();
    } catch (e) { setError(String(e)); setEnding(null); }
    finally { setBusy(false); }
  };
  return (
    <section className="settings-page" aria-labelledby="remote-sessions-title">
      <div className="settings-page-inner">
        <header className="flex flex-wrap items-center gap-3">
          <Button variant="ghost" size="icon-sm" title="返回工作区" aria-label="返回工作区" onClick={onBack}><ArrowLeft /></Button>
          <div className="min-w-0 flex-1">
            <h1 id="remote-sessions-title" className="text-lg font-semibold">{target.info.name} · 会话</h1>
            <p className="text-sm text-muted-foreground">断线或关闭标签后，会话仍在这台设备上运行。</p>
          </div>
          <Button variant="ghost" size="icon-sm" title="刷新会话" aria-label="刷新会话" disabled={loading} onClick={load}><RefreshCw /></Button>
          <Button onClick={() => onOpen()} disabled={loading || !!error}><Plus />新建会话</Button>
        </header>
        {error && <Alert variant="destructive"><AlertDescription>{error}</AlertDescription></Alert>}
        {loading ? <div className="loading-state"><LoaderCircle className="animate-spin" />正在读取会话…</div> : (
          <div className="flex flex-col gap-3">
            {!error && sessions.length === 0 && <p className="py-8 text-center text-muted-foreground">暂无后台会话，点击“新建会话”开始。</p>}
            {sessions.map((s) => (
              <div key={s.id} className="flex items-center gap-3 rounded-xl bg-muted-surface p-4">
                <Terminal className="size-5 shrink-0" />
                <div className="min-w-0 flex-1">
                  <p className="font-medium">会话 {s.id.slice(0, 8)}</p>
                  <p className="text-xs text-muted-foreground">{s.created_at ? new Date(s.created_at * 1000).toLocaleString() : "后台会话"}</p>
                </div>
                <Badge variant="secondary">{s.attached ? "已连接" : "后台运行"}</Badge>
                <Button variant="secondary" size="sm" onClick={() => onOpen(s.id)}>恢复</Button>
                <Button variant="ghost" size="icon-sm" aria-label={`结束会话 ${s.id.slice(0, 8)}`} title="结束会话及其中的程序" onClick={() => setEnding(s.id)}><Trash2 /></Button>
              </div>
            ))}
          </div>
        )}
      </div>
      {ending && (
        <AlertDialog open>
          <AlertDialogContent>
            <AlertDialogHeader><AlertDialogTitle>结束这个会话？</AlertDialogTitle><AlertDialogDescription>会话中的程序将被终止，尚未保存的内容可能丢失。只想暂时离开，请关闭终端标签。</AlertDialogDescription></AlertDialogHeader>
            <AlertDialogFooter><Button variant="outline" disabled={busy} onClick={() => setEnding(null)}>取消</Button><Button variant="destructive" disabled={busy} onClick={end}>{busy && <LoaderCircle className="animate-spin" />}结束会话</Button></AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      )}
    </section>
  );
}

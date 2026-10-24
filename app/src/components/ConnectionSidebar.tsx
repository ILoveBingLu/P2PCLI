import { History, Monitor, Trash2 } from "lucide-react";
import { api, formatCode, platformLabel, type Overview } from "../api";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { toast } from "@/components/ui/toast";

export function ConnectionSidebar({ ov, busy, onConnect, onRefresh }: {
  ov: Overview | null;
  busy: boolean;
  onConnect: (code: string) => void;
  onRefresh: () => void;
}) {
  return (
    <aside className="connection-sidebar" aria-label="连接历史">
      <section className="recent-section" aria-labelledby="recent-title">
        <div className="section-title">
          <h2 id="recent-title">
            <History />
            连接历史
          </h2>
          <Badge variant="secondary">{ov?.recent.length ?? 0}</Badge>
        </div>
        {ov && ov.recent.length > 0 ? (
          <div className="recent-list">
            {ov.recent.map((r) => (
              <div className="recent-device" key={r.code}>
                <Button
                  variant="ghost"
                  className="h-auto min-w-0 flex-1 justify-start px-2 py-3"
                  disabled={busy}
                  onClick={() => onConnect(r.code)}
                  title={`连接 ${r.name}`}
                >
                  <Monitor data-icon="inline-start" />
                  <span className="recent-copy">
                    <span className="truncate">{r.name}</span>
                    <span className="recent-meta">
                      {platformLabel(r.platform)} · {formatCode(r.code)}
                    </span>
                  </span>
                </Button>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label={`移除连接历史 ${r.name}`}
                  title="从连接历史移除"
                  onClick={() =>
                    api
                      .forgetRecent(r.code)
                      .then(onRefresh)
                      .catch((e) => toast.error(String(e)))
                  }
                >
                  <Trash2 />
                </Button>
              </div>
            ))}
          </div>
        ) : (
          <p className="recent-placeholder">连接过的设备会出现在这里。</p>
        )}
      </section>
    </aside>
  );
}

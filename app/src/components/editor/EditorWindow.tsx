// 代码编辑器的单独窗口：每台设备一个，主窗口里点编辑器按钮、终端里点文件路径时由后端打开或叫到前面。
// 窗口自己不管连接：按设备问后端现在用哪条，断开时编辑器保留打开的文件和没保存的修改，重连后接着用。
import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LoaderCircle } from "lucide-react";
import { api, type EditorRequest } from "../../api";
import { Button } from "@/components/ui/button";
import {
  AlertDialog, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { t } from "@/i18n";

// Monaco 十几 MB，窗口打开后再加载
const RemoteEditor = lazy(() => import("./RemoteEditor"));

/** 隔这么久问一次这台设备现在用哪条连接（断开、重连都能跟上） */
const CONNECTION_POLL_MS = 3000;

export default function EditorWindow() {
  const [device, setDevice] = useState<{ endpointId: string; name: string } | null>(null);
  const [connId, setConnId] = useState<number | null>(null);
  const [root, setRoot] = useState({ path: "", seq: 0 });
  const [open, setOpen] = useState<{ path: string; line: number; col: number; seq: number } | undefined>();
  const [dirty, setDirty] = useState(0);
  const [askClose, setAskClose] = useState(false);
  const dirtyRef = useRef(0);
  dirtyRef.current = dirty;

  // 取走排队的请求：启动时一次，主窗口又有事（终端里点了文件）时再取
  useEffect(() => {
    const apply = (requests: EditorRequest[]) => {
      for (const r of requests) {
        if (r.root != null) setRoot((old) => ({ path: r.root!, seq: old.seq + 1 }));
        if (r.path) setOpen((old) => ({ path: r.path!, line: r.line ?? 1, col: r.col ?? 1, seq: (old?.seq ?? 0) + 1 }));
      }
    };
    // 取走的请求一定要用掉（后端那边已经清了），组件就算刚被卸载重装也照样应用
    const take = () => api.editorTakeRequests().then((boot) => {
      apply(boot.requests);
      setDevice((old) => old ?? { endpointId: boot.endpoint_id, name: boot.device_name });
    }).catch(() => {});
    void take();
    const unlisten = listen("editor-request", () => void take());
    return () => { void unlisten.then((u) => u()); };
  }, []);

  // 这台设备现在用哪条连接
  useEffect(() => {
    if (!device) return;
    let stop = false;
    const check = () => api.editorConnection(device.endpointId).then((id) => { if (!stop) setConnId(id ?? null); }).catch(() => {});
    void check();
    const timer = window.setInterval(check, CONNECTION_POLL_MS);
    const unlisten = listen("remote-closed", () => void check());
    return () => { stop = true; window.clearInterval(timer); void unlisten.then((u) => u()); };
  }, [device]);

  // 标题里标出有没保存的文件
  useEffect(() => {
    if (!device) return;
    const title = t("{{device}} — 代码编辑器", { device: device.name });
    void getCurrentWindow().setTitle(dirty ? `● ${title}` : title).catch(() => {});
  }, [device, dirty]);

  // 点窗口的关闭按钮：有没保存的文件先问
  useEffect(() => {
    const unlisten = getCurrentWindow().onCloseRequested((event) => {
      if (dirtyRef.current > 0) {
        event.preventDefault();
        setAskClose(true);
      }
    });
    return () => { void unlisten.then((u) => u()); };
  }, []);

  const close = () => void getCurrentWindow().destroy();

  if (!device) {
    return <div className="editor-window"><div className="remote-editor-placeholder"><LoaderCircle className="animate-spin" />{t("正在加载编辑器…")}</div></div>;
  }
  return (
    <div className="editor-window">
      <Suspense fallback={<div className="remote-editor-placeholder"><LoaderCircle className="animate-spin" />{t("正在加载编辑器…")}</div>}>
        <RemoteEditor connId={connId} root={root.path} rootSeq={root.seq} openRequest={open} active
          onDirty={setDirty} onClose={close} />
      </Suspense>
      {askClose && <AlertDialog open>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("有 {{count}} 个文件未保存", { count: dirty })}</AlertDialogTitle>
            <AlertDialogDescription>{t("关闭编辑器会丢失这些修改。")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <Button variant="outline" onPress={() => setAskClose(false)}>{t("取消")}</Button>
            <Button variant="destructive" onPress={close}>{t("不保存，关闭编辑器")}</Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>}
    </div>
  );
}

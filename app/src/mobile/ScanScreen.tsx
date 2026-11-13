import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { Check, KeyRound, X } from "lucide-react";
import { parsePairLink, type PairInfo } from "@/lib/pairLink";
import { t } from "@/i18n";

type Phase = "scanning" | "found" | "wrong" | "denied" | "error";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * 扫码页。相机画面由扫码插件放在网页后面（windowed 模式），网页变透明，
 * 扫描框、提示、返回按钮都是这一页自己画的；系统返回键和返回手势走页面栈，关掉这一页而不是退出软件。
 * 扫到 P2PCLI 的二维码就震一下、框变绿，交给 onResult；扫到别的二维码提示一下接着扫。
 */
export function ScanScreen({ onResult, onClose, onManual }: {
  onResult: (pair: PairInfo) => void;
  onClose: () => void;
  /** 改成手输设备码 */
  onManual: () => void;
}) {
  const [phase, setPhase] = useState<Phase>("scanning");
  const [error, setError] = useState("");
  const done = useRef(onResult);
  done.current = onResult;

  useEffect(() => {
    let alive = true;
    // 网页铺的底色都去掉，露出后面的相机画面
    document.documentElement.dataset.scanning = "";
    const run = async () => {
      // 扫码插件只有手机端有，用到时才加载
      const scanner = await import("@tauri-apps/plugin-barcode-scanner");
      try {
        let permission = await scanner.checkPermissions();
        if (permission !== "granted") permission = await scanner.requestPermissions();
        if (!alive) return;
        if (permission !== "granted") return setPhase("denied");
      } catch (e) {
        if (alive) { setError(String(e)); setPhase("error"); }
        return;
      }
      // 插件扫到一个码就停；不是 P2PCLI 的码时提示一下，再开一次接着扫
      while (alive) {
        setPhase("scanning");
        let content: string;
        try {
          content = (await scanner.scan({ windowed: true, formats: [scanner.Format.QRCode] })).content;
        } catch (e) {
          // 关掉这一页时取消扫码也会走到这里
          if (alive && !/cancel/i.test(String(e))) { setError(String(e)); setPhase("error"); }
          return;
        }
        if (!alive) return;
        const pair = parsePairLink(content);
        if (pair) {
          setPhase("found");
          void invoke("plugin:barcode-scanner|vibrate").catch(() => {});
          await sleep(500);
          if (alive) done.current(pair);
          return;
        }
        setPhase("wrong");
        await sleep(1600);
      }
    };
    void run();
    return () => {
      alive = false;
      delete document.documentElement.dataset.scanning;
      void import("@tauri-apps/plugin-barcode-scanner").then((s) => s.cancel()).catch(() => {});
    };
  }, []);

  const openSettings = () => void import("@tauri-apps/plugin-barcode-scanner").then((s) => s.openAppSettings()).catch(() => {});

  return createPortal(
    <div className="m-scan" data-phase={phase} role="dialog" aria-modal="true" aria-label={t("扫码连接")}>
      <header className="m-scan-top">
        <button type="button" className="m-scan-round" aria-label={t("关闭扫码")} onClick={onClose}>
          <X />
        </button>
        <span className="m-scan-title">{t("扫码连接")}</span>
        <span className="m-scan-round-spacer" aria-hidden="true" />
      </header>

      {phase === "denied" || phase === "error" ? (
        <div className="m-scan-message">
          <h2>{phase === "denied" ? t("没有相机权限") : t("打不开相机")}</h2>
          <p>{phase === "denied" ? t("在系统设置里允许 P2PCLI 使用相机，或者输入设备码连接。") : error}</p>
          {phase === "denied" && (
            <button type="button" className="m-scan-pill m-scan-pill-primary" onClick={openSettings}>{t("去设置")}</button>
          )}
        </div>
      ) : (
        <div className="m-scan-center">
          <div className="m-scan-frame" aria-hidden="true">
            <span className="m-scan-corner" data-at="tl" />
            <span className="m-scan-corner" data-at="tr" />
            <span className="m-scan-corner" data-at="bl" />
            <span className="m-scan-corner" data-at="br" />
            <span className="m-scan-line" />
            <span className="m-scan-check"><Check /></span>
          </div>
          <p className="m-scan-hint" role="status">
            {phase === "found" ? t("已识别，正在连接…")
              : phase === "wrong" ? t("这不是 P2PCLI 的连接二维码")
              : t("对准电脑上 P2PCLI 显示的二维码")}
          </p>
        </div>
      )}

      <footer className="m-scan-bottom">
        <button type="button" className="m-scan-pill" onClick={onManual}>
          <KeyRound />
          {t("输入设备码")}
        </button>
      </footer>
    </div>,
    document.body,
  );
}

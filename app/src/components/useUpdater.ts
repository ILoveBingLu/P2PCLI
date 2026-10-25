import { useCallback, useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { api, type UpdateProgress, type UpdateView } from "../api";

/** 启动后过一会儿查一次，之后每 6 小时查一次 */
const FIRST_CHECK_DELAY = 5_000;
const CHECK_EVERY = 6 * 60 * 60 * 1000;

export function useUpdater() {
  const [version, setVersion] = useState<string | null>(null);
  const [update, setUpdate] = useState<UpdateView | null>(null);
  const [checking, setChecking] = useState(false);
  const [checkedAt, setCheckedAt] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  /** 下载安装失败的原因；和后台检查失败分开，检查失败不算更新失败 */
  const [installError, setInstallError] = useState<string | null>(null);
  /** 点了“稍后”的版本，这次运行里不再提示；有更新的版本照样提示 */
  const [dismissed, setDismissed] = useState<string | null>(null);
  const busy = useRef(false);

  const check = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    setChecking(true);
    setError(null);
    try {
      setUpdate(await api.checkUpdate());
      setCheckedAt(Date.now());
    } catch (e) {
      setError(String(e));
    } finally {
      busy.current = false;
      setChecking(false);
    }
  }, []);

  const install = useCallback(async () => {
    setInstalling(true);
    setError(null);
    setInstallError(null);
    setProgress(null);
    try {
      await api.installUpdate(setProgress);
    } catch (e) {
      setError(String(e));
      setInstallError(String(e));
      setInstalling(false);
    }
  }, []);

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
    const first = window.setTimeout(() => void check(), FIRST_CHECK_DELAY);
    const timer = window.setInterval(() => void check(), CHECK_EVERY);
    return () => {
      window.clearTimeout(first);
      window.clearInterval(timer);
    };
  }, [check]);

  return {
    version,
    update,
    checking,
    checkedAt,
    error,
    installing,
    installError,
    progress,
    check,
    install,
    /** 主界面要不要提示新版本 */
    notify: !!update && (installing || dismissed !== update.version),
    dismiss: () => update && setDismissed(update.version),
  };
}

/** 下载进度的文字 */
export function progressText(p: UpdateProgress | null) {
  if (!p) return "正在准备下载…";
  if (p.event === "installing") return "下载完成，正在安装，马上重启…";
  const mb = (n: number) => (n / 1024 / 1024).toFixed(1);
  return p.total
    ? `正在下载 ${Math.floor((p.downloaded / p.total) * 100)}%（${mb(p.downloaded)} / ${mb(p.total)} MB）`
    : `正在下载 ${mb(p.downloaded)} MB`;
}

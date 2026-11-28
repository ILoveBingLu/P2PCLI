import { useEffect, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";

/** 应用的版本号（读一次就记住）；不在 Tauri 里（网页预览）时是空 */
let cached: Promise<string | null> | null = null;
function readVersion(): Promise<string | null> {
  cached ??= isTauri() ? getVersion().catch(() => null) : Promise.resolve(null);
  return cached;
}

/** 当前版本号，比如 "0.1.26"；还没读到或读不到时是空 */
export function useAppVersion(): string | null {
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    void readVersion().then((v) => alive && setVersion(v));
    return () => {
      alive = false;
    };
  }, []);
  return version;
}

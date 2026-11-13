import React, { lazy, Suspense, useEffect } from "react";
import ReactDOM from "react-dom/client";
import { useTranslation } from "react-i18next";
import App from "./App";
import "./styles.css";
import { syncOsLocale } from "./i18n";
import { startSelftest } from "./selftest";
import { isMobile } from "./lib/platform";

// 手机的样式都挂在 html[data-mobile] 下面
if (isMobile) document.documentElement.dataset.mobile = "";

// 代码编辑器的单独窗口（index.html?window=editor）只渲染编辑器，主窗口照旧
const isEditorWindow = new URLSearchParams(location.search).get("window") === "editor";
const EditorWindow = lazy(() => import("./components/editor/EditorWindow"));

function LocaleRoot({ children }: { children: React.ReactNode }) {
  const { i18n } = useTranslation();
  useEffect(() => {
    document.documentElement.lang = i18n.resolvedLanguage ?? i18n.language;
  }, [i18n, i18n.language, i18n.resolvedLanguage]);
  return children;
}

void (async () => {
  await Promise.race([
    syncOsLocale(),
    new Promise((resolve) => setTimeout(resolve, 400)),
  ]);
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <LocaleRoot>
        {isEditorWindow ? <Suspense fallback={null}><EditorWindow /></Suspense> : <App />}
      </LocaleRoot>
    </React.StrictMode>,
  );
  if (!isEditorWindow) startSelftest();
})();

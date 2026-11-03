import React, { useEffect } from "react";
import ReactDOM from "react-dom/client";
import { useTranslation } from "react-i18next";
import App from "./App";
import "./styles.css";
import { syncOsLocale } from "./i18n";
import { startSelftest } from "./selftest";

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
        <App />
      </LocaleRoot>
    </React.StrictMode>,
  );
  startSelftest();
})();

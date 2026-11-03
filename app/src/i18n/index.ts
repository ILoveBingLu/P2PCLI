import { invoke, isTauri } from "@tauri-apps/api/core";
import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import {
  readStoredLocale,
  resolveUiLocale,
  LANGUAGE_STORAGE_KEY,
  type LocaleChoice,
  type UiLocale,
} from "./locale";
import en from "./locales/en.json";
import zhCN from "./locales/zh-CN.json";

export { LANGUAGE_STORAGE_KEY, type LocaleChoice, type UiLocale };

const initialLocale = resolveUiLocale({
  stored: typeof localStorage !== "undefined" ? readStoredLocale(localStorage) : null,
  navigatorLanguage: typeof navigator !== "undefined" ? navigator.language : null,
});

// Keys are the Simplified Chinese source strings. zh-CN values repeat them;
// en values are the English UI. keySeparator is off so punctuation stays in the key.
void i18n.use(initReactI18next).init({
  resources: {
    en: { translation: en },
    "zh-CN": { translation: zhCN },
  },
  lng: initialLocale,
  fallbackLng: "en",
  keySeparator: false,
  nsSeparator: false,
  interpolation: { escapeValue: false },
  returnNull: false,
  react: { useSuspense: false },
});

if (typeof document !== "undefined") document.documentElement.lang = initialLocale;

/** Translate a UI string. Key is the Simplified Chinese source. */
export function t(key: string, options?: Record<string, unknown>): string {
  return i18n.t(key, options);
}

/**
 * Backend copy that is still Chinese. Chinese UI shows it unchanged.
 * English UI uses the dictionary when the sentence is one we translated.
 */
export function tr(text: string): string {
  if (!text || i18n.language.toLowerCase().startsWith("zh")) return text;
  return i18n.t(text, { lng: "en" });
}

export function intlLocale(): string {
  return i18n.resolvedLanguage || i18n.language || "en";
}

export function storedLocaleChoice(): LocaleChoice {
  if (typeof localStorage === "undefined") return "system";
  return readStoredLocale(localStorage) ?? "system";
}

/** Read LANG / LC_ALL / LC_MESSAGES from the desktop shell when no choice is saved. */
export async function syncOsLocale(): Promise<void> {
  if (typeof localStorage !== "undefined" && readStoredLocale(localStorage)) {
    if (typeof document !== "undefined") document.documentElement.lang = i18n.language;
    return;
  }
  let osLocale: string | null = null;
  if (isTauri()) {
    try {
      osLocale = await invoke<string | null>("system_locale");
    } catch {
      osLocale = null;
    }
  }
  const next = resolveUiLocale({
    stored: null,
    osLocale,
    navigatorLanguage: typeof navigator !== "undefined" ? navigator.language : null,
  });
  if (i18n.language !== next) await i18n.changeLanguage(next);
  if (typeof document !== "undefined") document.documentElement.lang = i18n.language;
}

export async function applyLocaleChoice(choice: LocaleChoice): Promise<void> {
  if (typeof localStorage !== "undefined") {
    if (choice === "system") localStorage.removeItem(LANGUAGE_STORAGE_KEY);
    else localStorage.setItem(LANGUAGE_STORAGE_KEY, choice);
  }
  if (choice === "system") {
    await syncOsLocale();
    return;
  }
  await i18n.changeLanguage(choice);
  if (typeof document !== "undefined") document.documentElement.lang = choice;
}

export { i18n };
export default i18n;

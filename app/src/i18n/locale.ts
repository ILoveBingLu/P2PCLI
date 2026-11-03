/** Desktop UI locales. Chinese tags stay Chinese; everything else is English. */
export const UI_LOCALES = ["en", "zh-CN"] as const;
export type UiLocale = (typeof UI_LOCALES)[number];

/** Persisted settings override. Absent means follow the system locale. */
export const LANGUAGE_STORAGE_KEY = "p2pcli.uiLanguage";

export type LocaleChoice = "system" | UiLocale;

export function normalizeLocaleTag(raw: string | null | undefined): string {
  return (raw ?? "").trim().replaceAll("_", "-");
}

/** C / POSIX / empty are not a user's language. */
export function isConcreteLanguageTag(raw: string | null | undefined): boolean {
  const tag = normalizeLocaleTag(raw);
  if (!tag) return false;
  const lower = tag.toLowerCase();
  if (lower === "c" || lower === "posix" || lower.startsWith("c.")) return false;
  return /^[a-z]{2,3}(?:-|$)/i.test(tag);
}

/** zh* → zh-CN. Any other concrete tag → en. Unusable tags → null. */
export function localeFromTag(raw: string | null | undefined): UiLocale | null {
  if (!isConcreteLanguageTag(raw)) return null;
  const tag = normalizeLocaleTag(raw).toLowerCase();
  return tag.startsWith("zh") ? "zh-CN" : "en";
}

export function readStoredLocale(storage: { getItem(key: string): string | null } | null): UiLocale | null {
  const value = storage?.getItem(LANGUAGE_STORAGE_KEY);
  return value === "en" || value === "zh-CN" ? value : null;
}

/**
 * A saved choice wins. Otherwise the OS locale (LANG / LC_* / Tauri) wins when
 * it names a language. Otherwise the webview's navigator.language. Default English.
 */
export function resolveUiLocale(input: {
  stored?: string | null;
  osLocale?: string | null;
  navigatorLanguage?: string | null;
}): UiLocale {
  if (input.stored === "en" || input.stored === "zh-CN") return input.stored;
  return localeFromTag(input.osLocale) ?? localeFromTag(input.navigatorLanguage) ?? "en";
}

import assert from "node:assert/strict";
import { test } from "node:test";
import { localeFromTag, resolveUiLocale } from "./locale.ts";

test("Chinese locales map to zh-CN", () => {
  for (const tag of ["zh", "zh-CN", "zh-TW", "zh-HK", "zh-Hans", "zh-Hant", "zh_CN", "zh_CN.UTF-8", "ZH-cn", "zh-Hans-CN"]) {
    assert.equal(localeFromTag(tag), "zh-CN", tag);
  }
});

test("non-Chinese locales map to English", () => {
  for (const tag of ["en", "en-US", "en_GB.UTF-8", "fr", "fr-FR", "ja", "ja-JP", "de", "pt-BR", "es", "ko"]) {
    assert.equal(localeFromTag(tag), "en", tag);
  }
});

test("empty and POSIX locales are not a language", () => {
  for (const tag of ["", "   ", "C", "POSIX", "C.UTF-8", "c.utf8", null, undefined]) {
    assert.equal(localeFromTag(tag), null, String(tag));
  }
});

test("saved choice overrides OS and navigator", () => {
  assert.equal(resolveUiLocale({ stored: "en", osLocale: "zh_CN.UTF-8", navigatorLanguage: "zh-CN" }), "en");
  assert.equal(resolveUiLocale({ stored: "zh-CN", osLocale: "en_US.UTF-8", navigatorLanguage: "en-US" }), "zh-CN");
});

test("OS locale wins over the webview language", () => {
  assert.equal(resolveUiLocale({ osLocale: "zh_CN.UTF-8", navigatorLanguage: "en-US" }), "zh-CN");
  assert.equal(resolveUiLocale({ osLocale: "en_US.UTF-8", navigatorLanguage: "zh-CN" }), "en");
  assert.equal(resolveUiLocale({ osLocale: "fr_FR.UTF-8", navigatorLanguage: "zh-Hans" }), "en");
});

test("falls through C/POSIX to navigator, then English", () => {
  assert.equal(resolveUiLocale({ osLocale: "C.UTF-8", navigatorLanguage: "zh-TW" }), "zh-CN");
  assert.equal(resolveUiLocale({ osLocale: "POSIX", navigatorLanguage: "de-DE" }), "en");
  assert.equal(resolveUiLocale({ osLocale: "", navigatorLanguage: "" }), "en");
  assert.equal(resolveUiLocale({}), "en");
});

// Monaco 自带的各语言高亮规则（Monarch），包里没有给它们写类型
declare module "monaco-editor/languages/definitions/*.js" {
  import type { languages } from "monaco-editor";
  export const conf: languages.LanguageConfiguration;
  export const language: languages.IMonarchLanguage;
}

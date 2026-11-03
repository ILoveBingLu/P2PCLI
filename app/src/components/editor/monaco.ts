// Monaco 只在第一次打开编辑器时才加载（十几 MB 的脚本），不拖慢软件启动。
import * as monaco from "monaco-editor";
import EditorWorker from "monaco-editor/editor/editor.worker.js?worker";
import TsWorker from "monaco-editor/languages/features/typescript/ts.worker.js?worker";
import JsonWorker from "monaco-editor/languages/features/json/json.worker.js?worker";
import CssWorker from "monaco-editor/languages/features/css/css.worker.js?worker";
import HtmlWorker from "monaco-editor/languages/features/html/html.worker.js?worker";

// 自己建 worker：Monaco 默认用 blob: 地址起 worker，会被软件的内容安全策略挡住
self.MonacoEnvironment = {
  getWorker(_id: string, label: string) {
    if (label === "typescript" || label === "javascript") return new TsWorker();
    if (label === "json") return new JsonWorker();
    if (label === "css" || label === "scss" || label === "less") return new CssWorker();
    if (label === "html" || label === "handlebars" || label === "razor") return new HtmlWorker();
    return new EditorWorker();
  },
};

// 编辑器里只有单个文件，没有整个项目和依赖：类型检查会把每个 import 都标成找不到，“已弃用”之类的提示也不准，只留语法错误
for (const defaults of [monaco.typescript.typescriptDefaults, monaco.typescript.javascriptDefaults]) {
  defaults.setDiagnosticsOptions({ noSemanticValidation: true, noSuggestionDiagnostics: true, noSyntaxValidation: false });
}

monaco.editor.defineTheme("p2pcli-dark", {
  base: "vs-dark",
  inherit: true,
  rules: [],
  colors: {
    "editor.background": "#111114",
    "editorGutter.background": "#111114",
    "minimap.background": "#111114",
    "editor.lineHighlightBackground": "#ffffff08",
    "editorLineNumber.foreground": "#5a5a66",
    "editorLineNumber.activeForeground": "#b8b8c4",
    "editorWidget.background": "#1b1b20",
    "editorWidget.border": "#2a2a31",
  },
});

/** 按文件名挑语言：先看完整文件名（Dockerfile、Makefile），再看扩展名 */
export function languageFor(name: string): string {
  const lower = name.toLowerCase();
  let best: { id: string; length: number } | null = null;
  for (const language of monaco.languages.getLanguages()) {
    if (language.filenames?.some((f) => f.toLowerCase() === lower)) return language.id;
    for (const ext of language.extensions ?? []) {
      if (lower.endsWith(ext.toLowerCase()) && (!best || ext.length > best.length)) best = { id: language.id, length: ext.length };
    }
  }
  return best?.id ?? "plaintext";
}

export { monaco };

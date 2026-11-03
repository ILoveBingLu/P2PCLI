// 代码补全、跳转、报错：对方电脑上跑语言服务（rust-analyzer、gopls、pyright、clangd……），
// 消息经 P2P 转过来，这里接到 Monaco 上。每个编辑器一个 LspManager；同一个项目的文件共用一个语言服务。
import { api, LSP_MISSING_PREFIX, type LspEventView } from "../../api";
import { monaco } from "./monaco";
import { baseName, dirName, isUnder } from "./paths";
import { JsonRpc, pathKey, pathToUri, uriToPath, type CancelToken } from "./lspClient";

/** Monaco 的语言 → 对方电脑上哪种语言服务 */
const FAMILIES: Record<string, string> = {
  rust: "rust", go: "go", python: "python", typescript: "typescript", javascript: "typescript",
  c: "c", cpp: "c", "objective-c": "c", swift: "swift", lua: "lua", ruby: "ruby", php: "php",
  shell: "shell", csharp: "csharp", java: "java", kotlin: "kotlin",
  "p2pcli-typescript": "typescript", "p2pcli-javascript": "typescript",
};

/**
 * TS/JS 交给语言服务以后换成这两种语言：高亮一样，但 Monaco 自带的单文件 TypeScript 功能不作用在它们上面，
 * 补全、悬停、报错不会出两份。Monaco 自带的那套第一次用到时就注册好了，事后关不掉，只能这样绕开。
 */
const ALIASES: Record<string, string> = { typescript: "p2pcli-typescript", javascript: "p2pcli-javascript" };
const ORIGINAL: Record<string, string> = { "p2pcli-typescript": "typescript", "p2pcli-javascript": "javascript" };
let aliasesReady: Promise<void> | null = null;
function registerAliases() {
  aliasesReady ??= (async () => {
    const [ts, js] = await Promise.all([
      import("monaco-editor/languages/definitions/typescript/typescript.js"),
      import("monaco-editor/languages/definitions/javascript/javascript.js"),
    ]);
    for (const [id, mod, alias] of [["p2pcli-typescript", ts, "TypeScript"], ["p2pcli-javascript", js, "JavaScript"]] as const) {
      monaco.languages.register({ id, aliases: [alias] });
      monaco.languages.setMonarchTokensProvider(id, mod.language);
      monaco.languages.setLanguageConfiguration(id, mod.conf);
    }
  })();
  return aliasesReady;
}

export function familyOf(languageId: string): string | null {
  return FAMILIES[languageId] ?? null;
}

/** 语言服务认的语言名：.tsx/.jsx 要单独说，不然 JSX 会报错 */
function lspLanguageId(languageId: string, path: string): string {
  languageId = ORIGINAL[languageId] ?? languageId;
  const lower = path.toLowerCase();
  if (lower.endsWith(".tsx")) return "typescriptreact";
  if (lower.endsWith(".jsx")) return "javascriptreact";
  if (languageId === "shell") return "shellscript";
  return languageId;
}

export type LspState = "starting" | "ready" | "busy" | "error" | "missing";

export interface LspStatus {
  family: string;
  state: LspState;
  /** 语言服务的程序名 */
  server?: string;
  root?: string;
  /** 正在做什么（索引、检查），或出错的原因 */
  detail?: string;
}

/** 编辑器提供给语言服务用的几件事 */
export interface LspHost {
  connId(): number | null;
  /** 编辑器打开的目录 */
  root(): string | null;
  /** 跳转、查引用时要用到其他文件：给出已打开的模型，或者从对方电脑上读出来建一个不显示的模型 */
  loadModel(path: string): Promise<monaco.editor.ITextModel | null>;
  /** 这个模型对应对方电脑上的哪个文件 */
  pathOf(uri: monaco.Uri): string | null;
  /** 打开文件并选中这段（跳转到别的文件时） */
  open(path: string, range: monaco.IRange): void;
  /** 状态变了，界面要刷新 */
  changed(): void;
}

interface Doc {
  model: monaco.editor.ITextModel;
  path: string;
  uri: string;
  session: Session;
  sub: monaco.IDisposable;
}

interface Session {
  family: string;
  root: string;
  server?: string;
  lspId?: number;
  rpc?: JsonRpc;
  caps: any;
  state: LspState;
  detail?: string;
  progress: Map<string | number, string>;
  docs: Set<Doc>;
  stopping: boolean;
  ready: Promise<boolean>;
}

const docs = new Map<string, Doc>();
const managers = new Set<LspManager>();

function docFor(model: monaco.editor.ITextModel): Doc | null {
  const doc = docs.get(model.uri.toString());
  return doc && doc.session.rpc && !doc.session.rpc.closed && doc.session.caps ? doc : null;
}

const ident = (doc: Doc) => ({ uri: doc.uri });
const position = (p: monaco.IPosition) => ({ line: p.lineNumber - 1, character: p.column - 1 });
const toRange = (r: any) => new monaco.Range(r.start.line + 1, r.start.character + 1, r.end.line + 1, r.end.character + 1);
const toLspRange = (r: monaco.IRange) => ({ start: { line: r.startLineNumber - 1, character: r.startColumn - 1 }, end: { line: r.endLineNumber - 1, character: r.endColumn - 1 } });
const toEdit = (e: any): monaco.languages.TextEdit => ({ range: toRange(e.range), text: e.newText });

function escapeMarkdown(text: string) {
  return text.replace(/[\\`*_{}[\]()#+\-.!|<>]/g, "\\$&");
}

/** 悬停、补全说明里的文字 → Monaco 的 Markdown */
function markdown(content: any): monaco.IMarkdownString[] {
  if (content == null) return [];
  if (Array.isArray(content)) return content.flatMap(markdown);
  if (typeof content === "string") return content ? [{ value: content }] : [];
  if ("kind" in content) {
    if (!content.value) return [];
    return [{ value: content.kind === "markdown" ? content.value : escapeMarkdown(content.value) }];
  }
  if ("language" in content) return content.value ? [{ value: "```" + content.language + "\n" + content.value + "\n```" }] : [];
  return [];
}

function documentation(content: any): string | monaco.IMarkdownString | undefined {
  if (content == null) return undefined;
  if (typeof content === "string") return content || undefined;
  return markdown(content)[0];
}

const KINDS = [
  "Text", "Method", "Function", "Constructor", "Field", "Variable", "Class", "Interface", "Module", "Property", "Unit", "Value", "Enum",
  "Keyword", "Snippet", "Color", "File", "Reference", "Folder", "EnumMember", "Constant", "Struct", "Event", "Operator", "TypeParameter",
] as const;
const completionKind = (kind?: number) =>
  monaco.languages.CompletionItemKind[KINDS[(kind ?? 1) - 1] ?? "Text"] ?? monaco.languages.CompletionItemKind.Text;

/** Monaco 自己认识的命令：补全以后弹出参数提示、再弹补全 */
const KNOWN_COMMANDS = new Set(["editor.action.triggerParameterHints", "editor.action.triggerSuggest"]);

type Suggestion = monaco.languages.CompletionItem & { lsp?: any; doc?: Doc };

function suggestion(item: any, doc: Doc, wordRange: monaco.IRange, defaults: any): Suggestion {
  const edit = item.textEdit ?? (defaults?.editRange ? { ...defaults.editRange, newText: item.textEditText ?? item.insertText ?? item.label } : null);
  let range: monaco.IRange | monaco.languages.CompletionItemRanges = wordRange;
  if (edit?.range) range = toRange(edit.range);
  else if (edit?.insert) range = { insert: toRange(edit.insert), replace: toRange(edit.replace) };
  const snippet = (item.insertTextFormat ?? defaults?.insertTextFormat) === 2;
  return {
    label: item.labelDetails ? { label: item.label, detail: item.labelDetails.detail, description: item.labelDetails.description } : item.label,
    kind: completionKind(item.kind),
    tags: item.deprecated || item.tags?.includes(1) ? [monaco.languages.CompletionItemTag.Deprecated] : undefined,
    detail: item.detail,
    documentation: documentation(item.documentation),
    sortText: item.sortText,
    filterText: item.filterText,
    preselect: item.preselect,
    insertText: edit?.newText ?? item.insertText ?? item.label,
    insertTextRules: snippet ? monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet : undefined,
    range,
    commitCharacters: item.commitCharacters ?? defaults?.commitCharacters,
    additionalTextEdits: item.additionalTextEdits?.map(toEdit),
    command: item.command && KNOWN_COMMANDS.has(item.command.command) ? { id: item.command.command, title: item.command.title ?? "" } : undefined,
    lsp: item,
    doc,
  };
}

/** 跳转结果里的位置：别的文件先读出来建模型，Monaco 才能预览、跳过去 */
async function locations(doc: Doc, result: any): Promise<monaco.languages.Location[]> {
  const list: any[] = !result ? [] : Array.isArray(result) ? result : [result];
  const spots = list.map((l) => "targetUri" in l ? { uri: l.targetUri, range: l.targetSelectionRange ?? l.targetRange } : l);
  const manager = [...managers].find((m) => m.owns(doc));
  if (!manager) return [];
  const paths = new Map<string, string>();
  for (const spot of spots) {
    const path = uriToPath(spot.uri);
    // 一次最多读 30 个文件，引用太多时后面的不显示
    if (path && !paths.has(pathKey(path)) && paths.size < 30) paths.set(pathKey(path), path);
  }
  const models = new Map<string, monaco.editor.ITextModel | null>();
  await Promise.all([...paths].map(async ([key, path]) => {
    models.set(key, pathKey(doc.path) === key ? doc.model : await manager.host.loadModel(path).catch(() => null));
  }));
  return spots.flatMap((spot) => {
    const path = uriToPath(spot.uri);
    const model = path ? models.get(pathKey(path)) : null;
    return model ? [{ uri: model.uri, range: toRange(spot.range) }] : [];
  });
}

const registered = new Set<string>();

/** 每种 Monaco 语言注册一次；具体用哪个语言服务，按模型找 */
function registerProviders(family: string) {
  const languages = Object.keys(FAMILIES).filter((id) => FAMILIES[id] === family && !registered.has(id));
  for (const language of languages) {
    registered.add(language);
    monaco.languages.registerCompletionItemProvider(language, {
      triggerCharacters: [".", ":", ">", "<", "\"", "'", "/", "@", "#", "$", "(", ",", "-", "*", "\\"],
      async provideCompletionItems(model, pos, context, token) {
        const doc = docFor(model);
        const cap = doc?.session.caps.completionProvider;
        if (!doc || !cap) return undefined;
        const trigger = context.triggerCharacter;
        // 这个字符不是这个语言服务的触发字符：不打扰
        if (context.triggerKind === monaco.languages.CompletionTriggerKind.TriggerCharacter && !(cap.triggerCharacters ?? []).includes(trigger)) {
          return { suggestions: [] };
        }
        const result: any = await doc.session.rpc!.request("textDocument/completion", {
          textDocument: ident(doc), position: position(pos),
          context: { triggerKind: context.triggerKind + 1, triggerCharacter: trigger },
        }, token).catch(() => null);
        if (!result) return { suggestions: [] };
        const word = model.getWordUntilPosition(pos);
        const wordRange = new monaco.Range(pos.lineNumber, word.startColumn, pos.lineNumber, word.endColumn);
        const items: any[] = Array.isArray(result) ? result : result.items ?? [];
        return {
          suggestions: items.map((item) => suggestion(item, doc, wordRange, Array.isArray(result) ? null : result.itemDefaults)),
          incomplete: !Array.isArray(result) && !!result.isIncomplete,
        };
      },
      async resolveCompletionItem(item: Suggestion, token) {
        const doc = item.doc;
        if (!doc?.session.rpc || doc.session.rpc.closed || !doc.session.caps.completionProvider?.resolveProvider || !item.lsp) return item;
        const resolved: any = await doc.session.rpc.request("completionItem/resolve", item.lsp, token).catch(() => null);
        if (!resolved) return item;
        return {
          ...item,
          detail: resolved.detail ?? item.detail,
          documentation: documentation(resolved.documentation) ?? item.documentation,
          additionalTextEdits: resolved.additionalTextEdits?.map(toEdit) ?? item.additionalTextEdits,
        };
      },
    });
    monaco.languages.registerHoverProvider(language, {
      async provideHover(model, pos, token) {
        const doc = docFor(model);
        if (!doc?.session.caps.hoverProvider) return undefined;
        const result: any = await doc.session.rpc!.request("textDocument/hover", { textDocument: ident(doc), position: position(pos) }, token).catch(() => null);
        const contents = markdown(result?.contents);
        if (!contents.length) return undefined;
        return { contents, range: result.range ? toRange(result.range) : undefined };
      },
    });
    monaco.languages.registerSignatureHelpProvider(language, {
      signatureHelpTriggerCharacters: ["(", ",", "<"],
      signatureHelpRetriggerCharacters: [")"],
      async provideSignatureHelp(model, pos, token, context) {
        const doc = docFor(model);
        const cap = doc?.session.caps.signatureHelpProvider;
        if (!doc || !cap) return undefined;
        if (context.triggerKind === monaco.languages.SignatureHelpTriggerKind.TriggerCharacter
          && ![...(cap.triggerCharacters ?? []), ...(cap.retriggerCharacters ?? [])].includes(context.triggerCharacter)) return undefined;
        const result: any = await doc.session.rpc!.request("textDocument/signatureHelp", {
          textDocument: ident(doc), position: position(pos),
          context: { triggerKind: context.triggerKind, triggerCharacter: context.triggerCharacter, isRetrigger: context.isRetrigger },
        }, token).catch(() => null);
        if (!result?.signatures?.length) return undefined;
        return {
          value: {
            signatures: result.signatures.map((s: any) => ({
              label: s.label,
              documentation: documentation(s.documentation),
              parameters: (s.parameters ?? []).map((p: any) => ({ label: p.label, documentation: documentation(p.documentation) })),
              activeParameter: s.activeParameter,
            })),
            activeSignature: result.activeSignature ?? 0,
            activeParameter: result.activeParameter ?? 0,
          },
          dispose() {},
        };
      },
    });
    const jump = (method: string, cap: string) => ({
      async provide(model: monaco.editor.ITextModel, pos: monaco.Position, token: CancelToken) {
        const doc = docFor(model);
        if (!doc?.session.caps[cap]) return undefined;
        const result = await doc.session.rpc!.request(method, { textDocument: ident(doc), position: position(pos) }, token).catch(() => null);
        return locations(doc, result);
      },
    });
    const definition = jump("textDocument/definition", "definitionProvider");
    const typeDefinition = jump("textDocument/typeDefinition", "typeDefinitionProvider");
    const implementation = jump("textDocument/implementation", "implementationProvider");
    monaco.languages.registerDefinitionProvider(language, { provideDefinition: (m, p, t) => definition.provide(m, p, t) });
    monaco.languages.registerTypeDefinitionProvider(language, { provideTypeDefinition: (m, p, t) => typeDefinition.provide(m, p, t) });
    monaco.languages.registerImplementationProvider(language, { provideImplementation: (m, p, t) => implementation.provide(m, p, t) });
    monaco.languages.registerReferenceProvider(language, {
      async provideReferences(model, pos, context, token) {
        const doc = docFor(model);
        if (!doc?.session.caps.referencesProvider) return undefined;
        const result = await doc.session.rpc!.request("textDocument/references", {
          textDocument: ident(doc), position: position(pos), context: { includeDeclaration: context.includeDeclaration },
        }, token).catch(() => null);
        return locations(doc, result);
      },
    });
    monaco.languages.registerDocumentHighlightProvider(language, {
      async provideDocumentHighlights(model, pos, token) {
        const doc = docFor(model);
        if (!doc?.session.caps.documentHighlightProvider) return undefined;
        const result: any[] | null = await doc.session.rpc!.request("textDocument/documentHighlight", { textDocument: ident(doc), position: position(pos) }, token).catch(() => null);
        return result?.map((h) => ({ range: toRange(h.range), kind: (h.kind ?? 1) - 1 }));
      },
    });
    monaco.languages.registerDocumentFormattingEditProvider(language, {
      async provideDocumentFormattingEdits(model, options, token) {
        const doc = docFor(model);
        if (!doc?.session.caps.documentFormattingProvider) return undefined;
        const result: any[] | null = await doc.session.rpc!.request("textDocument/formatting", {
          textDocument: ident(doc), options: { tabSize: options.tabSize, insertSpaces: options.insertSpaces },
        }, token).catch(() => null);
        return result?.map(toEdit);
      },
    });
    monaco.languages.registerDocumentRangeFormattingEditProvider(language, {
      async provideDocumentRangeFormattingEdits(model, range, options, token) {
        const doc = docFor(model);
        if (!doc?.session.caps.documentRangeFormattingProvider) return undefined;
        const result: any[] | null = await doc.session.rpc!.request("textDocument/rangeFormatting", {
          textDocument: ident(doc), range: toLspRange(range), options: { tabSize: options.tabSize, insertSpaces: options.insertSpaces },
        }, token).catch(() => null);
        return result?.map(toEdit);
      },
    });
  }
}

// 跳到别的文件：Monaco 自己不会开新文件，交给编辑器打开标签
let openerRegistered = false;
function registerOpener() {
  if (openerRegistered) return;
  openerRegistered = true;
  monaco.editor.registerEditorOpener({
    openCodeEditor(source, resource, selection) {
      if (source.getModel()?.uri.toString() === resource.toString()) return false;
      for (const manager of managers) {
        const path = manager.host.pathOf(resource);
        if (!path) continue;
        const range = !selection ? new monaco.Range(1, 1, 1, 1)
          : "startLineNumber" in selection ? selection
          : new monaco.Range(selection.lineNumber, selection.column, selection.lineNumber, selection.column);
        manager.host.open(path, range);
        return true;
      }
      return false;
    },
  });
}

const SEVERITY = [monaco.MarkerSeverity.Error, monaco.MarkerSeverity.Warning, monaco.MarkerSeverity.Info, monaco.MarkerSeverity.Hint];

function markers(diagnostics: any[]): monaco.editor.IMarkerData[] {
  return diagnostics.map((d) => ({
    severity: SEVERITY[(d.severity ?? 1) - 1] ?? monaco.MarkerSeverity.Error,
    message: d.message,
    source: d.source,
    code: d.codeDescription?.href ? { value: String(d.code ?? ""), target: monaco.Uri.parse(d.codeDescription.href) } : d.code != null ? String(d.code) : undefined,
    startLineNumber: d.range.start.line + 1,
    startColumn: d.range.start.character + 1,
    endLineNumber: d.range.end.line + 1,
    endColumn: d.range.end.character + 1,
    tags: d.tags,
  }));
}

/** 一个编辑器的语言服务们 */
export class LspManager {
  private sessions: Session[] = [];
  /** 找不到语言服务的语言：记下时间，过一会儿再试 */
  private missing = new Map<string, number>();
  /** 编辑器里开着的文件，断线重连后要重新交给语言服务 */
  private attached = new Map<monaco.editor.ITextModel, string>();
  private disposed = false;
  private changeTimer = 0;

  constructor(readonly host: LspHost) {
    managers.add(this);
    registerOpener();
  }

  owns(doc: Doc) { return this.sessions.includes(doc.session); }

  /** 编辑器打开了一个文件 */
  attach(path: string, model: monaco.editor.ITextModel) {
    this.attached.set(model, path);
    void this.bind(path, model);
  }

  /** 关掉文件，或者改名前 */
  detach(model: monaco.editor.ITextModel) {
    this.attached.delete(model);
    const doc = docs.get(model.uri.toString());
    if (!doc || !this.owns(doc)) return;
    this.closeDoc(doc, true);
  }

  /** 保存成功：有的语言服务（rust-analyzer）保存时才跑完整检查 */
  saved(model: monaco.editor.ITextModel) {
    const doc = docFor(model);
    const save = doc?.session.caps.textDocumentSync?.save;
    if (!doc || !save) return;
    doc.session.rpc!.notify("textDocument/didSave", { textDocument: ident(doc), text: save.includeText ? model.getValue() : undefined });
  }

  /** 状态栏显示用 */
  status(path: string, languageId: string): LspStatus | null {
    const family = familyOf(languageId);
    if (!family) return null;
    const session = this.sessions.find((s) => s.family === family && [...s.docs].some((d) => d.path === path))
      ?? this.sessions.find((s) => s.family === family && (s.state === "starting" || isUnder(path, s.root)));
    if (session) return { family, state: session.state, server: session.server, root: session.root, detail: session.detail };
    if (this.missing.has(family)) return { family, state: "missing" };
    return null;
  }

  /** 出错或没找到以后，用户点重试 */
  retry(family: string) {
    this.missing.delete(family);
    for (const session of this.sessions.filter((s) => s.family === family && s.state === "error")) this.drop(session);
    for (const [model, path] of this.attached) if (familyOf(model.getLanguageId()) === family) void this.bind(path, model);
    this.changed();
  }

  /** 连接换了（断开或重连）：旧的语言服务都不能用了，有连接时重新开 */
  reset() {
    for (const session of [...this.sessions]) this.stop(session);
    this.missing.clear();
    if (this.host.connId() != null) for (const [model, path] of this.attached) void this.bind(path, model);
    this.changed();
  }

  dispose() {
    this.disposed = true;
    for (const session of [...this.sessions]) this.stop(session);
    this.attached.clear();
    managers.delete(this);
    window.clearTimeout(this.changeTimer);
  }

  private changed() {
    if (this.changeTimer || this.disposed) return;
    // 索引进度一秒能来几十条，攒一下再刷新界面
    this.changeTimer = window.setTimeout(() => { this.changeTimer = 0; if (!this.disposed) this.host.changed(); }, 150);
  }

  private async bind(path: string, model: monaco.editor.ITextModel) {
    const family = familyOf(model.getLanguageId());
    if (!family || model.isDisposed()) return;
    const session = await this.sessionFor(family, path);
    // 等的时候文件关了、改名了、连接断了
    if (!session || this.attached.get(model) !== path || model.isDisposed() || !this.sessions.includes(session)) return;
    if (docs.has(model.uri.toString())) return;
    this.openDoc(session, path, model);
  }

  private async sessionFor(family: string, path: string): Promise<Session | null> {
    for (;;) {
      const found = this.sessions.find((s) => s.family === family && (s.state === "starting" || isUnder(path, s.root)));
      if (!found) break;
      if (found.state === "starting") {
        await found.ready;
        // 刚启动好的那个管不到这个文件（别的项目）：再找
        if (found.state !== "starting" && !isUnder(path, found.root)) {
          if (!this.sessions.some((s) => s !== found && s.family === family && (s.state === "starting" || isUnder(path, s.root)))) break;
          continue;
        }
      }
      return found.state === "error" || !this.sessions.includes(found) ? null : found;
    }
    const missingAt = this.missing.get(family);
    if (missingAt && Date.now() - missingAt < 5 * 60_000) return null;
    return this.start(family, path);
  }

  private start(family: string, path: string): Promise<Session | null> {
    const conn = this.host.connId();
    if (conn == null) return Promise.resolve(null);
    let finish!: (ok: boolean) => void;
    const session: Session = {
      family, root: dirName(path), caps: null, state: "starting", progress: new Map(), docs: new Set(), stopping: false,
      ready: new Promise<boolean>((resolve) => { finish = resolve; }),
    };
    this.sessions.push(session);
    this.changed();
    const early: string[] = [];
    const onEvent = (event: LspEventView) => {
      if (event.kind === "message") {
        if (session.rpc) session.rpc.receive(event.message);
        else early.push(event.message);
      } else {
        this.closed(session, event.reason);
      }
    };
    return (async () => {
      try {
        const opened = await api.lspStart(conn, family, path, this.host.root() ?? dirName(path), onEvent);
        session.lspId = opened.lsp_id;
        session.server = opened.server;
        session.root = opened.root;
        if (this.disposed || !this.sessions.includes(session)) {
          void api.lspStop(opened.lsp_id).catch(() => {});
          return null;
        }
        const rpc = new JsonRpc((messages) => api.lspSend(opened.lsp_id, messages));
        session.rpc = rpc;
        this.handlers(session, rpc);
        for (const message of early.splice(0)) rpc.receive(message);
        const result: any = await rpc.request("initialize", this.initializeParams(session.root));
        rpc.notify("initialized", {});
        session.caps = result?.capabilities ?? {};
        if (session.state === "starting") session.state = "ready";
        if (family === "typescript") await registerAliases();
        registerProviders(family);
        return session;
      } catch (e) {
        const message = String(e).replace(/^Error: /, "");
        if (message.includes(LSP_MISSING_PREFIX)) {
          this.missing.set(family, Date.now());
          this.drop(session);
        } else if (this.sessions.includes(session)) {
          session.state = "error";
          session.detail = message;
          if (session.lspId != null) void api.lspStop(session.lspId).catch(() => {});
        }
        return null;
      } finally {
        finish(session.state !== "error");
        this.changed();
      }
    })();
  }

  private initializeParams(root: string) {
    const uri = pathToUri(root);
    return {
      processId: null,
      clientInfo: { name: "P2PCLI" },
      locale: navigator.language,
      rootPath: root,
      rootUri: uri,
      workspaceFolders: [{ uri, name: baseName(root) }],
      capabilities: {
        general: { positionEncodings: ["utf-16"] },
        window: { workDoneProgress: true, showMessage: {} },
        workspace: { configuration: true, workspaceFolders: true, applyEdit: false },
        textDocument: {
          synchronization: { didSave: true, dynamicRegistration: false },
          completion: {
            contextSupport: true,
            completionItem: {
              snippetSupport: true, commitCharactersSupport: true, documentationFormat: ["markdown", "plaintext"],
              deprecatedSupport: true, preselectSupport: true, insertReplaceSupport: true, labelDetailsSupport: true,
              tagSupport: { valueSet: [1] }, resolveSupport: { properties: ["documentation", "detail", "additionalTextEdits"] },
            },
            completionItemKind: { valueSet: KINDS.map((_, i) => i + 1) },
            completionList: { itemDefaults: ["commitCharacters", "editRange", "insertTextFormat"] },
          },
          hover: { contentFormat: ["markdown", "plaintext"] },
          signatureHelp: {
            contextSupport: true,
            signatureInformation: { documentationFormat: ["markdown", "plaintext"], parameterInformation: { labelOffsetSupport: true }, activeParameterSupport: true },
          },
          definition: { linkSupport: true },
          typeDefinition: { linkSupport: true },
          implementation: { linkSupport: true },
          references: {},
          documentHighlight: {},
          formatting: {},
          rangeFormatting: {},
          publishDiagnostics: { tagSupport: { valueSet: [1, 2] }, codeDescriptionSupport: true },
        },
        // rust-analyzer 用它告诉我们项目加载好没有、有没有出错
        experimental: { serverStatusNotification: true },
      },
    };
  }

  private handlers(session: Session, rpc: JsonRpc) {
    const folders = () => [{ uri: pathToUri(session.root), name: baseName(session.root) }];
    rpc.onRequest("workspace/configuration", (params) => (params?.items ?? []).map(() => null));
    rpc.onRequest("workspace/workspaceFolders", folders);
    for (const method of ["client/registerCapability", "client/unregisterCapability", "window/workDoneProgress/create", "window/showMessageRequest",
      "workspace/semanticTokens/refresh", "workspace/inlayHint/refresh", "workspace/codeLens/refresh", "workspace/diagnostic/refresh"]) {
      rpc.onRequest(method, () => null);
    }
    rpc.onRequest("workspace/applyEdit", () => ({ applied: false }));
    rpc.onNotification("textDocument/publishDiagnostics", (params) => {
      const path = uriToPath(params?.uri ?? "");
      if (!path) return;
      const key = pathKey(path);
      for (const doc of session.docs) {
        if (pathKey(doc.path) === key && !doc.model.isDisposed()) monaco.editor.setModelMarkers(doc.model, "lsp", markers(params.diagnostics ?? []));
      }
    });
    rpc.onNotification("$/progress", (params) => {
      const value = params?.value;
      if (!value?.kind) return;
      if (value.kind === "end") session.progress.delete(params.token);
      else {
        const old = session.progress.get(params.token);
        const title = value.kind === "begin" ? value.title : old?.split(" · ")[0] ?? "";
        const text = [title, value.message].filter(Boolean).join(" · ") + (value.percentage != null ? ` ${Math.round(value.percentage)}%` : "");
        session.progress.set(params.token, text);
      }
      this.refreshState(session);
    });
    rpc.onNotification("experimental/serverStatus", (params) => {
      (session as any).health = params;
      this.refreshState(session);
    });
    rpc.onNotification("window/showMessage", (params) => {
      if (params?.type === 1) {
        (session as any).lastError = params.message;
        this.refreshState(session);
      }
    });
  }

  private refreshState(session: Session) {
    if (session.state === "error" || session.state === "starting") return;
    const health = (session as any).health;
    const progress = [...session.progress.values()].pop();
    const busy = !!progress || health?.quiescent === false;
    session.state = busy ? "busy" : "ready";
    session.detail = progress ?? (health?.health && health.health !== "ok" ? health.message : undefined) ?? (session as any).lastError;
    this.changed();
  }

  private openDoc(session: Session, path: string, model: monaco.editor.ITextModel) {
    const rpc = session.rpc!;
    const doc: Doc = { model, path, uri: pathToUri(path), session, sub: { dispose() {} } };
    docs.set(model.uri.toString(), doc);
    session.docs.add(doc);
    const alias = ALIASES[model.getLanguageId()];
    if (alias) monaco.editor.setModelLanguage(model, alias);
    rpc.notify("textDocument/didOpen", {
      textDocument: { uri: doc.uri, languageId: lspLanguageId(model.getLanguageId(), path), version: model.getVersionId(), text: model.getValue() },
    });
    doc.sub = model.onDidChangeContent((e) => {
      const sync = session.caps?.textDocumentSync;
      const kind = typeof sync === "number" ? sync : sync?.change ?? 0;
      if (!kind || rpc.closed) return;
      // 增量同步：改动按 Monaco 给的顺序一条条应用；换行符变了这类没有具体改动的，整份发过去
      const contentChanges = kind === 2 && e.changes.length && !e.isFlush
        ? e.changes.map((c) => ({ range: toLspRange(c.range), rangeLength: c.rangeLength, text: c.text }))
        : [{ text: model.getValue() }];
      rpc.notify("textDocument/didChange", { textDocument: { uri: doc.uri, version: model.getVersionId() }, contentChanges });
    });
  }

  private closeDoc(doc: Doc, notify: boolean) {
    doc.sub.dispose();
    docs.delete(doc.model.uri.toString());
    doc.session.docs.delete(doc);
    if (!doc.model.isDisposed()) {
      monaco.editor.setModelMarkers(doc.model, "lsp", []);
      // 没有语言服务了：换回原来的语言，Monaco 自带的 TypeScript 功能接着用
      const original = ORIGINAL[doc.model.getLanguageId()];
      if (original) monaco.editor.setModelLanguage(doc.model, original);
    }
    if (notify && doc.session.rpc && !doc.session.rpc.closed) doc.session.rpc.notify("textDocument/didClose", { textDocument: ident(doc) });
  }

  /** 语言服务自己退出或断线 */
  private closed(session: Session, reason: string | null) {
    session.rpc?.close(reason ?? "语言服务已经结束");
    for (const doc of [...session.docs]) this.closeDoc(doc, false);
    if (session.stopping || !this.sessions.includes(session)) return;
    session.state = "error";
    session.detail = reason ?? undefined;
    this.changed();
  }

  private drop(session: Session) {
    this.sessions = this.sessions.filter((s) => s !== session);
  }

  private stop(session: Session) {
    this.drop(session);
    session.stopping = true;
    for (const doc of [...session.docs]) this.closeDoc(doc, false);
    const rpc = session.rpc;
    const id = session.lspId;
    if (id == null) return;
    // 先请它自己退出，一秒半内没退就直接断开（对方会结束它）
    const done = () => void api.lspStop(id).catch(() => {});
    if (!rpc || rpc.closed) return done();
    const timer = window.setTimeout(done, 1500);
    rpc.request("shutdown").then(() => rpc.notify("exit"), () => {}).finally(() => { window.clearTimeout(timer); window.setTimeout(done, 300); });
  }
}

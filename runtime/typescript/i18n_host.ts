interface LocaleScope {
  locale: string;
}

interface ScopeStore {
  run<T>(scope: LocaleScope, fn: () => T): T;
  getStore(): LocaleScope | undefined;
}

// Not imported from node:async_hooks / node:fs: this file also ships in the browser builds, which have neither.
const node = (globalThis as { process?: { getBuiltinModule?: (id: string) => any; env?: Record<string, string | undefined>; cwd?: () => string } }).process;
const hooks = node?.getBuiltinModule?.("node:async_hooks");
// Not one module variable on Node: node:http interleaves requests across awaits, so each request holds its own locale.
const store: ScopeStore | null = hooks ? new hooks.AsyncLocalStorage() : null;
let shared = "";
let table: string | null = null;

export class I18nHost {
  static per_request<T>(fn: () => T): T {
    if (store) return store.run({ locale: "" }, fn);
    shared = "";
    return fn();
  }

  static locale_name(): string {
    const scope = store?.getStore();
    return scope ? scope.locale : shared;
  }

  static set_locale_name(name: string): string {
    const scope = store?.getStore();
    if (scope) scope.locale = name;
    else shared = name;
    return name;
  }

  static env_name(): string {
    return node?.env?.RAILS_ENV ?? node?.env?.RACK_ENV ?? "development";
  }

  static table_text(): string {
    if (table === null) {
      const fs = node?.getBuiltinModule?.("node:fs");
      const path = node?.getBuiltinModule?.("node:path");
      if (!fs || !path || !node?.cwd) throw new Error("config/i18n_translations: no file system to read it from");
      table = fs.readFileSync(path.join(node.cwd(), "config", "i18n_translations"), "utf8") as string;
    }
    return table;
  }
}

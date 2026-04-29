// Roundhouse TypeScript view-helpers runtime.
//
// Hand-written, shipped alongside generated code (copied in by the
// TS emitter as `src/view_helpers.ts`). Provides the Rails-
// compatible view helpers emitted view fns call into: `linkTo`,
// `buttonTo`, `formWrap`, FormBuilder methods, `turboStreamFrom`,
// `domId`, `pluralize`, plus a `RenderCtx` for layout slots
// (notice / alert / title).
//
// Mirrors `runtime/rust/view_helpers.rs` in intent + method
// signatures. Implementations are deliberately minimal — enough
// HTML for the scaffold blog's tests to pass, without pretending
// to be a full Rails port. A later phase can swap faithful output
// (full escape, ARIA, complete option support) behind the same
// signatures; emitted views stay the same.

/** Layout-slot context threaded through views. Populated by
 *  `content_for`; layouts would consult but Phase 4d doesn't wire
 *  a layout dispatcher. */
export class RenderCtx {
  notice?: string;
  alert?: string;
  title?: string;
}

// ── Per-request render state ────────────────────────────────────
//
// Rails' `yield` / `content_for` / `yield :slot` idiom assumes a
// shared render context between the inner view (which sets slots
// + returns a body) and the outer layout (which reads them). Our
// runtime threads this via a module-level slot map, reset per
// request by the server before dispatching. Node is single-
// threaded per-event-loop so there's no need for AsyncLocalStorage
// — the `handleRequest` function runs start-to-finish without
// interleaving.

const slots: Map<string, string> = new Map();
let yieldBody: string = "";

/** Called by the server at the start of each request to wipe any
 *  stale slot values left over from a prior render. */
export function resetRenderState(): void {
  slots.clear();
  yieldBody = "";
}

/** Set the main (unnamed) yield body — the controller's view
 *  output that the layout's `<%= yield %>` will emit. */
export function setYield(body: string): void {
  yieldBody = body;
}

/** Read the main yield body. Called by the layout's `<%= yield %>`. */
export function getYield(): string {
  return yieldBody;
}

/** Read a named slot. Called by the layout's `<%= yield :slot %>`
 *  and by `<%= content_for(:slot) %>` (getter form). Returns the
 *  empty string when unset so string concat in the emit doesn't
 *  produce "undefined". */
export function getSlot(name: string): string {
  return slots.get(name) ?? "";
}

/** `content_for(:slot, "body")` — setter form. The 2-arg variant
 *  stashes into a named slot; layouts read via `getSlot` (or
 *  emitted `<%= yield :slot %>`). Returns empty string so the
 *  surrounding concat doesn't inject the stashed value twice. */
export function contentFor(slot: string, body?: string): string {
  if (body !== undefined) {
    // Rails semantics: content_for appends on repeated calls. We
    // mirror that so a view can build up a slot across multiple
    // fragments.
    const prior = slots.get(slot) ?? "";
    slots.set(slot, prior + body);
    return "";
  }
  return slots.get(slot) ?? "";
}

// ── Rails layout helpers ────────────────────────────────────────
//
// Produce HTML shaped like Rails' output of the same-named
// helpers so the compare tool's structural diff passes when the
// normalizer strips per-request / per-build values. Fingerprint
// masking happens in the compare tool's default config; here we
// emit the canonical structural shape.

/** `<%= csrf_meta_tags %>` — emits csrf-param + csrf-token meta
 *  tags. The token value is empty; the compare tool's default
 *  config drops the whole `<meta name="csrf-token">` element for
 *  equivalence purposes. */
export function csrfMetaTags(): string {
  return `<meta name="csrf-param" content="authenticity_token" />\n<meta name="csrf-token" content="" />`;
}

/** `<%= csp_meta_tag %>` — CSP nonce meta tag. Rails only emits
 *  this when a Content-Security-Policy is configured; a fresh
 *  scaffold has none, so the helper returns empty. Matching that
 *  default keeps the rendered head structurally identical. */
export function cspMetaTag(): string {
  return "";
}

/** `<%= stylesheet_link_tag name, opts %>` — `<link rel="stylesheet">`.
 *  Emits the canonical `/assets/<name>.css` path; Rails' real
 *  output appends a fingerprint digest that compare-tool config
 *  strips. Option keys pass through as HTML attributes. */
export function stylesheetLinkTag(name: string, opts: Record<string, string> = {}): string {
  let attrs = "";
  for (const [k, v] of Object.entries(opts)) {
    attrs += ` ${k}="${escapeHtml(v)}"`;
  }
  return `<link rel="stylesheet" href="/assets/${escapeHtml(name)}.css"${attrs} />`;
}

/** `<%= javascript_importmap_tags %>` — the full importmap + its
 *  modulepreload hints + the bootstrap `<script type="module">`.
 *  Per-app pin list is emitted into `src/importmap.ts` by the
 *  ingester (parsing `config/importmap.rb` + walking
 *  `app/javascript/controllers/` for `pin_all_from`); this helper
 *  just shapes it into the Rails-compatible HTML. `main_entry` is
 *  the module imported by the bootstrap script (usually
 *  `application`; overridable per the importmap config). */
export function javascriptImportmapTags(
  pins: ReadonlyArray<readonly [string, string]>,
  main_entry: string = "application",
): string {
  const imports: Record<string, string> = {};
  for (const [name, path] of pins) {
    imports[name] = path;
  }
  const mapJson = JSON.stringify({ imports }, null, 2);
  let out = `<script type="importmap" data-turbo-track="reload">${mapJson}</script>`;
  for (const href of Object.values(imports)) {
    out += `\n<link rel="modulepreload" href="${href}">`;
  }
  out += `\n<script type="module">import "${escapeHtml(main_entry)}"</script>`;
  return out;
}

/** `<a href="url" ...attrs>text</a>`. `opts` is an attribute map. */
export function linkTo(text: string, url: string, opts: Record<string, string> = {}): string {
  let attrs = "";
  for (const [k, v] of Object.entries(opts)) {
    attrs += ` ${k}="${escapeHtml(v)}"`;
  }
  return `<a href="${escapeHtml(url)}"${attrs}>${escapeHtml(text)}</a>`;
}

/** `<form method="post" action="..."><button>text</button></form>`.
 *  Mirrors Rails' `button_to` shape. Option keys:
 *   - `method: "delete" | "patch" | "put"` — emitted as a hidden
 *     `_method` input; the server's method-override middleware
 *     rewrites the request.
 *   - `class:` — goes on the `<button>`. Historical versions used
 *     this for the form; Rails switched to `form_class:` when it
 *     needed to style both independently.
 *   - `form_class:` — goes on the `<form>`. Defaults to Rails'
 *     `"button_to"` class when omitted.
 *   - `data: { ... }` — flattened to `data-*` attrs on the button.
 *  Attribute order and presence match Rails' scaffold output so
 *  the compare tool's DOM diff passes. */
export function buttonTo(text: string, target: string, opts: Record<string, any> = {}): string {
  const method = typeof opts.method === "string" ? opts.method : "post";
  const buttonCls = typeof opts.class === "string" ? opts.class : "";
  const formCls = typeof opts.form_class === "string" ? opts.form_class : "button_to";
  const methodLower = method.toLowerCase();
  const methodInput =
    methodLower !== "post" && methodLower !== "get"
      ? `<input type="hidden" name="_method" value="${escapeHtml(method)}" />`
      : "";
  // Data attributes: accept two shapes — a `data:` subhash
  // (Rails surface form) OR pre-flattened `"data-key": val`
  // entries (emitter lowering convention, since it flattens
  // during lowering for attribute-order stability). Iterating
  // once handles both without double-counting.
  let dataAttrs = "";
  if (opts.data && typeof opts.data === "object") {
    for (const [k, v] of Object.entries(opts.data)) {
      const key = String(k).replace(/_/g, "-");
      dataAttrs += ` data-${escapeHtml(key)}="${escapeHtml(String(v))}"`;
    }
  }
  for (const [k, v] of Object.entries(opts)) {
    if (k.startsWith("data-")) {
      dataAttrs += ` ${escapeHtml(k)}="${escapeHtml(String(v))}"`;
    }
  }
  const buttonClsAttr = buttonCls ? ` class="${escapeHtml(buttonCls)}"` : "";
  const csrfInput = `<input type="hidden" name="authenticity_token" value="">`;
  return `<form class="${escapeHtml(formCls)}" method="post" action="${escapeHtml(target)}">${methodInput}<button${buttonClsAttr}${dataAttrs} type="submit">${escapeHtml(text)}</button>${csrfInput}</form>`;
}

/** Form-tag wrapper. Rails' `form_with(model: record)` computes
 *  the action URL from the record's persistence state: new
 *  records POST to the resource's collection URL; persisted
 *  records PATCH to the member URL. Method override for PATCH /
 *  PUT / DELETE uses a hidden `_method` input so browsers (which
 *  only natively support GET/POST in forms) can still issue the
 *  right HTTP verb — the server's handleRequest honors it.
 *
 *  `resourcePath` is the Rails `polymorphic_path` equivalent:
 *  "/articles" for a new Article, "/articles/123" for an
 *  existing one. The emitter computes this from the view's
 *  resource context + record.id. */
export function formWrap(
  record: { id?: number | null } | null,
  resourcePath: string,
  cls: string,
  inner: string,
): string {
  const persisted = !!(record && record.id);
  const action = persisted ? `${resourcePath}` : resourcePath;
  // Turbo's standard shape: hidden `_method` input for PATCH /
  // PUT / DELETE (the server's handleRequest reads this to
  // override the HTTP verb) + `authenticity_token` hidden input
  // for CSRF. Roundhouse's handleRequest doesn't verify CSRF
  // today; we still emit the field so Rails-convention form
  // submissions match the shape Turbo expects.
  const methodInput = persisted
    ? `<input type="hidden" name="_method" value="patch">`
    : "";
  const csrfInput = `<input type="hidden" name="authenticity_token" value="">`;
  const classAttr = cls ? ` class="${escapeHtml(cls)}"` : "";
  // Rails' form_with always emits `accept-charset="UTF-8"` on the
  // generated `<form>` tag. Matches Rails' `UTF8_ENFORCER_TAG`
  // injection in UTF-8-safe form submission handling.
  return `<form${classAttr} action="${escapeHtml(action)}" accept-charset="UTF-8" method="post">${methodInput}${csrfInput}${inner}</form>`;
}

/** Humanize a snake_case field name for a label: `"first_name"`
 *  → `"First name"`. Rails' default `label` helper does this. */
function humanize(field: string): string {
  const spaced = field.replace(/_/g, " ");
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

/** FormBuilder for the scaffold shape. `record` is the record
 *  being edited (used for field values + HTML id-es); `prefix`
 *  is the Rails `name` prefix (`"article"` → inputs get
 *  `name="article[title]"`). Options pass through as a record;
 *  `class` gets set as the input's HTML class attribute; `rows`
 *  / `cols` set on textarea; other keys pass through as HTML
 *  attributes. */
export class FormBuilder {
  record: Record<string, any> | null;
  prefix: string;

  constructor(record: Record<string, any> | null, prefix: string) {
    this.record = record;
    this.prefix = prefix;
  }

  private _id(field: string): string {
    return `${this.prefix}_${field}`;
  }

  private _name(field: string): string {
    return `${this.prefix}[${field}]`;
  }

  private _value(field: string): string {
    const v = this.record?.[field];
    return v == null ? "" : String(v);
  }

  label(field: string, opts: Record<string, any> = {}): string {
    const cls = opts.class ? ` class="${escapeHtml(String(opts.class))}"` : "";
    return `<label for="${escapeHtml(this._id(field))}"${cls}>${escapeHtml(humanize(field))}</label>`;
  }

  textField(field: string, opts: Record<string, any> = {}): string {
    const cls = opts.class ? ` class="${escapeHtml(String(opts.class))}"` : "";
    // Rails omits `value=""` on empty text-fields — the attribute
    // only appears when there's something to render. Matching
    // that conserves the attribute set for byte-equal compare.
    const v = this._value(field);
    const valueAttr = v ? ` value="${escapeHtml(v)}"` : "";
    return `<input type="text"${cls} name="${escapeHtml(this._name(field))}" id="${escapeHtml(this._id(field))}"${valueAttr}>`;
  }

  textArea(field: string, opts: Record<string, any> = {}): string {
    const cls = opts.class ? ` class="${escapeHtml(String(opts.class))}"` : "";
    const rows = opts.rows != null ? ` rows="${escapeHtml(String(opts.rows))}"` : "";
    // Rails' `text_area` always wraps the value in newlines —
    // `<textarea>\n<value>\n</textarea>` even when the value is
    // empty. That shape is part of the HTML5 "textarea element
    // with default-value-preserved" idiom: the opening-tag
    // newline is stripped by parsers, so the visible content
    // matches the value exactly. Matching this byte-for-byte is
    // required for cross-rendering compare equivalence.
    const value = this._value(field);
    return `<textarea${rows}${cls} name="${escapeHtml(this._name(field))}" id="${escapeHtml(this._id(field))}">\n${escapeHtml(value)}</textarea>`;
  }

  // Rails' Ruby form helper is `textarea` in newer versions,
  // `text_area` historically. Support both identifier spellings.
  textarea(field: string, opts: Record<string, any> = {}): string {
    return this.textArea(field, opts);
  }

  submit(opts: Record<string, any> = {}): string {
    const cls = opts.class ? ` class="${escapeHtml(String(opts.class))}"` : "";
    // Rails capitalizes the resource name: `Update Article`,
    // `Create Article`. Our prefix is the lowercase singular
    // (`"article"`); capitalize the first letter to match.
    const humanPrefix = this.prefix.charAt(0).toUpperCase() + this.prefix.slice(1);
    const label = typeof opts.label === "string"
      ? opts.label
      : (this.record && this.record.id ? `Update ${humanPrefix}` : `Create ${humanPrefix}`);
    // Rails' scaffold form.submit emits `name="commit"` and
    // `data-disable-with="<label>"` — both part of Rails UJS's
    // double-submit protection. Matching the attribute set is
    // what the compare tool checks; the data-* value stays in
    // sync with the label.
    const esc = escapeHtml(label);
    return `<input type="submit" name="commit" value="${esc}"${cls} data-disable-with="${esc}">`;
  }
}

/** `<%= errorMessagesFor(article, "article") %>` — renders the
 *  standard Rails-scaffold error block if the record has
 *  validation errors, otherwise empty string. Consolidates the
 *  `if record.errors.any? ... end` + iteration pattern the ERB
 *  form partial uses, so the emitter doesn't have to translate
 *  those control-flow shapes view-by-view. */
export function errorMessagesFor(record: { errors?: { none?: boolean; any?: boolean; count?: number } & Record<string, any> } | null, noun: string): string {
  if (!record || !record.errors) return "";
  const errs = record.errors as any;
  const none = typeof errs.none === "boolean" ? errs.none : !(errs.any ?? false);
  if (none) return "";
  const count = typeof errs.count === "number" ? errs.count : 0;
  // Scaffold shape: list of "<field> <message>" lines from the
  // ErrorCollection's internal list. juntos.ts' ErrorCollection
  // exposes the raw array via a non-public field; we reach in for
  // message rendering. Production would expose a fullMessages()
  // method symmetrical to Rails'.
  const list = (errs as any)._errors as Array<{ field: string; message: string }> | undefined;
  const items = list
    ? list.map((e) => `<li>${escapeHtml(humanize(e.field))} ${escapeHtml(e.message)}</li>`).join("")
    : "";
  return `<div id="error_explanation" class="bg-red-50 text-red-500 px-3 py-2 font-medium rounded-md mt-3"><h2>${count} error${count === 1 ? "" : "s"} prohibited this ${escapeHtml(noun)} from being saved:</h2><ul class="list-disc ml-6">${items}</ul></div>`;
}

/** `<%= turbo_stream_from "articles" %>` — subscribes the page
 *  to a Turbo Stream channel. The Turbo client reads the
 *  `signed-stream-name` attribute, sends it over Action Cable
 *  as a `subscribe` command's identifier, and the server's
 *  cable handler decodes the base64 prefix to recover the
 *  channel name for broadcast routing.
 *
 *  Rails signs the stream name with an HMAC so the server can
 *  trust the decoded value. Roundhouse's cable handler doesn't
 *  verify the signature today — it just parses the base64 part
 *  and routes. An "unsigned" suffix is fine for the acceptance
 *  scenario; production deployments that care about authenticity
 *  would upgrade both sides to HMAC-sign-and-verify.
 *  `escapeHtml` isn't needed on the encoded value — base64 is
 *  URL- and HTML-safe by construction. */
export function turboStreamFrom(channel: string): string {
  const encoded = Buffer.from(JSON.stringify(channel), "utf-8").toString("base64");
  return `<turbo-cable-stream-source channel="Turbo::StreamsChannel" signed-stream-name="${encoded}--unsigned"></turbo-cable-stream-source>`;
}

/** `dom_id(record, prefix?)` → Rails convention:
 *    one-arg  → `"<singular>_<id>"`                    (article_1)
 *    two-arg  → `"<prefix>_<singular>_<id>"`           (comments_count_article_1)
 *  Singular derives from the record's constructor.name
 *  (CamelCase → snake_case). Prefix is a symbol or string and
 *  comes through as-is (no transformation). */
export function domId(record: any, prefix?: string): string {
  if (!record) return "";
  const singular = String(record.constructor?.name ?? "record")
    .replace(/([a-z])([A-Z])/g, "$1_$2")
    .toLowerCase();
  const id = record.id != null ? String(record.id) : "new";
  const base = `${singular}_${id}`;
  return prefix ? `${prefix}_${base}` : base;
}

// `pluralize` is generated from `runtime/ruby/inflector.rb` and
// shipped at `src/inflector.ts`. Re-export here so existing
// `Helpers.pluralize(...)` call sites continue to resolve via the
// view_helpers namespace.
export { pluralize } from "./inflector.js";

/** `truncate(text, length: N, omission: "…")` — shorten a string
 *  to at most `length` chars (default 30), appending `omission`
 *  (default `"..."`) when truncation actually happened. Rails'
 *  helper splits on character boundaries — fine for scaffold
 *  body text; production may want grapheme-aware splitting. */
export function truncate(
  text: string,
  opts: { length?: number; omission?: string } = {},
): string {
  const length = opts.length ?? 30;
  const omission = opts.omission ?? "...";
  if (text.length <= length) return text;
  const cut = Math.max(0, length - omission.length);
  return text.slice(0, cut) + omission;
}

// `contentFor` defined above (supports both getter and setter
// forms; persists to the module-level slot map).

/** True if any ValidationError in `errors` targets the named
 *  field. Feeds the scaffold's conditional form-field classes
 *  (`class: [..., {"red-class": article.errors[:body].any?}]`)
 *  lowered by the emitter. Accepts both plain arrays (rust-style)
 *  and the `ErrorCollection` wrapper that TS models expose (which
 *  keeps its backing array on `_errors`). Missing/empty → false. */
export function fieldHasError(
  errors: any,
  field: string,
): boolean {
  if (!errors) return false;
  // TS's ErrorCollection holds the array on a private-ish slot;
  // reach in rather than expose a new API. Production would add
  // a `forField(name): boolean` method and make this a thin call.
  const list: Array<{ field: string }> | undefined = Array.isArray(errors)
    ? errors
    : errors._errors;
  if (!list) return false;
  for (const e of list) {
    if (e.field === field) return true;
  }
  return false;
}

/** Conservative HTML escaping. Enough for scaffold blog output. */
function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

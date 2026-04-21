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
 *  Mirrors Rails' `button_to` shape; `method: :delete` becomes a
 *  hidden `_method` input. */
export function buttonTo(text: string, target: string, opts: Record<string, string | Record<string, string>> = {}): string {
  const methodRaw = opts.method;
  const method = typeof methodRaw === "string" ? methodRaw : "post";
  const classRaw = opts.class;
  const cls = typeof classRaw === "string" ? classRaw : "";
  const methodInput =
    method.toLowerCase() !== "post" && method.toLowerCase() !== "get"
      ? `<input type="hidden" name="_method" value="${escapeHtml(method)}"/>`
      : "";
  return `<form method="post" action="${escapeHtml(target)}" class="${escapeHtml(cls)}">${methodInput}<button>${escapeHtml(text)}</button></form>`;
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
  return `<form method="post" action="${escapeHtml(action)}"${classAttr}>${methodInput}${csrfInput}${inner}</form>`;
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
    return `<input type="text" name="${escapeHtml(this._name(field))}" id="${escapeHtml(this._id(field))}" value="${escapeHtml(this._value(field))}"${cls}>`;
  }

  textArea(field: string, opts: Record<string, any> = {}): string {
    const cls = opts.class ? ` class="${escapeHtml(String(opts.class))}"` : "";
    const rows = opts.rows != null ? ` rows="${escapeHtml(String(opts.rows))}"` : "";
    return `<textarea name="${escapeHtml(this._name(field))}" id="${escapeHtml(this._id(field))}"${cls}${rows}>${escapeHtml(this._value(field))}</textarea>`;
  }

  // Rails' Ruby form helper is `textarea` in newer versions,
  // `text_area` historically. Support both identifier spellings.
  textarea(field: string, opts: Record<string, any> = {}): string {
    return this.textArea(field, opts);
  }

  submit(opts: Record<string, any> = {}): string {
    const cls = opts.class ? ` class="${escapeHtml(String(opts.class))}"` : "";
    const label = typeof opts.label === "string"
      ? opts.label
      : (this.record && this.record.id ? `Update ${this.prefix}` : `Create ${this.prefix}`);
    return `<input type="submit" value="${escapeHtml(label)}"${cls}>`;
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

/** `dom_id(record)` → `"<singular>_<id>"`. Takes `any`; expects
 *  `.id`. Models that override can implement their own `.dom_id`
 *  method and generated code can prefer that. */
export function domId(record: any): string {
  return record?.id != null ? `record_${record.id}` : "";
}

/** Naive pluralization — append `s` when count != 1. */
export function pluralize(count: number, word: string): string {
  return count === 1 ? `1 ${word}` : `${count} ${word}s`;
}

// `contentFor` defined above (supports both getter and setter
// forms; persists to the module-level slot map).

/** Conservative HTML escaping. Enough for scaffold blog output. */
function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

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

/** `content_for(slot, body)` stashes for layout consumption.
 *  Phase 4d's emitted views don't route through a layout, so this
 *  returns an empty string. */
export function contentFor(_slot: string, _body: string): string {
  return "";
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

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

/** Form-tag wrapper. Called by the emitter after rendering a
 *  `form_with` block's inner buffer. */
export function formWrap(action: string | null, cls: string, inner: string): string {
  const actionAttr = action != null ? ` action="${escapeHtml(action)}"` : "";
  return `<form method="post"${actionAttr} class="${escapeHtml(cls)}">${inner}</form>`;
}

/** Stub FormBuilder. One instance per form_with block. Minimal
 *  option support — the scaffold tests don't check input
 *  attributes. */
export class FormBuilder {
  record: unknown;
  cls: string;

  constructor(record: unknown, cls: string = "") {
    this.record = record;
    this.cls = cls;
  }

  label(field: string): string {
    return `<label for="${escapeHtml(field)}">${escapeHtml(field)}</label>`;
  }

  textField(field: string): string {
    return `<input type="text" name="${escapeHtml(field)}"/>`;
  }

  textarea(field: string): string {
    return `<textarea name="${escapeHtml(field)}"></textarea>`;
  }

  submit(): string {
    return `<input type="submit" value="Submit"/>`;
  }
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

// Roundhouse TypeScript HTTP runtime — Phase 4c compile-only stubs.
//
// Hand-written, shipped alongside generated code (copied in by the TS
// emitter as `src/http.ts`). Provides just enough surface that emitted
// controller actions type-check under tsc: `Response`, a `params()`
// accessor, and free functions for `render` / `redirectTo` / `head` /
// `respondTo`.
//
// Mirrors the Rust/Crystal/Go/Elixir twins in intent; since tsconfig's
// `strict` is off and TypeScript's structural typing is permissive,
// the stubs lean on `any` / `unknown` more than the typed targets do.
// Real HTTP behavior lands in Phase 4e+; controller tests stay
// `test.skip`-ped, so nothing executes during `node:test`.

/** Opaque response value. Every action returns one. */
export class Response {}

/** Stub `render(...)`. Accepts any arg shape — template symbol, an
 *  options record, or a model instance — and ignores them all. */
export function render(..._args: unknown[]): Response {
  return new Response();
}

/** Stub `redirectTo(...)`. Accepts a target + optional options record. */
export function redirectTo(..._args: unknown[]): Response {
  return new Response();
}

/** Stub `head(status)`. */
export function head(..._args: unknown[]): Response {
  return new Response();
}

/** `respond_to do |format| ... end` lowers to this. Phase 4c wires
 *  only the HTML branch; the JSON branch is replaced at the call
 *  site with a `// TODO: JSON branch` comment. */
export function respondTo(
  fn: (fr: FormatRouter) => unknown,
): Response {
  fn(new FormatRouter());
  return new Response();
}

export class FormatRouter {
  html(fn: () => unknown): Response {
    fn();
    return new Response();
  }

  // Phase 4c replaces call sites with a TODO comment; this stub stays
  // callable so hand-written code outside the emitter still compiles.
  json(_fn: () => unknown): Response {
    return new Response();
  }
}

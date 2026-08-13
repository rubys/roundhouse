// Narrowing accessors over the recursive request-params tree.
//
// `Roundhouse::ParamValue` was a per-target TYPE WITH NO OPERATIONS, so
// every consumer narrowed at its own call site with `is_a?`, and each
// emitter recognized that as an IDIOM (a type test in `If`-condition
// position whose branches ARE the narrowed value) rather than modeling
// the test. Move the same fact to any other position and it falls off a
// cliff — measured, three spellings each broke a different target.
//
// So the operations live beside the type, hand-written per target the
// way `Db` is: these bodies inspect this target's representation, which
// is what makes them a primitive rather than framework logic.
//
// Semantics fixed by a Rails 8.1 oracle: `permit` keeps a blank "" and
// `Parameters#compact` drops only an explicit nil. So blank counts as
// provided; absent, JSON null, and non-scalar do not.

import type { ParamValue, ParamValueObject } from "./param_value.js";

export function sub(params: Record<string, ParamValue>, key: string): ParamValueObject {
  const value = params[key];
  if (typeof value === "object" && value !== null && !Array.isArray(value)) {
    return value;
  }
  return {};
}

export function str(sub: Record<string, ParamValue>, key: string, fallback: string): string {
  const value = sub[key];
  return typeof value === "string" ? value : fallback;
}

export function provided(sub: Record<string, ParamValue>, key: string): boolean {
  return typeof sub[key] === "string";
}

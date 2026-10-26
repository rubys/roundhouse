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

// The scalar readers the params lowering emits, and its record of
// multiparameter keys. Those keys (`title(1)`) are modeled on the Ruby
// family only (runtime/ruby/params.rb); this target ignores them, as it
// always has, so these pass through.
export function scalar_provided(sub: Record<string, ParamValue>, key: string): boolean {
  return provided(sub, key);
}

export function scalar(sub: Record<string, ParamValue>, key: string): string {
  return str(sub, key, "");
}

export function no_fields(): string[] {
  return [];
}

export function multiparameter_fields(_sub: Record<string, ParamValue>, _keys: string[]): string[] {
  return [];
}

export function multiparameter_unsupported(_sub: Record<string, ParamValue>, _keys: string[]): boolean {
  return false;
}

export function any_of(list: string[], names: string[]): boolean {
  return names.some((n) => list.includes(n));
}

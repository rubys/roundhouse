// Recursive type for request parameters. See
// `runtime/crystal/param_value.cr` for the cross-target rationale —
// every target's runtime carries an equivalent recursive type so the
// shared RBS abstraction `Hash[String, Roundhouse::ParamValue]`
// renders to a faithful realization per language.
//
// TS uses an interface for the Hash variant to break the structural
// self-reference TypeScript would otherwise reject in a `type` alias.

export interface ParamValueObject {
  [key: string]: ParamValue;
}

export type ParamValue = string | ParamValueObject | ParamValue[];

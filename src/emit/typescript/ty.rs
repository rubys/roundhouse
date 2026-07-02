use crate::ident::ClassId;
use crate::ty::Ty;

/// Map a Roundhouse `Ty` to its TypeScript type expression.
/// Conservative — gradual escape hatch (`Untyped`) → `any`.
pub fn ts_ty(ty: &Ty) -> String {
    match ty {
        Ty::Int | Ty::Float => "number".into(),
        Ty::Bool => "boolean".into(),
        Ty::Str | Ty::Sym => "string".into(),
        Ty::Nil => "null".into(),
        Ty::Array { elem } => format!("{}[]", ts_ty(elem)),
        Ty::Hash { key, value } => format!("Record<{}, {}>", ts_ty(key), ts_ty(value)),
        Ty::Class { id, .. } => ts_class_ty(id),
        // TS has a native `Date`. Temporal columns store ISO-8601 text
        // (a `_<col>: string` backing field) and read back as a real
        // `Date` via a computed `get <col>(): Date | null` that parses
        // the backing — see the temporal branch in typescript.rs's
        // `js_library_class` and `RhDateTime.parse`. (The
        // `class_is_temporal` → "string" path in ts_class_ty is
        // separate: it's for hand-written-rbs Time in the shared
        // runtime, not the first-class `Ty::Time` column type.)
        Ty::Time => "Date".into(),
        Ty::Untyped => "any".into(),
        Ty::Bottom => "never".into(),
        // A temporal reader's `Time | Nil` union → `Date | null`. Only
        // Time-containing unions are rendered here (the datetime Stage-2
        // reader return type); other unions still fall through to `any`
        // (no general union rendering wired for TS yet).
        Ty::Union { variants } if variants.iter().any(|v| matches!(v, Ty::Time)) => {
            let mut parts: Vec<String> = variants.iter().map(ts_ty).collect();
            parts.dedup();
            parts.join(" | ")
        }
        _ => "any".into(),
    }
}

/// Render a `Ty` for the return-type slot of a TS function/method.
/// Differs from `ts_ty` only at the OUTERMOST level: bare `Ty::Nil`
/// becomes `void` (the function returns nothing meaningful) instead
/// of `null` (a value type). Inner positions — including unions
/// containing Nil — recurse to `ts_ty` so `Ty::Union { Article, Nil }`
/// renders as `Article | null`, the right shape for a value the
/// caller might inspect.
pub fn ts_return_ty(ty: &Ty) -> String {
    match ty {
        Ty::Nil => "void".into(),
        _ => ts_ty(ty),
    }
}

/// Render a `Ty` for the return slot of an `async` TS function or
/// method — wraps `ts_return_ty` in `Promise<...>`. `Promise<void>`
/// is the canonical TS shape for an async function with no return
/// value (`await`-ing one yields `undefined`, which is what the
/// caller sees from a void await).
///
/// `ts_return_ty` already maps `Ty::Nil` → `void` at the outermost
/// level; this helper inherits that and wraps the result, so a
/// `Ty::Nil` return on an async method emits `Promise<void>` (not
/// `Promise<null>`). Inner Nil positions stay `null` per the
/// non-async behavior.
pub fn ts_async_return_ty(ty: &Ty) -> String {
    format!("Promise<{}>", ts_return_ty(ty))
}

fn ts_class_ty(id: &ClassId) -> String {
    let raw = id.0.as_str();
    if class_is_temporal(id) {
        return "string".into();
    }
    // Ruby builtins whose TS spelling differs:
    //   `Regexp` — JS calls it `RegExp` (capital E).
    //   `Hash`   — no TS class; `Record<string, any>` is the
    //              shape Ruby Hash flows through.
    //   `Symbol` — JS has `symbol` (lowercase) as a primitive
    //              type; method/field positions use `string` since
    //              Ruby symbols typically map to string keys.
    match raw {
        "Regexp" => return "RegExp".into(),
        "Hash" => return "Record<string, any>".into(),
        _ => {}
    }
    // Module-qualified class refs collapse to the bare last segment —
    // that's the import name in the corresponding .ts file. Within
    // the defining file (`src/active_record_base.ts` defining `class
    // Base`), the bare name is the class itself; in importing files,
    // `import { Base }` brings it into scope under the same name.
    let last = raw.rsplit("::").next().unwrap_or(raw);
    last.into()
}

fn class_is_temporal(id: &ClassId) -> bool {
    matches!(
        id.0.as_str(),
        "Date" | "Time" | "DateTime" | "ActiveSupport::TimeWithZone"
    )
}

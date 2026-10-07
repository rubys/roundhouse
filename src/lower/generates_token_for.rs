//! `generates_token_for :purpose, expires_in: D do <value> end` — Rails
//! 7.1's single-use-ish tokens, synthesized in the shared model lowering
//! (all targets): `generate_token_for(purpose)`, `Model.find_by_token_for
//! (purpose, token)` and its bang form.
//!
//! The analyzer already types the three methods
//! (`register_generates_token_for`); this pass defines them.
//!
//! ## What Rails does, and what this does
//!
//! A token is signed for one purpose, optionally expires, and carries
//! the record id plus the VALUE of the declaration block evaluated on
//! the record — so the token stops verifying when that value changes
//! (a password-reset token dies when the password does). `find_by_token_for`
//! verifies, finds the record by id, re-evaluates the block on it and
//! compares; any failure is nil. The bang form raises
//! `ActiveSupport::MessageVerifier::InvalidSignature` instead, and
//! `RecordNotFound` for a token naming a row that is gone.
//!
//! The wire half is `ActiveRecord::TokenFor`
//! (runtime/ruby/active_record/token_for.rb), the runtime
//! `has_secure_password`'s reset token already stands on, and the
//! format is Rails' own: the `[id]` or `[id, value]` payload under the
//! purpose `"<Model>\n<purpose>\n<expires_in seconds>"`. The purpose is
//! a compile-time fact, so `expires_in:` must fold to seconds here — an
//! Integer or `N.<unit>` literal (`lower::duration::literal_seconds`).
//!
//! ## Synthesis
//!
//! Ruby source, re-ingested — the same route `ingest::current_attributes`
//! takes — because the block body is an instance-level expression the
//! finder has to evaluate on the record it found. Two purpose
//! dispatchers carry everything per-declaration: `__token_data(purpose)`,
//! the payload the record produces now; and `__token_meta(purpose)`, the
//! `[full_purpose, expires_in]` pair minting and the finders share:
//!
//! ```ruby
//! def __token_data(purpose)
//!   case purpose
//!   when :email_change
//!     ActiveRecord::TokenFor.value_data(id, unconfirmed_email)
//!   else
//!     raise "unknown token purpose"
//!   end
//! end
//!
//! def self.__token_meta(purpose)
//!   case purpose
//!   when :email_change then ["User\\nemail_change\\n3600", 3600]
//!   else raise "unknown token purpose"
//!   end
//! end
//!
//! def generate_token_for(purpose)
//!   data = __token_data(purpose)
//!   meta = self.class.__token_meta(purpose)
//!   ActiveRecord::TokenFor.generate(data, meta[0], meta[1])
//! end
//! ```
//!
//! A block value goes into the payload as the JSON Rails' `as_json`
//! writes for a String, an Integer or a boolean (nil as `null`), chosen
//! by the type the analyzer gave the block. Any other value goes in as
//! its String form, so a Time or a Float token verifies in the emitted
//! app but not across to Rails.
//!
//! ## Claimed and declined
//!
//! Claimed: a plain Symbol purpose (`:email_change`, not `:"a-b"`), an
//! optional `expires_in:` that folds to seconds, and an optional block
//! without parameters. Anything else — a computed expiry,
//! `expires_at:`, a block taking the record as a parameter — stays
//! unclaimed and keeps its unsupported warning: half an expansion is
//! worse than none. The methods dispatch on a purpose passed at
//! runtime, so it is all or nothing per model: a purpose whose LAST
//! declaration is unclaimed (Rails keeps the last), a computed purpose,
//! or a key that is not an Integer `id` declines the whole model, and
//! every declaration on it warns.
//! `token_for_decls` is the one place that decides, and
//! `report_unclaimed_unknowns` asks `claims` by span.

use super::duration::literal_seconds;
use super::model_to_library::fn_sig;
use crate::dialect::{MethodDef, Model, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

/// One `generates_token_for` declaration this pass claims.
pub(crate) struct TokenForDecl {
    pub(crate) purpose: Symbol,
    /// `expires_in:` in seconds; 0 means the token never expires.
    pub(crate) expires_in: i64,
    /// The block body; `None` means the token carries only the id.
    pub(crate) value: Option<Expr>,
    pub(crate) span: Span,
}

/// The declarations `model` gets token methods for, one per purpose:
/// Rails keeps the LAST declaration of a purpose
/// (`token_definitions.merge`). All or nothing. The methods dispatch on
/// a purpose the caller passes at runtime, so a purpose whose last
/// declaration this pass cannot expand would leave a typed
/// `generate_token_for(:that)` raising in the emitted app. An earlier
/// form must not stand in for it either. That declines the whole model,
/// as do a computed purpose (it could replace any of them) and a key
/// that is not an Integer `id`: the payload is `[id, …]` with the id as
/// a JSON number, and Rails writes a uuid or other String key as a JSON
/// string the runtime does not read back.
pub(crate) fn token_for_decls(model: &Model) -> Vec<TokenForDecl> {
    model_decls(model).unwrap_or_default()
}

/// Whether the declaration at `span` is one this pass handles: an
/// expandable form on a model it does not decline, including one a
/// later declaration supersedes, which Rails discards too.
/// `report_unclaimed_unknowns` asks.
pub(crate) fn claims(model: &Model, span: Span) -> bool {
    model_decls(model).is_some()
        && parse_decls(&model.body).iter().any(|d| matches!(d, Ok(decl) if decl.span == span))
}

/// The last declaration of each purpose, or `None` when the model is
/// declined whole (see `token_for_decls`).
fn model_decls(model: &Model) -> Option<Vec<TokenForDecl>> {
    if !integer_id(model) {
        return None;
    }
    let mut last: Vec<(Symbol, Option<TokenForDecl>)> = Vec::new();
    for d in parse_decls(&model.body) {
        let (purpose, decl) = match d {
            Ok(decl) => (decl.purpose.clone(), Some(decl)),
            Err(purpose) => (purpose?, None),
        };
        last.retain(|(p, _)| *p != purpose);
        last.push((purpose, decl));
    }
    // Explicit all-or-nothing: any purpose whose last declaration cannot
    // expand declines the whole model (do not rely on Option collect).
    if last.iter().any(|(_, decl)| decl.is_none()) {
        return None;
    }
    Some(last.into_iter().filter_map(|(_, decl)| decl).collect())
}

fn integer_id(model: &Model) -> bool {
    let named_id = model.primary_key.as_ref().is_none_or(|k| k.as_str() == "id");
    named_id && model.attributes.fields.get(&Symbol::from("id")).is_none_or(|t| *t == Ty::Int)
}

/// A purpose the synthesized source can spell as a bare Symbol literal
/// (`when :email_change`). A quoted one (`:"share-link"`) would come out
/// as other Ruby, so it stays unlowered.
fn plain_purpose(purpose: &Symbol) -> bool {
    let mut chars = purpose.as_str().chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Every `generates_token_for` call in `body`, in source order: the
/// declaration when this pass can expand it, otherwise the purpose it
/// names (`None` for a computed one).
fn parse_decls(body: &[ModelBodyItem]) -> Vec<Result<TokenForDecl, Option<Symbol>>> {
    let mut out = Vec::new();
    for item in body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, block, .. } = &*expr.node else {
            continue;
        };
        if method.as_str() != "generates_token_for" {
            continue;
        }
        let mut purpose: Option<Symbol> = None;
        let mut expires_in: i64 = 0;
        let mut ok = true;
        for (i, arg) in args.iter().enumerate() {
            match &*arg.node {
                ExprNode::Lit { value: Literal::Sym { value } } if i == 0 => {
                    purpose = Some(value.clone());
                }
                ExprNode::Hash { entries, .. } if i == 1 => {
                    for (k, v) in entries {
                        match &*k.node {
                            ExprNode::Lit { value: Literal::Sym { value: key } }
                                if key.as_str() == "expires_in" =>
                            {
                                match literal_seconds(v) {
                                    Some(secs) if secs > 0 => expires_in = secs,
                                    _ => ok = false,
                                }
                            }
                            _ => ok = false,
                        }
                    }
                }
                _ => ok = false,
            }
        }
        let value = match block {
            None => None,
            Some(b) => match &*b.node {
                ExprNode::Lambda { params, rest_param, block_param, body, .. }
                    if params.is_empty() && rest_param.is_none() && block_param.is_none() =>
                {
                    Some(body.clone())
                }
                _ => {
                    ok = false;
                    None
                }
            },
        };
        out.push(match purpose {
            Some(purpose) if ok && plain_purpose(&purpose) => {
                Ok(TokenForDecl { purpose, expires_in, value, span: expr.span })
            }
            purpose => Err(purpose),
        });
    }
    out
}

/// Synthesize the model's token methods from its declarations and
/// append them to `methods`; a method the model writes itself wins.
pub(crate) fn push_token_for_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    let decls = token_for_decls(model);
    if decls.is_empty() {
        return;
    }
    let src = synthesized_source(model, &decls);
    let (parsed, diags) = crate::ingest::prism::scope(|| {
        crate::ingest::ingest_library_classes(src.as_bytes(), "<generates_token_for>")
    });
    let synthesized: Vec<MethodDef> = match parsed {
        Ok(classes) if diags.is_empty() => classes.into_iter().flat_map(|c| c.methods).collect(),
        Ok(_) => {
            crate::ingest::survey::record_synthesis_failure(
                "<generates_token_for>",
                &format!("generates_token_for methods for `{}`", model.name.0.as_str()),
                &diags,
            );
            return;
        }
        Err(err) => {
            crate::ingest::survey::record(&err);
            return;
        }
    };
    let record = Ty::Class { id: model.name.clone(), args: vec![] };
    let nilable_record = Ty::Union { variants: vec![record.clone(), Ty::Nil] };
    let meta_ty = Ty::Array { elem: Box::new(Ty::Untyped) };
    for mut m in synthesized {
        // Declared signatures, so the sidecar the strict targets compile
        // from says what the registry already says (`register_generates_
        // token_for`) instead of `untyped`. Name every helper explicitly
        // — a catch-all would stamp the wrong shape after `__token_meta`.
        m.signature = match m.name.as_str() {
            "generate_token_for" => Some(fn_sig(vec![(Symbol::from("purpose"), Ty::Sym)], Ty::Str)),
            "find_by_token_for" => Some(fn_sig(
                vec![(Symbol::from("purpose"), Ty::Sym), (Symbol::from("token"), Ty::Str)],
                nilable_record.clone(),
            )),
            "find_by_token_for!" => Some(fn_sig(
                vec![(Symbol::from("purpose"), Ty::Sym), (Symbol::from("token"), Ty::Str)],
                record.clone(),
            )),
            "__token_data" => Some(fn_sig(vec![(Symbol::from("purpose"), Ty::Sym)], Ty::Str)),
            "__token_meta" => Some(fn_sig(vec![(Symbol::from("purpose"), Ty::Sym)], meta_ty.clone())),
            other => {
                crate::ingest::survey::record_synthesis_failure(
                    "<generates_token_for>",
                    &format!("unexpected synthesized method `{other}` on `{}`", model.name.0.as_str()),
                    &[],
                );
                continue;
            }
        };
        // `methods` already holds the model's own (push_user_methods
        // runs first), and one it writes itself wins.
        let own = methods.iter().any(|x| x.name == m.name && x.receiver == m.receiver);
        if !own {
            methods.push(m);
        }
    }
}

/// Rails' `TokenDefinition#full_purpose`, `[class, purpose,
/// expires_in].join("\n")` — a nil expiry joins as "" — JSON-escaped
/// the way `TokenFor.verified_data` compares it, and escaped once more
/// to sit in a Ruby string literal: `"User\\nemail_change\\n3600"`.
fn full_purpose(model: &Model, d: &TokenForDecl) -> String {
    let expires = if d.expires_in > 0 { d.expires_in.to_string() } else { String::new() };
    format!("{}\\\\n{}\\\\n{expires}", model.name.0.as_str(), d.purpose.as_str())
}

/// The `TokenFor` call that writes `[id, value]` for a block value
/// `src`, chosen by the type the analyzer gave it, so the JSON is what
/// Rails' `as_json` writes: a String or an Integer or a boolean, each
/// possibly nil. Any other value goes over as its String form, which
/// verifies in the emitted app but not across to Rails (a Time or a
/// Float would need Rails' own `as_json` formatting).
fn value_payload(e: &Expr, src: &str) -> String {
    let non_nil: Vec<&Ty> = match &e.ty {
        Some(Ty::Union { variants }) => variants.iter().filter(|t| **t != Ty::Nil).collect(),
        Some(t) => vec![t],
        None => Vec::new(),
    };
    // Always parenthesize `src`: a modifier-`if` or multi-statement
    // block body is illegal as a bare call argument, and without
    // parentheses synthesis fails while the analyzer still types the
    // methods (invariant 6).
    match non_nil.as_slice() {
        [Ty::Str] => format!("value_data(id, ({src}))"),
        [Ty::Int] => format!("int_value_data(id, ({src}))"),
        [Ty::Bool] => format!("bool_value_data(id, ({src}))"),
        _ => format!("value_data(id, ({src})&.to_s)"),
    }
}

fn synthesized_source(model: &Model, decls: &[TokenForDecl]) -> String {
    use crate::emit::ruby::emit_expr;
    let class = model.name.0.as_str();
    let case = |arms: String| {
        format!("    case purpose\n{arms}    else\n      raise \"unknown token purpose\"\n    end\n")
    };
    let arms = |arm: &dyn Fn(&TokenForDecl) -> String| {
        decls
            .iter()
            .map(|d| format!("    when :{}\n      {}\n", d.purpose.as_str(), arm(d)))
            .collect::<String>()
    };

    let data = case(arms(&|d| match &d.value {
        Some(e) => format!("ActiveRecord::TokenFor.{}", value_payload(e, &emit_expr(e))),
        None => "ActiveRecord::TokenFor.id_data(id)".to_string(),
    }));
    let meta = case(arms(&|d| {
        format!("[\"{}\", {}]", full_purpose(model, d), d.expires_in)
    }));

    // Rails: the finder answers nil for a token that does not verify,
    // names no row, or whose payload the record no longer produces. The
    // bang form raises InvalidSignature for the first and last, and
    // `find`'s RecordNotFound for a row that is gone.
    format!(
        "class {class}
  def __token_data(purpose)
{data}  end

  def self.__token_meta(purpose)
{meta}  end

  def generate_token_for(purpose)
    data = __token_data(purpose)
    meta = self.class.__token_meta(purpose)
    ActiveRecord::TokenFor.generate(data, meta[0], meta[1])
  end

  def self.find_by_token_for(purpose, token)
    data = ActiveRecord::TokenFor.verified_data(token, {class}.__token_meta(purpose)[0])
    return nil if data == \"\"
    record = {class}.find_by(id: ActiveRecord::TokenFor.data_id(data))
    return nil if record.nil?
    record.__token_data(purpose) == data ? record : nil
  end

  def self.find_by_token_for!(purpose, token)
    data = ActiveRecord::TokenFor.verified_data(token, {class}.__token_meta(purpose)[0])
    raise ActiveSupport::MessageVerifier::InvalidSignature if data == \"\"
    record = {class}.find(ActiveRecord::TokenFor.data_id(data))
    raise ActiveSupport::MessageVerifier::InvalidSignature unless record.__token_data(purpose) == data
    record
  end
end
"
    )
}

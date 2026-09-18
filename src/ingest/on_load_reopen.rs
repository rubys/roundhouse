//! `ActiveSupport.on_load(:hook) do … end` at the top of a `lib/` file
//! — a class reopen deferred until the framework class it reopens has
//! loaded. The class finder skips blocks, so nothing inside one is a
//! library class, and until this pass the whole file was DROPPED
//! SILENTLY: campfire's `lib/rails_ext/action_text_attachables.rb`
//! reopens `ActionText::Attachment.from_node` inside one, and the tree
//! neither carried it nor said so.
//!
//! WHAT IS CARRIED, AND HOW. That reopen exists so rotating
//! `SECRET_KEY_BASE` does not orphan every @mention: an sgid whose
//! signature fails is decoded by hand (Rails' `_rails.data` envelope,
//! and the Rails-7 Marshal shape behind it) and, for the models a
//! `%w[…]` constant names, the record is found anyway. The body is
//! thirty lines of `JSON.parse(…).dig`, a `rescue`-chained base64
//! fallback, a regex over Marshal bytes and a `GlobalID.find` on an
//! untyped string — every one a typing fight — for a method whose
//! whole CONTENT is that list. So the pass reads the shape and bakes
//! the fact: the list lands on `App::attachable_unsigned_models`,
//! `project::apply_attachable_locate` writes it into the emitted
//! `global_id_locator.rb` as `Attachment.permitted_without_signature`,
//! and the runtime's `SignedGlobalId.unverified_uri` is the decode,
//! transcribed once (`runtime/ruby/action_text.rb`). The same rule
//! `lower::attachable` applies to the mint: the compile-time fact is
//! the model name, not the code that reads it off `self.class`.
//!
//! The FINGERPRINT, all three required: the reopened class is
//! `ActionText::Attachment`, it defines `from_node` on its singleton,
//! and its body names the `"_rails"` envelope key — that string is the
//! hand-decode, and a `from_node` reopen without it is doing something
//! else. The list is the one constant in the body whose value is an
//! array of string literals; two such constants, or none, is a shape
//! this pass does not know.
//!
//! Anything else inside an `on_load` block — a reopen of another
//! class, a `from_node` reopen without the fingerprint, a module — is
//! reported as a survey line naming the hook and the class, rather
//! than dropped in silence. That is the contract the route ingest
//! took for `to: redirect(…)`: a hole nobody can see is how a gap
//! stays open.

use ruby_prism::Node;

use crate::app::App;
use crate::ident::Symbol;

use super::util::{
    class_name_path, constant_id_str, constant_path_of, find_all_classes_with_scope,
    find_all_modules_with_scope, module_name_path,
};
use super::{survey, IngestError};

/// The reopened class the tolerant-sgid shape lives on.
const ATTACHMENT: &str = "ActionText::Attachment";
/// The envelope key the hand-decode reads; its presence is what says
/// the reopen is the rotated-secret tolerance and not another
/// `from_node`.
const ENVELOPE_KEY: &str = "_rails";

/// Scan one file's top-level `ActiveSupport.on_load` blocks. Records
/// the tolerant-sgid list on `app` when the fingerprint matches, and a
/// survey line for every other declaration inside such a block.
pub(super) fn ingest_on_load_reopens(source: &[u8], file: &str, app: &mut App) {
    let result = super::prism::parse(source, file);
    let root = result.node();
    let src = String::from_utf8_lossy(source);
    let Some(prog) = root.as_program_node() else { return };
    for stmt in prog.statements().body().iter() {
        let Some(call) = stmt.as_call_node() else { continue };
        if constant_id_str(&call.name()) != "on_load" {
            continue;
        }
        let Some(recv) = call.receiver() else { continue };
        if constant_path_of(&recv).map(|p| p.join("::")).as_deref() != Some("ActiveSupport") {
            continue;
        }
        let hook = call
            .arguments()
            .and_then(|args| args.arguments().iter().next())
            .and_then(|a| a.as_symbol_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned()))
            .unwrap_or_else(|| "?".to_string());
        let Some(body) = call.block().and_then(|b| b.as_block_node()).and_then(|b| b.body()) else {
            continue;
        };

        for (scope, class) in find_all_classes_with_scope(&body) {
            let mut path = scope.clone();
            path.extend(class_name_path(&class).unwrap_or_default());
            let name = path.join("::");
            if name == ATTACHMENT {
                if let Some(models) = tolerant_sgid_models(&class, &src) {
                    app.attachable_unsigned_models = models;
                    continue;
                }
            }
            not_carried(file, &hook, &name);
        }
        for (scope, module) in find_all_modules_with_scope(&body) {
            let mut path = scope.clone();
            path.extend(module_name_path(&module).unwrap_or_default());
            not_carried(file, &hook, &path.join("::"));
        }
    }
}

fn not_carried(file: &str, hook: &str, name: &str) {
    survey::record(&IngestError::Unsupported {
        file: file.into(),
        message: format!(
            "`ActiveSupport.on_load(:{hook})` reopens `{name}`, which is not carried: a reopen \
             of a framework class inside a load hook is dropped (the tolerant-sgid \
             `ActionText::Attachment.from_node` shape is the one read)"
        ),
    });
}

/// The `%w[…]` of model names when `class` is the tolerant `from_node`
/// reopen, else None.
fn tolerant_sgid_models(class: &ruby_prism::ClassNode<'_>, src: &str) -> Option<Vec<Symbol>> {
    let body = class.body()?;
    let loc = body.location();
    let text = &src[loc.start_offset()..loc.end_offset()];
    if !text.contains(&format!("\"{ENVELOPE_KEY}\"")) {
        return None;
    }
    let mut has_from_node = false;
    let mut lists: Vec<Vec<Symbol>> = Vec::new();
    collect(&body, false, &mut has_from_node, &mut lists);
    if !has_from_node {
        return None;
    }
    match lists.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// Walk the class body through `class << self`, noting a class-side
/// `from_node` (`def self.from_node`, or a bare `def` inside the
/// singleton block) and every constant whose value is an array of
/// string literals.
fn collect(node: &Node<'_>, class_side: bool, has_from_node: &mut bool, lists: &mut Vec<Vec<Symbol>>) {
    if let Some(s) = node.as_statements_node() {
        for stmt in s.body().iter() {
            collect(&stmt, class_side, has_from_node, lists);
        }
        return;
    }
    if let Some(sc) = node.as_singleton_class_node() {
        if let Some(b) = sc.body() {
            collect(&b, true, has_from_node, lists);
        }
        return;
    }
    if let Some(def) = node.as_def_node() {
        if constant_id_str(&def.name()) == "from_node" && (class_side || def.receiver().is_some()) {
            *has_from_node = true;
        }
        return;
    }
    if let Some(cw) = node.as_constant_write_node() {
        if let Some(arr) = cw.value().as_array_node() {
            let mut out = Vec::new();
            for el in arr.elements().iter() {
                let Some(s) = el.as_string_node() else { return };
                out.push(Symbol::from(String::from_utf8_lossy(s.unescaped()).as_ref()));
            }
            lists.push(out);
        }
    }
}

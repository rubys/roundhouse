//! `validates_with SomeValidator, max: 5, if: -> { … }, on: :create`
//! (#1348). Rails 8.1.4 builds one `SomeValidator.new(options)` per
//! declaration and calls its `validate(record)` while validating, gated
//! by `if:` / `unless:` / `on:`; the validator adds the record's errors.
//!
//! On the ruby family (`BuildTarget::lowers_validates_with`, whose
//! runtime defines `ActiveModel::Validator`) each declaration becomes
//! model source, ingested like the app's own:
//!
//! ```ruby
//! def __validates_with_article_1
//!   SomeValidator.new(max: 5).validate(self)
//! end
//! validate :__validates_with_article_1, if: -> { … }, on: :create
//! ```
//!
//! so the conditions take `validate`'s existing path. The validator is
//! built on each validation rather than once per declaration, which only
//! a validator keeping state between calls would notice, and its
//! `options` hold the declaration's own keys without the conditions.
//! Several validator classes in one call, or a condition `validate`
//! does not take (`on: :custom`), keep the declaration the reported gap,
//! as do strict targets.

use crate::app::App;
use crate::dialect::ModelBodyItem;
use crate::expr::ExprNode;
use crate::ingest::prism_api::PrismLocation;

/// Expand every reproducible `validates_with` in a model body, in place,
/// and mark the runtime as defining `ActiveModel::Validator`.
pub(crate) fn expand(app: &mut App) {
    app.ports_active_model_validator = true;
    let sources = app.sources.clone();
    // One count for the app, not per model: a parent model's method must
    // not share a name with its subclass's, which would override it, and
    // owners can underscore alike (`Foo_Bar`, `Foo::Bar`). The owner in
    // the name is only for reading.
    let mut counter = 0usize;
    for model in &mut app.models {
        let owner = crate::naming::underscore_words(model.name.0.as_str());
        let mut body = Vec::with_capacity(model.body.len());
        for item in std::mem::take(&mut model.body) {
            let expanded = match &item {
                ModelBodyItem::Unknown { expr, .. } => match &*expr.node {
                    ExprNode::Send { recv: None, method, .. } if method.as_str() == "validates_with" => {
                        let text = source_text(&sources, expr.span);
                        text.and_then(|t| {
                            counter += 1;
                            let name = format!("__validates_with_{owner}_{counter}");
                            generated_source(t, &name)
                                .and_then(|src| ingest_source(&src, &model.name))
                        })
                    }
                    _ => None,
                },
                _ => None,
            };
            match expanded {
                Some(items) => body.extend(items),
                None => body.push(item),
            }
        }
        model.body = body;
    }
}

fn source_text(sources: &[crate::span::SourceFile], span: crate::span::Span) -> Option<&str> {
    let file = sources.get((span.file.0 as usize).checked_sub(1)?)?;
    file.text.get(span.start as usize..span.end as usize)
}

/// The model source for one `validates_with` call's text, or None when
/// it isn't one validator class with keyword options.
fn generated_source(call_text: &str, name: &str) -> Option<String> {
    let result = ruby_prism::parse(call_text.as_bytes());
    let root = result.node();
    let call = root.as_program_node()?.statements().body().iter().next()?.as_call_node()?;
    let args: Vec<_> = call.arguments()?.arguments().iter().collect();
    let slice = |loc: ruby_prism::Location<'_>| -> Option<String> {
        call_text.get(loc.start_offset()..loc.end_offset()).map(str::to_string)
    };
    let (class, rest) = args.split_first()?;
    if class.as_constant_read_node().is_none() && class.as_constant_path_node().is_none() {
        return None;
    }
    let class = slice(class.location())?;
    let mut own = Vec::new();
    let mut conditions = Vec::new();
    match rest {
        [] => {}
        [options] => {
            let hash = options.as_keyword_hash_node()?;
            for el in hash.elements().iter() {
                let assoc = el.as_assoc_node()?;
                let key = assoc.key().as_symbol_node()?;
                let key_name = String::from_utf8_lossy(key.unescaped()).into_owned();
                let text = slice(assoc.location())?;
                if matches!(key_name.as_str(), "if" | "unless" | "on") {
                    conditions.push(text);
                } else {
                    own.push(text);
                }
            }
        }
        _ => return None,
    }
    let built = if own.is_empty() { format!("{class}.new") } else { format!("{class}.new({})", own.join(", ")) };
    let validate = std::iter::once(format!(":{name}")).chain(conditions).collect::<Vec<_>>().join(", ");
    Some(format!(
        "class GeneratedValidatesWith\n  def {name}\n    {built}.validate(self)\n  end\n\n  validate {validate}\nend\n"
    ))
}

/// The body items of `src`'s one class, ingested as model source under
/// its own isolated scope (as `delegate` forwarders are). None when it
/// doesn't ingest cleanly, or `validate` doesn't take its conditions.
pub(super) fn ingest_source(src: &str, owner: &crate::ident::ClassId) -> Option<Vec<ModelBodyItem>> {
    let (items, diags) = crate::ingest::prism::scope(|| -> Option<Vec<ModelBodyItem>> {
        let result = crate::ingest::prism::parse(src.as_bytes(), "<validates_with>");
        let root = result.node();
        let class = root.as_program_node()?.statements().body().iter().next()?.as_class_node()?;
        let body = class.body()?;
        let mut out = Vec::new();
        for stmt in super::util::flatten_statements(body) {
            out.extend(
                super::model::ingest_model_body_items(&stmt, owner, "<validates_with>", Vec::new(), None, &Default::default())
                    .ok()?,
            );
        }
        Some(out)
    });
    if !diags.is_empty() {
        return None;
    }
    let items = items?;
    // `validate` declined its conditions: it stayed an Unknown.
    if items.iter().any(|i| matches!(i, ModelBodyItem::Unknown { .. })) {
        return None;
    }
    Some(items)
}

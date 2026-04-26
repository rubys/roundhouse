//! Library-class ingestion for files under `app/models/` whose class
//! does not extend `ApplicationRecord` / `ActiveRecord::Base` — for
//! example `ArticleCommentsProxy` produced by has_many specialization.
//! The model ingest's table-name/columns/associations/validations
//! machinery doesn't apply; we just collect methods and `include`
//! directives.

use ruby_prism::parse;

use crate::dialect::LibraryClass;
use crate::expr::{Expr, ExprNode};
use crate::span::Span;
use crate::{ClassId, Symbol};

use super::expr::ingest_expr;
use super::util::{
    class_name_path, constant_id_str, constant_path_of, find_all_classes, find_first_class,
    flatten_statements,
};
use super::{IngestError, IngestResult};

pub fn ingest_library_class(
    source: &[u8],
    file: &str,
) -> IngestResult<Option<LibraryClass>> {
    let result = parse(source);
    let root = result.node();
    let Some(class) = find_first_class(&root) else {
        return Ok(None);
    };
    Ok(Some(library_class_from_node(&class, file)?))
}

/// Plural variant — returns one `LibraryClass` per class declaration in
/// the file (descending through modules and nested classes). Used by
/// the library-shape ingest path where a file like
/// `runtime/active_record/errors.rb` declares several classes side by
/// side inside one module.
pub fn ingest_library_classes(
    source: &[u8],
    file: &str,
) -> IngestResult<Vec<LibraryClass>> {
    let result = parse(source);
    let root = result.node();
    let mut out = Vec::new();
    for class in find_all_classes(&root) {
        out.push(library_class_from_node(&class, file)?);
    }
    Ok(out)
}

fn library_class_from_node(
    class: &ruby_prism::ClassNode<'_>,
    file: &str,
) -> IngestResult<LibraryClass> {
    let name_path = class_name_path(class).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "library class name must be a simple constant or path".into(),
    })?;
    let class_name = Symbol::from(name_path.join("::"));
    let owner = ClassId(class_name.clone());

    let parent = class.superclass().and_then(|n| {
        constant_path_of(&n).map(|p| ClassId(Symbol::from(p.join("::"))))
    });

    let mut includes: Vec<ClassId> = Vec::new();
    let mut methods: Vec<crate::dialect::MethodDef> = Vec::new();

    if let Some(class_body) = class.body() {
        for stmt in flatten_statements(class_body) {
            if let Some(def) = stmt.as_def_node() {
                methods.push(ingest_library_method(&def, &owner, file)?);
                continue;
            }
            if let Some(call) = stmt.as_call_node() {
                if call.receiver().is_none() && constant_id_str(&call.name()) == "include" {
                    if let Some(args) = call.arguments() {
                        for arg in args.arguments().iter() {
                            if let Some(path) = constant_path_of(&arg) {
                                includes
                                    .push(ClassId(Symbol::from(path.join("::"))));
                            }
                        }
                    }
                }
                // Other top-level calls (alias_method, etc.) — drop on
                // the floor for now; each emitter that cares can lift
                // these as needed.
            }
            // `attr_accessor`, etc. — not a model body, but a library
            // class might still use them. Defer until a fixture forces
            // it.
            //
            // Nested class declarations (`class Outer; class Inner;`)
            // also fall through here. They surface as separate
            // `LibraryClass` entries via the plural API above; the
            // singular path drops them.
        }
    }

    Ok(LibraryClass {
        name: owner,
        parent,
        includes,
        methods,
    })
}

fn ingest_library_method(
    def: &ruby_prism::DefNode<'_>,
    owner: &ClassId,
    file: &str,
) -> IngestResult<crate::dialect::MethodDef> {
    use crate::dialect::{MethodDef, MethodReceiver};

    let name = Symbol::from(constant_id_str(&def.name()));
    let receiver = if def.receiver().is_some() {
        MethodReceiver::Class
    } else {
        MethodReceiver::Instance
    };

    // Collect parameters across all kinds Ruby supports. Mirrors
    // runtime_src::method_params; the flat list loses the kind
    // distinction (re-derived from the def node when needed by emit).
    // Bodies under app/models/ legitimately use optionals (`attrs = {}`)
    // and keywords (`columns:`); the model ingest doesn't need them yet
    // but library classes do.
    let mut params: Vec<Symbol> = Vec::new();
    if let Some(pn) = def.parameters() {
        for req in pn.requireds().iter() {
            if let Some(rp) = req.as_required_parameter_node() {
                params.push(Symbol::from(constant_id_str(&rp.name())));
            }
        }
        for opt in pn.optionals().iter() {
            if let Some(op) = opt.as_optional_parameter_node() {
                params.push(Symbol::from(constant_id_str(&op.name())));
            }
        }
        if let Some(rest) = pn.rest() {
            if let Some(rp) = rest.as_rest_parameter_node() {
                if let Some(loc) = rp.name() {
                    if let Ok(s) = std::str::from_utf8(loc.as_slice()) {
                        params.push(Symbol::from(s));
                    }
                }
            }
        }
        for post in pn.posts().iter() {
            if let Some(pp) = post.as_required_parameter_node() {
                params.push(Symbol::from(constant_id_str(&pp.name())));
            }
        }
        for kw in pn.keywords().iter() {
            if let Some(rkp) = kw.as_required_keyword_parameter_node() {
                if let Ok(s) = std::str::from_utf8(rkp.name().as_slice()) {
                    params.push(Symbol::from(s));
                }
            } else if let Some(okp) = kw.as_optional_keyword_parameter_node() {
                if let Ok(s) = std::str::from_utf8(okp.name().as_slice()) {
                    params.push(Symbol::from(s));
                }
            }
        }
        if let Some(krest) = pn.keyword_rest() {
            if let Some(krp) = krest.as_keyword_rest_parameter_node() {
                if let Some(loc) = krp.name() {
                    if let Ok(s) = std::str::from_utf8(loc.as_slice()) {
                        params.push(Symbol::from(s));
                    }
                }
            }
        }
        if let Some(block) = pn.block() {
            if let Some(loc) = block.name() {
                if let Ok(s) = std::str::from_utf8(loc.as_slice()) {
                    params.push(Symbol::from(s));
                }
            }
        }
    }

    let body = match def.body() {
        Some(b) => ingest_expr(&b, file)?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };

    Ok(MethodDef {
        name,
        receiver,
        params,
        body,
        signature: None,
        effects: crate::effect::EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
    })
}

/// Quick classifier: does the file's first class extend
/// `ApplicationRecord` or `ActiveRecord::Base`? If yes the file is a
/// model; otherwise it's a library class. Files with no class at all
/// return `None`.
pub fn classify_class_file(source: &[u8]) -> Option<ClassKind> {
    let result = parse(source);
    let root = result.node();
    let class = find_first_class(&root)?;
    let parent_path = class
        .superclass()
        .and_then(|n| constant_path_of(&n))
        .map(|p| p.join("::"));

    Some(match parent_path.as_deref() {
        Some("ApplicationRecord") | Some("ActiveRecord::Base") => ClassKind::Model,
        _ => ClassKind::LibraryClass,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassKind {
    Model,
    LibraryClass,
}

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
    class_name_path, constant_id_str, constant_path_of, find_first_class, flatten_statements,
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

    let name_path = class_name_path(&class).ok_or_else(|| IngestError::Unsupported {
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
        }
    }

    Ok(Some(LibraryClass {
        name: owner,
        parent,
        includes,
        methods,
    }))
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

    // Required positional parameters only — same shape as the model
    // ingest. Richer kinds (optional, keyword, block) land when a
    // fixture drives the gap.
    let params: Vec<Symbol> = match def.parameters() {
        Some(pn) => pn
            .requireds()
            .iter()
            .filter_map(|req| req.as_required_parameter_node())
            .map(|rp| Symbol::from(constant_id_str(&rp.name())))
            .collect(),
        None => Vec::new(),
    };

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

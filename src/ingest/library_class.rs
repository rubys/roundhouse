//! Library-class ingestion for files under `app/models/` whose class
//! does not extend `ApplicationRecord` / `ActiveRecord::Base` — for
//! example `ArticleCommentsProxy` produced by has_many specialization.
//! The model ingest's table-name/columns/associations/validations
//! machinery doesn't apply; we just collect methods and `include`
//! directives.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use ruby_prism::{parse, Node};

use crate::dialect::{LibraryClass, MethodDef, MethodReceiver, Param};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::VarId;
use crate::span::Span;
use crate::{ClassId, Symbol};

use super::expr::{ingest_expr, ingest_expr_strict};
use super::visibility::{self, Visibility};
use super::util::{
    class_name_path, constant_id_str, constant_path_of, find_all_classes_with_scope,
    find_all_module_declarations_with_scope, find_all_modules_with_scope, find_first_class,
    flatten_statements, module_name_path, symbol_value,
};
use super::{IngestError, IngestResult};

/// Ingest the first class declaration, or return `None` if there is none.
/// Custom Data blocks require plural ingestion because their lifted methods
/// belong to additional library classes; reject them rather than lose that IR.
pub fn ingest_library_class(
    source: &[u8],
    file: &str,
) -> IngestResult<Option<LibraryClass>> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();
    let Some(class) = find_first_class(&root) else {
        return Ok(None);
    };
    if class.body().is_some_and(|body| flatten_statements(body).iter()
        .any(|statement| super::data_factory::declaration(statement).is_some()))
    {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "Data.define blocks require plural library-class ingestion".into(),
        });
    }
    Ok(Some(library_class_from_node(&class, file)?))
}

/// Plural variant — returns one `LibraryClass` per class declaration
/// AND per module-as-namespace (a module whose body contains direct
/// `def`s) in the file, descending through nested classes and modules.
/// Used by the library-shape ingest path where a file like
/// `runtime/active_record/errors.rb` declares several classes side by
/// side inside one module, or like `runtime/inflector.rb` declares a
/// module-with-self-methods.
///
/// Modules-as-namespaces are lowered to `LibraryClass` with `parent:
/// None` (per the YAGNI-on-round-trip decision: surface
/// module-vs-class distinction is sacrificed for downstream
/// uniformity, which is fine when callers only use the module as a
/// dotted-call namespace). Mixin modules (whose instance methods get
/// `include`d into a class) are NOT handled by this path yet.
pub fn ingest_library_classes(
    source: &[u8],
    file: &str,
) -> IngestResult<Vec<LibraryClass>> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();
    let mut out = Vec::new();
    for (scope, class) in find_all_classes_with_scope(&root) {
        let (mut lc, struct_base) = library_class_and_struct_base(&class, &scope, file)?;
        // BEFORE the class it serves: a superclass has to be defined
        // when the `class X < Y` line runs, and these two share a file.
        if let Some(base) = struct_base {
            out.push(base);
        }
        let owner = lc.name.clone();
        let structs = struct_constant_classes(&owner, class.body(), file)?;
        lc.constants.retain(|(name, _)| !structs.iter().any(|(s, _)| s == name));
        out.extend(super::data_factory::collect(class.body(), &owner, file)?);
        out.push(lc);
        out.extend(data_block_classes(class.body(), &owner, file)?);
        // Not before the owner: the struct is named under it, so the owner has to exist first.
        out.extend(structs.into_iter().map(|(_, s)| s));
    }
    for (scope, module) in find_all_modules_with_scope(&root) {
        // A nested `ClassMethods` is not a namespace of its own — it's
        // ActiveSupport::Concern's class-side carrier, already folded
        // into its parent as Class-receiver methods by `walk_decl_body`.
        // Emitting it separately would define every method twice.
        if !scope.is_empty()
            && module_name_path(&module).as_deref() == Some(&["ClassMethods".to_string()])
        {
            continue;
        }
        let mut lc = library_class_from_module_node_with_scope(&module, &scope, file)?;
        let owner = lc.name.clone();
        let structs = struct_constant_classes(&owner, module.body(), file)?;
        lc.constants.retain(|(name, _)| !structs.iter().any(|(s, _)| s == name));
        out.extend(super::data_factory::collect(module.body(), &owner, file)?);
        out.push(lc);
        out.extend(data_block_classes(module.body(), &owner, file)?);
        out.extend(structs.into_iter().map(|(_, s)| s));
    }
    // Constants written at FILE level, outside any class — lobsters'
    // `search_parser.rb` opens with `MYISAM_STOPWORDS = %w[…]` and the
    // parser's `rule(:stopword)` reads it. Ruby puts these on Object,
    // so every class in the file sees them; we hoist them into the
    // first class instead, ahead of its own constants.
    //
    // That is an approximation in exactly one direction: a file-level
    // constant read UNQUALIFIED from a different file would resolve
    // under Ruby and won't here. Reading it from inside the owning
    // class — including from inside a block in its body, which is what
    // a class-body DSL is — resolves either way, because a block
    // carries the lexical scope it was written in. The corpus has no
    // cross-file reader; a new one surfaces as a NameError naming the
    // constant, not as a silent wrong answer.
    if let Some(first) = out.first_mut() {
        let mut file_constants = file_level_constants(&root, file)?;
        if !file_constants.is_empty() {
            file_constants.extend(std::mem::take(&mut first.constants));
            first.constants = file_constants;
        }
    }
    Ok(out)
}

/// The block of `NAME = Data.define(:a, :b) do def … end end`, when that
/// is what `value` is: `Data` (or `::Data`) receiving `define` with
/// Symbol members only, a parameterless block, and a body containing
/// only method definitions and bare visibility markers. Any other block
/// stays with the constant's own ingest, where a `def` is not an expression.
pub(super) fn data_define_block<'pr>(value: &Node<'pr>) -> Option<ruby_prism::BlockNode<'pr>> {
    let call = value.as_call_node()?;
    if constant_id_str(&call.name()) != "define" {
        return None;
    }
    if constant_path_of(&call.receiver()?)?.iter().filter(|s| !s.is_empty()).ne(["Data"].iter().copied()) {
        return None;
    }
    if call
        .arguments()
        .is_some_and(|args| args.arguments().iter().any(|arg| arg.as_symbol_node().is_none()))
    {
        return None;
    }
    let block = call.block()?.as_block_node()?;
    if block.parameters().is_some() {
        return None;
    }
    let body = block.body()?;
    body.as_statements_node()?;
    flatten_statements(body)
        .iter()
        .all(|stmt| {
            stmt.as_def_node().is_some()
                || stmt.as_call_node().is_some_and(|call| {
                    matches!(constant_id_str(&call.name()), "public" | "protected" | "private")
                        && call.receiver().is_none()
                        && call.arguments().is_none()
                        && call.block().is_none()
                })
        })
        .then_some(block)
}

fn data_define_members(value: &Node<'_>) -> Option<Vec<Symbol>> {
    let call = value.as_call_node()?;
    let Some(arguments) = call.arguments() else {
        return Some(Vec::new());
    };
    arguments
        .arguments()
        .iter()
        .map(|arg| symbol_value(&arg).map(Symbol::from))
        .collect()
}

pub(super) fn data_factory_methods(
    owner: &ClassId,
    members: &[Symbol],
    mut methods: Vec<MethodDef>,
) -> Vec<MethodDef> {
    for member in members {
        if !methods.iter().any(|method| {
            method.receiver == MethodReceiver::Instance && method.name == *member
        }) {
            methods.push(synth_attr_reader(owner, member, MethodReceiver::Instance));
        }
    }
    if !methods.iter().any(|method| {
        method.receiver == MethodReceiver::Instance && method.name.as_str() == "initialize"
    }) {
        let params = members.iter().cloned().map(Param::positional).collect();
        let assigns = members
            .iter()
            .map(|member| {
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Assign {
                        target: LValue::Ivar {
                            name: member.clone(),
                        },
                        value: Expr::new(
                            Span::synthetic(),
                            ExprNode::Var {
                                id: VarId(0),
                                name: member.clone(),
                            },
                        ),
                    },
                )
            })
            .collect();
        methods.push(MethodDef {
            name_span: Span::synthetic(),
            name: Symbol::from("initialize"),
            receiver: MethodReceiver::Instance,
            visibility: crate::dialect::MethodVisibility::Private,
            params,
            unsupported_formals: None,
            has_anonymous_block: false,
            body: Expr::new(Span::synthetic(), ExprNode::Seq { exprs: assigns }),
            signature: None,
            effects: EffectSet::default(),
            enclosing_class: Some(owner.0.clone()),
            kind: crate::dialect::AccessorKind::Method,
            is_async: false,
            mutates_self: true,
            block_param: None,
        });
    }
    methods
}

/// `ContentKey = Data.define(:digest) do def cache_key = digest end` —
/// the block is `class_eval`ed on the new class, so its `def`s are that
/// class's methods, exactly as a later `class ContentKey; def …; end`
/// reopen would define them. Each such constant becomes a library class
/// of its own, `Owner::ContentKey`, carrying those methods; the constant
/// keeps the block-less factory (see `walk_decl_body`), and the Ruby
/// emitter renders the methods back into the block.
fn data_block_classes(
    body: Option<Node<'_>>,
    owner: &ClassId,
    file: &str,
) -> IngestResult<Vec<LibraryClass>> {
    let mut out = Vec::new();
    let Some(body) = body else { return Ok(out) };
    for stmt in flatten_statements(body) {
        let Some(cw) = stmt.as_constant_write_node() else { continue };
        let value = cw.value();
        let Some(block) = data_define_block(&value) else { continue };
        let Some(members) = data_define_members(&value) else { continue };
        // The custom factory pass retains these methods with Data-specific
        // origin metadata and validation. Do not create a second generic
        // class for the same declaration.
        if super::data_factory::declaration(&stmt).is_some() {
            continue;
        }
        let name = ClassId(Symbol::from(format!("{}::{}", owner.0.as_str(), constant_id_str(&cw.name()))));
        let DeclBody { includes, methods, constants, unknown_calls, class_initializers, class_attributes: _ } =
            walk_decl_body(block.body(), &name, file, DeclBodyMode::Instance)?;
        let methods = data_factory_methods(&name, &members, methods);
        debug_assert!(includes.is_empty() && constants.is_empty() && unknown_calls.is_empty());
        out.push(LibraryClass {
            name,
            is_module: false,
            parent: None,
            parent_span: Span::synthetic(),
            includes,
            methods,
            nullable_columns: Vec::new(),
            origin: Some(crate::dialect::LibraryClassOrigin::DataFactory {
                declaration_span: super::util::node_span(&value, file),
                members,
            }),
            constants,
            unknown_calls,
            class_ivar_initializers: class_initializers,
        });
    }
    Ok(out)
}

/// `NAME = <expr>` statements at the top level of a file, in source
/// order. Only direct program-body statements — anything inside a
/// class or module body is that declaration's own constant and is
/// collected by `walk_decl_body`.
fn file_level_constants(
    root: &ruby_prism::Node<'_>,
    file: &str,
) -> IngestResult<Vec<(Symbol, Expr)>> {
    let mut out = Vec::new();
    let Some(prog) = root.as_program_node() else {
        return Ok(out);
    };
    for stmt in prog.statements().body().iter() {
        let Some(cw) = stmt.as_constant_write_node() else { continue };
        if is_sorbet_type_alias(&cw.value()) {
            continue;
        }
        out.push((
            Symbol::from(constant_id_str(&cw.name())),
            ingest_expr(&cw.value(), file)?,
        ));
    }
    Ok(out)
}

pub(super) fn library_class_from_node(
    class: &ruby_prism::ClassNode<'_>,
    file: &str,
) -> IngestResult<LibraryClass> {
    library_class_from_node_with_scope(class, &[], file)
}

/// `class << Rails.application ... end` — the site-wide-settings idiom
/// in config/application.rb: config methods (`read_only?`, `name`,
/// `domain`) defined on the application *instance's* singleton at the
/// top level of the file, outside the Application class body. Returns
/// the def'd methods with Instance receivers — callers reach them as
/// `Rails.application.<m>`, so once the class is emitted as a
/// `Rails::Application` reopen they're plain instance methods (the
/// application object is a singleton, making instance-vs-singleton
/// definition indistinguishable to callers). Empty when the file has
/// no such block.
pub fn ingest_rails_application_singleton_methods(
    source: &[u8],
    file: &str,
) -> IngestResult<Vec<MethodDef>> {
    let result = super::prism::parse(source, file);
    let root = result.node();
    let owner = ClassId(Symbol::from("Rails::Application"));
    let mut out: Vec<MethodDef> = Vec::new();
    let Some(prog) = root.as_program_node() else {
        return Ok(out);
    };
    for stmt in prog.statements().body().iter() {
        let Some(sc) = stmt.as_singleton_class_node() else { continue };
        let Some(call) = sc.expression().as_call_node() else { continue };
        if constant_id_str(&call.name()) != "application" || call.arguments().is_some() {
            continue;
        }
        let Some(recv) = call.receiver() else { continue };
        let Some(path) = constant_path_of(&recv) else { continue };
        if path.join("::") != "Rails" {
            continue;
        }
        let body = walk_decl_body(sc.body(), &owner, file, DeclBodyMode::Instance)?;
        if !body.class_initializers.is_empty() {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "Rails application singleton class-variable initialization is not modeled".into(),
            });
        }
        out.extend(body.methods);
    }
    Ok(out)
}

/// Build a LibraryClass for a class declaration, prepending the
/// enclosing module path (`scope`) to the class's own constant-path
/// name. `module ActiveRecord; class Base` becomes `ClassId
/// ("ActiveRecord::Base")`. Top-level classes (empty scope) keep
/// their bare name. The fully-qualified ClassId aligns with the
/// shape RBS scope tracking now produces — body-typer registry
/// keys + RBS-derived `Ty::Class { id }` use the same path string.
pub(super) fn library_class_from_node_with_scope(
    class: &ruby_prism::ClassNode<'_>,
    scope: &[String],
    file: &str,
) -> IngestResult<LibraryClass> {
    library_class_and_struct_base(class, scope, file).map(|(lc, _)| lc)
}

/// The class, plus the synthesized base a `Struct.new(...)` superclass
/// expression turned into (`None` for every ordinary class).
pub(super) fn library_class_and_struct_base(
    class: &ruby_prism::ClassNode<'_>,
    scope: &[String],
    file: &str,
) -> IngestResult<(LibraryClass, Option<LibraryClass>)> {
    let name_path = class_name_path(class).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "library class name must be a simple constant or path".into(),
    })?;
    let mut full_path: Vec<String> = scope.to_vec();
    full_path.extend(name_path);
    let owner = ClassId(Symbol::from(full_path.join("::")));

    let source_parent_span = class
        .superclass()
        .map(|n| super::util::node_span(&n, file))
        .unwrap_or_default();
    let parent = class.superclass().and_then(|n| {
        constant_path_of(&n).map(|p| ClassId(Symbol::from(p.join("::"))))
    });
    // A superclass EXPRESSION — `class Image < Struct.new(:asset_path,
    // :width, :height)`. `constant_path_of` has no answer for a call
    // node, so the parent used to come back None and the class emitted
    // as a bare `class Image`: its `super(...)` reached
    // `BasicObject#initialize` and the app died at LOAD time with
    // "wrong number of arguments". See `struct_superclass_members`.
    let struct_members = if parent.is_none() {
        class.superclass().and_then(|n| struct_superclass_members(&n))
    } else {
        None
    };
    let parent = match &struct_members {
        Some(_) => Some(struct_base_id(&owner)),
        None => parent,
    };

    let DeclBody {
        mut includes,
        mut methods,
        mut constants,
        mut unknown_calls,
        mut class_initializers,
        class_attributes: _,
    } = walk_decl_body(class.body(), &owner, file, DeclBodyMode::Instance)?;

    // A `T::Struct` is a class GENERATOR, not an annotation: `const
    // :name, String` IS the constructor and the reader. Lower it into
    // the plain Ruby it stands for, so the emitted tree needs no
    // sorbet-runtime to build one.
    let parent = if parent.as_ref().is_some_and(is_sorbet_struct_parent) {
        let members = sorbet_struct_members(class.body(), file);
        let comparable = includes
            .iter()
            .any(|i| i.0.as_str() == "T::Struct::ActsAsComparable");
        // The declarations become methods, so they must not also be
        // emitted as calls; the sorbet mixins have nothing left to
        // provide.
        unknown_calls.retain(|call| !is_struct_declaration(call));
        includes.retain(|i| !i.0.as_str().starts_with("T::"));
        let mut synthesized = synth_sorbet_struct_methods(&owner, &members, comparable, true);
        synthesized.append(&mut methods);
        methods = synthesized;
        None
    } else if parent.as_ref().is_some_and(is_sorbet_enum_parent) {
        // The body walk already read the members out of `enums do` as
        // constants of this class, receiver spelled out, so they are
        // read from there rather than from the block a second time.
        // Each member is told the constant it is bound to. sorbet reads
        // that off the constant table when the `enums` block finishes;
        // here it is known at ingest, and `inspect` needs it.
        for (name, value) in constants.iter_mut() {
            if !is_enum_member(&owner, value) {
                continue;
            }
            if let ExprNode::Send { args, .. } = &mut *value.node {
                // `Desktop = new` — no serialized value given. sorbet
                // derives it from the constant name, and derives it by
                // DOWNCASING and nothing else: `PartiallyCompleted` is
                // "partiallycompleted", not "partially_completed"
                // (`const_to_serialized_val`, whose own comment says
                // the lowercase form is historical). Without this the
                // constant name landed in the value's slot and the
                // member was built with one argument where the
                // constructor takes two — an ArgumentError at LOAD
                // time, which takes the whole tree with it.
                if args.is_empty() {
                    args.push(str_lit(&name.as_str().to_lowercase()));
                }
                args.push(str_lit(name.as_str()));
            }
        }
        let members = sorbet_enum_members(&owner, &constants);
        unknown_calls.retain(|call| !is_enums_declaration(call));
        let mut synthesized = synth_sorbet_enum_methods(&owner, &members);
        synthesized.append(&mut methods);
        methods = synthesized;
        None
    } else {
        parent
    };
    let base = struct_members
        .as_ref()
        .map(|members| struct_base_class(&owner, members));
    class_initializers.extend(take_class_ivar_initializers(&mut unknown_calls));
    Ok((
        LibraryClass {
            name: owner,
            is_module: false,
            parent,
            // Struct.new bases replace the source superclass expression;
            // keep the source span only when the Const parent survived.
            parent_span: if struct_members.is_some() {
                Span::synthetic()
            } else {
                source_parent_span
            },
            includes,
            methods,
            nullable_columns: Vec::new(),
            origin: None,
            constants,
            unknown_calls,
            class_ivar_initializers: class_initializers,
        },
        base,
    ))
}

/// `NAME = T.type_alias { … }` — a constant holding a TYPE. Unlike
/// `T::Struct` and `T::Enum` it generates nothing and answers nothing:
/// the only thing that ever reads it is a `sig`, and those are read
/// before they are dropped. So the constant goes with them rather than
/// being carried into an emitted tree that has no sorbet-runtime to
/// build the type object with.
pub(super) fn is_sorbet_type_alias(value: &ruby_prism::Node<'_>) -> bool {
    let Some(call) = value.as_call_node() else { return false };
    match constant_id_str(&call.name()) {
        "type_alias" => call
            .receiver()
            .and_then(|r| r.as_constant_read_node())
            .is_some_and(|c| constant_id_str(&c.name()) == "T"),
        // `EventType = type_member { { upper: Event } }` — a type
        // PARAMETER, declared by a receiverless call `T::Generic`
        // provides. The annotations pass drops that `extend`, which is
        // right, and left this behind, which was not: the emitted
        // class called a method nothing defines and the tree stopped
        // loading there. A type parameter has no more runtime than the
        // alias above it, and the only thing that reads either is a
        // `sig`.
        "type_member" | "type_template" => call.receiver().is_none(),
        _ => false,
    }
}

/// `T::Enum` in superclass position. Like `T::Struct` it is a class
/// GENERATOR, not an annotation: `enums do Fill = new("fill") end`
/// declares members other code names and a serialization surface other
/// code calls, so the class is lowered into the plain Ruby it stands
/// for rather than dropped or carried.
fn is_sorbet_enum_parent(parent: &ClassId) -> bool {
    parent.0.as_str() == "T::Enum"
}

/// The `enums do … end` call in the collected class-body calls. The
/// members were read out of it as constants by the body walk, so
/// replaying the call would re-declare them against a `T::Enum` that
/// is not there.
fn is_enums_declaration(call: &Expr) -> bool {
    matches!(
        &*call.node,
        ExprNode::Send { recv: None, method, .. } if method.as_str() == "enums"
    )
}

/// One normalized member, shared by Sorbet's synthesized surface and
/// consumers such as Rails enum mapping ingestion. Constructor layout stays
/// here; consumers read the serialized expression, not positional arguments.
pub(super) struct SorbetEnumMember {
    pub(super) name: Symbol,
    pub(super) serialized: Expr,
}

pub(super) fn sorbet_enum_members(owner: &ClassId, constants: &[(Symbol, Expr)]) -> Vec<SorbetEnumMember> {
    constants.iter().filter(|(_, value)| is_enum_member(owner, value))
        .map(|(name, value)| {
            let ExprNode::Send { args, .. } = &*value.node else { unreachable!() };
            SorbetEnumMember {
                name: name.clone(),
                serialized: args.first().expect("enum constructors are normalized before projection").clone(),
            }
        }).collect()
}

/// The literal-only subset safe for Rails mapping expansion. Guard the
/// original source BEFORE annotation erasure, then read the same normalized
/// member projection that supplies Sorbet's emitted `values`/`serialize`.
pub(super) fn literal_sorbet_members(
    class: &ruby_prism::ClassNode<'_>,
    scope: &[String],
    file: &str,
) -> Option<Vec<(Symbol, Literal)>> {
    let base = class.superclass()?.as_constant_path_node()?;
    let namespace = base.parent()?;
    if constant_path_of(&base.as_node())?.join("::") != "T::Enum"
        || !(namespace.as_constant_read_node().is_some()
            || namespace.as_constant_path_node().is_some_and(|path| path.parent().is_none()))
    {
        return None;
    }
    let statements = flatten_statements(class.body()?);
    let [declaration] = statements.as_slice() else { return None };
    let call = declaration.as_call_node()?;
    if constant_id_str(&call.name()) != "enums"
        || call.receiver().is_some()
        || call.arguments().is_some()
    {
        return None;
    }
    let block = call.block()?.as_block_node()?;
    if block.parameters().is_some() {
        return None;
    }
    let statements = flatten_statements(block.body()?);
    if statements.is_empty() || statements.iter().any(|statement| {
        let Some(write) = statement.as_constant_write_node() else { return true };
        let Some(call) = write.value().as_call_node() else { return true };
        call.receiver().is_some() || call.block().is_some()
            || call.arguments().is_none_or(|args| {
                let arguments: Vec<_> = args.arguments().iter().collect();
                !matches!(arguments.as_slice(), [argument] if argument.as_string_node().is_some())
            })
    }) {
        return None;
    }
    let library = library_class_from_node_with_scope(class, scope, file).ok()?;
    let members = sorbet_enum_members(&library.name, &library.constants);
    if members.len() != statements.len() {
        return None;
    }
    let mut names = HashSet::new();
    let mut serialized = HashSet::new();
    members.into_iter().map(|member| {
        if !names.insert(member.name.clone()) {
            return None;
        }
        let ExprNode::Lit { value: Literal::Str { value } } = &*member.serialized.node else {
            return None;
        };
        if !serialized.insert(value.clone()) {
            return None;
        }
        Some((member.name, Literal::Str { value: value.clone() }))
    }).collect()
}

/// A class-body constant initialized by this class's own `new` — which
/// is what the body walk makes of a member inside `enums do`, receiver
/// spelled out. A `T::Enum` may hold other constants (an alias for a
/// member, a lookup table); those are not members and are left alone.
fn is_enum_member(owner: &ClassId, value: &Expr) -> bool {
    let ExprNode::Send { recv: Some(recv), method, .. } = &*value.node else { return false };
    if method.as_str() != "new" {
        return false;
    }
    let ExprNode::Const { path } = &*recv.node else { return false };
    path.iter().map(Symbol::as_str).collect::<Vec<_>>().join("::") == owner.0.as_str()
}

fn str_lit(value: &str) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: crate::expr::Literal::Str { value: value.to_string() } },
    )
}

fn call(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

fn local(name: &str) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: Symbol::from(name) })
}

/// The plain Ruby a `T::Enum` stands for. The surface is sorbet's own,
/// read off `T::Enum` rather than guessed: `serialize` answers the
/// value a member was built from, `values` lists the members,
/// `try_deserialize` maps a value back to the member that carries it
/// (nil when none does) and `from_serialized` / `deserialize` raise
/// `KeyError` there instead, `has_serialized?` asks without raising,
/// and `to_s` delegates to `inspect`.
///
/// `==` is deliberately absent: sorbet's is identity (`super`) outside
/// migration mode, and members are single instances bound to
/// constants, so the plain class already answers it the same way.
fn synth_sorbet_enum_methods(owner: &ClassId, members: &[SorbetEnumMember]) -> Vec<MethodDef> {
    let method = |name: &str, params: Vec<Param>, receiver: MethodReceiver, body: Expr| MethodDef {
        name_span: Span::synthetic(),
        name: Symbol::from(name),
        receiver,
        visibility: if name == "initialize" { crate::dialect::MethodVisibility::Private } else { crate::dialect::MethodVisibility::Public },
        params,
        unsupported_formals: None,
        has_anonymous_block: false,
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: name == "initialize",
        block_param: None,
    };
    let member_const = |name: &Symbol| {
        Expr::new(
            Span::synthetic(),
            ExprNode::Const {
                path: owner
                    .0
                    .as_str()
                    .split("::")
                    .map(Symbol::from)
                    .chain(std::iter::once(name.clone()))
                    .collect(),
            },
        )
    };
    let ivar = |name: &str| Expr::new(Span::synthetic(), ExprNode::Ivar { name: Symbol::from(name) });
    let assign = |name: &str, value: Expr| {
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign { target: LValue::Ivar { name: Symbol::from(name) }, value },
        )
    };
    // `find { |member| member.serialize == value }` over the members.
    let lookup = || {
        let matches = call(
            Some(call(Some(local("member")), "serialize", vec![])),
            "==",
            vec![local("value")],
        );
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(call(Some(self_class()), "values", vec![])),
                method: Symbol::from("find"),
                args: vec![],
                block: Some(block_of("member", matches)),
                parenthesized: false,
            },
        )
    };
    let value_param = || vec![Param::positional(Symbol::from("value"))];

    let mut methods = vec![
        method(
            "initialize",
            vec![
                Param::positional(Symbol::from("serialized")),
                Param::positional(Symbol::from("const_name")),
            ],
            MethodReceiver::Instance,
            Expr::new(
                Span::synthetic(),
                ExprNode::Seq {
                    exprs: vec![
                        assign("serialized_val", local("serialized")),
                        assign("const_name", local("const_name")),
                    ],
                },
            ),
        ),
        method("serialize", vec![], MethodReceiver::Instance, ivar("serialized_val")),
        // sorbet's `inspect` is `#<Enum::Member>`, and its `to_s`
        // delegates to it rather than to the serialized value — a
        // difference that would otherwise show up wherever an enum is
        // interpolated into a string, with nothing at the call site to
        // say it changed.
        method(
            "inspect",
            vec![],
            MethodReceiver::Instance,
            call(
                Some(call(
                    Some(str_lit(&format!("#<{}::", owner.0.as_str()))),
                    "+",
                    vec![ivar("const_name")],
                )),
                "+",
                vec![str_lit(">")],
            ),
        ),
        method(
            "to_s",
            vec![],
            MethodReceiver::Instance,
            call(Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)), "inspect", vec![]),
        ),
        method(
            "values",
            vec![],
            MethodReceiver::Class,
            Expr::new(
                Span::synthetic(),
                ExprNode::Array {
                    elements: members.iter().map(|m| member_const(&m.name)).collect(),
                    style: crate::expr::ArrayStyle::default(),
                },
            ),
        ),
        method("try_deserialize", value_param(), MethodReceiver::Class, lookup()),
        method(
            "has_serialized?",
            value_param(),
            MethodReceiver::Class,
            Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(call(Some(self_class()), "values", vec![])),
                    method: Symbol::from("any?"),
                    args: vec![],
                    block: Some(block_of(
                        "member",
                        call(
                            Some(call(Some(local("member")), "serialize", vec![])),
                            "==",
                            vec![local("value")],
                        ),
                    )),
                    parenthesized: false,
                },
            ),
        ),
    ];
    // `from_serialized` raises where `try_deserialize` answers nil, and
    // `deserialize` is sorbet's alias for it.
    let found = Expr::new(
        Span::synthetic(),
        ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::Or,
            surface: crate::expr::BoolOpSurface::default(),
            left: call(Some(self_class()), "try_deserialize", vec![local("value")]),
            right: call(
                None,
                "raise",
                vec![call(
                    Some(Expr::new(
                        Span::synthetic(),
                        ExprNode::Const { path: vec![Symbol::from("KeyError")] },
                    )),
                    "new",
                    // The offending value is in sorbet's message, and a
                    // KeyError without it is the one thing you need at
                    // the point it is raised.
                    vec![call(
                        Some(str_lit(&format!("Enum {} key not found: ", owner.0.as_str()))),
                        "+",
                        vec![call(Some(local("value")), "inspect", vec![])],
                    )],
                )],
            ),
        },
    );
    methods.push(method(
        "from_serialized",
        value_param(),
        MethodReceiver::Class,
        found,
    ));
    methods.push(method(
        "deserialize",
        value_param(),
        MethodReceiver::Class,
        call(Some(self_class()), "from_serialized", vec![local("value")]),
    ));
    methods
}

/// `{ |name| body }` attached to a call.
fn block_of(param: &str, body: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lambda { extra_params: Vec::new(),
            params: vec![Symbol::from(param)],
            rest_param: None,
            block_param: None,
            body,
            block_style: crate::expr::BlockStyle::Brace,
        },
    )
}


fn self_class() -> Expr {
    Expr::new(Span::synthetic(), ExprNode::SelfRef)
}

/// A `const` / `prop` call in the collected class-body calls, which
/// the struct lowering replaces with methods.
fn is_struct_declaration(call: &Expr) -> bool {
    matches!(
        &*call.node,
        ExprNode::Send { recv: None, method, .. }
            if matches!(method.as_str(), "const" | "prop")
    )
}

/// One `const` / `prop` in a `T::Struct` body.
struct SorbetStructMember {
    name: Symbol,
    /// `prop` is writable, `const` is not.
    writable: bool,
    /// The value an omitted keyword takes: `default:` verbatim, and a
    /// `factory: -> { … }`'s body, which Ruby evaluates per call just
    /// as sorbet evaluates the factory per instance.
    default: Option<Expr>,
}

/// `T::Struct` and its variants in superclass position. `T::Struct` is
/// a class GENERATOR, not an annotation: `const :name, String` is the
/// constructor and the reader, so the class is lowered into the plain
/// Ruby it stands for rather than dropped or carried.
fn is_sorbet_struct_parent(parent: &ClassId) -> bool {
    matches!(
        parent.0.as_str(),
        "T::Struct" | "T::ImmutableStruct" | "T::InexactStruct"
    )
}

/// The `const` / `prop` declarations in a class body, in source order —
/// which is the order the keyword constructor takes them in.
fn sorbet_struct_members(body: Option<ruby_prism::Node<'_>>, file: &str) -> Vec<SorbetStructMember> {
    let mut members = Vec::new();
    let Some(body) = body else { return members };
    for stmt in flatten_statements(body) {
        let Some(call) = stmt.as_call_node() else { continue };
        if call.receiver().is_some() {
            continue;
        }
        let method = call.name();
        let writable = match constant_id_str(&method) {
            "const" => false,
            "prop" => true,
            _ => continue,
        };
        let Some(arguments) = call.arguments() else { continue };
        let mut args = arguments.arguments().iter();
        let Some(name) = args.next().and_then(|n| {
            let symbol = n.as_symbol_node()?;
            Some(Symbol::from(
                String::from_utf8_lossy(symbol.value_loc()?.as_slice()).as_ref(),
            ))
        }) else {
            continue;
        };
        // The type is the second argument and belongs to the analyzer,
        // not to the emit: `ingest::sorbet_sig` has already read it.
        let _ty = args.next();
        let mut default = None;
        for arg in args {
            let Some(hash) = arg.as_keyword_hash_node() else { continue };
            for element in hash.elements().iter() {
                let Some(assoc) = element.as_assoc_node() else { continue };
                let Some(key) = symbol_value(&assoc.key()) else { continue };
                match key.as_str() {
                    "default" => default = ingest_expr(&assoc.value(), file).ok(),
                    "factory" => {
                        // `factory: -> { expr }` — the body is the
                        // default, evaluated per call.
                        default = assoc
                            .value()
                            .as_lambda_node()
                            .and_then(|l| l.body())
                            .and_then(|b| b.as_statements_node())
                            .and_then(|b| b.body().iter().next())
                            .and_then(|e| ingest_expr(&e, file).ok());
                    }
                    _ => {}
                }
            }
        }
        members.push(SorbetStructMember { name, writable, default });
    }
    members
}

/// The plain Ruby a `T::Struct` stands for: a reader per member, a
/// writer per `prop`, and the keyword constructor sorbet generates.
///
/// Keyword rather than positional, because that is the constructor
/// sorbet builds and therefore the one every call site in the app
/// already passes.
fn synth_sorbet_struct_methods(
    owner: &ClassId,
    members: &[SorbetStructMember],
    comparable: bool,
    with_constructor: bool,
) -> Vec<MethodDef> {
    let mut methods = Vec::new();
    for member in members {
        methods.push(synth_attr_reader(owner, &member.name, MethodReceiver::Instance));
        if member.writable {
            methods.push(synth_attr_writer(owner, &member.name, MethodReceiver::Instance));
        }
    }
    let params: Vec<Param> = members
        .iter()
        .map(|m| Param::keyword(m.name.clone(), m.default.clone()))
        .collect();
    let assigns: Vec<Expr> = members
        .iter()
        .map(|m| {
            Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: LValue::Ivar { name: m.name.clone() },
                    value: Expr::new(
                        Span::synthetic(),
                        ExprNode::Var { id: VarId(0), name: m.name.clone() },
                    ),
                },
            )
        })
        .collect();
    // A base that STAYS brings its own constructor. `T::Struct` is
    // lowered away, so the class would have none — but a gem base
    // whose ancestry reaches `T::Props` is still there at runtime and
    // `T::Props::Constructor` generates one from the same
    // declarations. Synthesizing ours on top overrides it, and a gem
    // that checks its subclasses' signatures rejects the class for
    // introducing required keywords its base does not declare.
    if !with_constructor {
        return methods;
    }
    methods.push(MethodDef {
        name_span: Span::synthetic(),
        name: Symbol::from("initialize"),
        receiver: MethodReceiver::Instance,
        visibility: crate::dialect::MethodVisibility::Private,
        params,
        unsupported_formals: None,
        has_anonymous_block: false,
        body: Expr::new(Span::synthetic(), ExprNode::Seq { exprs: assigns }),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    });
    if comparable {
        methods.push(synth_struct_equality(owner, members));
    }
    methods
}

/// `include T::Struct::ActsAsComparable` gives a struct value equality.
/// Dropping the include without this would leave object identity in its
/// place — the same expression answering differently, silently.
fn synth_struct_equality(owner: &ClassId, members: &[SorbetStructMember]) -> MethodDef {
    let other = || Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: Symbol::from("other") });
    let self_class = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
            method: Symbol::from("class"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let mut condition = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(other()),
            method: Symbol::from("is_a?"),
            args: vec![self_class],
            block: None,
            parenthesized: true,
        },
    );
    for member in members {
        let mine = Expr::new(Span::synthetic(), ExprNode::Ivar { name: member.name.clone() });
        let theirs = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(other()),
                method: member.name.clone(),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let same = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(mine),
                method: Symbol::from("=="),
                args: vec![theirs],
                block: None,
                parenthesized: false,
            },
        );
        condition = Expr::new(
            Span::synthetic(),
            ExprNode::BoolOp {
                op: crate::expr::BoolOpKind::And,
                surface: crate::expr::BoolOpSurface::default(),
                left: condition,
                right: same,
            },
        );
    }
    MethodDef {
        name_span: Span::synthetic(),
        name: Symbol::from("=="),
        receiver: MethodReceiver::Instance,
        visibility: crate::dialect::MethodVisibility::Public,
        params: vec![Param::positional(Symbol::from("other"))],
        unsupported_formals: None,
        has_anonymous_block: false,
        body: condition,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

/// `Struct.new(:a, :b, :c)` in SUPERCLASS position → its member names.
/// `None` for anything else, including the keyword-init form
/// (`Struct.new(:a, keyword_init: true)`) and a `Struct.new(...) do …
/// end` carrying a body: both mean something this synthesis does not
/// supply, and answering as though they were the positional form would
/// build a class with the wrong constructor.
fn struct_superclass_members(node: &ruby_prism::Node<'_>) -> Option<Vec<Symbol>> {
    let call = node.as_call_node()?;
    if call.name().as_slice() != b"new" || call.block().is_some() {
        return None;
    }
    let recv = call.receiver()?;
    if constant_path_of(&recv)? != vec!["Struct".to_string()] {
        return None;
    }
    let args = call.arguments()?;
    let mut members = Vec::new();
    for arg in args.arguments().iter() {
        // Symbol literals only — a `keyword_init:` keyword arrives as a
        // KeywordHashNode and lands here as a non-symbol, which is what
        // makes this a rejection rather than a silent drop.
        members.push(Symbol::from(symbol_value(&arg)?));
    }
    if members.is_empty() {
        return None;
    }
    Some(members)
}

/// The name given to the anonymous struct that stood in superclass
/// position: a SIBLING of the class it serves, not a nested constant.
/// `Sound::Image` gets `Sound::ImageStruct` — nesting it would put a
/// constant called `Struct` inside the class and shadow ::Struct for
/// every body in it.
fn struct_base_id(owner: &ClassId) -> ClassId {
    ClassId(Symbol::from(format!("{}Struct", owner.0.as_str())))
}

/// The class an anonymous `Struct.new(:a, :b)` becomes: a reader and a
/// writer per member, and a positional constructor that assigns them in
/// declaration order — which is what makes the subclass's `super(a, b)`
/// resolve.
///
/// WHAT IT IS NOT. `Struct` also gives `to_a`, `==`, `each`, `members`,
/// `[]` and `deconstruct`. None is reached in the corpus, and each is a
/// separate decision (`==` in particular is VALUE equality, which is
/// the whole reason a Ruby author reaches for Struct at all). They are
/// left out rather than approximated, so a call to one is a
/// NoMethodError naming the method instead of a wrong answer.
///
/// Every parameter defaults to nil, matching Struct: `Point.new(1)`
/// leaves `y` nil rather than raising.
fn struct_base_class(owner: &ClassId, members: &[Symbol]) -> LibraryClass {
    struct_class(
        struct_base_id(owner),
        members,
        false,
        crate::dialect::LibraryClassOrigin::StructSuperclass {
            owner: owner.0.clone(),
            members: members.to_vec(),
        },
    )
}

fn struct_class(
    base: ClassId,
    members: &[Symbol],
    keyword_init: bool,
    origin: crate::dialect::LibraryClassOrigin,
) -> LibraryClass {
    let mut methods = Vec::new();
    for m in members {
        methods.push(synth_attr_reader(&base, m, MethodReceiver::Instance));
        methods.push(synth_attr_writer(&base, m, MethodReceiver::Instance));
    }
    let params: Vec<Param> = members
        .iter()
        .map(|m| {
            let nil = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil });
            if keyword_init {
                Param::keyword(m.clone(), Some(nil))
            } else {
                Param::with_default(m.clone(), nil)
            }
        })
        .collect();
    let assigns: Vec<Expr> = members
        .iter()
        .map(|m| {
            Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: LValue::Ivar { name: m.clone() },
                    value: Expr::new(
                        Span::synthetic(),
                        ExprNode::Var { id: VarId(0), name: m.clone() },
                    ),
                },
            )
        })
        .collect();
    methods.push(MethodDef {
        name_span: crate::span::Span::synthetic(),
        name: Symbol::from("initialize"),
        receiver: MethodReceiver::Instance,
        visibility: crate::dialect::MethodVisibility::Private,
        params,
        unsupported_formals: None,
        has_anonymous_block: false,
        body: Expr::new(Span::synthetic(), ExprNode::Seq { exprs: assigns }),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(base.0.clone()),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    });
    LibraryClass {
        name: base,
        is_module: false,
        parent: None,
        parent_span: Span::synthetic(),
        includes: Vec::new(),
        methods,
        nullable_columns: Vec::new(),
        origin: Some(origin),
        constants: Vec::new(),
        unknown_calls: Vec::new(),
        class_ivar_initializers: Vec::new(),
    }
}

/// `Result = Struct.new(:a, :b, keyword_init: true) do … end` written
/// directly in a class or module body, read as the class it defines.
struct StructConstant<'pr> {
    name: Symbol,
    members: Vec<Symbol>,
    keyword_init: bool,
    body: Option<ruby_prism::Node<'pr>>,
}

fn struct_constants<'pr>(body: Option<ruby_prism::Node<'pr>>) -> Vec<StructConstant<'pr>> {
    let Some(statements) = body.as_ref().and_then(|b| b.as_statements_node()) else {
        return Vec::new();
    };
    statements
        .body()
        .iter()
        .filter_map(|stmt| {
            let write = stmt.as_constant_write_node()?;
            let spec = struct_constant_spec(&write.value())?;
            Some(StructConstant { name: Symbol::from(constant_id_str(&write.name())), ..spec })
        })
        .collect()
}

fn struct_constant_spec<'pr>(value: &ruby_prism::Node<'pr>) -> Option<StructConstant<'pr>> {
    let call = value.as_call_node()?;
    if call.name().as_slice() != b"new" || constant_path_of(&call.receiver()?)? != vec!["Struct".to_string()] {
        return None;
    }
    let mut members = Vec::new();
    let mut keyword_init = false;
    for arg in call.arguments()?.arguments().iter() {
        if let Some(options) = arg.as_keyword_hash_node() {
            let elements: Vec<_> = options.elements().iter().collect();
            let [pair] = elements.as_slice() else { return None };
            let pair = pair.as_assoc_node()?;
            if symbol_value(&pair.key()).as_deref() != Some("keyword_init") || pair.value().as_true_node().is_none() {
                return None;
            }
            keyword_init = true;
            continue;
        }
        let member = symbol_value(&arg)?;
        // Not `success?` / `valid!`: the member is also the ivar and the constructor's parameter, which cannot carry the suffix.
        if !member.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
            || !member.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            || crate::emit::ruby::expr::is_ruby_keyword(&member)
        {
            return None;
        }
        members.push(Symbol::from(member));
    }
    // Not the positional form: since Ruby 3.2 it takes keywords too, which a positional constructor would bind to its first member.
    if members.is_empty() || !keyword_init {
        return None;
    }
    let body = match call.block() {
        None => None,
        Some(block) => {
            let block = block.as_block_node()?;
            if block.parameters().is_some() {
                return None;
            }
            let body = block.body();
            // Not a general block body: only `def`s become the class's methods without running anything at definition time.
            if let Some(statements) = body.as_ref().and_then(|b| b.as_statements_node()) {
                if !statements.body().iter().all(|stmt| stmt.as_def_node().is_some()) {
                    return None;
                }
            } else if body.is_some() {
                return None;
            }
            body
        }
    };
    Some(StructConstant { name: Symbol::from(""), members, keyword_init, body })
}

/// The classes `owner`'s body defines through `X = Struct.new(…)`, with
/// the constant names they replace.
fn struct_constant_classes(
    owner: &ClassId,
    body: Option<ruby_prism::Node<'_>>,
    file: &str,
) -> IngestResult<Vec<(Symbol, LibraryClass)>> {
    let mut out = Vec::new();
    for spec in struct_constants(body) {
        let id = ClassId(Symbol::from(format!("{}::{}", owner.0.as_str(), spec.name.as_str())));
        let mut class = struct_class(
            id.clone(),
            &spec.members,
            spec.keyword_init,
            crate::dialect::LibraryClassOrigin::StructConstant {
                members: spec.members.clone(),
                keyword_init: spec.keyword_init,
            },
        );
        if spec.body.is_some() {
            class.methods.extend(walk_decl_body(spec.body, &id, file, DeclBodyMode::Instance)?.methods);
        }
        out.push((spec.name, class));
    }
    Ok(out)
}

/// Same as `library_class_from_node` but for module-as-namespace
/// declarations — modules whose body has at least one direct `def`,
/// surfaced via `find_all_modules`. Lowered to a `LibraryClass` with
/// `is_module: true` and `parent: None`. The `is_module` flag is
/// load-bearing: callers using `include` on the result need it to be
/// emitted as `module`, not `class`, or Ruby will raise TypeError.
pub(super) fn library_class_from_module_node_with_scope(
    module: &ruby_prism::ModuleNode<'_>,
    scope: &[String],
    file: &str,
) -> IngestResult<LibraryClass> {
    let name_path = module_name_path(module).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "library module name must be a simple constant or path".into(),
    })?;
    let mut full_path: Vec<String> = scope.to_vec();
    full_path.extend(name_path);
    let owner = ClassId(Symbol::from(full_path.join("::")));

    let visibility = Visibility::resolve(module.body().as_ref(), file, Some(&owner))?;
    let DeclBody {
        includes,
        methods,
        constants,
        mut unknown_calls,
        mut class_initializers,
        class_attributes: _,
    } = walk_decl_body_with_visibility(module.body(), &owner, file, DeclBodyMode::Instance, &visibility)?;
    class_initializers.extend(take_class_ivar_initializers(&mut unknown_calls));
    Ok(LibraryClass {
        name: owner,
        is_module: true,
        parent: None,
        parent_span: Span::synthetic(),
        includes,
        methods,
        nullable_columns: Vec::new(),
        origin: None,
        constants,
        unknown_calls,
        class_ivar_initializers: class_initializers,
    })
}

fn take_class_ivar_initializers(calls: &mut Vec<Expr>) -> Vec<Expr> {
    calls.extract_if(.., |expr| matches!(&*expr.node,
        ExprNode::Assign { target: LValue::Ivar { .. }, .. }
        | ExprNode::OpAssign { target: LValue::Ivar { .. }, .. }
    )).collect()
}

/// Walk a class or module body, collecting `include` directives and
/// method definitions (with `attr_*` lowered to synthesized methods).
/// Receiverless calls the walk doesn't recognize (`rule(:x) { … }`,
/// `alias_method`, …) are captured into `unknown_calls` rather than
/// dropped — see `LibraryClass::unknown_calls`. Nested class/module
/// declarations are still dropped; those surface separately via the
/// plural ingest entry points.
///
/// The declaration context determines which receivers/accessors are
/// synthesized and whether include-like calls affect the class side.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DeclBodyMode {
    Instance,
    ClassMethods,
    SingletonClass,
}

impl DeclBodyMode {
    fn is_class_side(self) -> bool {
        !matches!(self, Self::Instance)
    }
}
#[derive(Default)]
struct DeclBody {
    includes: Vec<ClassId>,
    methods: Vec<MethodDef>,
    constants: Vec<(Symbol, Expr)>,
    unknown_calls: Vec<Expr>,
    class_initializers: Vec<Expr>,
    /// `mattr_*` / `cattr_*` attribute names declared in this body —
    /// used when folding ClassMethods seeds onto the enclosing module.
    class_attributes: HashSet<Symbol>,
}

impl DeclBody {
    fn extend(&mut self, other: Self) {
        self.includes.extend(other.includes);
        self.methods.extend(other.methods);
        self.constants.extend(other.constants);
        self.unknown_calls.extend(other.unknown_calls);
        self.class_initializers.extend(other.class_initializers);
        self.class_attributes.extend(other.class_attributes);
    }

    fn finalize_classvars(
        &mut self,
        class_attributes: &HashSet<Symbol>,
        class_attr_defaults: &IndexMap<Symbol, Expr>,
        file: &str,
    ) -> IngestResult<()> {
        // Mixing an explicit `default:` / block with a source-spanned @@
        // initializer would drop or reorder the default — refuse.
        // Synthetic seeds alone are fine.
        if !class_attr_defaults.is_empty()
            && self
                .class_initializers
                .iter()
                .any(|expr| !expr.span.is_synthetic())
        {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "cattr/mattr defaults require source-order initialization".into(),
            });
        }
        // Rails mattr/cattr uses @@ shared across the hierarchy. Seed
        // `@@attr = <default>` when modeled, else `@@attr = nil` (matches
        // `class_variable_set` in Module#mattr_reader).
        let mut attrs: Vec<&Symbol> = class_attributes.iter().collect();
        attrs.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        for attr in attrs {
            let cvar = mattr_cvar_name(attr);
            let already = self.class_initializers.iter().any(|expr| {
                matches!(
                    &*expr.node,
                    ExprNode::Assign {
                        target: LValue::Var { name, .. },
                        ..
                    } if name == &cvar
                )
            });
            if !already {
                // Rails `mattr_reader`: non-nil defaults always set; nil
                // (including absent default) only when not already defined —
                // so a subclass redeclaration does not wipe the parent.
                let seed = class_attr_defaults
                    .get(attr)
                    .cloned()
                    .filter(|value| {
                        !matches!(&*value.node, ExprNode::Lit { value: Literal::Nil })
                    })
                    .map(|value| mattr_seed(attr, value))
                    .unwrap_or_else(|| mattr_nil_seed(attr));
                self.class_initializers.push(seed);
            }
        }
        // Source-spanned @@ initializers still need a body with no other
        // class-body buckets (ordering). Synthetic mattr seeds are
        // order-insensitive relative to includes/constants.
        let has_source_ordered_init = self
            .class_initializers
            .iter()
            .any(|expr| !expr.span.is_synthetic());
        if has_source_ordered_init
            && (!self.unknown_calls.is_empty()
                || !self.constants.is_empty()
                || !self.includes.is_empty())
        {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "native class-variable initialization alongside other class-body declarations requires source ordering".into(),
            });
        }
        Ok(())
    }
}

/// Synthetic mattr nil seeds, or source `@@attr = nil` for a declared
/// mattr/cattr attr — safe to fold from `module ClassMethods` onto the
/// enclosing module with the Class-receiver methods.
fn is_relocatable_mattr_seed(expr: &Expr, class_attributes: &HashSet<Symbol>) -> bool {
    if expr.span.is_synthetic() {
        return true;
    }
    matches!(
        &*expr.node,
        ExprNode::Assign {
            target: LValue::Var { name, .. },
            value,
        } if name.as_str().strip_prefix("@@").is_some_and(|bare| {
            class_attributes.iter().any(|attr| attr.as_str() == bare)
        }) && matches!(&*value.node, ExprNode::Lit { value: Literal::Nil })
    )
}

/// Receiverless class-body calls that are NOT safe to capture into
/// `unknown_calls`, because their meaning depends on where they sit
/// relative to the method definitions around them — and a
/// `LibraryClass` has no source-ordered body, so a captured call
/// replays ahead of every method. `private` replayed at the top of the
/// class body would make the whole class private rather than its tail.
///
/// `require` / `require_relative` are here for a different reason: the
/// emitted tree builds its own require graph (spinel's AOT stage
/// resolves it statically), so replaying a source require inside a
/// class body would point at a path that doesn't exist in the output.
/// `extend T::Sig` and its siblings — the mixins whose whole surface is
/// the annotations dropped above. `T::Sig` provides `sig`, `T::Helpers`
/// provides `abstract!`/`interface!`, `T::Generic` provides
/// `type_member`; with none of those left in the emit, the mixin is a
/// `NameError` waiting at load time and nothing more.
///
/// `include T::Struct::ActsAsComparable` is deliberately NOT here: it
/// gives a struct its `==`, which is behavior, and it goes when the
/// struct itself is lowered.
pub(super) fn is_sorbet_annotation_mixin(call: &ruby_prism::CallNode<'_>) -> bool {
    let name = call.name();
    if !matches!(constant_id_str(&name), "extend" | "include") {
        return false;
    }
    let Some(arguments) = call.arguments() else { return false };
    let args: Vec<_> = arguments.arguments().iter().collect();
    let [only] = args.as_slice() else { return false };
    let Some(path) = only.as_constant_path_node() else { return false };
    let written = constant_path_written(&path);
    matches!(written.as_str(), "T::Sig" | "T::Helpers" | "T::Generic")
}

/// `T::Sig` — the constant path as written.
fn constant_path_written(path: &ruby_prism::ConstantPathNode<'_>) -> String {
    let mut out = match path.parent() {
        Some(parent) => match parent.as_constant_read_node() {
            Some(read) => format!("{}::", constant_id_str(&read.name())),
            None => match parent.as_constant_path_node() {
                Some(inner) => format!("{}::", constant_path_written(&inner)),
                None => String::new(),
            },
        },
        None => String::new(),
    };
    if let Some(name) = path.name() {
        out.push_str(constant_id_str(&name));
    }
    out
}

/// sorbet-runtime's pure ANNOTATIONS: they carry types and nothing
/// else, and roundhouse has already read them (`ingest::sorbet_sig`)
/// by the time a class body is walked.
///
/// Dropped from the emit rather than round-tripped, because keeping
/// them would be the one thing this project exists not to do: a `sig`
/// makes sorbet-runtime wrap the method and re-check its types on
/// every call, which is per-request work whose answer cannot differ
/// between requests. The emitted tree carries no sorbet-runtime, so a
/// surviving `sig` is also a `NameError` waiting at load time.
///
/// `T::Struct` and `T::Enum` are NOT here: `const :name, String` is a
/// constructor and a reader, not an annotation, and deleting it would
/// leave a class that cannot be built. Those need lowering, not
/// dropping.
pub(super) const SORBET_ANNOTATIONS: &[&str] = &[
    "sig",
    "abstract!",
    "interface!",
    "final!",
    "sealed!",
    "type_parameters",
];

const POSITION_SENSITIVE_MARKERS: &[&str] = &[
    "private_constant",
    "public_constant",
    "require",
    "require_relative",
];

/// `def self.included(klass); class << klass; def foo; …; end; end; end`
/// — the vanilla-Ruby spelling of ActiveSupport::Concern's `class_methods
/// do … end` / `module ClassMethods` sugar (handled below in
/// [`walk_decl_body`] and mirrored in [`ingest_concern_class_method_spans`]).
/// Procore's shared search concerns (`app/concerns/search_engine/
/// {indexed,procore_search,tool_search,incrementally_backfillable}.rb`
/// and more) skip `ActiveSupport::Concern` entirely and open the
/// includer's singleton directly from the `Module#included` callback.
/// Semantically identical to Concern's `base.extend ClassMethods`: the
/// hook's own parameter IS the including class, so `class << klass`
/// reaches the same object. Without recognizing this shape, the
/// `SingletonClassNode` lands in the general expression walker (via
/// `ingest_library_method`'s body), which has no arm for a singleton
/// class opened on a local variable and hits the "unsupported
/// expression node" catch-all — failing the WHOLE file's ingest and
/// fanning out into thousands of downstream unresolved-type notes for
/// every reference into these widely-included concerns.
///
/// Deliberately narrow — only the `included` hook, only a single
/// required parameter and nothing else in the signature, and the
/// singleton must open exactly that parameter (not a differently-named
/// local or an ivar) — so this never mis-attributes unrelated method-
/// body metaprogramming as includer class methods (invariant 6).
pub(super) fn included_hook_class_methods_body<'pr>(
    def: &ruby_prism::DefNode<'pr>,
) -> Option<ruby_prism::Node<'pr>> {
    let param_name = included_hook_parameter(def)?;
    let stmts = flatten_statements(def.body()?);
    let [stmt] = &stmts[..] else { return None };
    let sc = stmt.as_singleton_class_node()?;
    let lv = sc.expression().as_local_variable_read_node()?;
    if constant_id_str(&lv.name()) != param_name {
        return None;
    }
    sc.body()
}

fn included_hook_parameter(def: &ruby_prism::DefNode<'_>) -> Option<String> {
    let receiver = def.receiver()?;
    receiver.as_self_node()?;
    if constant_id_str(&def.name()) != "included" {
        return None;
    }
    let params = def.parameters()?;
    if params.optionals().iter().next().is_some()
        || params.keywords().iter().next().is_some()
        || params.rest().is_some()
        || params.keyword_rest().is_some()
        || params.posts().iter().next().is_some()
        || params.block().is_some()
    {
        return None;
    }
    let mut requireds = params.requireds().iter();
    let only_param = requireds.next()?.as_required_parameter_node()?;
    if requireds.next().is_some() {
        return None;
    }
    Some(constant_id_str(&only_param.name()).to_string())
}

/// The complete vanilla-Ruby ClassMethods bridge. The carrier splice
/// replaces its only effect; retaining it would reference the nested
/// module that ingestion flattened away. Extra statements are NOT safe
/// to consume, nor is an extend of any other constant or receiver.
fn is_class_methods_bridge(def: &ruby_prism::DefNode<'_>) -> bool {
    let Some(param_name) = included_hook_parameter(def) else { return false };
    let Some(body) = def.body() else { return false };
    let stmts = flatten_statements(body);
    let [stmt] = &stmts[..] else { return false };
    let Some(call) = stmt.as_call_node() else { return false };
    if constant_id_str(&call.name()) != "extend" || call.block().is_some() {
        return false;
    }
    let Some(recv) = call.receiver().and_then(|r| r.as_local_variable_read_node()) else { return false };
    if constant_id_str(&recv.name()) != param_name {
        return false;
    }
    let Some(args) = call.arguments() else { return false };
    let args: Vec<_> = args.arguments().iter().collect();
    matches!(args.as_slice(), [arg] if arg.as_constant_read_node()
        .is_some_and(|c| constant_id_str(&c.name()) == "ClassMethods"))
}

/// `if enabled; attr_reader :token; end` is a declaration this walk
/// would lower when it stands alone. Inside a branch it sometimes runs,
/// which a synthesized method cannot express. Reject it rather than
/// drop the name. A modifier (`return x if x`) has no statement body,
/// so it is not this shape.
fn reject_conditional_accessor(node: &ruby_prism::Node<'_>, file: &str) -> IngestResult<()> {
    fn body_declares(body: Option<ruby_prism::Node<'_>>) -> bool {
        body.is_some_and(|body| {
            flatten_statements(body).iter().any(|stmt| {
                stmt.as_call_node().is_some_and(|call| {
                    call.receiver().is_none()
                        && matches!(
                            constant_id_str(&call.name()),
                            "attr_reader"
                                | "attr_writer"
                                | "attr_accessor"
                                | "cattr_reader"
                                | "cattr_writer"
                                | "cattr_accessor"
                                | "mattr_reader"
                                | "mattr_writer"
                                | "mattr_accessor"
                        )
                })
            })
        })
    }
    let declares = if let Some(branch) = node.as_if_node() {
        body_declares(branch.statements().map(|s| s.as_node()))
            || branch.subsequent().is_some_and(|sub| {
                sub.as_else_node()
                    .and_then(|e| e.statements())
                    .is_some_and(|s| body_declares(Some(s.as_node())))
                    || sub.as_if_node().is_some_and(|inner| {
                        body_declares(inner.statements().map(|s| s.as_node()))
                    })
            })
    } else if let Some(branch) = node.as_unless_node() {
        body_declares(branch.statements().map(|s| s.as_node()))
            || branch
                .else_clause()
                .and_then(|clause| clause.statements())
                .is_some_and(|s| body_declares(Some(s.as_node())))
    } else {
        false
    };
    if declares {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "conditional attr_reader, attr_writer, attr_accessor, cattr_*, or mattr_* \
                      is not a declaration this walk can keep"
                .into(),
        });
    }
    Ok(())
}

fn walk_decl_body<'pr>(
    body: Option<ruby_prism::Node<'pr>>,
    owner: &ClassId,
    file: &str,
    mode: DeclBodyMode,
) -> IngestResult<DeclBody> {
    let visibility = Visibility::resolve(body.as_ref(), file, None)?;
    walk_decl_body_with_visibility(body, owner, file, mode, &visibility)
}

/// Collect a declaration's methods, constants, and supported DSL expansions
/// using its resolved visibility. Custom Data constants retain only their call
/// heads here; the factory collector owns lifting and validating their blocks.
fn walk_decl_body_with_visibility<'pr>(
    body: Option<ruby_prism::Node<'pr>>,
    owner: &ClassId,
    file: &str,
    mode: DeclBodyMode,
    visibility: &Visibility,
) -> IngestResult<DeclBody> {
    let mut out = DeclBody::default();
    let mut class_attributes: HashSet<Symbol> = HashSet::new();
    let mut class_attr_defaults: IndexMap<Symbol, Expr> = IndexMap::new();
    // `module_function` (called bare inside a module body) marks every
    // subsequent direct `def` as a module-function — both an instance
    // method AND a class method. For our targets (which call these as
    // `Mod.x(...)`), we only need the class-method form, so flip the
    // receiver to Class. Doesn't affect nested `class`/`module` bodies
    // — they get their own walk_decl_body recursion.
    let mut module_function_active = false;
    // `extend self` exposes methods with their instance visibility; unlike
    // module_function, it neither makes a public copy nor ends at `private`.
    let mut extend_self_active = false;
    // Names from the `module_function :a, :b` form, plus the positions
    // of the direct `def`s in this body they may promote. Tracking
    // positions (rather than searching `methods` by name at the end)
    // keeps a `class << self` block's methods out of reach — those are
    // appended to the same vec but belong to a different scope.
    let mut module_function_named: Vec<String> = Vec::new();
    let mut direct_def_positions: Vec<usize> = Vec::new();

    let Some(b) = body else {
        return Ok(out);
    };

    let statements = flatten_statements(b);
    let has_class_methods = statements.iter().any(|stmt| stmt.as_module_node()
        .is_some_and(|m| module_name_path(&m).as_deref() == Some(&["ClassMethods".to_string()])));
    for statement in statements {
        // An `if` / `unless` around a `def`, a visibility marker, or an
        // accessor this walk would otherwise lower is not a statement it
        // can keep. Check the source statement, before a visibility
        // wrapper replaces it with its inner `def`. An accessor in the
        // branch has neither a `def` nor a marker, so reject it here:
        // skipping it would drop `attr_reader :token` with no error.
        if statement.as_if_node().is_some() || statement.as_unless_node().is_some() {
            Visibility::reject_conditional_declaration(&statement, file)?;
            reject_conditional_accessor(&statement, file)?;
            continue;
        }
        let definition = visibility::definition(&statement).map(|d| d.as_node());
        let stmt = definition.as_ref().unwrap_or(&statement);
        if stmt.as_def_node().is_none() && statement.as_call_node().is_some_and(|c| visibility::marker(&c)) {
            // Only bare instance-visibility markers end Ruby's module_function
            // mode. Named and inline forms don't change that lexical mode.
            let call = statement.as_call_node().unwrap();
            if call.arguments().is_none()
                && matches!(constant_id_str(&call.name()), "public" | "protected" | "private")
            {
                module_function_active = false;
            }
            continue;
        }
        // `enums do Fill = new("fill") end` — sorbet-runtime's `T::Enum`
        // declares its members inside a block, so the constants are one
        // level deeper than every other class-body constant. They are
        // constants of this class all the same, and reading them is what
        // lets `Mode::Fill` type as a Mode rather than as a class nobody
        // defined.
        if let Some(call) = stmt.as_call_node() {
            let name = call.name();
            if constant_id_str(&name) == "enums" && call.receiver().is_none() {
                if let Some(block) = call.block().and_then(|b| b.as_block_node()) {
                    if let Some(block_body) = block.body() {
                        for member in flatten_statements(block_body) {
                            let Some(cw) = member.as_constant_write_node() else { continue };
                            let name = Symbol::from(constant_id_str(&cw.name()));
                            let mut value = ingest_expr(&cw.value(), file)?;
                            // `Fill = new("fill")` — inside the class
                            // body, the receiverless `new` IS
                            // `Mode.new`, and spelling it out is what
                            // lets the constant registry type the
                            // member as an instance of its enum rather
                            // than leaving it untyped. (T::Enum makes
                            // the constructor private at run time; this
                            // is the ingest's reading of it, not an
                            // emitted call.)
                            if let ExprNode::Send { recv: recv @ None, method, .. } =
                                &mut *value.node
                            {
                                if method.as_str() == "new" {
                                    *recv = Some(Expr::new(
                                        crate::span::Span::synthetic(),
                                        ExprNode::Const {
                                            path: owner
                                                .0
                                                .as_str()
                                                .split("::")
                                                .map(Symbol::from)
                                                .collect(),
                                        },
                                    ));
                                }
                            }
                            out.constants.push((name, value));
                        }
                    }
                    continue;
                }
            }
        }
        // Class-level constant `NAME = <expr>` (e.g. `STORIES_PER_PAGE = 25`).
        if let Some(cw) = stmt.as_constant_write_node() {
            if is_sorbet_type_alias(&cw.value()) {
                continue;
            }
            let name = Symbol::from(constant_id_str(&cw.name()));
            let value = if let Some(call) = super::data_factory::declaration(&stmt) {
                super::data_factory::head(&call, file)?
            } else {
                ingest_expr(&cw.value(), file)?
            };
            out.constants.push((name, value));
            continue;
        }
        // A direct write initializes this class/module object. Inside
        // `class << self` the receiver is its singleton class instead.
        if !mode.is_class_side()
            && (stmt.as_instance_variable_write_node().is_some()
                || stmt.as_instance_variable_or_write_node().is_some()
                || stmt.as_instance_variable_and_write_node().is_some()
                || stmt.as_instance_variable_operator_write_node().is_some())
        {
            out.unknown_calls.push(ingest_expr(&stmt, file)?);
            continue;
        }
        // Retain native nil initialization in source order. Only a declared
        // cattr/mattr storage approximation may drop it, after the whole body
        // has been walked. Non-nil initializers remain outside this slice.
        if stmt.as_class_variable_write_node().is_some() {
            let initializer = ingest_expr(&stmt, file)?;
            if !matches!(&*initializer.node, ExprNode::Assign { value, .. }
                if matches!(&*value.node, ExprNode::Lit { value: Literal::Nil }))
            {
                return Err(IngestError::Unsupported {
                    file: file.into(),
                    message: "class-variable initializer with non-nil value".into(),
                });
            }
            out.class_initializers.push(initializer);
            continue;
        }
        if let Some(def) = stmt.as_def_node() {
            // `def self.included(klass); class << klass ... end; end` —
            // see `included_hook_class_methods_body`. Folds into the
            // same class-receiver-methods bucket as `class_methods do`
            // / `module ClassMethods`, and (like those) contributes no
            // `included` method of its own.
            if let Some(singleton_body) = included_hook_class_methods_body(&def) {
                out.extend(walk_decl_body_with_visibility(Some(singleton_body), owner, file, DeclBodyMode::ClassMethods, visibility)?);
                continue;
            }
            if has_class_methods && is_class_methods_bridge(&def) {
                continue;
            }
            let mut m = ingest_library_method(&def, owner, file)?;
            visibility.apply(&statement, &mut m);
            if module_function_active && m.receiver == MethodReceiver::Instance {
                // The retained singleton copy is public even if the original
                // instance definition is private/protected, unless
                // `private_class_method` / `public_class_method` already
                // recorded a class-side change at this def. extend self
                // shares the original method instead and must retain its
                // visibility.
                let class_visibility_changed = visibility.class_side_changed(m.name.as_str());
                if !class_visibility_changed {
                    m.visibility = crate::dialect::MethodVisibility::Public;
                }
            }
            if mode.is_class_side() || module_function_active || extend_self_active {
                m.receiver = MethodReceiver::Class;
            }
            // A real `def` replaces a synthesized attr_* half of the
            // same name (Ruby last-definition-wins for
            // `attr_accessor :x` then `def x; … end`). An earlier real
            // `def` is kept as duplicate evidence — `initialize` hooks
            // and visibility tests rely on both surviving ingest. Match
            // `push_user_methods`: only unsigned bare-ivar attr halves.
            if let Some(idx) = out
                .methods
                .iter()
                .position(|e| e.name == m.name && e.receiver == m.receiver)
            {
                let existing = &out.methods[idx];
                let existing_is_attr_half = existing.signature.is_none()
                    && match existing.kind {
                        crate::dialect::AccessorKind::AttributeReader => {
                            matches!(
                                &*existing.body.node,
                                ExprNode::Ivar { name } if name == &existing.name
                            )
                        }
                        crate::dialect::AccessorKind::AttributeWriter => {
                            let base = existing
                                .name
                                .as_str()
                                .strip_suffix('=')
                                .unwrap_or(existing.name.as_str());
                            matches!(
                                &*existing.body.node,
                                ExprNode::Assign {
                                    target: LValue::Ivar { name },
                                    ..
                                } if name.as_str() == base
                            )
                        }
                        crate::dialect::AccessorKind::Method => false,
                    };
                if existing_is_attr_half {
                    out.methods[idx] = m;
                    if !direct_def_positions.iter().any(|p| *p == idx) {
                        direct_def_positions.push(idx);
                    }
                } else {
                    // Duplicate real `def` — keep both.
                    direct_def_positions.push(out.methods.len());
                    out.methods.push(m);
                }
            } else {
                direct_def_positions.push(out.methods.len());
                out.methods.push(m);
            }
            continue;
        }
        // `class << self ... end` — singleton class block. Body
        // defines class-level methods on the enclosing scope.
        if let Some(sc) = stmt.as_singleton_class_node() {
            out.extend(walk_decl_body_with_visibility(sc.body(), owner, file, DeclBodyMode::SingletonClass, visibility)?);
            continue;
        }
        // `module ClassMethods … end` — ActiveSupport::Concern's OTHER
        // spelling for the class side, and the one campfire's
        // `User::Bot` uses for `create_bot!`/`authenticate_bot`.
        // `class_methods do` (below) is sugar that Concern turns into
        // exactly this module, so both have to arrive at the same
        // place: Class-receiver methods of the enclosing module, which
        // the registry's concern fold copies onto every includer.
        // `ingest_library_classes` skips the nested module for this
        // reason — otherwise the same defs would emit twice.
        if let Some(m) = stmt.as_module_node() {
            if module_name_path(&m).as_deref() == Some(&["ClassMethods".to_string()]) {
                let class_methods = walk_decl_body_with_visibility(m.body(), owner, file, DeclBodyMode::ClassMethods, visibility)?;
                // ClassMethods methods materialize on the enclosing module.
                // Mattr/cattr `@@attr = nil` seeds (synthetic or matching a
                // declared class attribute) relocate with them. Any other
                // native initializer stays owned by ClassMethods — refuse.
                if class_methods.class_initializers.iter().any(|expr| {
                    !is_relocatable_mattr_seed(expr, &class_methods.class_attributes)
                }) {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: "class-variable initialization in module ClassMethods is not modeled".into(),
                    });
                }
                out.extend(class_methods);
                continue;
            }
        }
        if let Some(alias) = stmt.as_alias_method_node() {
            let to = alias_keyword_name(&alias.new_name());
            let from = alias_keyword_name(&alias.old_name());
            let receiver = if mode.is_class_side() { MethodReceiver::Class } else { MethodReceiver::Instance };
            if let Some((to, from)) = to.zip(from) {
                if let Some(source) = out.methods.iter().rposition(|method| method.name.as_str() == from && method.receiver == receiver) {
                    let mut copy = out.methods[source].clone();
                    copy.name = Symbol::from(to.as_str());
                    copy.name_span = Span {
                        file: super::sources::file_id(file),
                        start: alias.location().start_offset() as u32,
                        end: alias.location().end_offset() as u32,
                    };
                    visibility.apply(&statement, &mut copy);
                    out.methods.push(copy);
                    continue;
                }
            }
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "alias names a method this body has not defined".into(),
            });
        }
        if let Some(call) = stmt.as_call_node() {
            // `include X` and `self.include(X)` are the same call: the
            // explicit-self spelling is how a file that must satisfy a
            // type checker writes it (`self #: as untyped` on the line
            // before `.include(Rails.application.routes.url_helpers)`),
            // and it means the body's own self, exactly as the bare form.
            if call.receiver().is_none_or(|r| r.as_self_node().is_some()) {
                let kw = constant_id_str(&call.name());
                // `class_methods do … end` — ActiveSupport::Concern's
                // class-side block: its defs become class methods of
                // every includer (`Account.find_local!`). Capture them
                // as Class-receiver methods of the module; the
                // registry's concern fold copies them onto includers.
                if kw == "class_methods" {
                    if let Some(block) = call.block().and_then(|blk| blk.as_block_node()) {
                        out.extend(walk_decl_body_with_visibility(block.body(), owner, file, DeclBodyMode::ClassMethods, visibility)?);
                        continue;
                    }
                }
                match kw {
                    "include" => {
                        if let Some(args) = call.arguments() {
                            let args: Vec<_> = args.arguments().iter().collect();
                            if mode == DeclBodyMode::SingletonClass
                                && args.iter().all(|arg| constant_path_of(arg).is_some())
                            {
                                let mut expr = ingest_expr(&stmt, file)?;
                                if let ExprNode::Send { method, .. } = &mut *expr.node {
                                    *method = Symbol::from("extend");
                                }
                                out.unknown_calls.push(expr);
                                continue;
                            }
                            // `include Resolvers.for(:product)`: a module
                            // computed at load time. Dropping it emitted
                            // the class without its mixin and told every
                            // reader the class had only its own methods.
                            // Kept as an unknown call: the Ruby family
                            // replays it, the rest see a class body they
                            // cannot model.
                            if args.iter().any(|arg| {
                                constant_path_of(&arg).is_none() && !crate::ingest::util::is_rails_url_helpers_chain(&arg)
                            }) {
                                if let Ok(e) = ingest_expr(&stmt, file) {
                                    out.unknown_calls.push(e);
                                }
                            }
                            for arg in args {
                                if let Some(path) = constant_path_of(&arg) {
                                    // lobsters' `TimeSeries` includes
                                    // `ActionView::Helpers::NumberHelper`
                                    // and calls one member, which emit
                                    // qualifies to `ActionView::
                                    // ViewHelpers.number_with_delimiter`
                                    // anyway. `ActiveModel::*` does NOT
                                    // drop here — see the predicate's
                                    // sibling.
                                    let segs: Vec<&str> =
                                        path.iter().map(|p| p.as_str()).collect();
                                    if crate::ingest::util::is_view_helper_marker_include(&segs) {
                                        continue;
                                    }
                                    out.includes.push(ClassId(Symbol::from(path.join("::"))));
                                } else if crate::ingest::util::is_rails_url_helpers_chain(&arg) {
                                    // `include Rails.application.routes.
                                    // url_helpers` (lobsters' Routes class,
                                    // inside `class << self`) — the whole
                                    // route-helper surface. Recorded as an
                                    // include of our generated RouteHelpers
                                    // module: the analyzer registers the
                                    // helper names off this marker and the
                                    // ruby emit rewrites `X.<helper>` call
                                    // sites through RouteHelpers.
                                    out.includes.push(ClassId(Symbol::from("RouteHelpers")));
                                }
                            }
                        }
                    }
                    "send" | "public_send"
                        if mode == DeclBodyMode::SingletonClass
                            && call.arguments().is_some_and(|args| {
                                args.arguments().iter().next().is_some_and(|arg| {
                                    symbol_value(&arg).as_deref() == Some("include")
                                })
                            }) =>
                    {
                        if let Ok(mut expr) = ingest_expr(&stmt, file) {
                            if let ExprNode::Send { args, .. } = &mut *expr.node {
                                if let Some(first) = args.first_mut() {
                                    first.node = Box::new(ExprNode::Lit {
                                        value: crate::expr::Literal::Sym { value: Symbol::from("extend") },
                                    });
                                }
                            }
                            out.unknown_calls.push(expr);
                        }
                    }
                    "attr_reader" | "attr_writer" | "attr_accessor"
                    | "cattr_reader" | "cattr_writer" | "cattr_accessor"
                    | "mattr_reader" | "mattr_writer" | "mattr_accessor" => {
                        // Lower to method definitions at ingest time
                        // (per the YAGNI-on-round-trip decision):
                        //   attr_reader :foo  → def foo; @foo; end
                        //   attr_writer :foo  → def foo=(v); @foo = v; end
                        //   attr_accessor :foo → both
                        // The `cattr_*` / `mattr_*` (ActiveSupport class- and
                        // module-level attribute accessors) use @@ storage
                        // shared across the class hierarchy. We model the
                        // class form (Rails also makes instance-level
                        // copies; models synthesize those separately).
                        let is_class_attr =
                            kw.starts_with("cattr_") || kw.starts_with("mattr_");
                        let mut names: Vec<Symbol> = Vec::new();
                        if let Some(args) = call.arguments() {
                            for arg in args.arguments().iter() {
                                if let Some(s) = symbol_value(&arg) {
                                    names.push(Symbol::from(s));
                                }
                            }
                        }
                        // Unmodeled mattr/cattr options (instance_*, splats,
                        // uningestible defaults) stay as unknown_calls — same
                        // honesty as model expand returning None — so a model
                        // file's dual library pass cannot abort ingest.
                        if is_class_attr {
                            match library_mattr_claim(&call, file)? {
                                LibraryMattrClaim::Unmodeled => {
                                    if let Ok(e) = ingest_expr(&stmt, file) {
                                        out.unknown_calls.push(e);
                                    }
                                    continue;
                                }
                                LibraryMattrClaim::Plain => {}
                                LibraryMattrClaim::Default(expr) => {
                                    for name in &names {
                                        class_attr_defaults.insert(name.clone(), expr.clone());
                                    }
                                }
                            }
                            class_attributes.extend(names.iter().cloned());
                            out.class_attributes.extend(names.iter().cloned());
                        }
                        let recv = if is_class_attr || mode.is_class_side() {
                            MethodReceiver::Class
                        } else {
                            MethodReceiver::Instance
                        };
                        // Keep generated accessors at their source declaration when
                        // initializers and methods are emitted in body order.
                        let name_span = Span {
                            file: super::sources::file_id(file),
                            start: call.location().start_offset() as u32,
                            end: call.location().end_offset() as u32,
                        };
                        for name in &names {
                            let want_reader = kw.ends_with("_reader") || kw.ends_with("_accessor");
                            let want_writer = kw.ends_with("_writer") || kw.ends_with("_accessor");
                            if want_reader {
                                let mut method = if is_class_attr {
                                    synth_mattr_reader(owner, name, recv)
                                } else {
                                    synth_attr_reader(owner, name, recv)
                                };
                                method.name_span = name_span;
                                visibility.apply(&statement, &mut method);
                                // Skip when a `def` of this name already
                                // walked (unusual order); a later `def`
                                // replaces via the push path above.
                                if !out.methods.iter().any(|e| {
                                    e.name == method.name && e.receiver == method.receiver
                                }) {
                                    out.methods.push(method);
                                }
                            }
                            if want_writer {
                                let mut method = if is_class_attr {
                                    synth_mattr_writer(owner, name, recv)
                                } else {
                                    synth_attr_writer(owner, name, recv)
                                };
                                method.name_span = name_span;
                                visibility.apply(&statement, &mut method);
                                if !out.methods.iter().any(|e| {
                                    e.name == method.name && e.receiver == method.receiver
                                }) {
                                    out.methods.push(method);
                                }
                            }
                        }
                    }
                    // `alias_method :eql?, :==` copies the method as it
                    // stands, so a copy of the def already walked is
                    // exact (a later redefinition of the original does
                    // not reach the alias in Ruby either). Shopify core
                    // has dozens; dropped, every call to the alias was a
                    // NoMethodError. One naming a method this body does
                    // not define (an inherited or gem method) is still
                    // captured below.
                    "alias_method"
                        if alias_source(&call, &out.methods, mode.is_class_side()).is_some() =>
                    {
                        let (to, source) =
                            alias_source(&call, &out.methods, mode.is_class_side()).unwrap();
                        let mut copy = out.methods[source].clone();
                        copy.name = Symbol::from(to.as_str());
                        copy.name_span = Span {
                            file: super::sources::file_id(file),
                            start: call.location().start_offset() as u32,
                            end: call.location().end_offset() as u32,
                        };
                        visibility.apply(&statement, &mut copy);
                        out.methods.push(copy);
                    }
                    // `extend self` — the spelling campfire's
                    // `RestrictedHTTP::PrivateNetworkGuard` uses. Ruby
                    // makes every instance method a singleton method
                    // too, so `PrivateNetworkGuard.resolve(host)` reaches
                    // the `def resolve` below it. Dropped, the module
                    // emitted its methods as instance-only and every
                    // dotted call was a NoMethodError — which is what
                    // left `Opengraph::Metadata.from_url` fetching
                    // nothing at all.
                    //
                    // Our targets retain only the class-method form, but
                    // unlike module_function, it keeps instance visibility.
                    "extend"
                        if call
                            .arguments()
                            .map(|a| {
                                let args: Vec<_> = a.arguments().iter().collect();
                                args.len() == 1 && args[0].as_self_node().is_some()
                            })
                            .unwrap_or(false) =>
                    {
                        extend_self_active = true;
                    }
                    "module_function" => {
                        // Bare `module_function` (no args) — flip the
                        // flag for every subsequent direct `def` in
                        // this body.
                        if call.arguments().is_none() {
                            module_function_active = true;
                        } else if let Some(names) =
                            crate::runtime_src::module_function_arg_names(&stmt)
                        {
                            // `module_function :foo, :bar` names its
                            // methods rather than flipping a mode. Ruby
                            // requires them to be defined already (it
                            // copies the existing definition), so the
                            // promotion is retroactive — recorded here
                            // and applied after the body walk, which
                            // also makes it order-independent.
                            module_function_named.extend(names);
                        }
                    }
                    _ => {
                        // A call this walk doesn't model. Capture it so
                        // the targets that CAN replay a class-body DSL
                        // do (`LibraryClass::unknown_calls`); the
                        // position-sensitive markers stay dropped
                        // because a capture would replay them in the
                        // wrong place.
                        //
                        // An expression we can't even ingest is left
                        // dropped rather than failing the whole class:
                        // the class still has its methods, and the emit
                        // side reports the gap. That keeps a single
                        // exotic call in one library class from taking
                        // the app's ingest down.
                        if !POSITION_SENSITIVE_MARKERS.contains(&kw)
                            && !SORBET_ANNOTATIONS.contains(&kw)
                            && !is_sorbet_annotation_mixin(&call)
                        {
                            if let Ok(e) = ingest_expr(&stmt, file) {
                                out.unknown_calls.push(e);
                            }
                        }
                    }
                }
            } else if call.receiver().is_some_and(|r| r.as_self_node().is_some()) {
                // `self.default_success = Success` — a class-level
                // attribute write. The whole branch above requires NO
                // receiver, so this never reached the capture and was
                // dropped without even the diagnostic a dropped
                // receiverless call gets.
                //
                // It matters for the same classes the replay rule
                // exists for: a base roundhouse does not model, whose
                // DSL IS the class body. A gem that validates its own
                // subclasses at load time (`must explicitly call
                // self.default_success = …`) rejects the emitted class
                // outright, so the tree stops there.
                if let Ok(e) = ingest_expr(&stmt, file) {
                    out.unknown_calls.push(e);
                }
            }
        }
        // Nested class/module declarations also fall through here; they
        // surface as separate entries via the plural API.
        // An `if` / `unless` that wraps a `def` or a visibility marker
        // is not one of those. Leaving it unrecorded dropped the method
        // with no diagnostic. A modifier (`return x if x`) has no `def`
        // in its body and stays an ordinary expression.
    }

    // `module_function :a, :b` promotions, applied before the classvar
    // finalization so a named method gets exactly what the bare
    // form's methods get (the flag there is set before the def is even
    // pushed, so it is already Class by this point).
    //
    // Ruby keeps BOTH copies — a module method plus a private instance
    // method — and lobsters uses both spellings of the same name
    // (`EmailBlocklistValidation.email_on_blocklist?` from a mailer and
    // a view; a bare `email_on_blocklist?(email)` from the sibling
    // validation method that runs on an includer instance). We emit one
    // method per name, so promoting alone would just move the breakage
    // from the module spelling to the instance one. Retarget the
    // sibling bare calls to the module spelling instead, which keeps a
    // single definition and leaves both call sites resolving.
    if !module_function_named.is_empty() {
        let mut promoted: Vec<Symbol> = Vec::new();
        for pos in &direct_def_positions {
            if module_function_named
                .iter()
                .any(|n| n == out.methods[*pos].name.as_str())
            {
                out.methods[*pos].receiver = MethodReceiver::Class;
                // Same rule as the bare marker: the copy starts public,
                // and a later `private_class_method :name` keeps the
                // visibility already recorded for that def.
                if !visibility.class_side_changed(out.methods[*pos].name.as_str()) {
                    out.methods[*pos].visibility = crate::dialect::MethodVisibility::Public;
                }
                promoted.push(out.methods[*pos].name.clone());
            }
        }
        if !promoted.is_empty() {
            for m in &mut out.methods {
                // The promoted method's own body is included: a
                // self-recursive call needs the same retarget.
                retarget_module_function_calls(&mut m.body, owner, &promoted);
            }
        }
    }

    out.finalize_classvars(&class_attributes, &class_attr_defaults, file)?;
    Ok(out)
}

/// Outcome of claiming a library-class `mattr_*` / `cattr_*` declaration.
enum LibraryMattrClaim {
    /// No `default:` / block — expand accessors and nil-seed.
    Plain,
    /// Representable `default:` / block — expand and seed that value.
    Default(Expr),
    /// Options we do not model (splats, `instance_*`, uningestible
    /// defaults). Leave the call in `unknown_calls` rather than nil-seed
    /// over a dropped value.
    Unmodeled,
}

/// Parse a library-class `mattr_*` / `cattr_*` `default:` / block.
fn library_mattr_claim(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
) -> IngestResult<LibraryMattrClaim> {
    let mut default: Option<Expr> = None;
    let mut other_kwargs = false;
    let mut unmodeled = false;
    if let Some(args) = call.arguments() {
        for arg in args.arguments().iter() {
            if symbol_value(&arg).is_some() {
                continue;
            }
            if let Some(hash) = arg.as_keyword_hash_node() {
                for element in hash.elements().iter() {
                    let Some(assoc) = element.as_assoc_node() else {
                        unmodeled = true;
                        break;
                    };
                    match symbol_value(&assoc.key()).as_deref() {
                        Some("default") if default.is_none() => {
                            // Strict top-level + nested survey recovery:
                            // recursive `ingest_expr` can still substitute
                            // nil under survey — reject if the collector grew.
                            let before = super::survey::recorded().len();
                            match ingest_expr_strict(&assoc.value(), file) {
                                Ok(expr)
                                    if !super::survey::is_active()
                                        || super::survey::recorded().len() == before =>
                                {
                                    default = Some(expr);
                                }
                                Ok(_) => unmodeled = true,
                                Err(err) => {
                                    if super::survey::is_active() {
                                        super::survey::record(&err);
                                    }
                                    unmodeled = true;
                                }
                            }
                        }
                        Some("default") => unmodeled = true,
                        // `instance_reader:` / friends are ignored when no
                        // default is claimed; mixed with `default:` they
                        // would silently drop half the declaration.
                        _ => other_kwargs = true,
                    }
                }
                continue;
            }
            unmodeled = true;
        }
    }
    if let Some(block) = call.block() {
        let Some(block_node) = block.as_block_node() else {
            return Ok(LibraryMattrClaim::Unmodeled);
        };
        if block_node.parameters().is_some() || default.is_some() {
            return Ok(LibraryMattrClaim::Unmodeled);
        }
        let Some(body) = block_node.body() else {
            return Ok(LibraryMattrClaim::Unmodeled);
        };
        let before = super::survey::recorded().len();
        match ingest_expr_strict(&body, file) {
            Ok(expr)
                if !super::survey::is_active()
                    || super::survey::recorded().len() == before =>
            {
                default = Some(expr);
            }
            Ok(_) => return Ok(LibraryMattrClaim::Unmodeled),
            Err(err) => {
                if super::survey::is_active() {
                    super::survey::record(&err);
                }
                return Ok(LibraryMattrClaim::Unmodeled);
            }
        }
    }
    if unmodeled || (default.is_some() && other_kwargs) {
        return Ok(LibraryMattrClaim::Unmodeled);
    }
    // Other kwargs without a default stay expanded (pre-existing library
    // surface); only a claimed default forces the unmodeled gate above.
    Ok(match default {
        Some(expr) => LibraryMattrClaim::Default(expr),
        None => LibraryMattrClaim::Plain,
    })
}

/// For `alias_method :new, :old`: the new name, and the index of the
/// last `old` already walked on the same side (instance, or class inside
/// `class << self`). None when either name is not a literal symbol or
/// the body has not defined `old`.
pub(super) fn alias_keyword_name(node: &ruby_prism::Node<'_>) -> Option<String> {
    if let Some(symbol) = symbol_value(node) {
        return Some(symbol);
    }
    node.as_call_node()
        .filter(|call| call.receiver().is_none() && call.arguments().is_none())
        .map(|call| constant_id_str(&call.name()).to_string())
}

pub(super) fn alias_source(
    call: &ruby_prism::CallNode<'_>,
    methods: &[MethodDef],
    class_side: bool,
) -> Option<(String, usize)> {
    let args: Vec<String> =
        call.arguments()?.arguments().iter().filter_map(|a| symbol_value(&a)).collect();
    let [to, from] = args.as_slice() else { return None };
    let receiver = if class_side { MethodReceiver::Class } else { MethodReceiver::Instance };
    let source =
        methods.iter().rposition(|m| m.name.as_str() == from.as_str() && m.receiver == receiver)?;
    Some((to.clone(), source))
}

/// Synthesize `def <name>; @<name>; end` (instance receiver) or
/// `def self.<name>; @<name>; end` (class receiver).
pub(crate) fn synth_attr_reader(owner: &ClassId, name: &Symbol, receiver: MethodReceiver) -> MethodDef {
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Ivar { name: name.clone() },
    );
    MethodDef {
        name_span: crate::span::Span::synthetic(),
        name: name.clone(),
        receiver,
        visibility: crate::dialect::MethodVisibility::Public,
        params: Vec::new(),
        unsupported_formals: None,
        has_anonymous_block: false,
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::AttributeReader,
        is_async: false,
            mutates_self: false,
            block_param: None,
    }
}

/// Class-variable name for a `mattr_*` / `cattr_*` attribute (`@@channel`).
pub(crate) fn mattr_cvar_name(attr: &Symbol) -> Symbol {
    Symbol::from(format!("@@{}", attr.as_str()))
}

/// Rails `mattr_*` / `cattr_*` seed: `@@attr = <value>`.
pub(crate) fn mattr_seed(attr: &Symbol, value: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var {
                id: VarId(0),
                name: mattr_cvar_name(attr),
            },
            value,
        },
    )
}

/// Rails `mattr_*` / `cattr_*` nil seed: `@@attr = nil` only when the
/// class variable is not already defined. Matches
/// `Module#mattr_reader`'s `class_variable_set` guard so a subclass
/// redeclaration does not wipe an inherited value.
///
/// Uses `defined?(@@attr)` (not bare `class_variable_defined?`) so the
/// class-body seed types cleanly without a Module-protocol receiver.
pub(crate) fn mattr_nil_seed(attr: &Symbol) -> Expr {
    let span = Span::synthetic();
    let cvar = mattr_cvar_name(attr);
    let assign = mattr_seed(
        attr,
        Expr::new(
            span,
            ExprNode::Lit {
                value: Literal::Nil,
            },
        ),
    );
    let defined = Expr::new(
        span,
        ExprNode::Defined {
            operand: Expr::new(
                span,
                ExprNode::Var {
                    id: VarId(0),
                    name: cvar,
                },
            ),
        },
    );
    Expr::new(
        span,
        ExprNode::If {
            cond: defined,
            then_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
            else_branch: assign,
        },
    )
}

/// `def self.<name>; @@<name>; end` — Rails mattr/cattr class reader
/// (shared across the inheritance hierarchy).
pub(crate) fn synth_mattr_reader(owner: &ClassId, name: &Symbol, receiver: MethodReceiver) -> MethodDef {
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Var {
            id: VarId(0),
            name: mattr_cvar_name(name),
        },
    );
    MethodDef {
        name_span: Span::synthetic(),
        name: name.clone(),
        receiver,
        visibility: crate::dialect::MethodVisibility::Public,
        params: Vec::new(),
        unsupported_formals: None,
        has_anonymous_block: false,
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::AttributeReader,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

/// `def self.<name>=(value); @@<name> = value; end` — Rails mattr/cattr
/// writer; subclass writes update the declaring class's value.
pub(crate) fn synth_mattr_writer(owner: &ClassId, name: &Symbol, receiver: MethodReceiver) -> MethodDef {
    let value_param = Symbol::from("value");
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var {
                id: VarId(0),
                name: mattr_cvar_name(name),
            },
            value: Expr::new(
                Span::synthetic(),
                ExprNode::Var {
                    id: VarId(0),
                    name: value_param.clone(),
                },
            ),
        },
    );
    MethodDef {
        name_span: Span::synthetic(),
        name: Symbol::from(format!("{}=", name.as_str())),
        receiver,
        visibility: crate::dialect::MethodVisibility::Public,
        params: vec![Param::positional(value_param)],
        unsupported_formals: None,
        has_anonymous_block: false,
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::AttributeWriter,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

/// Rewrite receiver-less calls to a `module_function`-promoted name
/// into `<Owner>.name(...)`.
///
/// Only bare sends match, so an explicit receiver (`other.foo`) is left
/// alone. A local variable shadowing the name would be a `Var` node,
/// not a `Send`, so it can't be caught here either.
fn retarget_module_function_calls(expr: &mut Expr, owner: &ClassId, promoted: &[Symbol]) {
    expr.node
        .for_each_child_mut(&mut |c| retarget_module_function_calls(c, owner, promoted));
    let ExprNode::Send { recv, method, .. } = &mut *expr.node else {
        return;
    };
    if recv.is_some() || !promoted.iter().any(|p| p == method) {
        return;
    }
    *recv = Some(Expr::new(
        expr.span,
        ExprNode::Const {
            path: vec![owner.0.clone()],
        },
    ));
}

/// Synthesize the writer pair for `attr_writer` / `attr_accessor`,
/// honoring the receiver (Instance vs Class).
pub(crate) fn synth_attr_writer(owner: &ClassId, name: &Symbol, receiver: MethodReceiver) -> MethodDef {
    let value_param = Symbol::from("value");
    let rhs = Expr::new(
        Span::synthetic(),
        ExprNode::Var {
            id: VarId(0),
            name: value_param.clone(),
        },
    );
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Ivar { name: name.clone() },
            value: rhs,
        },
    );
    let setter_name = Symbol::from(format!("{}=", name.as_str()));
    MethodDef {
        name_span: crate::span::Span::synthetic(),
        name: setter_name,
        receiver,
        visibility: crate::dialect::MethodVisibility::Public,
        params: vec![Param::positional(value_param)],
        unsupported_formals: None,
        has_anonymous_block: false,
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::AttributeWriter,
        is_async: false,
            mutates_self: false,
            block_param: None,
    }
}

/// Ingest a method with the ordinary library keyword-flattening policy.
/// Callers that need to retain the keyword contract use the explicit variant.
pub(super) fn ingest_library_method(
    def: &ruby_prism::DefNode<'_>,
    owner: &ClassId,
    file: &str,
) -> IngestResult<crate::dialect::MethodDef> {
    ingest_library_method_with_keywords(def, owner, file, false)
}

/// Ingest a library method, optionally retaining keyword parameters even where
/// ordinary library ingestion would flatten them. Data initializers require this
/// to preserve Ruby's keyword binding and forwarding through `super`.
pub(super) fn ingest_library_method_with_keywords(
    def: &ruby_prism::DefNode<'_>,
    owner: &ClassId,
    file: &str,
    preserve_keywords: bool,
) -> IngestResult<crate::dialect::MethodDef> {
    use crate::dialect::{MethodDef, MethodReceiver};

    let formals = super::forwarding::parse(def);
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
    let mut params: Vec<Param> = Vec::new();
    if let Some(pn) = def.parameters() {
        for req in pn.requireds().iter() {
            if let Some(rp) = req.as_required_parameter_node() {
                params.push(Param::positional(Symbol::from(constant_id_str(&rp.name()))));
            }
        }
        for opt in pn.optionals().iter() {
            if let Some(op) = opt.as_optional_parameter_node() {
                let name = Symbol::from(constant_id_str(&op.name()));
                // Capture the default Expr so per-target emit can
                // produce `name: T = <default>` signatures. Without
                // it, `def label(field, opts = {})` lowers to
                // `label(field, opts?: Record<...>)` and callers
                // omitting `opts` see `undefined`, breaking
                // downstream `Object.entries(opts)` /
                // `opts.merge(...)` chains in framework code.
                let default = ingest_expr(&op.value(), file)?;
                params.push(Param::with_default(name, default));
            }
        }
        if let Some(rest) = pn.rest() {
            if let Some(rp) = rest.as_rest_parameter_node() {
                if let Some(loc) = rp.name() {
                    if let Ok(s) = std::str::from_utf8(loc.as_slice()) {
                        params.push(Param::rest(Symbol::from(s)));
                    }
                } else if let Some(name) = &formals.anonymous_rest_name {
                    // An unforwarded `*`: see `forwarding`'s module doc.
                    params.push(Param::rest(name.clone()));
                }
            }
        }
        for post in pn.posts().iter() {
            if let Some(pp) = post.as_required_parameter_node() {
                params.push(Param::positional(Symbol::from(constant_id_str(&pp.name()))));
            }
        }
        // An optional keyword can only take the positional-with-default
        // approximation below when nothing else in the signature forces
        // Ruby's ordering rules. Two shapes do, and campfire has one of
        // each: a REQUIRED keyword beside it (`def initialize(name:,
        // text: nil)` → `(name:, text = nil)`, Sound::Image) and a rest
        // param before it (`def f(*messages, count: 1)` → `(*messages,
        // count = 1)`). Neither parses. Keep the keyword group honest
        // in those defs; elsewhere the approximation stands, because
        // the trailing-kwargs normalize path depends on it.
        //
        // A `**kwrest` BESIDE an optional keyword (`def f(x, style:
        // :time, **attributes)`) parses fine flattened, so it stays on
        // the approximation — but the two adjacent positionals are
        // indistinguishable to a caller forwarding a bundle, which
        // silently binds the keyword instead of the rest. That is why
        // both flattenings are MARKED below: `lower::kwrest_forward`
        // repairs the call, and the marks are the only record that
        // these slots were not positional in the source.
        let keeps_keywords = preserve_keywords || params.iter().any(|p| p.rest)
            // Nameless `**` must keep the adjacent keyword group too:
            // a flattened optional would otherwise bind its default
            // while the keyword disappears into this rest slot.
            || formals.anonymous == Some(super::forwarding::AnonymousFormal::KeywordRest)
            || pn
                .keywords()
                .iter()
                .any(|kw| kw.as_required_keyword_parameter_node().is_some())
            // A keyword named after a Ruby reserved word (`next: nil`)
            // cannot be flattened: `def f(next = nil)` does not parse,
            // while `def f(next: nil)` does.
            || pn.keywords().iter().any(|kw| {
                let Some(okp) = kw.as_optional_keyword_parameter_node() else { return false };
                let name = okp.name();
                std::str::from_utf8(name.as_slice())
                    .is_ok_and(|s| is_ruby_reserved_word(s.trim_end_matches(':')))
            });
        for kw in pn.keywords().iter() {
            if let Some(rkp) = kw.as_required_keyword_parameter_node() {
                if let Ok(s) = std::str::from_utf8(rkp.name().as_slice()) {
                    // Marked keyword so passes that cannot forward
                    // kwargs positionally (mailer/job class-side
                    // wrappers) see the truth and ledger instead of
                    // synthesizing a mis-binding wrapper. (The
                    // optional-keyword branch below deliberately stays
                    // positional-with-default — the trailing-kwargs
                    // normalize path depends on that shape.)
                    params.push(Param::keyword(
                        Symbol::from(s.trim_end_matches(':')),
                        None,
                    ));
                }
            } else if let Some(okp) = kw.as_optional_keyword_parameter_node() {
                if let Ok(s) = std::str::from_utf8(okp.name().as_slice()) {
                    // Capture the default Expr so emit can produce
                    // `status: T = :found` rather than `status?: T`
                    // (which binds undefined when the caller omits
                    // the kwarg). action_controller/base.rb's
                    // `redirect_to(path, notice: nil, alert: nil,
                    // status: :found)` is the load-bearing case —
                    // without the default, every redirect loses
                    // its 302 status and the test client sees 200.
                    let default = ingest_expr(&okp.value(), file)?;
                    params.push(if keeps_keywords {
                        Param::keyword(Symbol::from(s), Some(default))
                    } else {
                        // Flattened to a positional-with-default, and
                        // MARKED as flattened: the emitted shape says
                        // "an optional positional you may fill", which
                        // the original Ruby did not offer. A caller that
                        // appears to fill it is an erased `**`, and
                        // `lower::kwrest_forward` needs that fact.
                        let mut p = Param::with_default(Symbol::from(s), default);
                        p.from_keyword = true;
                        p
                    });
                }
            }
        }
        if let Some(krest) = pn.keyword_rest() {
            if let Some(krp) = krest.as_keyword_rest_parameter_node() {
                if let Some(loc) = krp.name() {
                    if let Ok(s) = std::str::from_utf8(loc.as_slice()) {
                        // `**options` is OPTIONAL in Ruby — it binds to
                        // `{}` when the caller passes no keywords — and
                        // the trailing positional it becomes here has to
                        // say so, or every bare call is an ArgumentError.
                        // campfire's `avatar_tag(user, **options)` is
                        // called with one argument from the message row,
                        // the user list and the sidebar.
                        // `target(**params)` forwards. `skip_before_action :name,
                        // **options` consumes the hash as filter options and
                        // must keep the flattened positional binding.
                        let body_forwards_rest = def.body().is_some_and(|body| {
                            let text = String::from_utf8_lossy(body.location().as_slice());
                            text.contains(&format!("(**{s})")) || text.contains(&format!(", **{s})"))
                        });
                        if keeps_keywords || body_forwards_rest {
                            // The keyword group is kept in this def, so
                            // `**rest` stays a keyword-rest: flattened to
                            // `rest = {}` after a `name:` it does not parse
                            // and dropping it beside `*args` changes the
                            // rest array even when the keyword-rest is unread.
                            let mut p = Param::keyword(Symbol::from(s), None);
                            p.rest = true;
                            params.push(p);
                        } else {
                            let mut p = Param::with_default(
                                Symbol::from(s),
                                Expr::new(
                                    Span::synthetic(),
                                    ExprNode::Hash { entries: vec![], kwargs: false },
                                ),
                            );
                            p.from_kwrest = true;
                            params.push(p);
                        }
                    }
                }
            }
        }
    }

    // `&block` rides in `MethodDef.block_param`, not the flat list —
    // it occupies the call-site `block:` slot, never `args:`. Mirrors
    // the runtime_src split (see runtime_src::method_params).
    let block_param = def
        .parameters()
        .and_then(|pn| pn.block())
        .map(|block| {
            let name = block
                .name()
                .and_then(|loc| std::str::from_utf8(loc.as_slice()).ok())
                // Ruby 3.4 anonymous block param (`def f(&)`) — synthesize a
                // name so body-side bare-`&` forwarding (`__blk`) binds.
                .unwrap_or("__blk");
            Param::positional(Symbol::from(name))
        });

    let body = match def.body() {
        Some(b) => ingest_expr(&b, file)?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };

    params.extend(formals.anonymous.map(super::forwarding::AnonymousFormal::into_param));

    Ok(MethodDef {
        name_span: super::util::def_name_span(def, file),
        name,
        receiver,
        visibility: crate::dialect::MethodVisibility::Public,
        params,
        unsupported_formals: formals.unsupported,
        has_anonymous_block: formals.has_anonymous_block,
        body,
        signature: None,
        effects: crate::effect::EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        // Source-defined `def` lands as Method by default; ingest
        // for `attr_*` calls sets AttributeReader/Writer above. A
        // future refinement could pattern-match on body shape
        // (zero-arg `@ivar` body → AttributeReader) for source code
        // that didn't use the attr_* sugar.
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param,
    })
}

/// Quick classifier: does the file's first class extend
/// `ApplicationRecord` or `ActiveRecord::Base`? If yes the file is a
/// model; otherwise it's a library class. Files with no class at all
/// return `None`.
/// `self.abstract_class = true` or `primary_abstract_class` in a class
/// body.
///
/// Both spellings: the marker is what Rails 7 added, and the
/// assignment is what its own multiple-database guide still writes.
fn declares_abstract_class(class: &ruby_prism::ClassNode<'_>) -> bool {
    let Some(body) = class.body() else { return false };
    let Some(stmts) = body.as_statements_node() else { return false };
    stmts.body().iter().any(|stmt| {
        let Some(call) = stmt.as_call_node() else { return false };
        let name = constant_id_str(&call.name());
        if call.receiver().is_none() {
            return name == "primary_abstract_class";
        }
        if call.receiver().and_then(|r| r.as_self_node()).is_none() || name != "abstract_class=" {
            return false;
        }
        call.arguments()
            .and_then(|a| a.arguments().iter().next())
            .is_some_and(|arg| arg.as_true_node().is_some())
    })
}

/// The classes an app declares as ActiveRecord bases, resolved
/// transitively.
///
/// `classify_class_file` matched a superclass against two literals, so
/// a model descending through the app's OWN abstract base — the shape
/// Rails' multiple-database guide prescribes, and what an engine or a
/// packwerk package gets by default — was ingested as a plain library
/// class and lost its associations, validations and scopes.
///
/// Seeded with the two names, then closed over the `class X < Y` pairs
/// the pre-pass collects, so a chain of any depth resolves.
#[derive(Debug, Default, Clone)]
pub struct ModelBases {
    names: std::collections::HashSet<String>,
}

impl ModelBases {
    pub fn new() -> Self {
        let mut names = std::collections::HashSet::new();
        names.insert("ApplicationRecord".to_string());
        names.insert("ActiveRecord::Base".to_string());
        // Rails' Action Text abstract base (`ActionText::Record <
        // ActiveRecord::Base; self.abstract_class = true`). The gem
        // file is not ingested, but Writebook's
        // `lib/rails_ext/action_text_markdown.rb` subclasses the
        // lexical bare `Record` under `module ActionText`. Seeding the
        // qualified name lets `has_active_record_base` + lexical
        // resolution classify that class as a model rather than a
        // library class that emits `class Markdown < Record`.
        names.insert("ActionText::Record".to_string());
        Self { names }
    }

    /// Is `name` (possibly after lexical qualification) an AR base?
    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// Superclass name written into emitted model IR. Gem abstract bases
    /// that are seeded for classification but not ingested (today:
    /// `ActionText::Record`) parent as `ApplicationRecord`, matching
    /// RichText synthesis — callers must not special-case the name.
    pub fn emit_superclass(&self, resolved: &str) -> String {
        if resolved == "ActionText::Record" {
            "ApplicationRecord".to_string()
        } else {
            resolved.to_string()
        }
    }

    /// Resolve a superclass path against enclosing modules the way Ruby
    /// constant lookup walks `module_parents`: bare `Record` under
    /// `module ActionText` becomes `ActionText::Record` when that base
    /// is known. Qualified paths are unchanged. Falls back to the
    /// lexical spelling when no enclosing candidate is a known base.
    pub fn resolve_superclass(&self, scope: &[String], parent_path: &[String]) -> String {
        let joined = parent_path.join("::");
        // Bare names: search enclosing scopes first (Ruby constant
        // lookup). A global `ApplicationRecord` base must not win over
        // a closer `Foo::ApplicationRecord` when both are known.
        if parent_path.len() == 1 {
            let bare = &parent_path[0];
            let mut segs = scope.to_vec();
            while !segs.is_empty() {
                let candidate = format!("{}::{}", segs.join("::"), bare);
                if self.contains(&candidate) {
                    return candidate;
                }
                segs.pop();
            }
        }
        if self.contains(&joined) {
            return joined;
        }
        joined
    }

    /// One file's `class X < Y` pairs, for the closure below — but
    /// only for classes that declare themselves ABSTRACT.
    ///
    /// Closing over every model would reclassify a single-table
    /// inheritance subclass (`class Rooms::Open < Room`) as a model,
    /// which it is in Rails but not in this ingest: STI is handled
    /// elsewhere, and the library-class path is what feeds it. A
    /// concrete parent with a table of its own means STI; an abstract
    /// one means a base chain. The full suite caught the difference
    /// after targeted tests missed it.
    pub fn record(&mut self, source: &[u8], pairs: &mut Vec<(String, String)>) {
        let result = parse(source);
        let root = result.node();
        for (scope, class) in find_all_classes_with_scope(&root) {
            let Some(path) = class_name_path(&class) else { continue };
            let mut full = scope.clone();
            full.extend(path);
            let Some(parent) = class.superclass().and_then(|n| constant_path_of(&n)) else {
                continue;
            };
            if !declares_abstract_class(&class) {
                continue;
            }
            // Resolve bare parents (`Record` under `module ActionText`)
            // before close_over, which matches on the stored parent
            // spelling against seeded qualified bases.
            let parent = self.resolve_superclass(&scope, &parent);
            pairs.push((full.join("::"), parent));
        }
    }

    /// Close the set: anything whose parent is already a base is one.
    /// Iterated rather than recursive because the pairs arrive in file
    /// order, and a base can be declared after its user.
    ///
    /// `record` stores a bare parent (`MidBase`) when that name is not
    /// yet a known base. After a later iteration inserts the qualified
    /// form (`ActionText::MidBase`), match the stored spelling against
    /// the child's enclosing modules the same way `resolve_superclass`
    /// does at record time.
    pub fn close_over(&mut self, pairs: &[(String, String)]) {
        loop {
            let before = self.names.len();
            for (child, parent) in pairs {
                if self.parent_is_known_base(child, parent) {
                    self.names.insert(child.clone());
                }
            }
            if self.names.len() == before {
                break;
            }
        }
    }

    fn parent_is_known_base(&self, child: &str, parent: &str) -> bool {
        if self.names.contains(parent) {
            return true;
        }
        if parent.contains("::") {
            return false;
        }
        let mut segs: Vec<&str> = child.split("::").collect();
        if segs.len() < 2 {
            return false;
        }
        segs.pop();
        while !segs.is_empty() {
            let candidate = format!("{}::{}", segs.join("::"), parent);
            if self.names.contains(&candidate) {
                return true;
            }
            segs.pop();
        }
        false
    }

}

/// Does this file's first class descend from an ActiveRecord base?
///
/// NARROWER than `classify_class_file`, deliberately. That also
/// answers `Model` for a superclass-less class that includes
/// `ActiveModel::Model` — a TABLELESS model, which outside
/// `app/models` is left as a library class on purpose: a reopen of a
/// framework class from `lib/` includes things the emit handles its
/// own way, and routing it to the model path breaks that.
///
/// So the rule outside `app/models` is ancestry to ActiveRecord, and
/// nothing else. Lexical superclass resolution applies: bare `Record`
/// under `module ActionText` matches the seeded `ActionText::Record`
/// base (Writebook Markdown).
pub fn has_active_record_base(source: &[u8], bases: &ModelBases) -> bool {
    let result = parse(source);
    let root = result.node();
    let Some((scope, class)) = find_all_classes_with_scope(&root).into_iter().next() else {
        return false;
    };
    class
        .superclass()
        .and_then(|n| constant_path_of(&n))
        .is_some_and(|p| bases.contains(&bases.resolve_superclass(&scope, &p)))
}

pub fn classify_class_file(source: &[u8], bases: &ModelBases) -> Option<ClassKind> {
    let result = parse(source);
    let root = result.node();
    let Some((scope, class)) = find_all_classes_with_scope(&root).into_iter().next() else {
        // No class node. A bare top-level module under app/models/
        // (`module InactiveUser; def self.x; …; end`) is a namespace of
        // singleton methods, not a model — classify it as a library
        // class so the module-aware (plural) ingest registers its
        // `def self.x` as dotted-call class methods. (`find_first_class`
        // already descends modules, so a model nested in a namespace
        // module — `module Admin; class User < ApplicationRecord` — is
        // still found above and classified Model.)
        if !find_all_modules_with_scope(&root).is_empty() {
            return Some(ClassKind::LibraryClass);
        }
        return None;
    };
    let parent_path = class
        .superclass()
        .and_then(|n| constant_path_of(&n))
        .map(|p| bases.resolve_superclass(&scope, &p));

    Some(match parent_path.as_deref() {
        // Resolved through the app's own bases, not against two
        // literals: `Gauge < PkgRecord < ApplicationRecord` is a model.
        Some(p) if bases.contains(p) => ClassKind::Model,
        // A superclass-less class that `include`s the ActiveModel
        // validation surface (lobsters' Search) is a tableless model:
        // the model path lowers its `validates` DSL and synthesizes
        // `valid?`/`errors`; the library-class path would drop them.
        None if includes_active_model(&class) => ClassKind::Model,
        _ => ClassKind::LibraryClass,
    })
}

fn includes_active_model(class: &ruby_prism::ClassNode<'_>) -> bool {
    let Some(body) = class.body() else { return false };
    let Some(stmts) = body.as_statements_node() else { return false };
    stmts.body().iter().any(|stmt| {
        let Some(call) = stmt.as_call_node() else { return false };
        if call.receiver().is_some() || call.name().as_slice() != b"include" {
            return false;
        }
        call.arguments().is_some_and(|args| {
            args.arguments().iter().any(|arg| {
                constant_path_of(&arg).is_some_and(|path| {
                    path.join("::") == "ActiveModel::Validations"
                        || path.join("::") == "ActiveModel::Model"
                })
            })
        })
    })
}

/// Is `parent` one of Rails' own subclassable bases that the runtime
/// does NOT port? A subclass of one is app code the analyzer can read
/// — `check` still types a mailbox's `process` — but no emitted tree
/// can load it: there is no `ActionMailbox` to name, so the file
/// raises `NameError` at require time and, through the `app/models.rb`
/// aggregator, takes the whole app down with it. That is what
/// lobsters' `ApplicationMailbox < ActionMailbox::Base` did the day
/// `app/mailboxes/` became a support root: the tree emitted, then
/// would not boot. `lower::unported_rails_subclasses` drops the class
/// after analysis, out loud.
///
/// Spelled out, and Rails' own only. The ported bases — the ones
/// `runtime/ruby/` defines and the ruby emitter's
/// `require_path_for_parent` anchors — are simply not on it. A gem's
/// base is not on it either, even one parked under a Rails namespace
/// (`ActiveModel::Serializer`, Mastodon's 160 serializers): that is
/// the gem-DSL path, where the class body replays for the gem to run
/// on the ruby lane. An unlisted Rails base is the old behavior, a
/// replay that fails at load; add it here when it turns up.
pub fn is_unported_rails_base(parent: &str) -> bool {
    matches!(
        parent,
        "ActionMailbox::Base"
            | "ActiveJob::Serializers::ObjectSerializer"
            | "ActiveModel::Validator"
            | "ActiveModel::EachValidator"
            | "ActiveRecord::Migration"
            | "Rails::Generators::Base"
            | "Rails::Generators::NamedBase"
            | "Rails::Railtie"
            | "Rails::Engine"
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassKind {
    Model,
    LibraryClass,
}

/// Filters declared inside a concern module's `included do` block:
/// `module AccountOwnedConcern … included do before_action :set_account,
/// … end end` → `(AccountOwnedConcern, [Filter(set_account), …])`.
/// Rails evaluates that block in each including class, so these filters
/// belong to every includer — analyze consumes the returned pairs (via
/// `App::concern_filters`) to extend each including controller's filter
/// chain. Modules without an `included do`, and `included do` statements
/// that aren't filter calls, contribute nothing here (the module's
/// method defs are captured separately by [`ingest_library_classes`]).
/// Per module, the names its ActiveSupport::Concern CLASS-SIDE CARRIER
/// declares — `module ClassMethods … end` or `class_methods do … end`.
///
/// `walk_decl_body` flattens both into the parent module as
/// Class-receiver methods, which is right for resolution and loses the
/// one fact an includer needs: whether a given class-side method is
/// inherited. Concern's `append_features` runs `base.extend
/// ClassMethods` and nothing else, so ONLY these cross. A module's own
/// singletons — `module_function :x`, `class << self` — are also
/// Class-receiver methods after the flatten and are NOT inherited.
///
/// Without the distinction, the model concern splice invented
/// `User.email_on_blocklist?` on three lobsters models, from
/// `EmailBlocklistValidation`'s `module_function :email_on_blocklist?`.
///
/// Read with its own parse, like `ingest_concern_filters` and
/// `ingest_concern_model_items` beside it, rather than widening
/// `DeclBody` — the class IR reaches 25 `LibraryClass` construction
/// sites, nearly all of them synthesizing classes that can never have a
/// concern carrier.
/// Every `helper_method :name, …` the file declares.
///
/// Rails' `helper_method` is the app SAYING which controller methods a
/// view may call — campfire's `SetPlatform` exposes `platform`,
/// `Authentication` exposes `signed_in?`, `TrackedRoomVisit` exposes
/// `last_room_visited`, and the room page calls all three. Our views
/// lower to module functions with no controller instance, so a bare
/// `platform` there resolved to nothing and the page died on a
/// NameError.
///
/// A whole-file VISIT rather than a per-module statement scan: the
/// declaration is spelled the same in a controller class body, inside a
/// concern's `included do`, and inside `class_methods do`, and what the
/// call-site rewrite wants is one NAME SET. Rails scopes the exposure to
/// the declaring controller and its descendants; a name is only routed
/// where a view actually calls it, and that rewrite is already shadowed
/// by the module's own methods and its params.
pub fn ingest_helper_method_names(source: &[u8]) -> Vec<Symbol> {
    struct HelperMethodVisitor {
        names: Vec<Symbol>,
    }

    impl<'pr> ruby_prism::Visit<'pr> for HelperMethodVisitor {
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            if node.receiver().is_none() && constant_id_str(&node.name()) == "helper_method" {
                if let Some(args) = node.arguments() {
                    for arg in args.arguments().iter() {
                        if let Some(sym) = arg.as_symbol_node() {
                            self.names.push(Symbol::from(
                                String::from_utf8_lossy(sym.unescaped()).as_ref(),
                            ));
                        }
                    }
                }
            }
            ruby_prism::visit_call_node(self, node);
        }
    }

    let result = parse(source);
    let mut visitor = HelperMethodVisitor { names: Vec::new() };
    ruby_prism::Visit::visit(&mut visitor, &result.node());
    visitor.names
}

/// Definition identities, not just names: a module singleton with the
/// same name as a carrier method is a separate, non-inherited method.
/// Keep bridge identities and actual nested-carrier declarations too,
/// so a bridge can be consumed across reopenings without treating an
/// arbitrary class-side block as proof that `ClassMethods` exists.
pub struct ConcernClassMethodSpans {
    pub owner: ClassId,
    pub methods: Vec<Span>,
    pub bridges: Vec<Span>,
    pub has_nested_carrier: bool,
    /// Literal framework identity and calls that require it to be installed.
    /// Finite configuration uses these; factory bridge splicing is unchanged.
    pub concern_extensions: Vec<Span>,
    pub concern_calls: Vec<Span>,
    pub has_other_extensions: bool,
}

pub fn ingest_concern_class_method_spans(
    source: &[u8],
    file: &str,
) -> (Vec<ConcernClassMethodSpans>, HashSet<ClassId>) {
    fn defs_in(body: Option<ruby_prism::Node<'_>>, file: &str, out: &mut Vec<Span>) {
        let Some(body) = body else { return };
        for stmt in flatten_statements(body) {
            if let Some(def) = visibility::definition(&stmt) {
                // The carrier's instance definitions become includer
                // class methods. Its own singletons do not cross.
                if def.receiver().is_none() {
                    out.push(super::util::def_name_span(&def, file));
                }
            }
        }
    }

    let result = parse(source);
    let root = result.node();
    let mut out = Vec::new();
    for (scope, module) in find_all_module_declarations_with_scope(&root) {
        let Some(name_path) = module_name_path(&module) else { continue };
        // A nested `ClassMethods` is reported under its PARENT, which is
        // the module an app actually includes.
        if name_path.as_slice() == ["ClassMethods".to_string()] && !scope.is_empty() {
            continue;
        }
        let mut full_path: Vec<String> = scope.clone();
        full_path.extend(name_path);
        let id = ClassId(Symbol::from(full_path.join("::")));

        let Some(body) = module.body() else { continue };
        let mut spans: Vec<Span> = Vec::new();
        let mut bridges: Vec<Span> = Vec::new();
        let mut has_nested_carrier = false;
        let mut concern_extensions = Vec::new();
        let mut concern_calls = Vec::new();
        let mut has_other_extensions = false;
        for stmt in flatten_statements(body) {
            if let Some(m) = stmt.as_module_node() {
                if module_name_path(&m).as_deref() == Some(&["ClassMethods".to_string()]) {
                    has_nested_carrier = true;
                    defs_in(m.body(), file, &mut spans);
                }
                continue;
            }
            if let Some(call) = stmt.as_call_node() {
                if call.receiver().is_none() {
                    let loc = call.location();
                    let span = Span {
                        file: super::sources::file_id(file),
                        start: loc.start_offset() as u32,
                        end: loc.end_offset() as u32,
                    };
                    match constant_id_str(&call.name()) {
                        "extend" => {
                            if call.block().is_none() && call.arguments().is_some_and(|args| {
                                args.arguments().len() == 1 && args.arguments().iter().any(|arg| {
                                    constant_path_of(&arg).is_some_and(|p| p.join("::") == "ActiveSupport::Concern")
                                })
                            }) {
                                concern_extensions.push(span);
                            } else {
                                has_other_extensions = true;
                            }
                        }
                        "class_methods" | "include" | "prepend" => concern_calls.push(span),
                        _ => {}
                    }
                }
                if call.receiver().is_none()
                    && constant_id_str(&call.name()) == "class_methods"
                {
                    if let Some(block) = call.block().and_then(|b| b.as_block_node()) {
                        defs_in(block.body(), file, &mut spans);
                    }
                }
            }
            // `def self.included(klass); class << klass ... end; end` —
            // see `included_hook_class_methods_body`'s doc comment. The
            // third spelling of Concern's class-side carrier; needs its
            // own arm here (this is a from-scratch parse, deliberately
            // not sharing `walk_decl_body`'s result — see the doc comment
            // above this function) so the concern fold copies these
            // names onto includers exactly as it does for `class_methods
            // do` / `module ClassMethods`.
            if let Some(singleton) = stmt.as_singleton_class_node() {
                // `class << self` is a class-method carrier. Record the
                // defs so an includer receives them. `module_function`
                // is not this node and stays on the module.
                if singleton.expression().as_self_node().is_some() {
                    defs_in(singleton.body(), file, &mut spans);
                }
            }
            if let Some(def) = super::visibility::definition(&stmt) {
                if let Some(singleton_body) = included_hook_class_methods_body(&def) {
                    defs_in(Some(singleton_body), file, &mut spans);
                }
                if is_class_methods_bridge(&def) {
                    bridges.push(super::util::def_name_span(&def, file));
                }
            }
        }
        if !spans.is_empty() || !bridges.is_empty() || has_nested_carrier
            || !concern_extensions.is_empty() || has_other_extensions
        {
            out.push(ConcernClassMethodSpans {
                owner: id,
                methods: spans,
                bridges,
                has_nested_carrier,
                concern_extensions,
                concern_calls,
                has_other_extensions,
            });
        }
    }
    // Framework identity must not depend on which declarations survive
    // library-shape ingestion. This walk records binding barriers only;
    // it neither evaluates constants nor executes class bodies.
    let mut shadows = HashSet::new();
    framework_shadow_scopes(&root, &mut shadows);
    (out, shadows.into_iter().map(|scope| ClassId(Symbol::from(scope.join("::")))).collect())
}

fn framework_shadow_scopes(
    node: &ruby_prism::Node<'_>,
    out: &mut HashSet<Vec<String>>,
) {
    struct Shadows<'a> {
        scope: Vec<String>,
        out: &'a mut HashSet<Vec<String>>,
    }
    impl Shadows<'_> {
        fn literal_path(node: &ruby_prism::Node<'_>) -> Option<Vec<String>> {
            if let Some(read) = node.as_constant_read_node() {
                return Some(vec![constant_id_str(&read.name()).to_string()]);
            }
            let path = node.as_constant_path_node()?;
            let mut names = path.parent().map_or(Some(Vec::new()), |p| Self::literal_path(&p))?;
            names.push(constant_id_str(&path.name()?).to_string());
            Some(names)
        }
        fn binding(&mut self, node: &ruby_prism::Node<'_>) {
            let name = node.as_constant_write_node().map(|n| n.name())
                .or_else(|| node.as_constant_or_write_node().map(|n| n.name()))
                .or_else(|| node.as_constant_and_write_node().map(|n| n.name()))
                .or_else(|| node.as_constant_operator_write_node().map(|n| n.name()))
                .or_else(|| node.as_constant_target_node().map(|n| n.name()));
            if let Some(name) = name {
                if constant_id_str(&name) == "ActiveSupport" {
                    self.out.insert(self.scope.clone());
                }
                if self.scope == ["ActiveSupport"] && constant_id_str(&name) == "Concern" {
                    self.out.insert(Vec::new());
                }
            }
            let target = node.as_constant_path_write_node().map(|n| n.target().as_node())
                .or_else(|| node.as_constant_path_or_write_node().map(|n| n.target().as_node()))
                .or_else(|| node.as_constant_path_and_write_node().map(|n| n.target().as_node()))
                .or_else(|| node.as_constant_path_operator_write_node().map(|n| n.target().as_node()));
            let (path, name) = if let Some(target) = target {
                (Self::literal_path(&target), target.as_constant_path_node().and_then(|n| n.name()))
            } else if let Some(target) = node.as_constant_path_target_node() {
                let mut path = target.parent().map_or(Some(Vec::new()), |p| Self::literal_path(&p));
                if let (Some(path), Some(name)) = (&mut path, target.name()) {
                    path.push(constant_id_str(&name).to_string());
                }
                (path, target.name())
            } else {
                return;
            };
            let Some(path) = path else {
                if name.is_some_and(|n| matches!(constant_id_str(&n), "ActiveSupport" | "Concern")) {
                    self.out.insert(Vec::new());
                }
                return;
            };
            let parent = match path.as_slice() {
                [parent @ .., name] if name == "ActiveSupport" => Some(parent),
                [parent @ .., namespace, name] if namespace == "ActiveSupport" && name == "Concern" => Some(parent),
                _ => None,
            };
            if let Some(parent) = parent {
                self.out.insert(parent.to_vec());
                let mut relative = self.scope.clone();
                relative.extend_from_slice(parent);
                self.out.insert(relative);
            }
        }
        fn declaration(&mut self, path: ruby_prism::Node<'_>, body: Option<ruby_prism::Node<'_>>, class: bool) {
            use super::util::constant_path_is_rooted;
            let Some(names) = Self::literal_path(&path) else {
                // A dynamic namespace cannot establish a safe lookup scope.
                self.out.insert(Vec::new());
                return;
            };
            let outer = self.scope.clone();
            if path.as_constant_path_node().is_some_and(|p| constant_path_is_rooted(&p)) {
                self.scope.clear();
            }
            self.scope.extend(names);
            // Only a root module reopening preserves the framework identity;
            // nested declarations and class declarations remain barriers.
            if self.scope.last().is_some_and(|name| name == "ActiveSupport")
                && (self.scope.len() > 1 || class)
            {
                self.out.insert(self.scope[..self.scope.len() - 1].to_vec());
            }
            if self.scope.as_slice() == ["ActiveSupport", "Concern"] {
                self.out.insert(Vec::new());
            }
            if let Some(body) = body {
                ruby_prism::Visit::visit(self, &body);
            }
            self.scope = outer;
        }
    }
    impl<'pr> ruby_prism::Visit<'pr> for Shadows<'_> {
        fn visit_branch_node_enter(&mut self, node: ruby_prism::Node<'pr>) { self.binding(&node); }
        fn visit_leaf_node_enter(&mut self, node: ruby_prism::Node<'pr>) { self.binding(&node); }
        fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
            self.declaration(node.constant_path(), node.body(), false);
        }
        fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
            if let Some(superclass) = node.superclass() { self.visit(&superclass); }
            self.declaration(node.constant_path(), node.body(), true);
        }
        fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
            if let Some(receiver) = node.receiver() { self.visit(&receiver); }
        }
    }
    ruby_prism::Visit::visit(&mut Shadows { scope: Vec::new(), out }, node);
}

pub fn ingest_concern_filters(
    source: &[u8],
    file: &str,
) -> Vec<(ClassId, Vec<crate::dialect::Filter>)> {
    let result = super::prism::parse_silent(source);
    let root = result.node();
    let mut out = Vec::new();
    for (scope, module) in find_all_modules_with_scope(&root) {
        let Some(name_path) = module_name_path(&module) else { continue };
        let mut full_path: Vec<String> = scope.clone();
        full_path.extend(name_path);
        let id = ClassId(Symbol::from(full_path.join("::")));

        let Some(body) = module.body() else { continue };
        let mut filters = Vec::new();
        for stmt in flatten_statements(body) {
            let Some(call) = stmt.as_call_node() else { continue };
            if call.receiver().is_some() || constant_id_str(&call.name()) != "included" {
                continue;
            }
            let Some(block) = call.block().and_then(|b| b.as_block_node()) else { continue };
            let Some(block_body) = block.body() else { continue };
            for inner in flatten_statements(block_body) {
                if let Some(fs) = super::controller::parse_filter_call(&inner, file) {
                    filters.extend(fs);
                } else if let Some(f) = block_form_concern_filter(&inner, file) {
                    filters.push(f);
                } else if let Some(name) = forgery_macro_name(&inner) {
                    // A forgery declaration `parse_filter_call` does not
                    // model (`prepend:`, a custom `store:`, an unknown
                    // `with:`) changes how every
                    // includer checks requests. The `included` block is
                    // otherwise dropped, so it would vanish without a
                    // trace where the same line in a controller is a
                    // survey gap; name it the same way.
                    super::survey::record(&super::IngestError::Unsupported {
                        file: file.to_string(),
                        message: format!(
                            "controller concern macro not recognized: `{name}` (its effect is dropped from the output)"
                        ),
                    });
                }
            }
        }
        if !filters.is_empty() {
            out.push((id, filters));
        }
    }
    out
}

/// `protect_from_forgery` / `skip_forgery_protection`, receiverless.
fn forgery_macro_name(stmt: &ruby_prism::Node<'_>) -> Option<&'static str> {
    let call = stmt.as_call_node()?;
    if call.receiver().is_some() {
        return None;
    }
    match constant_id_str(&call.name()) {
        "protect_from_forgery" => Some("protect_from_forgery"),
        "skip_forgery_protection" => Some("skip_forgery_protection"),
        _ => None,
    }
}

/// A block-form filter in a concern's `included do` — campfire's
/// `SetCurrentRequest` is nothing but `before_action do Current.request =
/// request end`. `parse_filter_call` returns `None` for it (no symbol
/// target), so it fell off the chain entirely: the splice never carried
/// it into any controller, the trace never listed it, and the emitted
/// controllers never set `Current.request`. Captured here as a `Filter`
/// whose `block` is the whole call, kept IN ORDER among the named
/// filters; the splice turns it back into the `Unknown` body item a
/// controller's own block-form filter is, so one lowering serves both.
/// The `__block__` target is a placeholder the splice never emits.
fn block_form_concern_filter(stmt: &ruby_prism::Node<'_>, file: &str) -> Option<crate::dialect::Filter> {
    use crate::dialect::{Filter, FilterKind};
    let call = stmt.as_call_node()?;
    if call.receiver().is_some() || call.block().is_none() {
        return None;
    }
    let kind = match constant_id_str(&call.name()) {
        "before_action" => FilterKind::Before,
        "around_action" => FilterKind::Around,
        "after_action" => FilterKind::After,
        _ => return None,
    };
    let expr = ingest_expr(stmt, file).ok()?;
    Some(Filter {
        target_span: crate::span::Span::synthetic(),
        kind,
        target: Symbol::from("__block__"),
        from_concern: None,
        only: Vec::new(),
        except: Vec::new(),
        only_style: crate::expr::ArrayStyle::default(),
        except_style: crate::expr::ArrayStyle::default(),
        if_cond: None,
        unless_cond: None,
        if_cond_expr: None,
        unless_cond_expr: None,
        block: Some(expr),
        prepend: false,
    })
}

/// Model DSL declared inside a concern module's `included do` block —
/// `Account::Associations` holds `has_many :statuses` etc. — captured
/// as classified [`crate::dialect::ModelBodyItem`]s per module. Rails
/// evaluates the block in each including model's class body, so these
/// items belong to every includer; analyze registers them (via
/// `App::concern_model_items`) exactly like the model's own
/// declarations. Only the DSL shapes (associations, scopes,
/// validations, callbacks) are kept — arbitrary statements stay with
/// the module. `with_options … do` wrappers are descended (their
/// kwargs refine defaults — `dependent:`, `inverse_of:` — that don't
/// affect the association's type); an item the classifier rejects is
/// survey-recorded and skipped so one exotic line doesn't cost the
/// rest of the block.
/// True when an Unknown body item is a receiverless block-form call to
/// a lifecycle hook the callback lowering handles.
fn unknown_is_block_callback(item: &crate::dialect::ModelBodyItem) -> bool {
    let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item else { return false };
    // The lowering's own recognizer, so a concern keeps exactly the
    // callbacks a model body would: blocks, zero-arity lambda arguments
    // (`before_update -> { @replaced = … }`) and their `on:` hash.
    crate::lower::model_to_library::block_callback_shape(expr).is_some()
}

/// Model-DSL MACROS that a lowering expands back out of the
/// `ModelBodyItem::Unknown` holding pen rather than from a variant of
/// their own — `lower::attached`, `lower::rich_text`,
/// `lower::secure_token`, `lower::secure_password`, `lower::has_json`,
/// `lower::typed_store`, `lower::broadcasts`.
///
/// Each one is per-includer DSL exactly like a `has_many`, so a
/// concern's `included do` has to carry it. It didn't: campfire's
/// `Message::Attachment` declares `has_one_attached :attachment` there,
/// the item was dropped on the floor, and `Message#attachment` was
/// never synthesized — six tests died on `undefined local variable or
/// method 'attachment'` while the concern's own `attachment?`, which
/// calls it, emitted right beside the hole.
///
/// Named, not inferred: `Unknown` is a holding pen for everything the
/// classifier doesn't claim, and most of what lands there really does
/// belong to the module rather than to its includers.
const CONCERN_MODEL_MACROS: &[&str] = &[
    "generates_token_for",
    "has_one_attached",
    "has_many_attached",
    "has_rich_text",
    "has_markdown",
    "has_secure_token",
    "has_secure_password",
    "has_json",
    "typed_store",
    "broadcasts_to",
    "delegate",
    // `included do include Other end` runs on the includer: spliced
    // after the includer's own `include` line, `Other` sits ahead of
    // this concern in the lookup order, as in Ruby.
    "include",
];

/// True when an Unknown body item is one of [`CONCERN_MODEL_MACROS`].
/// The block form counts — `has_one_attached :avatar do |attachable|
/// attachable.variant … end` declares variants we don't model, and the
/// attachment half still has to expand.
fn unknown_is_model_macro(item: &crate::dialect::ModelBodyItem) -> bool {
    use crate::expr::ExprNode;
    let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item else { return false };
    let ExprNode::Send { recv: None, method, .. } = &*expr.node else { return false };
    CONCERN_MODEL_MACROS.contains(&method.as_str())
}

/// Re-export: table payload lives next to [`super::model::EnumExpansion`].
pub use super::model::ConcernEnumDecl;

/// Second return value: `enum` columns declared inside an `included
/// do`, keyed by the concern module. They belong to every includer
/// exactly as the DSL items do; the splice folds them into each
/// including model's own `enums` table (and `enum_defaults` when
/// `default:` is present).
pub type ConcernModelItems = (
    Vec<(ClassId, Vec<crate::dialect::ModelBodyItem>)>,
    Vec<(ClassId, Vec<ConcernEnumDecl>)>,
);

fn walk_dsl_stmts<'pr>(body: ruby_prism::Node<'pr>, out: &mut Vec<ruby_prism::Node<'pr>>) {
    for stmt in flatten_statements(body) {
        if let Some(call) = stmt.as_call_node() {
            if call.receiver().is_none()
                && constant_id_str(&call.name()) == "with_options"
            {
                if let Some(block) = call.block().and_then(|b| b.as_block_node()) {
                    if let Some(inner) = block.body() {
                        walk_dsl_stmts(inner, out);
                    }
                    continue;
                }
            }
        }
        out.push(stmt);
    }
}

/// Only blocks with a retained candidate have a per-includer refusal gate.
/// Reuse the collector's traversal and IR recognizer, not a broader AST search.
pub(super) fn included_has_accessor(body: ruby_prism::Node<'_>, owner: &ClassId, file: &str) -> bool {
    let mut stmts = Vec::new();
    walk_dsl_stmts(body, &mut stmts);
    super::survey::without_recording(|| {
        stmts.iter().any(|stmt| {
            super::model::ingest_model_body_items(stmt, owner, file, Vec::new(), None)
                .is_ok_and(|items| items.iter().any(super::concern_accessors::is_candidate))
        })
    })
}

pub fn ingest_concern_model_items(source: &[u8], file: &str) -> ConcernModelItems {
    ingest_concern_model_items_with_constants(source, file, &super::model::EnumConstants::default())
}

/// Same as [`ingest_concern_model_items`], with the app-wide constant
/// table so `types: Leafable::TYPES` / bare `TYPES` inside `included do`
/// resolve the way model-side class-body DSL does.
pub(in crate::ingest) fn ingest_concern_model_items_with_constants(
    source: &[u8],
    file: &str,
    enum_constants: &super::model::EnumConstants,
) -> ConcernModelItems {
    use super::concern_accessors::{decline, is_candidate, is_supported};
    use crate::dialect::ModelBodyItem;

    let result = super::prism::parse_silent(source);
    let root = result.node();
    let mut out = Vec::new();
    let mut enums_out = Vec::new();
    for (scope, module) in find_all_modules_with_scope(&root) {
        let Some(name_path) = module_name_path(&module) else { continue };
        let mut full_path: Vec<String> = scope.clone();
        full_path.extend(name_path);
        let id = ClassId(Symbol::from(full_path.join("::")));
        let enum_owners = enum_constants
            .nesting
            .get(&(file.to_string(), module.location().start_offset()))
            .cloned()
            .unwrap_or_default();
        let resolve_constant = |node: &ruby_prism::Node<'_>| {
            enum_constants.resolve(node, &enum_owners)
        };

        let Some(body) = module.body() else { continue };
        let mut items: Vec<ModelBodyItem> = Vec::new();
        let mut enums: Vec<ConcernEnumDecl> = Vec::new();
        for stmt in flatten_statements(body) {
            let Some(call) = stmt.as_call_node() else { continue };
            if call.receiver().is_some() || constant_id_str(&call.name()) != "included" {
                continue;
            }
            let Some(block) = call.block().and_then(|b| b.as_block_node()) else { continue };
            let Some(block_body) = block.body() else { continue };
            let direct = flatten_statements(block_body);
            let block_start = items.len();
            let mut unclaimed = false;
            let mut stmts = Vec::new();
            walk_dsl_stmts(block.body().unwrap(), &mut stmts);
            for inner in stmts {
                // `enum` inside `included do` belongs to every includer
                // exactly like an association does — campfire declares
                // `enum :role, %i[member administrator bot]` in
                // User::Role. Expanded here for the same reason the
                // model walk expands it: one statement, many items.
                if let Some(call) = inner.as_call_node() {
                    match super::model::expand_class_body_dsl(
                        &call,
                        file,
                        &[],
                        &super::model::ClassConsts::default(),
                        &resolve_constant,
                    ) {
                        Ok(Some(super::model::ClassBodyExpansion::DelegatedType(expanded))) => {
                            items.extend(expanded);
                            continue;
                        }
                        Ok(Some(super::model::ClassBodyExpansion::Enum(expanded))) => {
                            if let Some(mapping) = expanded.mapping {
                                enums.push(ConcernEnumDecl {
                                    column: expanded.column,
                                    mapping,
                                    default: expanded.default,
                                });
                            }
                            items.extend(expanded.items);
                            continue;
                        }
                        Ok(None) => {}
                        Err(err) => {
                            super::survey::record(&err);
                            unclaimed = true;
                            continue;
                        }
                    }
                }
                // `_items`, plural: a multi-attribute `validates` (or
                // its `validates_presence_of` spelling) declares one
                // per attribute, and a concern splices ALL of them into
                // every includer — keeping only the first would fault
                // one field of several.
                match super::model::ingest_model_body_items(&inner, &id, file, Vec::new(), None) {
                    Ok(parsed) => {
                        for mut item in parsed {
                            match item {
                                ModelBodyItem::Association { .. }
                                | ModelBodyItem::Scope { .. }
                                | ModelBodyItem::Validation { .. }
                                | ModelBodyItem::Callback { .. } => items.push(item),
                                // Block-form lifecycle callbacks
                                // (`after_initialize do … end` —
                                // lobsters' Token concern generates its
                                // unique token there) surface as Unknown
                                // items; keep the ones the callback
                                // lowering understands so the concern
                                // splice carries them into each
                                // includer. Other Unknowns stay with the
                                // module.
                                ModelBodyItem::Unknown { .. } => {
                                    if is_candidate(&item) {
                                        if !is_supported(&item) {
                                            decline(&mut item, "unsupported accessor shape: only direct, nonempty, literal-Symbol attr_accessor is modeled");
                                        } else if !direct.iter().any(|stmt| stmt.location().start_offset() == inner.location().start_offset()) {
                                            decline(&mut item, "inside with_options is not modeled");
                                        }
                                    }
                                    if unknown_is_block_callback(&item)
                                        || unknown_is_model_macro(&item)
                                        || is_candidate(&item)
                                    {
                                        items.push(item);
                                    } else if !matches!(&item, ModelBodyItem::Unknown { expr, .. }
                                        if matches!(&*expr.node, crate::expr::ExprNode::Lit { .. }))
                                    {
                                        unclaimed = true;
                                    }
                                }
                                _ => unclaimed = true,
                            }
                        }
                    }
                    Err(err) => {
                        super::survey::record(&err);
                        unclaimed = true;
                    }
                }
            }
            // A dropped statement can alter a carried accessor's
            // visibility or definition. Defer the refusal until a
            // model actually includes it; dormant blocks stay inert.
            if unclaimed && items[block_start..].iter().any(is_candidate) {
                for item in &mut items[block_start..] {
                    if is_candidate(item) {
                        decline(item, "alongside unmodeled included-block statements is not supported");
                    }
                }
            }
        }
        if !enums.is_empty() {
            enums_out.push((id.clone(), enums));
        }
        if !items.is_empty() {
            out.push((id, items));
        }
    }
    (out, enums_out)
}

/// Does this class's ancestry, as a `sig/**/*.rbs` sidecar states it,
/// reach `T::Props`?
///
/// Transitively, because the real shape is a gem including a module
/// that includes `T::Props` — flattening that by hand at the sidecar
/// would be the app asserting something it did not write.
fn includes_t_props(id: &ClassId, includes: &HashMap<ClassId, Vec<ClassId>>) -> bool {
    let mut seen: HashSet<String> = HashSet::new();
    let mut stack = vec![id.clone()];
    while let Some(current) = stack.pop() {
        if !seen.insert(current.0.as_str().to_string()) {
            continue;
        }
        let Some(modules) = includes.get(&current) else { continue };
        for m in modules {
            if m.0.as_str() == "T::Props" {
                return true;
            }
            stack.push(m.clone());
        }
    }
    false
}

/// One `const` / `prop` call, read back out of `unknown_calls`.
///
/// The prism-side reader (`sorbet_struct_members`) runs while the AST
/// is still in hand. A base known only through a sidecar is not
/// recognizable until the sidecars have been read, which is after the
/// AST is gone — so this reads the same declaration from the `Expr`
/// the replay kept.
fn struct_member_from_expr(call: &Expr) -> Option<SorbetStructMember> {
    let ExprNode::Send { recv: None, method, args, .. } = &*call.node else {
        return None;
    };
    let writable = match method.as_str() {
        "const" => false,
        "prop" => true,
        _ => return None,
    };
    let mut args = args.iter();
    let name = match &*args.next()?.node {
        ExprNode::Lit { value: Literal::Sym { value } } => value.clone(),
        _ => return None,
    };
    // The type is the second argument and belongs to the analyzer.
    let _ty = args.next();
    let mut default = None;
    for arg in args {
        let ExprNode::Hash { entries, .. } = &*arg.node else { continue };
        for (key, value) in entries {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*key.node else {
                continue;
            };
            match key.as_str() {
                "default" => default = Some(value.clone()),
                // `factory: -> { expr }` — the body is the default,
                // evaluated per call, as sorbet evaluates the factory
                // per instance.
                "factory" => {
                    if let ExprNode::Lambda { body, .. } = &*value.node {
                        // A one-statement lambda body IS the value; a
                        // a `Seq` wrapping several is not a default
                        // anything here can use, so it is left alone.
                        default = match &*body.node {
                            ExprNode::Seq { exprs } => exprs.first().cloned(),
                            _ => Some(body.clone()),
                        };
                    }
                }
                _ => {}
            }
        }
    }
    Some(SorbetStructMember { name, writable, default })
}

/// Expand `const` / `prop` under a base whose ancestry a sidecar says
/// includes `T::Props`.
///
/// `T::Struct` gets this at ingest, matched on the base's NAME. A
/// gem's base is a different name for the same macro, and the tree
/// cannot see inside the gem to know that — so the sidecar supplies
/// the ancestry and this supplies the class, rather than the body
/// being replayed and the macro deferred to load time.
pub(super) fn expand_props_bases(app: &mut crate::App) {
    let includes = app.rbs_includes.clone();
    for lc in app.library_classes.iter_mut() {
        if lc.parent.as_ref().is_some_and(is_sorbet_struct_parent) {
            continue;
        }
        // The ancestry can arrive two ways, and only one of them is
        // the parent's. A class that does `include Vendor::Props` in
        // its OWN body gets `T::Props` without its base having it —
        // and its `const` is the same macro. Checking only the parent
        // left those replayed, which is how this was found: a sidecar
        // claiming the BASE had the module made the emit right for the
        // wrong reason, and the boot census contradicted the claim.
        let reached = lc
            .parent
            .as_ref()
            .is_some_and(|p| includes_t_props(p, &includes))
            || lc.includes.iter().any(|m| {
                m.0.as_str() == "T::Props" || includes_t_props(m, &includes)
            });
        if !reached {
            continue;
        }
        let members: Vec<SorbetStructMember> = lc
            .unknown_calls
            .iter()
            .filter_map(struct_member_from_expr)
            .collect();
        if members.is_empty() {
            continue;
        }
        let comparable = lc
            .includes
            .iter()
            .any(|i| i.0.as_str() == "T::Struct::ActsAsComparable");
        lc.unknown_calls.retain(|call| !is_struct_declaration(call));
        let mut synthesized = synth_sorbet_struct_methods(&lc.name, &members, comparable, false);
        synthesized.append(&mut lc.methods);
        lc.methods = synthesized;
    }
}

/// Ruby reserved words: a parameter with one of these names is only
/// legal as a keyword (`next: nil`), never as a positional.
fn is_ruby_reserved_word(m: &str) -> bool {
    matches!(
        m,
        "__ENCODING__" | "__LINE__" | "__FILE__" | "BEGIN" | "END" | "alias" | "and" | "begin"
            | "break" | "case" | "class" | "def" | "defined?" | "do" | "else" | "elsif" | "end"
            | "ensure" | "false" | "for" | "if" | "in" | "module" | "next" | "nil" | "not" | "or"
            | "redo" | "rescue" | "retry" | "return" | "self" | "super" | "then" | "true" | "undef"
            | "unless" | "until" | "when" | "while" | "yield"
    )
}

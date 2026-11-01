//! Library-shape Ruby emission — for transpiled-shape input where class
//! bodies already contain explicit methods (no Rails DSL expansion).
//! Mirrors `src/emit/typescript/library.rs` in scope; produces one
//! `app/models/<name>.rb` per `LibraryClass`.
//!
//! Ruby is implicit about ivar declaration and global about constant
//! resolution, so this emitter is shorter than the TS analog: no ivar
//! field block, no import partition.

use std::collections::BTreeSet;
use crate::det::HashMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use super::super::EmittedFile;
use crate::App;
use crate::dialect::{AccessorKind, LibraryClass, LibraryClassOrigin, MethodDef, MethodReceiver, MethodVisibility};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::span::Span;
use crate::ty::Ty;

/// Base's shared methods return Base in the runtime sidecar. A concrete
/// forwarder discards that ABI result and returns its own `self`, so the
/// model type promised by analysis is also true with Spinel's RBS seeds.
pub(super) fn preserve_record_receiver_returns(lcs: &mut [LibraryClass], app: &App) {
    let templates = crate::runtime_src::parse_methods(
        "def reload(options = nil)\n  super(options)\n  self\nend\ndef lock!(lock = true)\n  super(lock)\n  self\nend\n",
    ).expect("receiver-return forwarders parse");
    // The names of the models with a table, and one index of the source's
    // methods for every ask (each ask built its own over the whole app).
    let tabled: crate::det::HashSet<&ClassId> =
        app.models.iter().filter(|m| app.schema.tables.contains_key(&m.table.0)).map(|m| &m.name).collect();
    let source_methods = crate::analyze::SourceMethodIndex::new(app);
    for lc in lcs {
        if !tabled.contains(&lc.name) {
            continue;
        }
        let self_ty = Ty::Class { id: lc.name.clone(), args: vec![].into() };
        for template in &templates {
            if lc.methods.iter().any(|m| m.name == template.name && m.receiver == MethodReceiver::Instance)
                || record_return_overridden(&source_methods, &lc.name, template.name.as_str())
            {
                continue;
            }
            let mut method = template.clone();
            method.name_span = Span::synthetic();
            method.enclosing_class = Some(lc.name.0.clone());
            method.body.ty = Some(self_ty.clone());
            let params = method.params.iter().map(|p| crate::ty::Param {
                name: p.name.clone(),
                // `lock!` takes Rails' lock clause; `reload`'s options are open.
                ty: if method.name.as_str() == "reload" {
                    Ty::Untyped.into()
                } else {
                    Ty::Union { variants: vec![Ty::Bool, Ty::Str, Ty::Nil].into() }.into()
                },
                kind: crate::ty::ParamKind::Optional,
            }).collect::<Vec<_>>().into();
            method.signature = Some(Ty::Fn {
                params,
                ret: std::sync::Arc::new(self_ty.clone()),
                block: None,
                effects: crate::effect::EffectSet::default(),
            });
            lc.methods.push(method);
        }
    }
}

/// Source overrides, including those on ApplicationRecord or a mixin,
/// may return a different object. Do not interpose a self-returning wrapper.
/// `lock!` calls `reload`, so an override of either one answers the source.
fn record_return_overridden(
    source_methods: &crate::analyze::SourceMethodIndex<'_>,
    class: &ClassId,
    method: &str,
) -> bool {
    source_methods.instance_method(class, method)
        || (method == "lock!" && source_methods.instance_method(class, "reload"))
}

/// Lower library classes for Ruby and emit their source/RBS file pairs, grouping
/// nested declarations with their owners where possible. Lifted Data factories
/// participate in lowering but emit inside their owner's constant declaration,
/// never as standalone class files.
pub(super) fn emit_library_class_decls(app: &App) -> Vec<EmittedFile> {
    let _indexes = install_lookup_indexes(app);
    let mut lcs: Vec<LibraryClass> = app.library_classes.clone();
    // An alias of an ancestor's method is the native `alias_method` here.
    if super::expr::carries_body_statements() {
        crate::ingest::inherited_alias::drop_replayed_copies(&mut lcs);
    }
    // A block-taking tag helper gains a `<name>_into(io, …)` variant
    // that writes around a yield into the caller's buffer; the view
    // emit points its appends at it (`lower::tag_block_passing`).
    // FIRST, on the App's shape of the body: the passes below qualify
    // the `capture` the variant is recognised by, and the variant's
    // own body wants every one of them applied.
    crate::lower::tag_block_passing::apply_helpers(&mut lcs);
    apply_scope_lowering(&mut lcs, app);
    // ActiveJob payloads: the enqueue arm of each planned job's
    // `perform_later`, and `to_gid_uri` beside `to_gid_param`.
    // Ruby-family only, like the GlobalID runtime it writes through.
    crate::lower::job_payload::apply_ruby(&mut lcs, app);
    apply_library_partial_render_lowering(&mut lcs, app);
    // The Attachment's readers on a content-type attachable (campfire's
    // `OpengraphEmbed`): Rails hands the partial the Attachment, which
    // delegates to the record; the emitted partial takes the record,
    // so the record delegates the Attachment's own readers back
    // through a slot the render seam fills.
    for binding in crate::lower::attachable::attachable_partial_bindings(app) {
        if binding.content_type.is_none() {
            continue;
        }
        if let Some(lc) = lcs.iter_mut().find(|lc| lc.name == binding.class) {
            crate::lower::attachable::push_attachment_delegation(&mut lc.methods, &binding.class);
        }
    }
    apply_helper_lowering(&mut lcs, app);
    // A bare `<x>_path` that `apply_helper_lowering` did not claim and
    // the route table DOES answer is a route helper called somewhere
    // nothing qualifies — campfire's `Messages::AttachmentPresentation`
    // `delegate`s `rails_blob_path` to its context, which leaves three
    // calls to a method the emitted tree defines nowhere.
    crate::lower::route_helper_receiver::qualify_lcs(&mut lcs, app);
    apply_route_param_lowering(&mut lcs, app);
    apply_raw_helper_monomorphization(&mut lcs, app);
    // send→case grounding, update-kwargs inlining, mailer class-side
    // wrappers, and duration grounding run in the shared post-analyze
    // hook, which covers these library classes (dispatch's plural
    // duration-unit arms arrive already grounded).
    // Transpiled-shape classes carry hand-written accessors that
    // `synth_attr_reader` never sees, so the datetime reader/writer
    // rewrite still runs here for them (Ruby-only). Model-lowered classes
    // get the reader from `synth_attr_reader` (shared, all targets); this
    // re-applies the same reader idempotently and adds the Ruby writer
    // normalize.
    apply_datetime_lowering(&mut lcs, app);
    apply_rich_text_content_memo(&mut lcs);
    apply_boolean_lowering(&mut lcs, app);
    apply_typed_writer_cast_lowering(&mut lcs, app);
    parse_untyped_numericality_input(&mut lcs, app);
    apply_hydration_nil_lowering(&mut lcs, app);
    apply_time_format_lowering(&mut lcs, app);
    // Same rooting the view pipeline applies, for the same reason one
    // step over: a class nested in a namespace that SHADOWS what it
    // references. `Message::Broadcasts` is where the broadcast lowering
    // emits `Broadcasts.append(...)`, and inside `module Broadcasts`
    // that constant is the concern itself. Idempotent (an already-rooted
    // head is skipped), so running it in both pipelines is safe.

    apply_constant_rooting(&mut lcs, app, RootingScope::RuntimeOnly);
    // Lifted factories are lowered on `lcs`. Lookups that splice their methods
    // or read neighboring classes have to see that slice. Index it in place
    // instead of cloning the rest of the App (models, routes, views, assets).
    let lowered_classes = lcs.iter().any(|lc| {
        matches!(lc.origin, Some(LibraryClassOrigin::DataFactory { .. }))
    });
    let classes: &[LibraryClass] = if lowered_classes { &lcs } else { &app.library_classes };
    let factories = if lowered_classes {
        associate_data_factories(classes)
    } else {
        Vec::new()
    };
    let _factory_indexes = lowered_classes.then(|| install_lookup_indexes_for(app, classes));
    // A nested class goes in its parent's file, as the source wrote
    // it. Given its own, each part-file re-opened the outer class and
    // CLOSED it again, and `TracePoint(:end)` — how a gem implements
    // "end of class body" — fires on every close. A gem validating
    // there saw a body missing what the parent's own file sets, and
    // the tree stopped loading. Naming the same superclass in both
    // places (see `outer_header`) makes the ORDER irrelevant; it
    // cannot make the COUNT of body-ends one.
    // Only into an owner THIS collection emits. A class nested in a
    // MODEL (`Message::Broadcasts`) has its owner emitted by
    // `emit_lowered_models`, a different pipeline — filtering it out
    // here without anyone to splice it into made it vanish from the
    // tree entirely, which the broadcast tests caught at once. Those
    // keep their own file and the extra body-end with it; moving them
    // is a change to the model emit, not to this one.
    // `X = Data.define(:a) do def … end` — the block's methods travel as
    // a library class of X's name (`ingest::library_class::
    // data_block_classes`) and render back into the block, the one place
    // they run before anything reads the class.
    let spliced_factories: crate::det::HashSet<String> = lcs
        .iter()
        .flat_map(|lc| {
            lc.constants.iter().filter(|(_, value)| is_data_factory(value)).map(move |(name, _)| {
                format!("{}::{}", lc.name.0.as_str(), name.as_str())
            })
        })
        .collect();
    let is_spliced_factory = |lc: &LibraryClass| {
        !lc.is_module && lc.parent.is_none() && spliced_factories.contains(lc.name.0.as_str())
    };
    let data_blocks: Vec<&LibraryClass> = lcs.iter().filter(|lc| is_spliced_factory(lc)).collect();
    let emitted_here: crate::det::HashSet<&str> = lcs
        .iter()
        .filter(|lc| !is_spliced_factory(lc))
        .map(|lc| lc.name.0.as_str())
        .collect();
    let owner_in_this_tree = |lc: &LibraryClass| -> Option<String> {
        let owner = file_owner(lc.name.0.as_str(), app, classes);
        (owner != lc.name.0.as_str() && emitted_here.contains(owner.as_str())).then_some(owner)
    };

    let mut children: crate::det::HashMap<String, Vec<&LibraryClass>> =
        crate::det::HashMap::default();
    for lc in &lcs {
        if is_spliced_factory(lc) || matches!(lc.origin, Some(LibraryClassOrigin::DataFactory { .. })) {
            continue;
        }
        if let Some(owner) = owner_in_this_tree(lc) {
            children.entry(owner).or_default().push(lc);
        }
    }

    lcs.iter()
        .filter(|lc| !is_spliced_factory(lc))
        .filter(|lc| !matches!(lc.origin, Some(LibraryClassOrigin::DataFactory { .. })))
        .filter(|lc| owner_in_this_tree(lc).is_none())
        .flat_map(|lc| {
            // `underscore`, not `snake_case`: a namespaced reopen
            // (lobsters' `ActiveRecord::Base.q`, `Net::HTTP`,
            // `ShortId::CandidateId`) nests as `active_record/base.rb` —
            // a literal `::` in the filename breaks the emitted
            // Makefile's dependency list.
            let file_stem = crate::naming::underscore(lc.name.0.as_str());
            let out_path = PathBuf::from(format!("app/models/{file_stem}.rb"));
            let mut files = match children.get(lc.name.0.as_str()) {
                Some(kids) => emit_library_class_with_nested(lc, kids, app, classes, &factories, out_path),
                None => emit_library_class_pair_in(lc, app, classes, &factories, out_path),
            };
            for block in &data_blocks {
                splice_data_block(&mut files, block, app, classes, &factories);
            }
            files
        })
        .fold(Vec::<EmittedFile>::new(), |mut out, file| {
            // Not left to the tree's last-wins dedupe: a module opened in two files (rolify's `Rolify`) would lose one opening's methods.
            match out.iter_mut().find(|f| f.path == file.path) {
                Some(existing) if existing.content == file.content => {}
                Some(existing) => {
                    existing.content.push('\n');
                    existing.content.push_str(&file.content);
                }
                None => out.push(file),
            }
            out
        })
}

/// `Data.define(<symbols>)` — the factory `analyze::data` models.
fn is_data_factory(value: &crate::expr::Expr) -> bool {
    matches!(
        &*value.node,
        ExprNode::Send { recv: Some(recv), method, block: None, .. }
            if method.as_str() == "define"
                && matches!(&*recv.node, ExprNode::Const { path }
                    if path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).eq(["Data"]))
    )
}

/// One lifted `Data.define` block, tied to the owner constant whose
/// declaration span is the factory's `LibraryClassOrigin::DataFactory`.
pub(super) struct DataFactoryOwner {
    pub owner: String,
    pub constant: String,
    pub index: usize,
}

/// Pair each lifted factory with its owner constant once. Later Ruby and
/// RBS emission look up this list instead of scanning every library class
/// per constant.
fn associate_data_factories(classes: &[LibraryClass]) -> Vec<DataFactoryOwner> {
    let mut by_span: HashMap<Span, usize> = HashMap::default();
    for (index, class) in classes.iter().enumerate() {
        if let Some(LibraryClassOrigin::DataFactory { declaration_span }) = class.origin {
            by_span.entry(declaration_span).or_insert(index);
        }
    }
    let mut owners = Vec::new();
    for class in classes {
        for (name, value) in &class.constants {
            let Some(&index) = by_span.get(&value.span) else { continue };
            let owner = class.name.0.as_str();
            let constant = name.as_str();
            if owners.iter().any(|link: &DataFactoryOwner| link.owner == owner && link.constant == constant) {
                continue;
            }
            owners.push(DataFactoryOwner {
                owner: owner.to_string(),
                constant: constant.to_string(),
                index,
            });
        }
    }
    owners
}

pub(super) fn lifted_data_factory<'a>(
    owner: &LibraryClass,
    constant: &str,
    classes: &'a [LibraryClass],
    factories: &[DataFactoryOwner],
) -> Option<&'a LibraryClass> {
    if !factories.is_empty() {
        let owner_name = owner.name.0.as_str();
        let index = factories.iter().find(|link| link.owner == owner_name && link.constant == constant)?.index;
        return classes.get(index);
    }
    let span = owner.constants.iter().find(|(name, _)| name.as_str() == constant)?.1.span;
    classes.iter().find(|class| {
        matches!(class.origin, Some(LibraryClassOrigin::DataFactory { declaration_span })
            if declaration_span == span)
    })
}

/// Render `block`'s methods into the `X = Data.define(…)` line that
/// declares it, as the `do … end` the source wrote, and into the
/// sidecar's `class X < ::Data` declaration. The line is the one
/// `emit_library_class_decl` wrote for the owner, at the owner's body
/// indentation; a file that does not hold it is not the owner's.
fn splice_data_block(
    files: &mut [EmittedFile],
    block: &LibraryClass,
    app: &App,
    classes: &[LibraryClass],
    factories: &[DataFactoryOwner],
) {
    let name = block.name.0.as_str();
    let owner = file_owner(name, app, classes);
    let owner_stem = crate::naming::underscore(&owner);
    let owner_rb_path = PathBuf::from(format!("app/models/{owner_stem}.rb"));
    let owner_rbs_path = PathBuf::from(format!("app/models/{owner_stem}.rbs"));
    let depth = name.split("::").count();
    let last = name.rsplit("::").next().unwrap_or(name);
    let indent = "  ".repeat(depth - 1);
    let stem = crate::naming::underscore(name);
    let rb_path = PathBuf::from(format!("app/models/{stem}.rb"));
    let rendered = emit_library_class_decl_in(block, app, classes, factories, rb_path.clone(), &[], &[]);
    let lines: Vec<&str> = rendered.content.lines().collect();
    let mut hoisted = Vec::new();
    for line in lines.iter().filter(|line| line.starts_with("require_relative ")) {
        let target = line.trim_start_matches("require_relative ").trim().trim_matches('"');
        let rebased = rebase_relative(&stem, &owner, target, app);
        hoisted.push(format!("require_relative {rebased:?}"));
    }
    hoisted.extend(
        lines
            .iter()
            .filter(|line| line.starts_with("require \""))
            .map(|line| line.to_string()),
    );
    let body_lines: Vec<&str> = lines.iter().copied().filter(|line| !line.starts_with("require")).collect();
    let Some(body) = unwrapped(&body_lines, depth) else {
        assert!(
            block.methods.is_empty() && hoisted.is_empty(),
            "Data.define methods or requires were not emitted for {name}"
        );
        return;
    };
    let sidecar = super::rbs::emit_library_class_rbs(block, &rb_path);
    let sidecar_lines: Vec<&str> = sidecar.content.lines().collect();
    let sidecar_body = unwrapped(&sidecar_lines, depth);
    assert!(
        block.methods.is_empty() || sidecar_body.is_some(),
        "Data.define methods were not emitted to the RBS sidecar for {name}"
    );
    for file in files.iter_mut() {
        let is_rb = file.path.extension().is_some_and(|e| e == "rb");
        if (is_rb && file.path != owner_rb_path) || (!is_rb && file.path != owner_rbs_path) {
            continue;
        }
        let mut out: Vec<String> = Vec::new();
        let mut spliced = false;
        let mut lines = file.content.lines().peekable();
        while let Some(line) = lines.next() {
            if spliced {
                out.push(line.to_string());
                continue;
            }
            if is_rb {
                let factory = line
                    .strip_prefix(&indent)
                    .and_then(|rest| rest.strip_prefix(&format!("{last} = Data.define")))
                    .is_some_and(|rest| !rest.contains(" do") && (rest.is_empty() || rest.ends_with(')')));
                out.push(if factory { format!("{line} do") } else { line.to_string() });
                if factory {
                    out.push(body.clone());
                    out.push(format!("{indent}end"));
                    spliced = true;
                }
            } else {
                out.push(line.to_string());
                if line == format!("{indent}class {last} < ::Data") {
                    // The members' readers follow up to the class's `end`.
                    while let Some(next) = lines.peek() {
                        if *next == format!("{indent}end") {
                            break;
                        }
                        out.push(lines.next().unwrap().to_string());
                    }
                    if let Some(methods) = &sidecar_body {
                        out.push(methods.clone());
                    }
                    spliced = true;
                }
            }
        }
        if spliced {
            let trailing = if file.content.ends_with('\n') { "\n" } else { "" };
            let mut content = out.join("\n");
            if is_rb && !hoisted.is_empty() {
                content = format!("{}\n\n{content}", hoisted.join("\n"));
            }
            file.content = format!("{content}{trailing}");
        }
        if !block.methods.is_empty() {
            assert!(spliced, "Data.define methods were not spliced into {file:?} for {name}");
        }
    }
}

/// A child's `require_relative` target, expressed from its parent's
/// file instead of its own.
///
/// Both files live under `app/models/`, so the arithmetic is on that
/// tree: resolve the target against the directory the child's file
/// WOULD have had, then re-express it against the parent's.
fn rebase_relative(child_stem: &str, parent_name: &str, target: &str, app: &App) -> String {
    let _ = app;
    // Anchored at the TREE root, not at `app/models`: a require can
    // climb out of the models directory (`../../../runtime/x`), and
    // arithmetic that starts inside it loses those steps silently —
    // the `..` pops an empty stack and the path comes out too short.
    let mut child_dir: Vec<String> =
        vec!["app".to_string(), "models".to_string()];
    child_dir.extend(child_stem.split('/').map(str::to_string));
    child_dir.pop();
    let parent_stem = crate::naming::underscore(parent_name);
    let mut parent_dir: Vec<String> =
        vec!["app".to_string(), "models".to_string()];
    parent_dir.extend(parent_stem.split('/').map(str::to_string));
    parent_dir.pop();

    let mut resolved: Vec<String> = child_dir.clone();
    for seg in target.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                resolved.pop();
            }
            other => resolved.push(other.to_string()),
        }
    }
    // Longest common prefix, then `..` for what the parent has left.
    let common = parent_dir
        .iter()
        .zip(resolved.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let ups = parent_dir.len() - common;
    let mut out: Vec<String> = std::iter::repeat_n("..".to_string(), ups).collect();
    out.extend(resolved[common..].iter().cloned());
    out.join("/")
}

/// The lines of a nested class's render between its outer wrapper:
/// the `depth` header lines that re-open the parent and the `depth`
/// `end`s that close it. The lines between are already at the
/// indentation the parent's body wants. `None` when nothing is left
/// to splice.
fn unwrapped(body: &[&str], depth: usize) -> Option<String> {
    let first = body.iter().position(|l| !l.trim().is_empty())?;
    let open = first + depth;
    let last = body.iter().rposition(|l| l.trim() == "end")?;
    let close = (last + 1).checked_sub(depth)?;
    (open < close).then(|| body[open..close].join("\n"))
}

/// `content` with `blocks` placed right after the parent's own header
/// — the `class` line at `header_indent` — or `None` when there is no
/// such line, since anywhere else would be outside the body.
fn spliced_after_header(content: &str, header_indent: &str, blocks: &[String]) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    let mut inserted = false;
    for line in content.lines() {
        out.push(line);
        if !inserted
            && line
                .strip_prefix(header_indent)
                .is_some_and(|rest| rest.starts_with("class "))
        {
            out.extend(blocks.iter().map(String::as_str));
            inserted = true;
        }
    }
    inserted.then(|| out.join("\n"))
}

/// Put each nested class's block inside its parent's file, immediately
/// before its first load-time reference, or at the end of the body.
///
/// The child is rendered by the same function as before and its outer
/// wrapper removed by COUNT, not by pattern: the parent's name has D
/// segments, so the child's render opens with exactly D header lines
/// and closes with exactly D `end`s, and what lies between is already
/// at the indentation the parent's body wants.
///
/// The child's `.rbs` goes into the parent's sidecar the same way. A
/// child with no file of its own has no sidecar of its own either, and
/// a sidecar left out is a class declared nowhere: the typed view of
/// the tree lost every nested class while the `.rb` kept them all.
/// Spelled nested inside the parent's `class`, which is what the
/// standalone sidecar's `module Parent` wrapper said too.
fn emit_library_class_with_nested(
    parent: &LibraryClass,
    kids: &[&LibraryClass],
    app: &App,
    classes: &[LibraryClass],
    factories: &[DataFactoryOwner],
    out_path: PathBuf,
) -> Vec<EmittedFile> {
    let depth = parent.name.0.as_str().split("::").count();
    // Order the children so a superclass precedes its subclass. They
    // share one file now, so `class Image < Sound::ImageStruct` runs
    // where the base must already be defined — the requirement the
    // require between two files used to satisfy. Left to the
    // collection's own order this works by luck.
    // Order children so a superclass / included sibling precedes dependents.
    // Bare `Parent` inside `Owner` is `Owner::Parent` when a sibling of that
    // name is among the kids (`Class.new(Parent)` and ordinary
    // `class Child < Parent` may store the written bare name). Absolute
    // `::X` and names that already identify a collected child stay as written.
    let kids = {
        let names: crate::det::HashSet<&str> =
            kids.iter().map(|k| k.name.0.as_str()).collect();
        let owner = parent.name.0.as_str();
        let dependency_name = |written: &str| -> String {
            if let Some(rooted) = written.strip_prefix("::") {
                rooted.to_owned()
            } else if names.contains(written) {
                written.to_owned()
            } else {
                format!("{owner}::{written}")
            }
        };
        crate::library_class_order::order_by_deps(
            kids.to_vec(),
            |k| k.name.0.as_str(),
            |k| {
                k.parent
                    .iter()
                    .chain(k.includes.iter())
                    .map(|p| dependency_name(p.0.as_str()))
                    .collect()
            },
        )
    };
    let kids = &kids[..];
    let mut blocks: Vec<NestedBlock> = Vec::new();
    let mut sidecar_blocks: Vec<String> = Vec::new();
    let mut hoisted: Vec<String> = Vec::new();
    for kid in kids {
        let stem = crate::naming::underscore(kid.name.0.as_str());
        let rb_path = PathBuf::from(format!("app/models/{stem}.rb"));
        let rendered = emit_library_class_decl_in(kid, app, classes, factories, rb_path.clone(), &[], &[]);
        let lines: Vec<&str> = rendered.content.lines().collect();
        // Requires belong at the top of the file that now holds the
        // class, not buried inside a class body where they would still
        // execute but read as a mistake — and they have to be REBASED
        // on the way: the child's were computed relative to the file
        // it used to have, which sat one directory deeper per level of
        // nesting. Hoisted verbatim they read `../../x` where the
        // parent needs `../x`, which is a load error the day someone
        // reaches that constant.
        for line in lines.iter().filter(|l| l.starts_with("require_relative ")) {
            let Some(rest) = line.strip_prefix("require_relative ") else { continue };
            let target = rest.trim().trim_matches('"');
            let rebased = rebase_relative(&stem, parent.name.0.as_str(), target, app);
            // A sibling that is now in THIS file needs no require, and
            // naming it would name a path that no longer exists: one
            // child referencing another is the common case in a class
            // that nests several.
            let own_dir = format!("{}/", crate::naming::underscore(parent.name.0.as_str()));
            let own_dir = own_dir.rsplit('/').nth(1).map(|d| format!("{d}/"));
            if own_dir.is_some_and(|d| rebased.starts_with(&d)) {
                continue;
            }
            // The child requiring its parent names THIS file.
            let own_file = crate::naming::underscore(parent.name.0.as_str());
            let own_file = own_file.rsplit('/').next().unwrap_or(&own_file);
            if rebased == own_file {
                continue;
            }
            let line = format!("require_relative {rebased:?}");
            if !hoisted.contains(&line) {
                hoisted.push(line);
            }
        }
        // A plain `require "x"` names a library, not a path, and is
        // carried as written.
        for l in lines.iter().filter(|l| l.starts_with("require \"")) {
            if !hoisted.iter().any(|h| h == l) {
                hoisted.push(l.to_string());
            }
        }
        let body: Vec<&str> = lines
            .iter()
            .copied()
            .filter(|l| !l.starts_with("require"))
            .collect();
        let Some(block) = child_block(&body, depth, kid.name.0.as_str()) else { continue };
        blocks.push(NestedBlock {
            name: kid.name.0.as_str().to_string(),
            requires: nested_dependencies(kid, kids),
            content: block,
        });
        // The sidecar `emit_library_class_pair` would have written for
        // the child's own file, minus the same wrapper. Its type names
        // were resolved inside the child's enclosing segments, which
        // are the parent's followed by the child's own — the position
        // it is spliced into.
        let sidecar = super::rbs::emit_library_class_rbs(kid, &rb_path);
        let lines: Vec<&str> = sidecar.content.lines().collect();
        if let Some(block) = child_block(&lines, depth, kid.name.0.as_str()) {
            sidecar_blocks.push(block);
        }
    }
    if blocks.is_empty() {
        return emit_library_class_pair_in(parent, app, classes, factories, out_path);
    }
    let own_header_indent = "  ".repeat(depth - 1);
    let mut rb = emit_library_class_decl_in(parent, app, classes, factories, out_path.clone(), &[], &blocks);
    let compact = |blocks: &[String]| -> Vec<String> {
        blocks.iter().map(|b| indent_lines(b, 1)).collect()
    };
    let sidecar_blocks: Vec<String> = sidecar_blocks.iter().map(|b| indent_lines(b, depth)).collect();
    let raw_sidecar: Vec<String> = sidecar_blocks.iter().map(|b| dedent_lines(b, depth)).collect();
    let compact_header = format!("class {}", parent.name.0.as_str());
    let compact_form = rb.content.lines().any(|l| l == compact_header || l.starts_with(&format!("{compact_header} ")));
    // A require the parent's own file already carries is not repeated.
    hoisted.retain(|r| !rb.content.lines().any(|l| l == r));
    let mut content = String::new();
    for r in &hoisted {
        content.push_str(r);
        content.push('\n');
    }
    if !hoisted.is_empty() {
        content.push('\n');
    }
    content.push_str(&rb.content);
    rb.content = content;

    // Only once the `.rb` took the body: a sidecar must describe the
    // file beside it, and a child left out of the one stays out of the
    // other.
    let mut rbs = super::rbs::emit_library_class_rbs(parent, &out_path);
    let (indent, sidecar_blocks) = if compact_form {
        (String::new(), compact(&raw_sidecar))
    } else {
        (own_header_indent, sidecar_blocks)
    };
    if let Some(spliced) = spliced_after_header(&rbs.content, &indent, &sidecar_blocks) {
        rbs.content = format!("{spliced}\n");
    }
    vec![rb, rbs]
}

struct NestedBlock {
    name: String,
    requires: BTreeSet<String>,
    content: String,
}

fn nested_dependencies(kid: &LibraryClass, kids: &[&LibraryClass]) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for value in kid.constants.iter().map(|(_, value)| value)
        .chain(&kid.class_ivar_initializers).chain(&kid.unknown_calls)
    {
        walk_load_time_paths(value, kid, &mut BTreeSet::new(), &mut paths);
    }
    for class in kid.parent.iter().chain(&kid.includes) {
        paths.insert(class.0.as_str().split("::").map(str::to_string).collect());
    }
    let names: BTreeSet<String> = kids.iter().map(|class| class.name.0.as_str().to_string())
        .collect();
    nested_names_in_paths(kid, &paths, &names)
}

fn nested_names_in_paths(owner: &LibraryClass, paths: &BTreeSet<Vec<String>>, names: &BTreeSet<String>) -> BTreeSet<String> {
    let local: BTreeSet<&str> = owner.constants.iter().map(|(name, _)| name.as_str()).collect();
    let mut out = BTreeSet::new();
    for path in paths {
        if path.len() == 1 && local.contains(path[0].as_str()) {
            continue;
        }
        let rooted = path.first().is_some_and(String::is_empty);
        let path = path.iter().filter(|part| !part.is_empty()).cloned().collect::<Vec<_>>().join("::");
        let mut scope = Some(owner.name.0.as_str());
        let mut candidates = Vec::new();
        if !rooted {
            while let Some(prefix) = scope {
                candidates.push(format!("{prefix}::{path}"));
                scope = prefix.rsplit_once("::").map(|(outer, _)| outer);
            }
        }
        candidates.push(path);
        if let Some(name) = candidates.iter().find_map(|candidate| {
            names.iter().filter(|name| name.as_str() != owner.name.0.as_str()
                && (candidate == *name || candidate.starts_with(&format!("{name}::"))))
                .max_by_key(|name| name.len()).cloned()
        }) {
            out.insert(name);
        }
    }
    out
}

/// Lambda or proc invoked in this expression: `-> { … }.call`,
/// `lambda { … }.call`, `proc { … }[]`. A stored closure runs later;
/// one invoked here runs while the parent statement loads.
fn invoked_lambda(expr: &Expr) -> Option<&Expr> {
    let ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node else {
        return None;
    };
    if !matches!(method.as_str(), "call" | "[]") {
        return None;
    }
    match &*recv.node {
        ExprNode::Lambda { .. } => Some(recv),
        ExprNode::Send { recv: None, method, args, block: Some(block), .. }
            if args.is_empty() && matches!(method.as_str(), "lambda" | "proc") =>
        {
            Some(block)
        }
        _ => None,
    }
}

fn walk_param_defaults(params: &[crate::dialect::Param], owner: &LibraryClass, side: MethodReceiver, visited: &mut BTreeSet<String>, paths: &mut BTreeSet<Vec<String>>) {
    for param in params {
        if let Some(default) = &param.default {
            walk_load_time_paths_on(default, owner, side, visited, paths);
        }
    }
}

fn walk_load_time_paths(expr: &Expr, owner: &LibraryClass, visited: &mut BTreeSet<String>, paths: &mut BTreeSet<Vec<String>>) {
    walk_load_time_paths_on(expr, owner, MethodReceiver::Class, visited, paths);
}

// Not by name alone: a class-body `configure` runs `def self.configure`, never an instance method of that name.
fn walk_load_time_paths_on(expr: &Expr, owner: &LibraryClass, side: MethodReceiver, visited: &mut BTreeSet<String>, paths: &mut BTreeSet<Vec<String>>) {
    if let Some(lambda) = invoked_lambda(expr) {
        if let ExprNode::Lambda { extra_params, body, .. } = &*lambda.node {
            walk_param_defaults(extra_params, owner, side, visited, paths);
            walk_load_time_paths_on(body, owner, side, visited, paths);
        } else {
            walk_load_time_paths_on(lambda, owner, side, visited, paths);
        }
    }
    if let ExprNode::Lambda { .. } = &*expr.node {
        return;
    }
    if let ExprNode::Const { path } = &*expr.node {
        paths.insert(path.iter().map(|part| part.as_str().to_string()).collect());
    }
    if let ExprNode::Send { recv, method, args, block, .. } = &*expr.node {
        let own = recv.is_none() || recv.as_ref().is_some_and(|recv| match &*recv.node {
            ExprNode::SelfRef => true,
            ExprNode::Const { path } => path.iter().map(|part| part.as_str()).filter(|part| !part.is_empty()).collect::<Vec<_>>().join("::") == owner.name.0.as_str(),
            _ => false,
        });
        let overridden_new = owner.methods.iter().any(|m| m.name.as_str() == "new" && m.receiver == MethodReceiver::Class);
        let (called, on) = if method.as_str() == "new" && side == MethodReceiver::Class && !overridden_new {
            ("initialize", MethodReceiver::Instance)
        } else {
            (method.as_str(), side)
        };
        // An overridden `new` still reaches `initialize` through its `super`, so both are walked.
        let targets: Vec<(&str, MethodReceiver)> = if method.as_str() == "new" && side == MethodReceiver::Class && overridden_new {
            vec![(called, on), ("initialize", MethodReceiver::Instance)]
        } else {
            vec![(called, on)]
        };
        for (called, on) in targets {
            if own && visited.insert(format!("{}{called}", if on == MethodReceiver::Class { "." } else { "#" })) {
                for method in owner.methods.iter().filter(|method| method.name.as_str() == called && method.receiver == on) {
                    walk_param_defaults(&method.params, owner, on, visited, paths);
                    if let Some(block) = &method.block_param {
                        walk_param_defaults(std::slice::from_ref(block), owner, on, visited, paths);
                    }
                    walk_load_time_paths_on(&method.body, owner, on, visited, paths);
                }
            }
        }
        for value in recv.iter().chain(args) {
            walk_load_time_paths_on(value, owner, side, visited, paths);
        }
        let stores_block = matches!(method.as_str(), "proc" | "lambda")
            || (method.as_str() == "new" && matches!(recv.as_ref().map(|recv| &*recv.node),
                Some(ExprNode::Const { path }) if path.len() == 1 && path[0].as_str() == "Proc"));
        if !stores_block {
            if let Some(block) = block {
                let body = match &*block.node { ExprNode::Lambda { body, .. } => body, _ => block };
                walk_load_time_paths_on(body, owner, side, visited, paths);
            }
        }
        return;
    }
    expr.node.for_each_child(&mut |child| walk_load_time_paths_on(child, owner, side, visited, paths));
}

struct NestedEmitter<'a> {
    pending: Vec<&'a NestedBlock>,
    names: BTreeSet<String>,
}

impl NestedEmitter<'_> {
    fn write_names(&mut self, out: &mut String, depth: usize, names: &BTreeSet<String>) {
        while let Some(at) = self.pending.iter().position(|block| names.contains(&block.name)) {
            let block = self.pending.remove(at);
            self.write_names(out, depth, &block.requires);
            writeln!(out, "{}", indent_lines(&block.content, depth)).unwrap();
        }
    }

    fn write_expr(&mut self, out: &mut String, depth: usize, owner: &LibraryClass, expr: &Expr) {
        if self.pending.is_empty() { return; }
        let mut paths = BTreeSet::new();
        walk_load_time_paths(expr, owner, &mut BTreeSet::new(), &mut paths);
        self.write_names(out, depth, &nested_names_in_paths(owner, &paths, &self.names));
    }

    fn write_include(&mut self, out: &mut String, depth: usize, owner: &LibraryClass, include: &ClassId) {
        if self.pending.is_empty() { return; }
        let paths = BTreeSet::from([include.0.as_str().split("::").map(str::to_string).collect()]);
        self.write_names(out, depth, &nested_names_in_paths(owner, &paths, &self.names));
    }
}

/// A nested class's own declaration out of its standalone render, at
/// column 0: `module Stages` … `end`. The render opens either with the
/// parent's `depth` wrapper lines (`module WebPush` / `class
/// Connections`) around the child's own header, or — when an outer
/// segment is the runtime's — with ONE compact header naming the whole
/// path (`module WebPush::Connections::Stages`), which is renamed to
/// its last segment.
fn child_block(body: &[&str], depth: usize, full_name: &str) -> Option<String> {
    let first = body.iter().position(|l| !l.trim().is_empty())?;
    let header = body[first];
    for keyword in ["class ", "module "] {
        let compact = format!("{keyword}{full_name}");
        if header == compact || header.starts_with(&format!("{compact} ")) {
            let last = body.iter().rposition(|l| l.trim() == "end")?;
            let simple = full_name.rsplit("::").next().unwrap_or(full_name);
            let renamed = format!("{keyword}{simple}{}", &header[compact.len()..]);
            let mut out = vec![renamed];
            out.extend(body[first + 1..=last].iter().map(|l| l.to_string()));
            return Some(out.join("\n"));
        }
    }
    unwrapped(body, depth).map(|b| dedent_lines(&b, depth))
}

fn indent_lines(block: &str, levels: usize) -> String {
    let pad = "  ".repeat(levels);
    block
        .lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("{pad}{l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `block` with `levels` two-space indents taken off each line that has
/// them.
fn dedent_lines(block: &str, levels: usize) -> String {
    let cut = "  ".repeat(levels);
    block
        .lines()
        .map(|l| l.strip_prefix(cut.as_str()).unwrap_or(l.trim_start()))
        .collect::<Vec<_>>()
        .join("\n")
}

use crate::facades::{Facade, EXTRAS_FACADES};

/// Swap façade-fated extras emits (scaffold base: spinel + the trees
/// derived from it). No-op when the app doesn't define the class —
/// the path simply isn't present.
pub(super) fn apply_extras_facades(files: &mut [(String, String)], app: &App) {
    for f in EXTRAS_FACADES.iter().filter(|f| f.applies(app)) {
        let Facade { stem, rb, rbs, .. } = f;
        for (path, content) in files.iter_mut() {
            if path == &format!("{stem}.rb") {
                *content = (*rb).to_string();
            } else if path == &format!("{stem}.rbs") || path == &format!("sig/{stem}.rbs") {
                // BOTH forms, and the `sig/` one is the one that fires.
                // `emit_library_class_rbs` writes sidecars `sig/`-rooted
                // (`sig_path_for`), and `spin_shape` only flattens them to
                // file-adjacent LATER — so at this point every sidecar is
                // still `sig/app/models/<stem>.rbs`. Matching only the bare
                // form meant this arm never fired for any of the four
                // façades: the `.rb` swapped, the `.rbs` did not, and every
                // consumer read roundhouse's own inference instead of the
                // hand-written contract sitting beside the façade.
                //
                // What that cost, end to end: the (since retired) Sponge
                // contract declared
                // `OpenSSL::RandomSource.random_bytes: (Integer) -> String`,
                // and without it that call typed untyped, which widened
                // `str` in `Utils.random_str`, which made
                // `CandidateId#to_s` untyped, which put a
                // `def to_s: () -> untyped` in the program — and one of
                // those widens every poly `.to_s` (matz/spinel#4090). Ten
                // links on, the cookie jar's `@inbound[k.to_s] = v` had
                // untyped KEYS and the C build stopped on a PolyPolyHash
                // reaching a `Hash[String, String]` slot.
                *content = (*rbs).to_string();
            }
        }
    }
}

/// CRuby: put the verbatim source-shape emit back over the façades —
/// the real net/https / resolv / ipaddr are available there and the
/// vendored bodies run as written. Re-renders the library classes so
/// the restored bytes are exactly what the base would have emitted
/// without the swap.
pub(super) fn restore_extras_facades(files: &mut [(String, String)], app: &App) {
    let is_facade = |p: &str| {
        EXTRAS_FACADES.iter().any(|f| p == format!("{}.rb", f.stem) || p == format!("{}.rbs", f.stem))
    };
    if !files.iter().any(|(p, _)| is_facade(p)) {
        return;
    }
    for ef in emit_library_class_decls(app) {
        let p = ef.path.to_string_lossy().into_owned();
        if EXTRAS_FACADES
            .iter()
            .any(|f| p == format!("{}.rb", f.stem) || p == format!("{}.rbs", f.stem))
        {
            for (path, content) in files.iter_mut() {
                if *path == p {
                    content.clone_from(&ef.content);
                }
            }
        }
    }
}

/// Ruby-family pre-emit pass: a partial render in a LIBRARY-CLASS body
/// (lobsters' ApplicationHelper#link_post renders a partial from a
/// helper). A helper's render RETURNS the string, so the rewrite is the
/// bare `Views::<Mod>.<stem>(record, closure…, extras…)` call — locals
/// bind by name against the partial's contract, everything else nil
/// (a module body has no controller ivars to thread). Slashed partial
/// names only; a bare name has no module context here.
///
/// BOTH spellings Rails accepts: `render partial: "x/y", locals: {…}`
/// and the SHORTHAND `render "x/y"` (locals, if any, ride a trailing
/// hash). campfire's `MessagesHelper#message_tag` uses the shorthand in
/// a `rescue` body — `render "messages/unrenderable"` is what a message
/// row renders when building it raised — and unclaimed it emitted as a
/// bare `render` call, which is a method a helper module does not
/// have.
///
/// A name the locals do not bind falls back to the enclosing method's
/// own PARAMETER of that name before it falls back to nil. Same policy
/// as the controller-side rewrite ("a same-named local wins"), and here
/// it is what keeps the call type-correct: the partial's record
/// parameter is typed as its record (`unrenderable(Message message,
/// …)`), so a nil there is a seed contradiction that stops a strict
/// build. The divergence it buys is bounded and in the safe direction —
/// Rails supplies no local at all for a shorthand render, so a partial
/// that reads one raises there; ours renders with the caller's
/// same-named value.
pub(crate) fn apply_library_partial_render_lowering(lcs: &mut [LibraryClass], app: &App) {
    let contracts = crate::lower::view_to_library::partial_call_contracts(
        &app.views,
        &app.controllers,
        &app.library_classes,
    );
    if contracts.is_empty() {
        return;
    }
    let helper_modules = helper_module_ids(app);
    for lc in lcs.iter_mut() {
        let in_helper = helper_modules.contains(&lc.name);
        for m in &mut lc.methods {
            let in_scope: Vec<Symbol> = m.params.iter().map(|p| p.name.clone()).collect();
            rewrite_library_partial_render(&mut m.body, &contracts, &in_scope, in_helper);
        }
    }
}

/// Symbol-keyed entries of a hash ARG, as partial locals. Rails lets a
/// shorthand render carry its locals in a trailing hash (`render
/// "messages/message", message: m`), which is the same map `locals:`
/// spells one level down.
fn hash_locals(arg: Option<&Expr>) -> Vec<(Symbol, Expr)> {
    use crate::expr::Literal;
    let Some(arg) = arg else { return Vec::new() };
    let ExprNode::Hash { entries, .. } = &*arg.node else { return Vec::new() };
    entries
        .iter()
        .filter_map(|(k, v)| match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some((value.clone(), v.clone())),
            _ => None,
        })
        .collect()
}

fn library_partial_render(expr: &Expr) -> Option<(String, Vec<(Symbol, Expr)>)> {
    use crate::expr::Literal;
    let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { return None };
    if method.as_str() != "render" && method.as_str() != "render_to_string" {
        return None;
    }
    let first = args.first()?;
    let mut partial: Option<String> = None;
    let mut locals: Vec<(Symbol, Expr)> = Vec::new();
    match &*first.node {
        // The shorthand — the string IS the partial name.
        ExprNode::Lit { value: Literal::Str { value } } => {
            partial = Some(value.clone());
            locals = hash_locals(args.get(1));
        }
        ExprNode::Hash { entries, kwargs: true } => {
            for (k, v) in entries {
                let key = match &*k.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => value.as_str(),
                    _ => "",
                };
                match key {
                    "partial" => {
                        if let ExprNode::Lit { value: Literal::Str { value } } = &*v.node {
                            partial = Some(value.clone());
                        }
                    }
                    "locals" => {
                        if let ExprNode::Hash { entries: le, .. } = &*v.node {
                            for (lk, lv) in le {
                                if let ExprNode::Lit { value: Literal::Sym { value } } = &*lk.node {
                                    locals.push((value.clone(), lv.clone()));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => return None,
    }
    Some((partial?, locals))
}

fn library_partial_key(pname: &str) -> Option<(String, String)> {
    let (dir, base) = pname.rsplit_once('/')?;
    Some((
        crate::naming::camelize_path(&crate::naming::snake_case(dir)),
        base.trim_start_matches('_').to_string(),
    ))
}

fn rewrite_library_partial_render(
    expr: &mut Expr,
    contracts: &crate::det::HashMap<
        (String, String),
        crate::lower::view_to_library::PartialCallContract,
    >,
    in_scope: &[Symbol],
    in_helper: bool,
) {
    use crate::expr::Literal;
    expr.node
        .for_each_child_mut(&mut |c| rewrite_library_partial_render(c, contracts, in_scope, in_helper));
    let Some((pname, locals)) = library_partial_render(expr) else { return };
    let Some((module_camel, stem)) = library_partial_key(&pname) else { return };
    let Some(contract) = contracts.get(&(module_camel.clone(), stem)) else { return };
    let span = expr.span;
    let nil = || sp_expr(ExprNode::Lit { value: Literal::Nil });
    let locals_lookup = |name: &str| -> Option<Expr> {
        locals
            .iter()
            .find(|(k, _)| k.as_str() == name)
            .map(|(_, v)| v.clone())
    };
    let param_lookup = |name: &str| -> Option<Expr> {
        in_scope.iter().find(|p| p.as_str() == name).map(|p| {
            Expr::new(span, ExprNode::Var { id: VarId(0), name: p.clone() })
        })
    };
    let lookup = |name: &str| -> Option<Expr> {
        locals_lookup(name).or_else(|| param_lookup(name))
    };
    let caller_flash = [
        crate::lower::view_to_library::flash_param_in_scope(in_scope, "notice"),
        crate::lower::view_to_library::flash_param_in_scope(in_scope, "alert"),
    ];
    let caller_flash = match (&caller_flash[0], &caller_flash[1]) {
        (Some(n), Some(a)) => Some([n.clone(), a.clone()]),
        _ => None,
    };
    let mut view_args: Vec<Expr> = Vec::new();
    view_args.push(lookup(&contract.record).unwrap_or_else(nil));
    for n in &contract.closure {
        // Not the helper's same-named param: the partial's ivar is the rendering template's, which `rewrite_helper_ivars` reaches.
        let ivar = in_helper.then(|| Expr::new(span, ExprNode::Ivar { name: Symbol::from(n.as_str()) }));
        view_args.push(locals_lookup(n).or(ivar).or_else(|| param_lookup(n)).unwrap_or_else(nil));
    }
    view_args.extend(contract.extras_args(
        locals_lookup,
        param_lookup,
        caller_flash.as_ref(),
        nil,
        span,
    ));
    *expr = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(sp_expr(ExprNode::Const {
                path: vec![Symbol::from("Views"), Symbol::from(module_camel)],
            })),
            method: contract.method.clone(),
            args: view_args,
            block: None,
            parenthesized: true,
        },
    );
}

/// Ruby-family pre-emit pass: synthesize each model's scope class methods
/// and normalize scope chains (`Story.base(u).positive_ranked` ->
/// `Story.positive_ranked(Story.base(u))`) across a group of library
/// classes, so they run against `ActiveRecord::Relation`. Lives on the
/// Ruby emit path — these methods reference a runtime only the CRuby/JRuby
/// tree provides, so the shared `lower/` must stay target-agnostic. Run
/// once per LC group (models / controllers / library_classes) before
/// rendering. A strict no-op for scope-free apps (the blog).
/// Generate `app/models/relation_scopes.rb` — an
/// `ActiveRecord::Relation` reopen delegating each declared model
/// scope to the model's fixed-arity `__scope_` relation entry points
/// (`push_scope_variants`), so a scope chained on a relation VALUE
/// (lobsters' StoriesPaginator: `@scope.limit(n).for_presentation`
/// where `@scope` is an untyped ctor param) resolves without static
/// receiver-type knowledge. One def per scope NAME serves every model
/// sharing it, whatever their arities: SCOPE_UNSET sentinels detect
/// how many positionals the caller supplied, and each arm forwards
/// that exact shape. Positional-only arms dispatch through `klass` —
/// every model's `__scope_<name>__<n>` entry has the same full arity
/// by construction, which is what spinel's class-value dispatch
/// requires (it neither defaults omitted optionals through the
/// dispatch nor accepts kwargs). Keyword-carrying arms dispatch
/// through `case klass.name` constant receivers instead. An argument
/// shape no model accepts raises ArgumentError at the call, as Rails
/// would. Statically resolvable — explicit defs, no method_missing,
/// no splats. Emitted under app/models/ so the aggregator loads it.
/// None when the app declares no scopes. A name that can't render
/// (rest param, too many keywords, non-tail optional positionals,
/// `?`/`!` name) is skipped with a `lower_residue` diagnostic and a
/// mid-chain call raises NoMethodError.
pub(crate) fn emit_relation_scope_delegates(app: &App) -> Option<EmittedFile> {
    // Names Relation itself defines never delegate — a scope named
    // like a builtin would already dispatch to the builtin under
    // Rails' merge semantics, and the reopen must not clobber the
    // query-builder surface.
    const RELATION_BUILTINS: &[&str] = &[
        "where", "not", "or", "order", "limit", "offset", "group", "having", "joins",
        "left_outer_joins", "left_joins", "select", "distinct", "includes", "preload",
        "find", "find_by", "first", "last", "all", "each", "map", "to_a", "count",
        "exists?", "empty?", "any?", "none?", "sum", "maximum", "minimum", "pluck",
        "pick", "destroy_all", "delete_all", "update_all", "klass", "model", "where_clauses",
        "spawn", "find_each", "find_in_batches",
    ];
    let scopes = crate::lower::scope_chain::build_scope_registry(&app.models);
    // name -> [(model, params)] in app-model order, names sorted — the
    // generated file must be byte-stable across runs.
    let mut by_name: std::collections::BTreeMap<
        String,
        Vec<(&crate::ident::ClassId, &[crate::dialect::Param])>,
    > = Default::default();
    for model in &app.models {
        // The model's own scopes, then each ancestor's it does not declare
        // itself: an `ApplicationRecord` scope reached on `Article`'s
        // relation calls `Article.__scope_…`, which Ruby resolves to the
        // ancestor's entry.
        let mut chain: Vec<&crate::ident::ClassId> = Vec::new();
        let mut current = Some(&model.name);
        while let Some(id) = current.filter(|id| chain.len() < 32 && !chain.contains(id)) {
            chain.push(id);
            current = app.models.iter().find(|m| &m.name == id).and_then(|m| m.parent.as_ref());
        }
        let mut per: std::collections::HashMap<&Symbol, &Vec<crate::dialect::Param>> = Default::default();
        for owner in chain {
            for (n, params) in scopes.get(owner).into_iter().flatten() {
                per.entry(n).or_insert(params);
            }
        }
        if per.is_empty() {
            continue;
        }
        // The registry carries the SYNTHESIZED preload scopes too (it
        // must — the scope-body rewriter reads it to thread `__rel`
        // through a bare `with_attached_attachment`). They are handled
        // by the identity arm below, which is both faster and clearer
        // than a `__scope_` hop to a body that returns its argument;
        // leaving them here would make `by_name` claim them and the
        // identity arm skip them as "a declared scope of the same name".
        let plain_preload = if crate::lower::plain_text_attr::record_table_present(&app.schema) {
            crate::lower::plain_text_attr::preload_scope_names(model)
        } else {
            Vec::new()
        };
        let synthesized: crate::det::HashSet<String> =
            crate::lower::rich_text::preload_scope_names(model)
                .into_iter()
                .chain(plain_preload)
                .chain(crate::lower::attached::preload_scope_names(model))
                .map(|n| n.as_str().to_string())
                .collect();
        let mut names: Vec<&Symbol> = per.keys().copied().collect();
        names.sort_by_key(|n| n.as_str());
        for n in names {
            if RELATION_BUILTINS.contains(&n.as_str()) || synthesized.contains(n.as_str()) {
                continue;
            }
            by_name
                .entry(n.as_str().to_string())
                .or_default()
                .push((&model.name, per[n].as_slice()));
        }
    }
    // The SYNTHESIZED preload scopes — `with_attached_<attr>`,
    // `with_rich_text_<attr>`, `with_markdown_<attr>` — which Rails
    // declares beside the attachment / Action Text macros and this
    // compiler adds at emit time. They never pass through
    // `build_scope_registry`, which reads the app's own `scope`
    // declarations, so a call CHAINED ON A RELATION had no delegate at
    // all: campfire's
    // `find_autocompletable_users.with_attached_avatar.ordered` is a
    // NoMethodError on a class method that plainly exists, because the
    // receiver is a relation value and not the class.
    //
    // The delegate is `preload(:<assoc>)`, with no `__scope_` dispatch
    // behind it: there is no arity to detect and no model to pick,
    // because every model that declares the attachment preloads the
    // same named association (`<attr>_attachment`, `rich_text_<attr>`,
    // `markdown_<attr>`), and a model that does not never has the name
    // reached on it. It used to answer `self` — the scopes were
    // identity while the readers queried per record — and a delegate
    // that preloads is what lets the batch loader run when the scope is
    // reached mid-chain, not only from the class.
    let mut preloads: std::collections::BTreeMap<String, String> = Default::default();
    for model in &app.models {
        let plain_scopes = if crate::lower::plain_text_attr::record_table_present(&app.schema) {
            crate::lower::plain_text_attr::preload_scopes(model)
        } else {
            Vec::new()
        };
        let rich = crate::lower::rich_text::preload_scopes(model).into_iter().map(|(n, assoc)| {
            let spec = match crate::lower::rich_text::preload_scope_nested(&n) {
                Some(nested) => format!("{{ {}: :{} }}", assoc.as_str(), nested.as_str()),
                None => format!(":{}", assoc.as_str()),
            };
            (n, spec)
        });
        let scopes = rich.chain(
            plain_scopes
                .into_iter()
                .chain(crate::lower::attached::preload_scopes(model))
                .map(|(n, assoc)| (n, format!(":{}", assoc.as_str()))),
        );
        for (n, spec) in scopes {
            let n = n.as_str().to_string();
            // A declared scope of the same name wins — it has a real
            // body, and shadowing it would drop a filter.
            // A Relation builtin is never delegated, same rule as above.
            if by_name.contains_key(&n) || RELATION_BUILTINS.contains(&n.as_str()) {
                continue;
            }
            preloads.insert(n, spec);
        }
    }
    // Class methods some call site reaches THROUGH a relation, which
    // take that relation as a trailing `__rel` (see
    // `scope_chain::survey_assoc_class_methods`). The call-site rewrite
    // re-roots the chains it can recognize; a delegate here answers the
    // rest, whatever built the relation: campfire's
    // `Current.user.reachable_messages.search(q).last_page_of_matches(n)`
    // is a through-association, with no foreign key to seed from.
    let assocs = crate::lower::scope_chain::build_assoc_registry(&app.models);
    let (assoc_class_methods, _) =
        crate::lower::scope_chain::survey_assoc_class_methods(app, &assocs, &scopes);
    let mut class_methods: std::collections::BTreeMap<
        String,
        Vec<(&crate::ident::ClassId, Vec<crate::dialect::Param>)>,
    > = Default::default();
    for model in &app.models {
        let Some(per) = assoc_class_methods.get(&model.name) else { continue };
        let mut names: Vec<&Symbol> = per.keys().collect();
        names.sort_by_key(|n| n.as_str());
        for n in names {
            let key = n.as_str().to_string();
            if RELATION_BUILTINS.contains(&key.as_str())
                || by_name.contains_key(&key)
                || preloads.contains_key(&key)
            {
                continue;
            }
            class_methods.entry(key).or_default().push((&model.name, per[n].params.clone()));
        }
    }
    if by_name.is_empty() && preloads.is_empty() && class_methods.is_empty() {
        return None;
    }
    let mut skipped: Vec<String> = Vec::new();
    let mut body = String::new();
    for (name, decls) in &class_methods {
        match render_class_method_delegate(name, decls) {
            Ok(text) => body.push_str(&text),
            Err(reason) => {
                let borrowed: Vec<(&crate::ident::ClassId, &[crate::dialect::Param])> =
                    decls.iter().map(|(m, p)| (*m, p.as_slice())).collect();
                push_delegate_skip_diagnostic(name, &reason, &borrowed);
                skipped.push(name.clone());
            }
        }
    }
    for (name, decls) in &by_name {
        match render_scope_delegate(name, decls) {
            Ok(text) => body.push_str(&text),
            Err(reason) => {
                push_delegate_skip_diagnostic(name, &reason, decls);
                skipped.push(name.clone());
            }
        }
    }
    for (name, spec) in &preloads {
        body.push_str("\n    # Preload scope (Rails' `includes`): the association it names\n");
        body.push_str("    # joins this relation's preload specs, batched at `to_a`.\n");
        writeln!(body, "    def {name}").unwrap();
        writeln!(body, "      preload({spec})\n    end").unwrap();
    }
    let mut s = String::from(
        "# Generated Relation scope delegation (see\n\
         # emit_relation_scope_delegates): each model scope, callable on a\n\
         # relation value mid-chain, forwarding the caller's exact argument\n\
         # shape to the model's fixed-arity __scope_ entry — SCOPE_UNSET\n\
         # sentinels detect how many positionals were supplied.\n",
    );
    if !skipped.is_empty() {
        writeln!(
            s,
            "# No delegate (see the lower_residue diagnostics) — a mid-chain\n\
             # call raises NoMethodError: {}",
            skipped.join(", ")
        )
        .unwrap();
    }
    s.push_str("module ActiveRecord\n\x20\x20class Relation\n");
    s.push_str(
        "\x20\x20\x20\x20# Argument-omitted sentinel for the delegates below:\n\
         \x20\x20\x20\x20# distinguishes an omitted optional from every real value\n\
         \x20\x20\x20\x20# including nil. Compared with equal?, never ==.\n\
         \x20\x20\x20\x20SCOPE_UNSET = Object.new\n",
    );
    s.push_str(&body);
    s.push_str("  end\nend\n");
    Some(EmittedFile {
        path: PathBuf::from("app/models/relation_scopes.rb"),
        content: s,
    })
}

/// Everything the arm renderers need about one delegate name.
struct DelegateCtx<'a> {
    name: &'a Symbol,
    /// Display names for the delegate's positional params.
    pos_names: &'a [String],
    /// Every model declaring the name, with its admissible shape.
    shapes: &'a [(&'a crate::ident::ClassId, crate::lower::scope_chain::DelegableShape<'a>)],
    /// Max positional count across the shapes.
    n_max: usize,
}

/// One delegate def for `name` across every model declaring it, or the
/// reason it can't render (fed to the skip diagnostic).
/// One Relation delegate for a relation-taking class method: the
/// caller's positional arguments, then the relation itself as `__rel`.
/// Dispatched on `klass.name` with a constant receiver per model, so a
/// relation of a model without the method raises NoMethodError as Rails
/// would. Required positionals only; anything else is skipped and
/// reported.
fn render_class_method_delegate(
    name: &str,
    decls: &[(&crate::ident::ClassId, Vec<crate::dialect::Param>)],
) -> Result<String, String> {
    let arity = decls[0].1.len();
    for (model, params) in decls {
        if params.len() != arity {
            return Err(format!("arity differs across models ({})", model.0.as_str()));
        }
        if params.iter().any(|p| p.default.is_some() || p.keyword || p.rest || p.forwarding || p.name.as_str().is_empty()) {
            return Err(format!(
                "shape on {} is not required positionals only",
                model.0.as_str()
            ));
        }
    }
    let names: Vec<String> = decls[0].1.iter().map(|p| p.name.as_str().to_string()).collect();
    let sig = names.join(", ");
    let mut call_args = names.clone();
    call_args.push("self".to_string());
    let call = call_args.join(", ");
    let mut out = String::new();
    out.push_str("\n    # Class method run against this relation: the relation is its `__rel`.\n");
    if sig.is_empty() {
        writeln!(out, "    def {name}").unwrap();
    } else {
        writeln!(out, "    def {name}({sig})").unwrap();
    }
    out.push_str("      case klass.name\n");
    for (model, _) in decls {
        let m = model.0.as_str();
        writeln!(out, "      when \"{m}\" then {m}.{name}({call})").unwrap();
    }
    writeln!(
        out,
        "      else raise NoMethodError, \"undefined method '{name}' for a relation of #{{klass.name}}\""
    )
    .unwrap();
    out.push_str("      end\n    end\n");
    Ok(out)
}

fn render_scope_delegate(
    name: &str,
    decls: &[(&crate::ident::ClassId, &[crate::dialect::Param])],
) -> Result<String, String> {
    use crate::lower::scope_chain::{delegable_name, DelegableShape, MAX_DELEGATE_KEYWORDS};
    let name_sym = Symbol::from(name);
    if !delegable_name(&name_sym) {
        return Err("`?`/`!` names take no __scope_ arity mangling".into());
    }
    let mut shapes: Vec<(&crate::ident::ClassId, DelegableShape)> = Vec::new();
    for (model, params) in decls {
        let Some(shape) = DelegableShape::of(params) else {
            return Err(format!(
                "shape on {} has a rest param, more than {MAX_DELEGATE_KEYWORDS} keywords, \
                 or optional positionals not forming a tail",
                model.0.as_str()
            ));
        };
        shapes.push((*model, shape));
    }
    let n_max = shapes
        .iter()
        .map(|(_, s)| s.positionals.len())
        .max()
        .unwrap_or(0);
    // Keyword union across models, sorted by name — subset and variant
    // naming must line up with keyword_subsets/scope_variant_name.
    let mut kw_union: Vec<&crate::dialect::Param> = Vec::new();
    for (_, s) in &shapes {
        for kw in &s.keywords {
            if !kw_union.iter().any(|k| k.name == kw.name) {
                kw_union.push(kw);
            }
        }
    }
    kw_union.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    if kw_union.len() > MAX_DELEGATE_KEYWORDS {
        return Err(format!(
            "keyword union across models exceeds {MAX_DELEGATE_KEYWORDS}"
        ));
    }
    if kw_union.iter().any(|k| k.name.as_str() == "klass") {
        return Err("a keyword named `klass` would shadow Relation#klass".into());
    }
    // Positional display names: the models' own param name where every
    // shape agrees at that position, `a<i>` otherwise (or when the name
    // would shadow Relation#klass in the forward).
    let pos_names: Vec<String> = (0..n_max)
        .map(|i| {
            let mut names = shapes
                .iter()
                .filter_map(|(_, s)| s.positionals.get(i).map(|p| p.name.as_str()));
            let n = match names.next() {
                Some(first) if names.all(|other| other == first) => first.to_string(),
                _ => format!("a{i}"),
            };
            if n == "klass" { format!("a{i}") } else { n }
        })
        .collect();
    let mut s = String::new();
    let mut decl: Vec<String> = pos_names
        .iter()
        .map(|n| format!("{n} = SCOPE_UNSET"))
        .collect();
    for kw in &kw_union {
        decl.push(format!("{}: SCOPE_UNSET", kw.name.as_str()));
    }
    if decl.is_empty() {
        writeln!(s, "\x20\x20\x20\x20def {name}").unwrap();
    } else {
        writeln!(s, "\x20\x20\x20\x20def {name}({})", decl.join(", ")).unwrap();
    }
    let ctx = DelegateCtx { name: &name_sym, pos_names: &pos_names, shapes: &shapes, n_max };
    render_kw_tree(&mut s, 3, &kw_union, &mut Vec::new(), &ctx);
    writeln!(s, "\x20\x20\x20\x20end").unwrap();
    Ok(s)
}

/// Branch on which of the delegate's keywords the caller supplied
/// (each independently sentinel-checked), then render the arity arms
/// for that exact keyword subset at the leaf.
fn render_kw_tree<'a>(
    out: &mut String,
    level: usize,
    remaining: &[&'a crate::dialect::Param],
    selected: &mut Vec<&'a crate::dialect::Param>,
    ctx: &DelegateCtx,
) {
    let pad = "\x20\x20".repeat(level);
    match remaining.split_first() {
        None => {
            let mut subset = selected.clone();
            subset.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
            render_arity_arms(out, level, &subset, ctx);
        }
        Some((kw, rest)) => {
            writeln!(out, "{pad}if SCOPE_UNSET.equal?({})", kw.name.as_str()).unwrap();
            render_kw_tree(out, level + 1, rest, selected, ctx);
            writeln!(out, "{pad}else").unwrap();
            selected.push(kw);
            render_kw_tree(out, level + 1, rest, selected, ctx);
            selected.pop();
            writeln!(out, "{pad}end").unwrap();
        }
    }
}

/// The supplied-arity arms for one keyword subset: arm `n` fires when
/// `a<n>` is the first unset positional (guards run in ascending order,
/// each returning or raising). Positional-only arms go through `klass`;
/// keyword arms need constant receivers (`case klass.name`) because
/// kwargs don't survive spinel's class-value dispatch.
fn render_arity_arms(
    out: &mut String,
    level: usize,
    subset: &[&crate::dialect::Param],
    ctx: &DelegateCtx,
) {
    use crate::lower::scope_chain::scope_variant_name;
    let pad = "\x20\x20".repeat(level);
    let kw_list = subset
        .iter()
        .map(|k| format!("{}:", k.name.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    for n in 0..=ctx.n_max {
        // Models whose shape accepts exactly this call: arity within
        // the positional range, subset within the declared keywords,
        // every required keyword supplied.
        let eligible: Vec<&crate::ident::ClassId> = ctx
            .shapes
            .iter()
            .filter(|(_, s)| {
                n >= s.min_required
                    && n <= s.positionals.len()
                    && subset
                        .iter()
                        .all(|kw| s.keywords.iter().any(|d| d.name == kw.name))
                    && s.required_keywords()
                        .iter()
                        .all(|r| subset.iter().any(|kw| &kw.name == *r))
            })
            .map(|(m, _)| *m)
            .collect();
        let guard = (n < ctx.n_max)
            .then(|| format!(" if SCOPE_UNSET.equal?({})", ctx.pos_names[n]))
            .unwrap_or_default();
        let pos_args: String = ctx.pos_names[..n]
            .iter()
            .map(|p| format!(", {p}"))
            .collect();
        if subset.is_empty() {
            let line = if eligible.is_empty() {
                format!(
                    "raise ArgumentError, \"scope {} does not accept {n} positional argument(s)\"",
                    ctx.name.as_str()
                )
            } else {
                format!(
                    "return klass.{}(self{pos_args})",
                    scope_variant_name(ctx.name, n, &[]).as_str()
                )
            };
            writeln!(out, "{pad}{line}{guard}").unwrap();
        } else if eligible.is_empty() {
            writeln!(
                out,
                "{pad}raise ArgumentError, \"scope {} does not accept ({kw_list}) with {n} \
                 positional argument(s)\"{guard}",
                ctx.name.as_str()
            )
            .unwrap();
        } else {
            let kw_args: String = subset
                .iter()
                .map(|kw| format!(", {0}: {0}", kw.name.as_str()))
                .collect();
            let variant = scope_variant_name(ctx.name, n, subset);
            let inner = if n < ctx.n_max {
                writeln!(out, "{pad}if SCOPE_UNSET.equal?({})", ctx.pos_names[n]).unwrap();
                level + 1
            } else {
                level
            };
            let ipad = "\x20\x20".repeat(inner);
            writeln!(out, "{ipad}case klass.name").unwrap();
            for m in &eligible {
                writeln!(
                    out,
                    "{ipad}when \"{0}\" then return {0}.{1}(self{pos_args}{kw_args})",
                    m.0.as_str(),
                    variant.as_str()
                )
                .unwrap();
            }
            writeln!(
                out,
                "{ipad}else raise ArgumentError, \"scope {} with ({kw_list}) unavailable on \
                 #{{klass.name}}\"",
                ctx.name.as_str()
            )
            .unwrap();
            writeln!(out, "{ipad}end").unwrap();
            if n < ctx.n_max {
                writeln!(out, "{pad}end").unwrap();
            }
        }
    }
}

/// Ledger a name the delegate emitter skipped: modeling debt, not an
/// app error — `lower_residue`, Warning severity, one per name.
fn push_delegate_skip_diagnostic(
    name: &str,
    reason: &str,
    decls: &[(&crate::ident::ClassId, &[crate::dialect::Param])],
) {
    use crate::diagnostic::{Diagnostic, DiagnosticKind};
    let models = decls
        .iter()
        .map(|(m, _)| m.0.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let kind = DiagnosticKind::LowerResidue {
        pass: Symbol::from("relation_scope_delegates"),
        construct: Symbol::from(name),
        reason: Symbol::from(reason),
    };
    let d = Diagnostic {
        span: crate::span::Span::synthetic(),
        severity: Diagnostic::default_severity(&kind),
        kind,
        message: format!(
            "scope `{name}` (on {models}) gets no ActiveRecord::Relation mid-chain \
             delegate: {reason}; a mid-chain call raises NoMethodError"
        ),
    };
    crate::emit::diagnostics::push(d);
}

/// A class method a call site reaches through an association, whose
/// constructor cannot take the association's scope. Rails would have set
/// the foreign key here; we leave the call resolving against the folded
/// Array, which fails loudly, and say so.
fn push_assoc_scope_skip(model: &crate::ident::ClassId, method: &Symbol, reason: &str) {
    use crate::diagnostic::{Diagnostic, DiagnosticKind};
    let model = model.0.as_str();
    let name = method.as_str();
    let kind = DiagnosticKind::LowerResidue {
        pass: Symbol::from("assoc_class_method_scope"),
        construct: Symbol::from(format!("{model}.{name}")),
        reason: Symbol::from(reason),
    };
    let d = Diagnostic {
        span: crate::span::Span::synthetic(),
        severity: Diagnostic::default_severity(&kind),
        kind,
        message: format!(
            "`{model}.{name}` is called through an association but cannot take its scope: \
             {reason}; the foreign key Rails would preset is not set"
        ),
    };
    crate::emit::diagnostics::push(d);
}

/// Give `m` the trailing `__rel = ActiveRecord::Relation.new(self)` that
/// makes it relation-taking — in the params AND in the signature the
/// `.rbs` is written from.
///
/// The signature half is not optional: a body the typer could resolve
/// carries a `Ty::Fn`, and the emitted `.rbs` prefers it over the
/// params, so inserting on one side only publishes `() -> bool` for a
/// method every call site now passes a relation to. Placement is before
/// the first keyword in both, since `def f(__rel = …, k:)` is the only
/// legal ordering.
///
/// A method with a REST param (`*tags`) is the exception: an optional
/// POSITIONAL can't follow a splat (`def f(*tags, __rel = …)` is a
/// syntax error — no slot for it to default INTO), so there `__rel`
/// becomes an optional KEYWORD instead (`def f(*tags, __rel: …)`,
/// legal after a splat), typed as the Relation rather than Untyped so
/// the `.rbs` reads `?__rel: ActiveRecord::Relation` instead of
/// `?__rel: untyped` — nothing else about the method declares
/// keywords, so Untyped would be the only typed thing losing its type.
/// It goes ahead of a `**opts` (`def f(*tags, __rel: …, **opts)`), the
/// only thing a keyword can't follow; a `&blk` lives in `block_param`,
/// not `params`, and is rendered last anyway. A method without a rest
/// param is untouched: same positional-with-default as always.
fn insert_rel_param(m: &mut crate::dialect::MethodDef, rel_param: &Symbol, owner: &ClassId) -> bool {
    if m.params.iter().any(|p| p.forwarding) {
        crate::emit::diagnostics::report_unsupported(m.name_span, "ruby", "full argument forwarding",
            "full forwarding cannot use the relation-threading argument ABI");
        return false;
    }
    let has_rest = m.params.iter().any(|p| p.rest && !p.keyword);
    if has_rest {
        let at = m.params.iter().position(|p| p.rest && p.keyword).unwrap_or(m.params.len());
        m.params.insert(
            at,
            crate::dialect::Param::keyword(
                rel_param.clone(),
                Some(crate::lower::model_to_library::relation_new_self()),
            ),
        );
        if let Some(Ty::Fn { params, .. }) = &mut m.signature {
            let at = params
                .iter()
                .position(|p| {
                    matches!(p.kind, crate::ty::ParamKind::KeywordRest | crate::ty::ParamKind::Block)
                })
                .unwrap_or(params.len());
            params.insert(at, crate::ty::Param {
                name: rel_param.clone(),
                ty: Ty::Relation { of: owner.clone() }.into(),
                kind: crate::ty::ParamKind::Keyword { required: false },
            });
        }
        return true;
    }
    let insert_at = m.params.iter().position(|p| p.keyword).unwrap_or(m.params.len());
    m.params.insert(
        insert_at,
        crate::dialect::Param::with_default(
            rel_param.clone(),
            crate::lower::model_to_library::relation_new_self(),
        ),
    );
    if let Some(Ty::Fn { params, .. }) = &mut m.signature {
        let at = params
            .iter()
            .position(|p| matches!(p.kind, crate::ty::ParamKind::Keyword { .. }))
            .unwrap_or(params.len());
        params.insert(
            at,
            crate::ty::Param {
                name: rel_param.clone(),
                ty: Ty::Untyped.into(),
                kind: crate::ty::ParamKind::Optional,
            },
        );
    }
    true
}

/// Lower demanded model and association chains to Relations, including
/// scope-free apps; each body still has its own rewrite demand gate.
pub(crate) fn apply_scope_lowering(lcs: &mut [LibraryClass], app: &App) {
    // `has_rich_text` / `has_markdown` preload scopes, and
    // `has_one_attached`'s one. Ahead of the `any_scopes` early return
    // below, because an app can declare a rich-text / plain-text
    // attribute or an attachment and no `scope` at all — and these
    // still have to exist or every call site chaining through them is
    // a NoMethodError.
    // `attachable_sgid` for the models that mix in
    // `ActionText::Attachable` (campfire declares it one level down,
    // through `User::Mentionable`). Ruby-family only, like the
    // `MessageVerifier` the mint runs through.
    let attachable = crate::lower::attachable::attachable_models(app);
    for lc in lcs.iter_mut() {
        if let Some(model) = app.models.iter().find(|m| m.name == lc.name) {
            crate::lower::rich_text::push_preload_scope_methods(&mut lc.methods, model);
            if crate::lower::plain_text_attr::record_table_present(&app.schema) {
                crate::lower::plain_text_attr::push_preload_scope_methods(&mut lc.methods, model);
            }
            crate::lower::attached::push_preload_scope_methods(&mut lc.methods, model);
            crate::lower::attachable::push_attachable_sgid(&mut lc.methods, model, &attachable);
            crate::lower::broadcasts::push_to_gid_param(&mut lc.methods, model);
            crate::lower::job_payload::push_model_to_gid_uri(&mut lc.methods, model);
        }
    }
    let scopes = crate::lower::scope_chain::build_scope_registry(&app.models);
    let assocs = crate::lower::scope_chain::build_assoc_registry(&app.models);
    let query_arguments = crate::lower::scope_chain::query_arguments::QueryArguments::build(app);
    // Class methods reached THROUGH an association (`user.sessions
    // .start!`) — demand-gated on the call sites this app actually
    // writes, so an app without that shape emits exactly what it did
    // before. Surveyed over the whole App, not over `lcs`: this pass
    // runs once for models and again for controllers, and the demand
    // for `Session.start!` lives in a CONTROLLER while the parameter
    // has to be inserted on the MODEL.
    let (assoc_class_methods, declined) =
        crate::lower::scope_chain::survey_assoc_class_methods(app, &assocs, &scopes);
    for lc in lcs.iter_mut() {
        for method in &mut lc.methods {
            crate::lower::scope_chain::ground_literal_model_dispatch(&mut method.body, app, &assocs);
        }
    }
    // Reported by the pass that owns the model's own file — this runs
    // once per emitted family over a different `lcs`, and the ledger
    // line should appear once, beside the class it is about.
    for d in &declined {
        if lcs.iter().any(|lc| lc.name == d.model) {
            push_assoc_scope_skip(&d.model, &d.method, &d.reason);
        }
    }
    let models = crate::lower::scope_chain::model_set(&app.models);
    // A model-root query needs the same Relation seed even when the app
    // declares no scopes. Otherwise `Widget.order(...)` reaches no method
    // and `Widget.where.not(...)` reaches Base.where with no argument.
    // Keep the whole-app gate consistent with the per-body gate below.
    let mut wants_model_chain = false;
    crate::lower::for_each_hook_body_ref(app, &mut |body| {
        wants_model_chain = wants_model_chain
            || crate::lower::scope_chain::mentions_model_chain_start(body, &models);
    });
    // …and an association read continuing into relation surface
    // (`@user.notifications.offset(n)`) needs the seed whether or not
    // the app declares a single scope: without this the whole pass was
    // skipped for a scope-free app, and every such chain stayed on the
    // has_many reader's Array.
    // An association CONSTRUCTOR (`@room.messages.create!(…)`) and a
    // parked association read chained later count too: they are the same
    // reader-to-relation hop, and a scope-free app that only CREATES
    // through its associations skipped the pass and called `create!` on
    // the reader's Array (found building tests/model_scenario_lowerings.rs,
    // whose app declares no scope; campfire declares several, which is
    // why it never showed there).
    let mut wants_assoc_lookup = false;
    crate::lower::for_each_hook_body_ref(app, &mut |body| {
        wants_assoc_lookup = wants_assoc_lookup
            || crate::lower::scope_chain::mentions_assoc_lookup(body, &assocs)
            || crate::lower::scope_chain::mentions_assoc_constructor(body, &assocs)
            || crate::lower::scope_chain::mentions_assoc_alias(body, &assocs);
    });
    if !crate::lower::scope_chain::any_scopes(&scopes)
        && assoc_class_methods.is_empty()
        && !wants_assoc_lookup
        // An app with no scopes at all can still declare an association
        // extension, and its call sites need the same rewrite.
        && !crate::lower::scope_chain::any_assoc_extensions(&assocs)
        && !wants_model_chain
        && query_arguments.is_empty()
    {
        return;
    }
    let names = crate::lower::scope_chain::all_scope_names(&scopes);
    let user_returns = crate::lower::scope_chain::build_user_method_returns(&app.models);
    let unique_keys = crate::lower::scope_chain::build_unique_keys(&app.models, &app.schema);
    let regs = crate::lower::scope_chain::Registries {
        scopes: &scopes,
        models: &models,
        assocs: &assocs,
        assoc_class_methods: &assoc_class_methods,
        user_returns: &user_returns,
        unique_keys: &unique_keys,
        // The Ruby family's attrs Hash holds a record (see the field).
        owner_objects: true,
    };
    query_arguments.rewrite(lcs, &regs);
    let sole_includers = app.sole_includer_of_modules();
    for lc in lcs.iter_mut() {
        // Models gain their scope class methods (already chain-normalized).
        let is_model = app.models.iter().any(|m| m.name == lc.name);
        // A model CONCERN is a model body too — the third family
        // `lower::for_each_model_body` names. `module User::Bannable`'s
        // instance methods run on a `User`, so a bare `bans.create!` in
        // one is the same association read it would be in user.rb, and
        // resolving it needs the same owner. Without this the concern's
        // body reached `rewrite_call_site` with NO self model and the
        // reader's folded Array kept the call: `undefined method
        // 'create!' for an instance of Array`, which is what campfire's
        // `User::Bannable#create_bans_from_sessions` raised.
        //
        // MODULES ONLY, and only when the namespace names a model: a
        // plain nested class under a model namespace is its own
        // receiver, not the model's.
        let concern_owner: Option<crate::ident::ClassId> = (!is_model && lc.is_module)
            .then(|| lc.name.0.as_str().rsplit_once("::").map(|(ns, _)| ns.to_string()))
            .flatten()
            .map(|ns| crate::ident::ClassId(crate::ident::Symbol::from(ns.as_str())))
            .filter(|ns| app.models.iter().any(|m| &m.name == ns))
            // An STI SUBCLASS is the same fact by inheritance rather
            // than by namespace: `Rooms::Open < Room` declares no table,
            // so it is not a model, but its instance methods run on a
            // Room — and `memberships.grant_to(...)` in one is the same
            // association-extension call it would be in room.rb. Without
            // this it reached `rewrite_call_site` with no self model and
            // kept the un-flattened `memberships.grant_to`, which
            // nothing defines.
            .or_else(|| {
                if is_model {
                    return None;
                }
                let mut cursor = lc.parent.clone();
                for _ in 0..8 {
                    let parent = cursor?;
                    if app.models.iter().any(|m| m.name == parent) {
                        return Some(parent);
                    }
                    cursor = app
                        .library_classes
                        .iter()
                        .find(|other| other.name == parent)
                        .and_then(|other| other.parent.clone());
                }
                None
            })
            // Not only by namespace: a gem module that one model alone includes runs on that model.
            .or_else(|| {
                (lc.is_module && !is_model)
                    .then(|| sole_includers.get(&lc.name).cloned())
                    .flatten()
                    .filter(|owner| app.models.iter().any(|m| &m.name == owner))
            });
        if let Some(model) = app.models.iter().find(|m| m.name == lc.name) {
            crate::lower::model_to_library::push_scope_methods(
                &mut lc.methods,
                model,
                &scopes,
                &models,
                &assocs,
            );
        }
        // User-written class methods the registry admitted as
        // relation-taking (`def self.arrange_for_user` with bare
        // `order(...)` roots) get the same treatment push_scope_methods
        // gives declared scopes: a trailing `__rel =
        // ActiveRecord::Relation.new(self)` param, and their bare chain
        // roots threaded through it. Skip-if-last-param-is-__rel makes
        // this idempotent AND distinguishes user methods from the
        // already-synthesized scope methods.
        if is_model {
            let rel_param = Symbol::from("__rel");
            let registered: Vec<Symbol> = scopes
                .get(&lc.name)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            for m in &mut lc.methods {
                if m.receiver == MethodReceiver::Class
                    && registered.contains(&m.name)
                    // Any-position check: push_scope_methods inserts
                    // __rel BEFORE keyword params, so a kwarg-taking
                    // scope's __rel is not last.
                    && !m.params.iter().any(|p| p.as_str() == "__rel")
                {
                    if !insert_rel_param(m, &rel_param, &lc.name) { continue; }
                    crate::lower::scope_chain::rewrite_scope_body(
                        &mut m.body,
                        &lc.name,
                        &rel_param,
                        &scopes,
                        &models,
                        &assocs,
                    );
                }
            }
            // The association-scope side of the same idea: a class
            // method some call site reaches through a has_many takes
            // the relation the same way, and reads it in one or both of
            // two ways. As a CREATE scope — `create!(attrs)` becomes
            // `create!(__rel.scope_attributes.merge(attrs))`, which is
            // how Rails' `scope_for_create` gets `user_id` onto the row.
            // And as a QUERY scope — a bare `count` / `page_before(m)`
            // roots on `__rel` exactly as it would inside a scope body,
            // which is what makes `@room.messages.paged?` count THIS
            // room. The default `Relation.new(self)` is empty and
            // records no scope attributes, so a direct `Session.start!`
            // / `Message.paged?` is unchanged.
            let assoc_registered: Vec<(Symbol, bool, bool)> = assoc_class_methods
                .get(&lc.name)
                .map(|m| m.iter().map(|(n, e)| (n.clone(), e.creates, e.queries)).collect())
                .unwrap_or_default();
            for m in &mut lc.methods {
                let Some(&(_, creates, queries)) = assoc_registered
                    .iter()
                    .find(|(n, _, _)| *n == m.name)
                else {
                    continue;
                };
                if m.receiver != MethodReceiver::Class
                    || m.params.iter().any(|p| p.as_str() == "__rel")
                {
                    continue;
                }
                if !insert_rel_param(m, &rel_param, &lc.name) { continue; }
                if creates {
                    crate::lower::scope_chain::merge_scope_attributes(
                        &mut m.body,
                        &lc.name,
                        &rel_param,
                    );
                }
                if queries {
                    crate::lower::scope_chain::rewrite_assoc_scope_body(
                        &mut m.body,
                        &lc.name,
                        &rel_param,
                        &scopes,
                        &models,
                        &assocs,
                    );
                }
            }
        }
        // Scope bodies and relation-taking class method bodies hand their
        // relation on to the class methods they call at implicit self.
        if is_model {
            let rel_param = Symbol::from("__rel");
            for m in &mut lc.methods {
                if m.receiver == MethodReceiver::Class
                    && m.params.iter().any(|p| p.as_str() == "__rel")
                {
                    crate::lower::scope_chain::thread_rel_into_class_method_calls(
                        &mut m.body,
                        &lc.name,
                        &rel_param,
                        &assoc_class_methods,
                    );
                }
            }
        }
        // Every method body: normalize scope chains (call-site form).
        // Scope-free bodies still need the rewrite when they start a
        // query chain on a model constant — the arel inline pass bails
        // on dynamic-value where-hashes, and those chains only run
        // against a seeded Relation. A model's own CLASS methods
        // additionally seed bare implicit-self roots (`where(key: key)`
        // in `Keystore.value_for`), signalled via `class_self`.
        // Ivars every method that assigns them binds to one association
        // read, for the methods that only read them (a controller's
        // private helper after its `before_action`).
        let ivar_bindings = crate::lower::scope_chain::class_ivar_assoc_bindings(&lc.methods, &assocs);
        for m in &mut lc.methods {
            // A `__scope_` entry's forward body already carries `__rel`
            // exactly once — never re-thread it (re-entrancy guard for
            // a second apply_scope_lowering over the same lcs).
            if m.name.as_str().starts_with("__scope_") {
                continue;
            }
            let ivar_seeds = crate::lower::scope_chain::ivar_seeds_for(&m.body, &ivar_bindings);
            let class_self = (m.receiver == MethodReceiver::Class)
                .then(|| if is_model { Some(lc.name.clone()) } else { concern_owner.clone() })
                .flatten();
            // A model's own INSTANCE methods know their self model too —
            // `self.<has_many>.<scope>` there seeds a Relation from the
            // association's foreign key (recent_threads' comment chain).
            let instance_self = (m.receiver == MethodReceiver::Instance)
                .then(|| if is_model { Some(lc.name.clone()) } else { concern_owner.clone() })
                .flatten();
            let mentions = |e: &Expr| {
                crate::lower::scope_chain::mentions_scope(e, &names)
                    || crate::lower::scope_chain::mentions_model_chain_start(e, &models)
                    || crate::lower::scope_chain::mentions_assoc_constructor(e, &assocs)
                    || crate::lower::scope_chain::mentions_assoc_lookup(e, &assocs)
                    || crate::lower::scope_chain::mentions_assoc_alias(e, &assocs)
                    || crate::lower::scope_chain::mentions_assoc_extension(e, &assocs)
                    || crate::lower::scope_chain::mentions_model_insert_all(e, &models)
                    || crate::lower::scope_chain::mentions_assoc_class_method(
                        e,
                        &assocs,
                        &names,
                        &assoc_class_methods,
                    )
                    || (class_self.is_some()
                        && crate::lower::scope_chain::mentions_bare_chain_start(e))
            };
            if !ivar_seeds.is_empty() {
                crate::lower::scope_chain::rewrite_call_site_seeded(
                    &mut m.body,
                    &regs,
                    class_self.as_ref(),
                    instance_self.as_ref(),
                    &ivar_seeds,
                );
            } else if mentions(&m.body) {
                crate::lower::scope_chain::rewrite_call_site(
                    &mut m.body,
                    &regs,
                    class_self.as_ref(),
                    instance_self.as_ref(),
                );
            }
            // An instance method's parameter DEFAULT is evaluated in the
            // method's own scope, so it is a call site too — campfire's
            // `def notification(badge: user.memberships.unread.count, …)`.
            // (A class method's default stays as written; see
            // tests/default_self_recv.rs.)
            let instance_defaults = m.receiver == MethodReceiver::Instance;
            for p in m.params.iter_mut().filter(|_| instance_defaults) {
                if let Some(d) = p.default.as_mut() {
                    if mentions(d) {
                        crate::lower::scope_chain::rewrite_call_site(
                            d,
                            &regs,
                            class_self.as_ref(),
                            instance_self.as_ref(),
                        );
                    }
                }
            }
        }
    }
    // Fixed-arity relation entry points, generated AFTER every body
    // rewrite above: their forward bodies mention scope names on model
    // constants, which `rewrite_call_site` would re-thread if it saw
    // them. Placement here (not inside push_scope_methods) is
    // load-bearing for the same reason.
    for lc in lcs.iter_mut() {
        if let Some(per_model) = scopes.get(&lc.name) {
            if !per_model.is_empty() {
                crate::lower::model_to_library::push_scope_variants(
                    &mut lc.methods,
                    &lc.name,
                    per_model,
                );
            }
        }
    }
}

/// Ruby-family pre-emit pass: a `belongs_to` reader MEMOIZES.
///
/// Rails loads a belongs_to target once and hands back the same object
/// on every later read. Ours re-queried, so two reads of
/// `membership.user` were two different objects — and campfire's
/// `@membership.user.expects :reset_remote_connections` stubbed one
/// while the `after_destroy_commit` callback called the other. The
/// callback fired correctly; the expectation was watching an object
/// nothing would ever call.
///
/// ```ruby
/// def user
///   return @user_cache if @user_loaded
///   @user_cache = (… the query …)
///   @user_loaded = true
///   @user_cache
/// end
///
/// def user_id=(value)
///   @user_id = value
///   @user_loaded = false        # <- the other half
/// end
/// ```
///
/// THE INVALIDATION IS NOT OPTIONAL. Rails resets the loaded target
/// when the foreign key is written directly, and a memoizing reader
/// without it answers `membership.user` from an object the key no
/// longer points at. `apply_belongs_to_autosave` above declined to
/// populate the cache for SAVED targets precisely because that reset
/// did not exist yet; with it here, the reader can cache every read and
/// the hazard that note describes is closed.
///
/// Ruby-family only, and for a reason the shared lowering states in
/// `synth_has_many_reader`: a reader that writes an ivar is no longer
/// read-only, so Rust would emit `&mut self` and every immutable caller
/// — views iterating a collection and reading each record's
/// association — would stop borrowing. The strict targets keep the
/// re-query, which is correct, just not identity-preserving.
pub(crate) fn apply_belongs_to_memoization(lcs: &mut [LibraryClass], app: &App) {
    use crate::dialect::Association;

    for lc in lcs.iter_mut() {
        let Some(model) = app.models.iter().find(|m| m.name == lc.name) else { continue };
        let names: Vec<(Symbol, Symbol)> = model
            .associations()
            .filter_map(|a| match a {
                Association::BelongsTo { name, foreign_key, polymorphic: false, .. } => {
                    Some((name.clone(), foreign_key.clone()))
                }
                _ => None,
            })
            .collect();
        for (name, fk) in names {
            memoize_reader(lc, &name);
            invalidate_on_fk_write(lc, &name, &fk);
            invalidate_on_assoc_write(lc, &name, &fk);
        }
    }
}

/// `Seq[guard, query]` -> `Seq[guard, @cache = query, @loaded = true, @cache]`.
///
/// Splitting on the guard rather than reaching into the query is what
/// keeps this robust: the reader's second half differs per association
/// (a `Db.prepare` for most, a preloaded read for others) and this pass
/// does not need to know which.
fn memoize_reader(lc: &mut LibraryClass, name: &Symbol) {
    let cache = Symbol::from(format!("{}_cache", name.as_str()));
    let loaded = Symbol::from(format!("{}_loaded", name.as_str()));
    let Some(m) = lc
        .methods
        .iter_mut()
        .find(|m| &m.name == name && m.receiver == MethodReceiver::Instance)
    else {
        return;
    };
    let ExprNode::Seq { exprs } = &mut *m.body.node else { return };
    // Exactly the synthesized shape: the loaded-guard, then one
    // expression that produces the record. Anything else is a
    // hand-written reader and is left alone.
    if exprs.len() != 2 {
        return;
    }
    let guard_matches = matches!(&*exprs[0].node,
        ExprNode::If { cond, .. } if matches!(&*cond.node, ExprNode::Ivar { name } if name == &loaded));
    if !guard_matches {
        return;
    }
    let span = exprs[1].span;
    let query = exprs[1].clone();
    let syn = |n| Expr::new(span, n);
    *exprs = vec![
        exprs[0].clone(),
        syn(ExprNode::Assign { target: LValue::Ivar { name: cache.clone() }, value: query }),
        syn(ExprNode::Assign {
            target: LValue::Ivar { name: loaded },
            value: syn(ExprNode::Lit { value: Literal::Bool { value: true } }),
        }),
        syn(ExprNode::Ivar { name: cache }),
    ];
}

/// `def <name>=(value)` gains `@<name>_loaded = false` as its first
/// statement, when it writes `@<fk>` itself.
///
/// The association writer sets the key ivar directly rather than through
/// `<fk>=`, so the reset that writer carries never ran: after a read,
/// `comment.article = other` and `comment.article = nil` left the reader
/// answering the owner it had loaded first, where Rails answers `other`
/// and nil. The reset comes first so the unsaved-owner stash
/// (`apply_belongs_to_autosave`), which sets the flag again, still wins;
/// it also keeps a stash replaced by `= nil` from being autosaved.
fn invalidate_on_assoc_write(lc: &mut LibraryClass, name: &Symbol, fk: &Symbol) {
    let writer = Symbol::from(format!("{}=", name.as_str()));
    let Some(m) = lc
        .methods
        .iter_mut()
        .find(|m| m.name == writer && m.receiver == MethodReceiver::Instance)
    else {
        return;
    };
    fn writes_ivar(e: &Expr, ivar: &Symbol, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Assign { target: LValue::Ivar { name }, .. } = &*e.node {
            if name == ivar {
                *found = true;
                return;
            }
        }
        e.node.for_each_child(&mut |c| writes_ivar(c, ivar, found));
    }
    let mut found = false;
    writes_ivar(&m.body, fk, &mut found);
    if !found {
        return;
    }
    let span = m.body.span;
    let reset = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Ivar { name: Symbol::from(format!("{}_loaded", name.as_str())) },
            value: Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: false } }),
        },
    );
    let mut stmts = match &*m.body.node {
        ExprNode::Seq { exprs } => exprs.clone(),
        _ => vec![m.body.clone()],
    };
    stmts.insert(0, reset);
    m.body = Expr::new(span, ExprNode::Seq { exprs: stmts });
}

/// `def <fk>=(value); @<fk> = value; end` gains `@<name>_loaded = false`.
fn invalidate_on_fk_write(lc: &mut LibraryClass, name: &Symbol, fk: &Symbol) {
    let writer = Symbol::from(format!("{}=", fk.as_str()));
    let Some(m) = lc
        .methods
        .iter_mut()
        .find(|m| m.name == writer && m.receiver == MethodReceiver::Instance)
    else {
        return;
    };
    let span = m.body.span;
    let reset = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Ivar { name: Symbol::from(format!("{}_loaded", name.as_str())) },
            value: Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: false } }),
        },
    );
    let mut stmts = match &*m.body.node {
        ExprNode::Seq { exprs } => exprs.clone(),
        _ => vec![m.body.clone()],
    };
    stmts.insert(0, reset);
    m.body = Expr::new(span, ExprNode::Seq { exprs: stmts });
}

/// Ruby-family pre-emit pass: an STI row hydrates as its SUBCLASS.
///
/// `Room.first.open?` was false for a row whose `type` column says
/// `"Rooms::Open"`. Both hydration entry points build the BASE class
/// unconditionally, so `open?` — which campfire writes as
/// `is_a?(Rooms::Open)` — asked a question the object could never
/// answer yes to.
///
/// ```ruby
/// instance = case row.type
///            when "Rooms::Open" then Rooms::Open.new
///            when "Rooms::Closed" then Rooms::Closed.new
///            else self.new
///            end
/// ```
///
/// BOTH entry points, and that is the part that cost a probe. A model
/// has two: `from_stmt` reads the typed `Db.column_*` path, `from_row`
/// reads a Row through `instantiate`. `Room.first` goes to `to_a` ->
/// `instantiate` -> `from_row`; patching only `from_stmt` moved
/// nothing, exactly as adding `Request#head?` to one of the two
/// Requests moved nothing. When a runtime concept has two doors,
/// measure which one the failing call uses.
///
/// The scrutinee is CLONED off the body's own `instance.type = <expr>`
/// assignment rather than rebuilt: `from_stmt` reads
/// `Db.column_text(stmt, 4)` and `from_row` reads `row.type`, and the
/// column INDEX in the first is a fact only that body knows.
///
/// Ruby-family only, like its neighbours. A strict target's `Rooms::
/// Open` is not a subtype of `Room` — there is no inheritance to make
/// the case arms share a return type — so this is not expressible
/// there, and those targets keep hydrating the base class.
pub(crate) fn apply_sti_hydration(lcs: &mut [LibraryClass], app: &App) {
    // subclass -> base, from the one authority on the question.
    let bases = crate::lower::sti_bases(app);
    if bases.is_empty() {
        return;
    }
    let mut subs_of: HashMap<ClassId, Vec<ClassId>> = HashMap::default();
    for (sub, base) in &bases {
        subs_of.entry(base.clone()).or_default().push(sub.clone());
    }
    // Deterministic arm order — the map's iteration order is not.
    for v in subs_of.values_mut() {
        v.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    }
    for lc in lcs.iter_mut() {
        let Some(subs) = subs_of.get(&lc.name) else { continue };
        let base = lc.name.clone();
        for m in lc.methods.iter_mut() {
            // `instantiate` builds through the class it's called on, so a
            // subclass's relation (`Relation.new(Feature)`, whose `select`
            // rows `instantiate`) reaches `from_row` with `self` the
            // subclass, and a row without `type` is built as it.
            if m.name.as_str() == "instantiate" && m.receiver == crate::dialect::MethodReceiver::Class {
                self_from_row(&mut m.body, &base);
                continue;
            }
            if !matches!(m.name.as_str(), "from_stmt" | "from_row") {
                continue;
            }
            if m.receiver != crate::dialect::MethodReceiver::Class {
                continue;
            }
            let ExprNode::Seq { exprs } = &mut *m.body.node else { continue };
            // The column writes are SENDS (`instance.type=(row.type)`),
            // not `Assign { Attr }` — the model lowerer routes every
            // hydration write through the typed writer method. Both
            // spellings are matched anyway: this pass reads the body a
            // sibling pass wrote, and pinning it to one shape is how a
            // reader goes stale.
            let Some(type_read) = exprs.iter().find_map(|e| match &*e.node {
                ExprNode::Send { recv: Some(_), method, args, .. }
                    if method.as_str() == "type=" && args.len() == 1 =>
                {
                    Some(args[0].clone())
                }
                ExprNode::Assign { target: LValue::Attr { name, .. }, value }
                    if name.as_str() == "type" =>
                {
                    Some(value.clone())
                }
                _ => None,
            }) else {
                continue;
            };
            for e in exprs.iter_mut() {
                let ExprNode::Assign { target: LValue::Var { .. }, value } = &mut *e.node else {
                    continue;
                };
                // A model with an `after_initialize` hook builds through
                // `Base.new(ActiveRecord::Base::HYDRATE_ATTRS)` so
                // `initialize` skips the hook; every arm passes it on.
                let new_args = match &*value.node {
                    ExprNode::Send { recv: Some(r), method, args, .. }
                        if method.as_str() == "new"
                            && (args.is_empty() || (args.len() == 1 && is_hydrate_attrs(&args[0])))
                            && matches!(&*r.node, ExprNode::Const { path }
                                if const_path_is(path, &base)) =>
                    {
                        args.clone()
                    }
                    _ => continue,
                };
                *value = sti_dispatch(&type_read, subs, &new_args, value.span);
                break;
            }
        }
    }
}

/// `<Base>.from_row(…)` -> `self.from_row(…)`, anywhere in `body`.
fn self_from_row(body: &mut Expr, base: &ClassId) {
    body.node.for_each_child_mut(&mut |c| self_from_row(c, base));
    if let ExprNode::Send { recv: Some(r), method, .. } = &mut *body.node {
        if method.as_str() == "from_row"
            && matches!(&*r.node, ExprNode::Const { path } if const_path_is(path, base))
        {
            *r = Expr::new(r.span, ExprNode::SelfRef);
        }
    }
}

/// `ActiveRecord::Base::HYDRATE_ATTRS`, the sentinel a hooked model's
/// hydration factory passes to `new`.
fn is_hydrate_attrs(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Const { path }
        if path.iter().map(|s| s.as_str()).eq(["ActiveRecord", "Base", "HYDRATE_ATTRS"]))
}

fn const_path_is(path: &[Symbol], id: &ClassId) -> bool {
    let joined: Vec<&str> = path.iter().map(|s| s.as_str()).collect();
    joined.join("::") == id.0.as_str()
}

/// `case <type_read> when "<Sub>" then <Sub>.new(<args>) … else self.new(<args>) end`
fn sti_dispatch(type_read: &Expr, subs: &[ClassId], new_args: &[Expr], span: Span) -> Expr {
    let mut arms: Vec<crate::expr::Arm> = subs
        .iter()
        .map(|sub| crate::expr::Arm {
            pattern: crate::expr::Pattern::Lit {
                value: Literal::Str { value: sub.0.as_str().to_string() },
            },
            guard: None,
            body: Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(Expr::new(
                        span,
                        ExprNode::Const {
                            path: sub.0.as_str().split("::").map(Symbol::from).collect(),
                        },
                    )),
                    method: Symbol::from("new"),
                    args: new_args.to_vec(),
                    block: None,
                    parenthesized: true,
                },
            ),
        })
        .collect();
    // `else` — a row whose type names no known subclass, is empty, or
    // wasn't selected is the class `from_row` is called on: the base
    // for the base's relation, the subclass for a subclass's scope (its
    // relation holds the subclass, `lower::scope_chain`). Rails builds
    // such a row as the queried class.
    arms.push(crate::expr::Arm {
        pattern: crate::expr::Pattern::Wildcard,
        guard: None,
        body: Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(span, ExprNode::SelfRef)),
                method: Symbol::from("new"),
                args: new_args.to_vec(),
                block: None,
                parenthesized: true,
            },
        ),
    });
    Expr::new(span, ExprNode::Case { scrutinee: type_read.clone(), arms })
}

/// Ruby-family pre-emit pass: `belongs_to` autosave.
///
/// `room.creator = User.new(attrs); room.save!` is ordinary Rails —
/// the unsaved creator is saved with its owner and the foreign key
/// follows. The shared writer stores `value.id` and nothing else, so
/// the key stayed 0 and campfire's whole first run died on
/// `Validation failed: Creator must exist` (four tests, two files).
///
/// Three edits per non-polymorphic belongs_to:
///
///   1. the writer keeps the assigned value in the reader's cache, as
///      Rails' writer sets the association target — an UNSAVED one
///      because a record with no id yet cannot be found again from the
///      foreign key, a saved one because the reader then answers that
///      same object (`room.messages.new` hands its owner in this way,
///      the automatic inverse, and a singleton method or unsaved change
///      on the owner is what the record must see);
///   2. `_autosave_<name>` saves an unsaved one and takes the id,
///      folded into `_autosave_belongs_to`, which `save` calls before
///      `valid?` so that a direct `valid?` writes nothing, as in Rails;
///   3. the constructor's `attrs[:<name>]` goes through that writer,
///      as Rails routes every attrs key through its writer, instead of
///      the shared lowering's bare foreign-key copy.
///
/// ```ruby
/// def creator=(value)
///   if value.nil?
///     @creator_id = 0
///   else
///     @creator_id = value.id
///     @creator_cache = value
///     @creator_loaded = true
///   end
/// end
///
/// def _autosave_creator
///   if @creator_loaded && @creator_id == 0 && !@creator_cache.nil?
///     @creator_cache.save
///     @creator_id = @creator_cache.id
///   end
/// end
/// ```
///
/// A SAVED value once deliberately stayed out of the cache, so that a
/// later direct write to the key could not be answered from a stale
/// object. `apply_belongs_to_memoization` closed that hazard: the
/// foreign-key writer resets the cache, as Rails' does.
///
/// WHY RUBY-FAMILY AND NOT THE SHARED LOWERING. This started shared and
/// the strict targets refused it, each in its own way — the two of them
/// do not even agree on the shape of the slot being read:
///
///   * Rust emits the cache field as a plain `Article`, so `.nil?`
///     became `is_null()` on a struct that has no such method;
///   * Crystal emits it as `Article?`, so dropping the nil test to
///     satisfy Rust gave `undefined method 'save' for Nil`;
///   * `new_record?` lives on the runtime `Base`, which a strict
///     target's model — a plain struct — never inherits;
///   * and an unstamped `@fk = <cache>.id` typed the foreign-key column
///     `Ty::Var`, which Rust reads as "already a Value": `article_id`
///     flipped from `i64` to `serde_json::Value` and took eleven E0308s
///     with it.
///
/// There is no single spelling that satisfies both, so this joins
/// `apply_through_assoc_lowering` as a Ruby-family capability and the
/// strict targets keep the gap they already had, ledgered in
/// docs/pipeline/runtime.md.
///
/// The belongs_to `default:` statement in `before_validation` is
/// guarded on `@creator_id == 0`, which an explicitly assigned unsaved
/// creator also has. `save` calls `_autosave_belongs_to` ahead of
/// `valid?`, so the key is filled first; a direct `valid?` has no
/// autosave, so the guard also asks the reader
/// (`guard_default_on_reader`).
pub(crate) fn apply_belongs_to_autosave(lcs: &mut [LibraryClass], app: &App) {
    use crate::dialect::Association;

    for lc in lcs.iter_mut() {
        let Some(model) = app.models.iter().find(|m| m.name == lc.name) else { continue };
        let names: Vec<(Symbol, Symbol)> = model
            .associations()
            .filter_map(|a| match a {
                Association::BelongsTo { name, foreign_key, polymorphic: false, .. } => {
                    Some((name.clone(), foreign_key.clone()))
                }
                _ => None,
            })
            .collect();
        for (name, fk) in names {
            let writer = Symbol::from(format!("{}=", name.as_str()));
            let Some(w) = lc
                .methods
                .iter_mut()
                .find(|m| m.name == writer && m.receiver == MethodReceiver::Instance)
            else {
                continue;
            };
            // Only the SYNTHESIZED writer is patched, recognized by its
            // shape: `if value.nil? then @fk = 0 else @fk = value.id`.
            // A model that wrote its own `<name>=` keeps it untouched
            // and gets no autosave either — the stash is the only thing
            // `_autosave_<name>` reads, and watching a slot nobody
            // fills would be worse than the gap.
            let ExprNode::If { else_branch, .. } = &mut *w.body.node else { continue };
            if !matches!(&*else_branch.node, ExprNode::Assign { target: LValue::Ivar { name: n }, .. } if n == &fk)
            {
                continue;
            }
            let span = else_branch.span;
            let assign = else_branch.clone();
            *else_branch = Expr::new(
                span,
                ExprNode::Seq { exprs: vec![assign, stash_target(&name, span)] },
            );
            lc.methods.push(autosave_method(&lc.name, &name, &fk));
            fold_autosave_call_into_hook(&mut lc.methods, &lc.name, &name);
            guard_default_on_reader(&mut lc.methods, &name, &fk);
            route_init_through_writer(lc, &name, &fk);
        }
    }
}

/// The shared `default:` statement in `before_validation`,
/// `self.<name> = <default> if @fk.nil? || @fk == 0`, also requires
/// `<name>.nil?`, as Rails' `association.default` checks the reader.
/// An unsaved target assigned through the writer has a zero key until
/// `save` autosaves it, and a direct `valid?` runs `before_validation`
/// without that autosave. The key test stays first, so the reader
/// answers from the stash and never queries.
fn guard_default_on_reader(methods: &mut [MethodDef], name: &Symbol, fk: &Symbol) {
    let Some(hook) = methods
        .iter_mut()
        .find(|m| m.name.as_str() == "before_validation" && m.receiver == MethodReceiver::Instance)
    else {
        return;
    };
    let writer = format!("{}=", name.as_str());
    let guard = |stmt: &mut Expr| {
        let ExprNode::If { cond, then_branch, .. } = &mut *stmt.node else { return };
        let assigns = matches!(&*then_branch.node, ExprNode::Send { recv: Some(r), method, .. }
            if matches!(&*r.node, ExprNode::SelfRef) && method.as_str() == writer);
        // The default's statement only: a model's own block
        // before_validation can assign the association under its own
        // condition, and that keeps its meaning.
        if !assigns || !is_default_key_test(cond, fk) {
            return;
        }
        let span = cond.span;
        let syn = |n| Expr::new(span, n);
        let reader_nil = syn(ExprNode::Send {
            recv: Some(syn(ExprNode::Send {
                recv: None,
                method: name.clone(),
                args: vec![],
                block: None,
                parenthesized: false,
            })),
            method: Symbol::from("nil?"),
            args: vec![],
            block: None,
            parenthesized: false,
        });
        *cond = syn(ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::And,
            surface: crate::expr::BoolOpSurface::default(),
            left: cond.clone(),
            right: reader_nil,
        });
    };
    match &mut *hook.body.node {
        ExprNode::Seq { exprs } => exprs.iter_mut().for_each(guard),
        _ => guard(&mut hook.body),
    }
}

/// `@<fk>.nil? || @<fk> == 0`, the test `push_belongs_to_defaults`
/// (lower::model_to_library::markers) guards a `default:` with.
fn is_default_key_test(cond: &Expr, fk: &Symbol) -> bool {
    let is_fk = |e: &Expr| matches!(&*e.node, ExprNode::Ivar { name } if name == fk);
    let ExprNode::BoolOp { op: crate::expr::BoolOpKind::Or, left, right, .. } = &*cond.node else {
        return false;
    };
    let nil_test = matches!(&*left.node, ExprNode::Send { recv: Some(r), method, args, .. }
        if method.as_str() == "nil?" && args.is_empty() && is_fk(r));
    let zero_test = matches!(&*right.node, ExprNode::Send { recv: Some(r), method, args, .. }
        if method.as_str() == "==" && is_fk(r)
            && matches!(args.as_slice(), [a] if matches!(&*a.node, ExprNode::Lit { value: Literal::Int { value: 0 } })));
    nil_test && zero_test
}

/// `@<name>_cache = value; @<name>_loaded = true`
fn stash_target(name: &Symbol, span: Span) -> Expr {
    let syn = |n| Expr::new(span, n);
    let value = || syn(ExprNode::Var { id: VarId(0), name: Symbol::from("value") });
    syn(ExprNode::Seq {
        exprs: vec![
            syn(ExprNode::Assign {
                target: LValue::Ivar { name: Symbol::from(format!("{}_cache", name.as_str())) },
                value: value(),
            }),
            syn(ExprNode::Assign {
                target: LValue::Ivar { name: Symbol::from(format!("{}_loaded", name.as_str())) },
                value: syn(ExprNode::Lit { value: Literal::Bool { value: true } }),
            }),
        ],
    })
}

/// The constructor's `self.<fk> = attrs[:<name>].id if !attrs[:<name>].nil?`
/// (the shared `synth_initialize`, which cannot hand a strict target's
/// attrs bag a record) becomes `self.<name> = attrs[:<name>] if …`: the
/// writer stores the same key and keeps the object, so
/// `Message.new(room: room).room` is `room`, as in Rails.
fn route_init_through_writer(lc: &mut LibraryClass, name: &Symbol, fk: &Symbol) {
    let Some(init) = lc
        .methods
        .iter_mut()
        .find(|m| m.name.as_str() == "initialize" && m.receiver == MethodReceiver::Instance)
    else {
        return;
    };
    let fk_writer = format!("{}=", fk.as_str());
    let writer = Symbol::from(format!("{}=", name.as_str()));
    fn is_lookup(e: &Expr, name: &Symbol) -> bool {
        matches!(&*e.node, ExprNode::Send { recv: Some(_), method, args, block: None, .. }
            if method.as_str() == "[]" && args.len() == 1
                && matches!(&*args[0].node, ExprNode::Lit { value: Literal::Sym { value } } if value == name))
    }
    fn walk(e: &mut Expr, name: &Symbol, fk_writer: &str, writer: &Symbol, done: &mut bool) {
        if *done {
            return;
        }
        if let ExprNode::If { then_branch, .. } = &mut *e.node {
            let lookup = match &*then_branch.node {
                ExprNode::Send { recv: Some(r), method, args, .. }
                    if matches!(&*r.node, ExprNode::SelfRef)
                        && method.as_str() == fk_writer
                        && args.len() == 1 =>
                {
                    match &*args[0].node {
                        ExprNode::Send { recv: Some(obj), method: id, args: id_args, .. }
                            if id.as_str() == "id" && id_args.is_empty() =>
                        {
                            let inner = match &*obj.node {
                                ExprNode::Cast { value, .. } => value,
                                _ => obj,
                            };
                            is_lookup(inner, name).then(|| inner.clone())
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(lookup) = lookup {
                let span = then_branch.span;
                *then_branch = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(Expr::new(span, ExprNode::SelfRef)),
                        method: writer.clone(),
                        args: vec![lookup],
                        block: None,
                        parenthesized: false,
                    },
                );
                *done = true;
                return;
            }
        }
        e.node.for_each_child_mut(&mut |c| walk(c, name, fk_writer, writer, done));
    }
    let mut done = false;
    walk(&mut init.body, name, &fk_writer, &writer, &mut done);
    // The constructor's tail resets `@<name>_loaded = false` for every
    // association, which would forget the object the writer just kept.
    // The reset is already done ahead of it: the foreign-key column's
    // own `self.<fk> = attrs[:<fk>] || 0` runs first, and its writer
    // clears the cache (`apply_belongs_to_memoization`).
    if done {
        let loaded = Symbol::from(format!("{}_loaded", name.as_str()));
        if let ExprNode::Seq { exprs } = &mut *init.body.node {
            exprs.retain(|e| {
                !matches!(&*e.node, ExprNode::Assign { target: LValue::Ivar { name: n }, value }
                    if n == &loaded
                        && matches!(&*value.node, ExprNode::Lit { value: Literal::Bool { value: false } }))
            });
        }
    }
}

fn autosave_method(
    owner: &ClassId,
    name: &Symbol,
    fk: &Symbol,
) -> crate::dialect::MethodDef {
    let span = Span::synthetic();
    let syn = |n| Expr::new(span, n);
    let cache = || syn(ExprNode::Ivar { name: Symbol::from(format!("{}_cache", name.as_str())) });
    let send = |recv: Expr, method: &str| {
        syn(ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args: vec![],
            block: None,
            parenthesized: false,
        })
    };
    let and = |l: Expr, r: Expr| {
        syn(ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::And,
            surface: crate::expr::BoolOpSurface::default(),
            left: l,
            right: r,
        })
    };
    let fk_zero = syn(ExprNode::Send {
        recv: Some(syn(ExprNode::Ivar { name: fk.clone() })),
        method: Symbol::from("=="),
        args: vec![syn(ExprNode::Lit { value: Literal::Int { value: 0 } })],
        block: None,
        parenthesized: false,
    });
    let cond = and(
        and(
            syn(ExprNode::Ivar { name: Symbol::from(format!("{}_loaded", name.as_str())) }),
            fk_zero,
        ),
        send(send(cache(), "nil?"), "!"),
    );
    let body = syn(ExprNode::If {
        cond,
        then_branch: syn(ExprNode::Seq {
            exprs: vec![
                send(cache(), "save"),
                syn(ExprNode::Assign {
                    target: LValue::Ivar { name: fk.clone() },
                    value: send(cache(), "id"),
                }),
            ],
        }),
        else_branch: syn(ExprNode::Lit { value: Literal::Nil }),
    });
    crate::dialect::MethodDef {
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
        name: Symbol::from(format!("_autosave_{}", name.as_str())),
        receiver: MethodReceiver::Instance,
        visibility: MethodVisibility::Public,
        params: Vec::new(),
        body,
        signature: None,
        effects: crate::effect::EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    }
}

/// Prepended, as when this folded into `before_validation`, so the
/// order the targets are saved in is unchanged.
fn fold_autosave_call_into_hook(
    methods: &mut Vec<crate::dialect::MethodDef>,
    owner: &ClassId,
    name: &Symbol,
) {
    let span = Span::synthetic();
    let call = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from(format!("_autosave_{}", name.as_str())),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let hook = Symbol::from("_autosave_belongs_to");
    if let Some(existing) =
        methods.iter_mut().find(|m| m.name == hook && m.receiver == MethodReceiver::Instance)
    {
        let mut stmts = match &*existing.body.node {
            ExprNode::Seq { exprs } => exprs.clone(),
            _ => vec![existing.body.clone()],
        };
        stmts.insert(0, call);
        existing.body = Expr::new(span, ExprNode::Seq { exprs: stmts });
        return;
    }
    methods.push(crate::dialect::MethodDef {
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
        name: hook,
        receiver: MethodReceiver::Instance,
        visibility: MethodVisibility::Public,
        params: Vec::new(),
        body: call,
        signature: None,
        effects: crate::effect::EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    });
}

/// Ruby-family pre-emit pass: correct `has_many :through` readers. The
/// shared lowering synthesizes EVERY has_many reader as a direct
/// foreign-key query (`Tag.where(story_id: @id)`) — wrong for `through:`,
/// where the foreign key lives on the join table. Rebuild those readers
/// as a Relation join through the intermediate:
///
///   def tags
///     return @tags_cache if @tags_loaded
///     ActiveRecord::Relation.new(Tag)
///       .joins("INNER JOIN taggings ON taggings.tag_id = tags.id")
///       .where("taggings.story_id = ?", @id)
///   end
///
/// The source association on the through model, found by name
/// (`upvoted_stories, through: :votes, source: :story` finds
/// `Vote.belongs_to :story`), supplies the source foreign key when it is
/// a `belongs_to`; when the source is a `has_many` instead
/// the key is on the target table and the join reverses (campfire's
/// `reachable_messages, through: :rooms, source: :messages`). Nested chains — the source association on
/// the join model is itself `:through` (`Category has_many :stories,
/// through: :tags` where `Tag#stories` goes through taggings), or the
/// first hop is — recurse, adding one INNER JOIN per hop. Shapes a hop
/// can't prove (missing models, no matching source) are left on the
/// shared reader rather than guessed.
/// KNOWN GAP: association scope-lambdas (`-> { order(...) }`, the
/// upvoted vote-conditions) are dropped at ingest, so row order/filter
/// can diverge from Rails until the lambda lands in the IR.
pub(crate) fn apply_through_assoc_lowering(lcs: &mut [LibraryClass], app: &App) {
    use crate::dialect::Association;

    for lc in lcs.iter_mut() {
        let Some(model) = app.models.iter().find(|m| m.name == lc.name) else { continue };
        for assoc in model.associations() {
            let Association::HasMany {
                name, target, through: Some(_), scope: assoc_scope, ..
            } = assoc
            else {
                continue;
            };
            let Some((joins, edge_table, edge_fk)) =
                resolve_through_chain(&app.models, model, assoc, 0)
            else {
                continue;
            };
            let join_sql = joins.join(" ");
            let where_sql = format!("{edge_table}.{edge_fk} = ?");

            let Some(m) =
                lc.methods.iter_mut().find(|m| {
                    m.name == *name && m.receiver == crate::dialect::MethodReceiver::Instance
                })
            else {
                continue;
            };
            m.body = through_reader_body(name, target, &join_sql, &where_sql, assoc_scope);
        }
    }
}

/// Resolve the SQL join chain for `assoc`, a `has_many :through` on
/// `model`. Returns the INNER JOIN fragments (target-nearest first, ready
/// to `join(" ")`) plus the WHERE edge `(table, fk)` that points back at
/// the owner's id. One recursion per indirection: a first hop that is
/// itself `:through`, or a join-model source association that is.
/// `None` when a hop can't be proven — missing join model, no source
/// association ([`Model::through_source`](crate::dialect::Model::through_source)),
/// a source kind with no join here — and for pathological depth (cyclic
/// `through:` declarations).
fn resolve_through_chain(
    models: &[crate::dialect::Model],
    model: &crate::dialect::Model,
    assoc: &crate::dialect::Association,
    depth: usize,
) -> Option<(Vec<String>, String, Symbol)> {
    use crate::dialect::Association;
    use crate::naming::pluralize_snake;

    if depth > 4 {
        return None;
    }
    let Association::HasMany { name, target, through: Some(thr_name), .. } = assoc else {
        return None;
    };
    // The through association on the owner (`:votes`, `:taggings`, `:tags`).
    let thr = model
        .associations()
        .find(|a| matches!(a, Association::HasMany { name, .. } if name == thr_name))?;
    let Association::HasMany {
        target: thr_target, foreign_key: thr_fk, through: thr_through, as_interface, ..
    } = thr
    else {
        return None;
    };
    // How the join model's rows tie back to the owner: directly by the
    // sibling's fk, or through the sibling's own chain.
    // Owner edge is the sibling's foreign key. `through_writer_join`
    // copies that same key; asking it again cannot change the edge.
    let (back_joins, edge_table, edge_fk) = match thr_through {
        // Not the class name's plural: the owner edge is the join model's own table, as the join below reads it.
        None => (
            Vec::new(),
            models.iter().find(|m| &m.name == thr_target).map_or_else(|| pluralize_snake(thr_target.0.as_str()), |m| m.table.0.as_str().to_string()),
            thr_fk.clone(),
        ),
        Some(_) => resolve_through_chain(models, model, thr, depth + 1)?,
    };
    let thr_model = models.iter().find(|m| &m.name == thr_target)?;
    // Not the class name's plural: a model's `self.table_name =` names another table.
    let thr_table = thr_model.table.0.as_str().to_string();
    let target_table = models
        .iter()
        .find(|m| &m.name == target)
        .map_or_else(|| pluralize_snake(target.0.as_str()), |m| m.table.0.as_str().to_string());
    let owner_type = through_owner_type_predicate(&thr_table, as_interface, &model.name);
    let mut joins = match thr_model.through_source(assoc)? {
        // The source belongs_to on the join model (`Vote.belongs_to
        // :story`). A polymorphic one needs a type predicate this join
        // does not write, so it stays on the shared reader.
        Association::BelongsTo { foreign_key: src_fk, polymorphic: false, .. } => {
            let src_fk = if thr_through.is_none() {
                match crate::lower::model_to_library::through_writer_join(model, models, name) {
                    crate::lower::model_to_library::ThroughWriterJoin::Resolved(_, _, src_fk, _) => src_fk,
                    _ => return None,
                }
            } else {
                src_fk.clone()
            };
            vec![format!(
                "INNER JOIN {thr_table} ON {thr_table}.{src_fk} = {target_table}.id{owner_type}"
            )]
        }
        // A polymorphic source named by `source_type:` (lobsters'
        // `ModMail#comment_references`): the same join, restricted to the
        // rows of that type. Without the type the reader fell back to a
        // direct has_many and read a `comments.mod_mail_id` column.
        Association::BelongsTo { foreign_key: src_fk, polymorphic: true, .. } if thr_through.is_none() => {
            let crate::lower::model_to_library::ThroughWriterJoin::ResolvedPolymorphic(_, _, _, _, type_col, pk) =
                crate::lower::model_to_library::through_writer_join(model, models, name)
            else {
                return None;
            };
            let type_name = target.0.as_str().trim_start_matches("::").replace('\'', "''");
            vec![format!(
                "INNER JOIN {thr_table} ON {thr_table}.{src_fk} = {target_table}.{pk} AND {thr_table}.{type_col} = '{type_name}'{owner_type}"
            )]
        }
        // The source is a plain `has_many` on the join model, so the
        // foreign key is on the TARGET table and the join points the
        // other way (campfire: `has_many :reachable_messages, through:
        // :rooms, source: :messages` — Room#messages, `messages.room_id`).
        Association::HasMany { foreign_key: src_fk, through: None, .. } => vec![format!(
            "INNER JOIN {thr_table} ON {thr_table}.id = {target_table}.{src_fk}{owner_type}"
        )],
        // Nested: the join model reaches the target through its own
        // `:through` association. Resolve that chain (phrased from the
        // same target table), then graft the join model onto its owner
        // edge.
        src @ Association::HasMany { through: Some(_), .. } => {
            let (mut src_joins, src_edge_table, src_edge_fk) =
                resolve_through_chain(models, thr_model, src, depth + 1)?;
            src_joins.push(format!(
                "INNER JOIN {thr_table} ON {thr_table}.id = {src_edge_table}.{src_edge_fk}{owner_type}"
            ));
            src_joins
        }
        _ => return None,
    };
    joins.extend(back_joins);
    Some((joins, edge_table, edge_fk))
}

/// Keep the same polymorphic owner restriction on lazy and batched through joins.
/// An id alone is not unique across the classes sharing an `as:` interface.
fn through_owner_type_predicate(table: &str, as_interface: &Option<Symbol>, owner: &ClassId) -> String {
    match as_interface {
        Some(interface) => {
            let owner_name = owner.0.as_str().replace('\'', "''");
            format!(" AND {table}.{interface}_type = '{owner_name}'")
        }
        None => String::new(),
    }
}

/// The joined Relation chain, carrying the eager-load cache (see
/// `apply_through_assoc_lowering`).
///
/// ONE return path, and it is a Relation. The shared
/// `synth_has_many_reader` opens its body with `return @<name>_cache if
/// @<name>_loaded` — an Array — which is the right answer for a direct
/// has_many, whose reader materializes rows and is declared
/// `Array[T]`. A `through:` reader is declared `ActiveRecord::Relation`
/// (`associations.rs`, and the campfire routing bug its comment names),
/// so that guard made the method answer two unrelated types. On CRuby
/// that was latent — a preloaded `user.upvoted_stories.includes(:tags)
/// .order(...)` would reach `Array#includes`, which does not exist —
/// and under spinel's AOT it is a hard compile stop: `--rbs seed
/// contradicted: User#upvoted_stories is declared to return Relation
/// but this returns int_array` (spinel judges a seeded return as of
/// 2368afd7; the cache ivar types `int_array` off the bare `[]` in
/// `initialize` when nothing in the app preloads it). It took the
/// lobsters spinel bench lane down for three days.
///
/// So the preload seam moves ONTO the relation: `_preload_<name>` still
/// fills the cache ivar, and the reader hands it to the relation it was
/// going to answer with anyway. `preloaded` seeds the loaded-records
/// memo when the flag is set and is a no-op when it is not — the flag
/// cannot be inferred from the cache, which is `[]` both for "empty"
/// and for "never loaded". A caller that chains on further clears that
/// memo and re-queries, which is what every chain method here does and
/// what Rails does to a loaded relation.
fn through_reader_body(
    name: &Symbol,
    target: &ClassId,
    join_sql: &str,
    where_sql: &str,
    assoc_scope: &Option<Expr>,
) -> Expr {
    let span = Span::synthetic;
    let target_const = Expr::new(
        span(),
        ExprNode::Const {
            path: target.0.as_str().split("::").map(Symbol::from).collect(),
        },
    );
    let seed = Expr::new(
        span(),
        ExprNode::Send {
            recv: Some(Expr::new(
                span(),
                ExprNode::Const {
                    path: vec![Symbol::from("ActiveRecord"), Symbol::from("Relation")],
                },
            )),
            method: Symbol::from("new"),
            args: vec![target_const],
            block: None,
            parenthesized: true,
        },
    );
    let joined = Expr::new(
        span(),
        ExprNode::Send {
            recv: Some(seed),
            method: Symbol::from("joins"),
            args: vec![Expr::new(
                span(),
                ExprNode::Lit {
                    value: crate::expr::Literal::Str { value: join_sql.to_string() },
                },
            )],
            block: None,
            parenthesized: true,
        },
    );
    let chain = Expr::new(
        span(),
        ExprNode::Send {
            recv: Some(joined),
            method: Symbol::from("where"),
            args: vec![
                Expr::new(
                    span(),
                    ExprNode::Lit {
                        value: crate::expr::Literal::Str { value: where_sql.to_string() },
                    },
                ),
                Expr::new(span(), ExprNode::Ivar { name: Symbol::from("id") }),
            ],
            block: None,
            parenthesized: true,
        },
    );

    // Association scope lambda (`-> { where('votes.vote' => 1)... }`) —
    // graft its receiver-less chain onto the seeded relation so the
    // reader filters the way Rails does (without it /upvoted served
    // every joined row).
    let chain = match assoc_scope {
        Some(scope_body) => graft_chain_root(scope_body, chain),
        None => chain,
    };

    // Last in the chain, after any scope graft: the scope's conditions
    // belong to the QUERY, and this hands the finished relation the
    // records an eager load already fetched.
    Expr::new(
        span(),
        ExprNode::Send {
            recv: Some(chain),
            method: Symbol::from("preloaded"),
            args: vec![
                Expr::new(
                    span(),
                    ExprNode::Ivar { name: Symbol::from(format!("{}_cache", name.as_str())) },
                ),
                Expr::new(
                    span(),
                    ExprNode::Ivar { name: Symbol::from(format!("{}_loaded", name.as_str())) },
                ),
            ],
            block: None,
            parenthesized: true,
        },
    )
}

/// Replace the receiver-less root of a `where(...).order(...)` chain
/// with `seed`, turning an association-scope lambda body into a call
/// chain on the seeded relation. Non-chain shapes (a Seq, a literal)
/// return the seed untouched — better an unfiltered relation than a
/// mis-grafted one.
fn graft_chain_root(chain: &Expr, seed: Expr) -> Expr {
    match &*chain.node {
        ExprNode::Send { recv: Some(r), method, args, block, parenthesized } => {
            let new_recv = graft_chain_root(r, seed);
            Expr::new(
                chain.span,
                ExprNode::Send {
                    recv: Some(new_recv),
                    method: method.clone(),
                    args: args.clone(),
                    block: block.clone(),
                    parenthesized: *parenthesized,
                },
            )
        }
        ExprNode::Send { recv: None, method, args, block, parenthesized } => Expr::new(
            chain.span,
            ExprNode::Send {
                recv: Some(seed),
                method: method.clone(),
                args: args.clone(),
                block: block.clone(),
                parenthesized: *parenthesized,
            },
        ),
        _ => seed,
    }
}

/// Ruby-family pre-emit pass: resolve `app/helpers/*` references. Rails
/// mixes every helper module into every view as instance methods, but the
/// post-lowering IR emits views/controllers/helpers as module-functions
/// (`Views::Stories.listdetail`, `ApplicationHelper.avatar_img`), so a bare
/// `avatar_img(...)` rendered into a view body has no `self` to dispatch on
/// and raises `NoMethodError`. This pass (a) flips each helper module's own
/// methods to class methods so `ApplicationHelper.avatar_img` is a real
/// call target, and (b) rewrites every bare call whose name the helper
/// registry knows — in whatever LibraryClass bodies it's run over — into
/// `<DefiningModule>.method(...)`. Lives on the Ruby emit path: helper
/// modules are app-specific and the rewrite targets a CRuby call shape, so
/// shared `lower/` stays target-agnostic (same rule as scope lowering). A
/// strict no-op when the app has no non-empty helpers — the blog's helper
/// modules are empty, so `helper_method_index` is empty.
pub(crate) fn apply_helper_lowering(lcs: &mut [LibraryClass], app: &App) {
    // NO early return on an empty `helper_method_index`. This pass does
    // two jobs and only one of them is about the app's own helpers: the
    // other resolves bare FRAMEWORK helper calls (`image_tag`,
    // `sanitize`, `dom_id`) to `ActionView::ViewHelpers.<name>`, and an
    // app with no `app/helpers/` directory needs that just as much. The
    // guard that used to stand here made the qualification depend on
    // whether some unrelated file existed.
    let helper_modules: BTreeSet<ClassId> =
        app.helper_method_index.values().cloned().collect();
    PLAIN_HELPERS.with(|p| *p.borrow_mut() = plain_text_helpers(&app.library_classes, &helper_modules, &app.html_safe_method_owners));
    // Generated route-helper names (`active_path`, `story_path`, …) —
    // bare calls to these in layout/helper bodies resolve to the
    // generated `RouteHelpers` module. (The view walker rewrites route
    // helpers in the URL positions it classifies; bare calls nested in
    // unclassified expressions fall through to this pass.)
    let route_helpers: crate::det::HashSet<Symbol> =
        crate::lower::lower_routes_to_library_functions(app)
            .into_iter()
            .map(|f| f.name)
            .collect();
    // App classes that `include Rails.application.routes.url_helpers`
    // (ingest records the marker as an include of RouteHelpers —
    // lobsters' Routes). Explicit `X.<helper>` call sites anywhere in
    // the tree rewrite through RouteHelpers below. Sourced from
    // `app.library_classes`, not the local `lcs` slice — this pass
    // also runs over the lowered-models stack, whose slice doesn't
    // contain the including class itself.
    let url_helper_classes: crate::det::HashSet<Symbol> = app
        .library_classes
        .iter()
        .filter(|lc| lc.includes.iter().any(|i| i.0.as_str() == "RouteHelpers"))
        .map(|lc| lc.name.0.clone())
        .collect();
    let helper_ivars = helper_read_ivars(app);
    let view_ivars = view_assigned_ivars(app);
    // Instance-method names the app defines anywhere that collide with a
    // framework view helper; only these need the ancestry lookup above.
    let app_framework_helper_names: BTreeSet<String> = app
        .models
        .iter()
        .flat_map(|m| m.methods().map(|d| d.name.as_str().to_string()).collect::<Vec<_>>())
        .chain(app.library_classes.iter().flat_map(|c| c.methods.iter().map(|d| d.name.as_str().to_string())))
        .filter(|n| is_framework_view_helper(n))
        .collect();
    let source_methods = crate::analyze::SourceMethodIndex::new(app);
    let controller_names: crate::det::HashSet<&ClassId> = app.controllers.iter().map(|c| &c.name).collect();
    for lc in lcs.iter_mut() {
        // CONTROLLERS in the index provide `helper_method`s to views:
        // only the call-site rewrite applies to them — their methods
        // must NOT flip class-side wholesale (actions are instance
        // methods; the class-side clone for the marked helpers comes
        // from the controller lowering itself).
        let is_helper_module = helper_modules.contains(&lc.name) && !controller_names.contains(&lc.name);
        // Helper and view module functions have no controller context —
        // a bare `request` read there resolves to the per-dispatch
        // `ActionController::Current.request` (controllers keep their
        // own `request` accessor and are left alone).
        let rewrite_request =
            is_helper_module || lc.name.0.as_str().starts_with("Views::");
        // The class's own method names shadow the helper index inside its
        // bodies — a bare call to one of them is a self-dispatch, never a
        // cross-module helper reference.
        // Declared names per side (a helper module's methods are all about to
        // become module functions, so for it they are one set): an instance
        // body is not shadowed by a `def self.x`, nor a class body by `def x`.
        // A template's own method is not one of them: a template is no
        // method a bare call in it could reach (its renders are already
        // `Views::…` calls), so a bare call naming it is the helper of
        // that name — a renamed pair partial (`show_partial`) included.
        let is_template =
            lc.name.0.as_str() == "Views" || lc.name.0.as_str().starts_with("Views::");
        let declared_on = |class_side: bool| -> crate::det::HashSet<Symbol> {
            lc.methods
                .iter()
                .filter(|_| !is_template)
                .filter(|m| {
                    is_helper_module
                        || (m.receiver == MethodReceiver::Class) == class_side
                })
                .map(|m| m.name.clone())
                .collect()
        };
        let declared_instance = declared_on(false);
        let declared_class = declared_on(true);
        // A framework-helper name the app defines on a SUPERCLASS or in an
        // INCLUDED module (`ApplicationRecord#dom_class`, a concern's
        // `number_to_currency`) is the same self-dispatch: qualifying it to
        // `ViewHelpers` would call a different method than analysis kept.
        // Resolved by the analyzer's own ancestry walk, PER SIDE: an
        // instance method body is shadowed by inherited instance methods, a
        // class-method body only by class-side ones (the class object
        // cannot dispatch an instance method).
        let inherited = |class_side: bool| -> crate::det::HashSet<Symbol> {
            app_framework_helper_names
                .iter()
                .filter(|n| {
                    if class_side {
                        source_methods.class_method(&lc.name, n)
                    } else {
                        source_methods.instance_method(&lc.name, n)
                    }
                })
                .map(|n| Symbol::from(n.as_str()))
                .collect()
        };
        let inherited_instance = inherited(false);
        let inherited_class = inherited(true);
        if is_helper_module {
            // Not flipped too: `module_function`'s private instance copy would become a second, private `def self.x`.
            let class_side: Vec<(Symbol, crate::span::Span)> = lc
                .methods
                .iter()
                .filter(|m| m.receiver == MethodReceiver::Class)
                .map(|m| (m.name.clone(), m.body.span))
                .collect();
            lc.methods.retain(|m| {
                m.receiver == MethodReceiver::Class || !class_side.iter().any(|(n, span)| *n == m.name && *span == m.body.span)
            });
        }
        for m in &mut lc.methods {
            // A helper module's own methods become module-functions so the
            // rewritten `Module.method` call has a real target — Rails mixed
            // them into a view instance, but the emitted views are module
            // functions with no instance to receive them.
            if is_helper_module && m.receiver == MethodReceiver::Instance {
                m.receiver = MethodReceiver::Class;
            }
            // Ground `…url_helpers.<x>_url(record?, host:)` → absolute-URL
            // interpolation BEFORE `rewrite_helper_calls` collapses the
            // `Rails.application.routes.url_helpers` chain to `RouteHelpers`
            // (that collapse is children-first, so it would erase the chain
            // shape this grounding matches on). mod_note's `user_url` lives
            // in a model body, reached by this same pass.
            rewrite_url_helpers_absolute(&mut m.body);
            // The method's OWN params shadow the helper index too: a bare
            // `tag` in a partial whose strict-locals header declares `tag:`
            // is that local, not `ApplicationHelper.tag`. (Rails mixes
            // helpers BENEATH a template's locals.) Passed SEPARATELY from
            // own_methods so the arity guard (bare-reference only) applies
            // to params but not to real self-dispatched methods.
            let own_params: crate::det::HashSet<Symbol> =
                m.params.iter().map(|p| p.name.clone()).collect();
            // After the helper-module flip above: a module function is
            // class-side, so it is shadowed by class-side definitions only.
            let mut own_methods = match m.receiver {
                MethodReceiver::Instance => declared_instance.clone(),
                MethodReceiver::Class => declared_class.clone(),
            };
            own_methods.extend(match m.receiver {
                MethodReceiver::Instance => inherited_instance.iter().cloned(),
                MethodReceiver::Class => inherited_class.iter().cloned(),
            });
            for default in m.params.iter_mut().filter_map(|p| p.default.as_mut()) {
                rewrite_url_helpers_absolute(default);
                rewrite_helper_calls(
                    default,
                    &app.helper_method_index,
                    &route_helpers,
                    &url_helper_classes,
                    rewrite_request,
                    &own_methods,
                    &own_params,
                    &app.view_visible_controller_methods,
                );
                if is_helper_module {
                    rewrite_helper_ivars(default, &helper_ivars);
                }
            }
            rewrite_helper_calls(
                &mut m.body,
                &app.helper_method_index,
                &route_helpers,
                &url_helper_classes,
                rewrite_request,
                &own_methods,
                &own_params,
                &app.view_visible_controller_methods,
            );
            // A CONTROLLER IVAR read in a helper body. Rails mixes
            // helpers into the view instance, which carries the
            // controller's assigns, so `@room` there IS the
            // controller's — campfire's `RoomsHelper.link_to_edit_room`
            // reads it for the edit link's style and data attributes,
            // and lobsters' `StoriesHelper` reads `@user`/`@ribbon` the
            // same way. A module function has no instance, so the read
            // was a bare nil and every `rooms#show` died on
            // `undefined method 'id' for nil`.
            //
            // Routed through the same per-dispatch seam `flash` /
            // `cookies` / a `helper_method` already use, which is the
            // one place a module function can reach the live
            // controller.
            if is_helper_module {
                rewrite_helper_ivars(&mut m.body, &helper_ivars);
            }
            if lc.name.0.as_str().starts_with("Views::") {
                rewrite_view_ivar_writes(&mut m.body, &view_ivars);
            }
        }
        // The other half: the reader the rewrite dispatches to. It goes
        // on the BASE controller, so every controller inherits one —
        // a helper runs under whichever controller is current, and
        // Rails' answer for an assign that controller never made is
        // nil, which is exactly what reading an unassigned ivar gives.
        if is_base_controller(lc) {
            push_helper_ivar_readers(&mut lc.methods, &helper_ivars, &lc.name);
            // The name a template WRITES needs a reader too, not only
            // the setter: `page_title_tag` reads what `rooms/show`
            // set, and a name no helper happens to read still has to
            // answer nil rather than NoMethodError.
            push_helper_ivar_readers(&mut lc.methods, &view_ivars, &lc.name);
            push_helper_ivar_writers(&mut lc.methods, &view_ivars, &lc.name);
        }
    }
    // LAST, because it matches the qualified call shape the rewrite above
    // produces — and over the whole slice at once, because constant NAMES
    // have to be unique across every class that shares a Ruby namespace.
    hoist_pure_helper_constants(lcs);
}

/// Is this the app's base controller — the one every other controller
/// inherits from (`class ApplicationController < ActionController::Base`)?
fn is_base_controller(lc: &LibraryClass) -> bool {
    lc.parent.as_ref().is_some_and(|p| p.0.as_str() == "ActionController::Base")
}

/// The controller ivars helper-module bodies read.
///
/// Helper modules only — a `Views::` module's ivars were already bound
/// to locals by the view lowering's closure threading, and re-routing
/// one here would silently prefer a stale controller read over the
/// value the caller passed.
///
/// A name any controller already DEFINES as a method is excluded: the
/// reader this pass synthesizes would collide with it, and the app's
/// own method is the one that should win.
fn helper_read_ivars(app: &App) -> std::collections::BTreeSet<Symbol> {
    let helper_modules = helper_module_ids(app);
    let mut out = std::collections::BTreeSet::new();
    let mut rendered: Vec<(String, String)> = Vec::new();
    for lc in &app.library_classes {
        if !helper_modules.contains(&lc.name) {
            continue;
        }
        for m in &lc.methods {
            for default in m.params.iter().filter_map(|p| p.default.as_ref()) {
                collect_ivar_reads(default, &mut out);
            }
            collect_ivar_reads(&m.body, &mut out);
            collect_library_partial_keys(&m.body, &mut rendered);
        }
    }
    if !rendered.is_empty() {
        let contracts = crate::lower::view_to_library::partial_call_contracts(
            &app.views,
            &app.controllers,
            &app.library_classes,
        );
        for key in &rendered {
            if let Some(contract) = contracts.get(key) {
                out.extend(contract.closure.iter().map(|n| Symbol::from(n.as_str())));
            }
        }
    }
    let controller_methods: crate::det::HashSet<Symbol> = app
        .controllers
        .iter()
        .flat_map(|c| c.body.iter())
        .filter_map(|item| match item {
            crate::dialect::ControllerBodyItem::Action { action, .. } => {
                Some(action.name.clone())
            }
            _ => None,
        })
        .collect();
    out.retain(|n| !controller_methods.contains(n));
    out
}

fn helper_module_ids(app: &App) -> BTreeSet<ClassId> {
    app.helper_method_index
        .values()
        .filter(|id| !app.controllers.iter().any(|c| &c.name == *id))
        .cloned()
        .collect()
}

fn collect_library_partial_keys(e: &Expr, out: &mut Vec<(String, String)>) {
    if let Some(key) = library_partial_render(e).and_then(|(name, _)| library_partial_key(&name)) {
        out.push(key);
    }
    e.node.for_each_child(&mut |c| collect_library_partial_keys(c, out));
}

fn collect_ivar_reads(e: &Expr, out: &mut std::collections::BTreeSet<Symbol>) {
    if let ExprNode::Ivar { name } = &*e.node {
        out.insert(name.clone());
    }
    e.node.for_each_child(&mut |c| collect_ivar_reads(c, out));
}

/// `@room` -> `ActionController::Current.controller.room`, for the
/// names `helper_read_ivars` collected.
fn rewrite_helper_ivars(e: &mut Expr, names: &std::collections::BTreeSet<Symbol>) {
    e.node.for_each_child_mut(&mut |c| rewrite_helper_ivars(c, names));
    let name = match &*e.node {
        ExprNode::Ivar { name } if names.contains(name) => name.clone(),
        _ => return,
    };
    let span = e.span;
    let controller = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const {
                    path: vec![Symbol::from("ActionController"), Symbol::from("Current")],
                },
            )),
            method: Symbol::from("controller"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    *e.node = ExprNode::Send {
        recv: Some(controller),
        method: name,
        args: vec![],
        block: None,
        parenthesized: false,
    };
}

/// The ivars a `Views::` module ASSIGNS.
///
/// Rails' `@page_title = …` in a template writes the view context the
/// layout and every mixed-in helper share, so the write has to land
/// where the helper-side READ already looks — on the live controller.
/// The view lowering leaves the assign as an Ivar precisely so this
/// pass can see it; a helper module's ivar is a controller assign it
/// only reads, which is `helper_read_ivars`' half of the same seam.
///
/// Read off `app.views` — the TEMPLATE bodies — not off the lowered
/// `Views::` classes: `apply_helper_lowering` runs once per emitted
/// stack, and the stack holding the views is not the one holding the
/// base controller that has to grow the accessors. Sourced from the
/// App, both halves see the same names.
fn view_assigned_ivars(app: &App) -> std::collections::BTreeSet<Symbol> {
    let mut out = std::collections::BTreeSet::new();
    for view in &app.views {
        collect_ivar_writes(&view.body, &mut out);
    }
    out
}

fn collect_ivar_writes(e: &Expr, out: &mut std::collections::BTreeSet<Symbol>) {
    match &*e.node {
        ExprNode::Assign { target: LValue::Ivar { name }, .. }
        | ExprNode::OpAssign { target: LValue::Ivar { name }, .. } => {
            out.insert(name.clone());
        }
        _ => {}
    }
    e.node.for_each_child(&mut |c| collect_ivar_writes(c, out));
}

/// `@page_title = x` -> `ActionController::Current.controller.page_title = x`,
/// the write half of the seam `rewrite_helper_ivars` reads through.
fn rewrite_view_ivar_writes(e: &mut Expr, names: &std::collections::BTreeSet<Symbol>) {
    e.node.for_each_child_mut(&mut |c| rewrite_view_ivar_writes(c, names));
    let (name, value) = match &*e.node {
        ExprNode::Assign { target: LValue::Ivar { name }, value } if names.contains(name) => {
            (name.clone(), value.clone())
        }
        _ => return,
    };
    let span = e.span;
    *e.node = ExprNode::Assign {
        target: LValue::Attr { recv: current_controller_expr(span), name },
        value,
    };
}

/// The `ActionController::Current.controller` receiver both halves of
/// the seam dispatch through.
fn current_controller_expr(span: Span) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const {
                    path: vec![Symbol::from("ActionController"), Symbol::from("Current")],
                },
            )),
            method: Symbol::from("controller"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

/// `def page_title=(v); @page_title = v; end` on the base controller,
/// the target of `rewrite_view_ivar_writes`. Paired with the reader
/// `push_helper_ivar_readers` pushes for the same name — a title a
/// template sets and the layout's helper reads travels through both.
fn push_helper_ivar_writers(
    methods: &mut Vec<MethodDef>,
    names: &std::collections::BTreeSet<Symbol>,
    class_name: &ClassId,
) {
    for name in names {
        let setter = Symbol::from(format!("{}=", name.as_str()));
        if methods.iter().any(|m| m.name == setter) {
            continue;
        }
        let span = Span::synthetic();
        methods.push(MethodDef {
            unsupported_formals: None,
            has_anonymous_block: false,
            name_span: crate::span::Span::synthetic(),
            name: setter,
            receiver: MethodReceiver::Instance,
            visibility: MethodVisibility::Public,
            params: vec![crate::dialect::Param::positional(Symbol::from("value"))],
            body: Expr::new(
                span,
                ExprNode::Assign {
                    target: LValue::Ivar { name: name.clone() },
                    value: Expr::new(
                        span,
                        ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("value") },
                    ),
                },
            ),
            signature: None,
            effects: crate::effect::EffectSet::default(),
            enclosing_class: Some(class_name.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: true,
            block_param: None,
        });
    }
}

/// `def room; @room; end` on the base controller, one per name.
fn push_helper_ivar_readers(
    methods: &mut Vec<MethodDef>,
    names: &std::collections::BTreeSet<Symbol>,
    class_name: &ClassId,
) {
    for name in names {
        if methods.iter().any(|m| &m.name == name) {
            continue;
        }
        methods.push(MethodDef {
            unsupported_formals: None,
            has_anonymous_block: false,
            name_span: crate::span::Span::synthetic(),
            name: name.clone(),
            receiver: MethodReceiver::Instance,
            visibility: MethodVisibility::Public,
            params: vec![],
            body: Expr::new(Span::synthetic(), ExprNode::Ivar { name: name.clone() }),
            signature: None,
            effects: crate::effect::EffectSet::default(),
            enclosing_class: Some(class_name.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        });
    }
}

/// Framework view helpers callable from a helper/model body that the
/// view-template classifier (which runs only on views) never reaches —
/// plus bare calls in view bodies the classifier has no kind for (it
/// handles a fixed set; the rest fall through to this pass). They
/// resolve to `ActionView::ViewHelpers.<name>`. Grown as GET / surfaces
/// each one: asset helpers (`avatar_img` → `image_tag` → `image_path`),
/// then the date helpers + `content_tag` (`time_ago_in_words_label`
/// calls both bare).
fn is_framework_view_helper(name: &str) -> bool {
    matches!(
        name,
        "image_tag"
            // `include ActionView::RecordIdentifier` brings `dom_id`
            // (below) and its sibling `dom_class` into non-view classes.
            | "dom_class"
            | "image_path"
            // The general asset helper beside the image one — same
            // body, and campfire asks it for an mp3.
            | "asset_path"
            // The general asset helper beside the image one — same
            // body, and campfire asks it for an mp3.
            | "image_url"
            | "path_to_javascript"
            | "javascript_path"
            | "javascript_include_tag"
            | "number_with_precision"
            | "number_with_delimiter"
            | "number_to_currency"
            | "number_to_human"
            | "number_to_human_size"
            | "number_to_percentage"
            | "number_to_phone"
            | "fragment_name_with_digest"
            | "content_security_policy_nonce"
            | "class_names"
            | "label_tag"
            | "url_for"
            // `polymorphic_url(record, only_path: true)` — beside
            // `url_for`, which is the same question asked of a value
            // the lowerings could answer statically. campfire's
            // `BroadcastsHelper` asks it of an Active Storage
            // representation, so the runtime member it now resolves to
            // is a raising stub; unqualified it was a bare call NOTHING
            // defines, which stops a strict build outright.
            | "polymorphic_url"
            // `polymorphic_url(record, only_path: true)` — beside
            // `url_for`, which is the same question asked of a value
            // the lowerings could answer statically. campfire's
            // `BroadcastsHelper` asks it of an Active Storage
            // representation, so the runtime member it now resolves to
            // is a raising stub; unqualified it was a bare call NOTHING
            // defined, which stops a strict build outright.
            | "submit_tag"
            // `link_to_if cond, name, url, opts` — `link_to` behind a
            // condition, the name escaped bare otherwise. campfire's
            // link-preview partial links the title only when the
            // preview kept an href (`web_url` drops a non-web one).
            | "link_to_if"
            // The bare (builder-less) hidden field, beside its
            // `label_tag`/`submit_tag` siblings in the same overlay
            // file. campfire's quick-boost forms carry the emoji this
            // way, one per reaction on every message.
            | "hidden_field_tag"
            | "text_area_tag"
            // Rails' array-of-fragments concat. campfire's
            // `current_user_meta_tags` builds the two `<meta>` tags the
            // page's `<head>` needs and joins them with it.
            | "safe_join"
            | "form_tag"
            // Blockless `form_with` written in a HELPER body. The view
            // walker macro-inlines every `form_with` a TEMPLATE writes;
            // a helper module has no accumulator to splice into and its
            // options are a runtime value, so the call fell through
            // unqualified — a bare name nothing defines. Under CRuby
            // that was `undefined method 'form_with' for module
            // FormsHelper`; under spinel the helper's whole body failed
            // to compile and the LINKER stopped on the caller's
            // reference to the vanished symbol. Same hole and same
            // answer as `polymorphic_url` above.
            | "form_with"
            | "content_tag"
            | "time_ago_in_words"
            | "distance_of_time_in_words"
            | "raw"
            | "link_to"
            // campfire's user page links the address with a bare
            // `mail_to @user.email_address`. The VIEW classifier has no
            // kind for it, so it falls through to this pass like the
            // rest of the flat helpers.
            | "mail_to"
            | "content_for?"
            | "capture"
            | "concat"
            // Every campfire message row is built by `MessagesHelper
            // .message_tag`, which opens with `dom_id(message)`. The
            // VIEW classifier has always known `dom_id`; a helper body
            // never reached it, and the bare call raised NoMethodError
            // — inside a `rescue Exception` that hid which call it was.
            | "dom_id"
            // `ActionView::Helpers::SanitizeHelper`, included into a
            // MODEL — campfire's `Opengraph::Metadata` calls
            // `sanitize(strip_tags(title))` on its own attributes. Both
            // were registry-known (analyze types them) and had no
            // runtime member to dispatch to until
            // view_helpers_ext.rb grew them; the bare calls emitted
            // bare, and the surviving `include` named a namespace no
            // target ships.
            | "sanitize"
            | "strip_tags"
            // `h` — Rails' escape alias, and `auto_link` beside it.
            // campfire's `message_presentation` is one expression built
            // from both: `auto_link h(ContentFilters…apply(message.body
            // .body)), html: { target: "_blank" }`. Unqualified, each
            // was a NoMethodError inside that method's own `rescue
            // Exception`, which returns "" — so every message rendered
            // an EMPTY BODY, with no diagnostic anywhere.
            | "h"
            | "auto_link"
    )
}

/// Is this a call on `ActionView::ViewHelpers` (or a bare `ViewHelpers`
/// const)? Shared by the post-rewrite transforms below.
fn is_view_helpers_const(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Const { path }
        if path.last().map(|s| s.as_str() == "ViewHelpers").unwrap_or(false))
}

/// View helpers a call site can be hoisted OUT of, when every argument is
/// a literal: each is a pure function of its arguments and always returns
/// a freshly built String.
///
/// PURITY IS DECLARED HERE, NOT INFERRED, and the list is grown by
/// measurement rather than by reasoning about what looks pure. Two traps
/// make inference the wrong tool:
///
///   - `csrf_token_hidden_input` takes NO arguments, so "every argument is
///     a literal" is vacuously true of it. Hoisting it would compute one
///     session's token at load and serve it to every reader afterwards.
///     Arity is not evidence of purity.
///   - An empty `EffectSet` on the call is not evidence either: it means
///     the analyzer did not model the callee, not that the callee is pure.
///
/// The "freshly built String" half matters as much as the purity half.
/// `image_path("/already/absolute")` RETURNS ITS ARGUMENT, so hoisting it
/// would put a literal's own String object in a constant where a caller
/// could mutate it; the three below all interpolate into a new String.
///
/// `stylesheet_link_tag` / `javascript_include_tag` map a literal name to
/// a path through the immutable emitted asset table, so they are pure
/// over their literals the way `image_tag`
/// is; `javascript_importmap_tags` is pure over the importmap table,
/// which is the one non-literal argument this list admits (see
/// `HOISTABLE_CONST_READERS`). campfire's layout pays all three on every
/// page — 27 stylesheet links, and the importmap's 92 pins rebuilt as
/// Hashes and rendered to JSON — for a header that never changes
/// between one boot and the next: about 360 allocations per request
/// that the room page's profile put beside the message loop's.
const HOISTABLE_TAG_HELPERS: &[&str] = &[
    "image_tag",
    "content_tag",
    "hidden_field_tag",
    "stylesheet_link_tag",
    "javascript_include_tag",
    "javascript_importmap_tags",
];

/// Argument-less reads of emitted TABLES that a hoistable call may take
/// as an argument and still be a per-process constant: `Importmap.pins`
/// and `Importmap.entry` are `config/importmap.rb` rendered as methods
/// answering literals (see `emit_importmap`), so a call over them is as
/// constant as a call over the literals themselves. Declared, like the
/// helpers: a reader is on this list because its body was read, not
/// because it takes no arguments.
const HOISTABLE_CONST_READERS: &[(&str, &str)] = &[("Importmap", "pins"), ("Importmap", "entry")];

/// A canonical spelling of an all-literal expression — `None` the moment
/// any part of it is not a literal. Doubles as the dedupe key, so two call
/// sites spelled the same way share one constant, and as the guard that
/// decides whether a call is hoistable at all.
fn literal_key(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Lit { value } => Some(match value {
            Literal::Nil => "nil".to_string(),
            Literal::Bool { value } => format!("b:{value}"),
            Literal::Int { value } => format!("i:{value}"),
            Literal::Float { value } => format!("f:{value}"),
            Literal::Str { value } => format!("s:{value:?}"),
            Literal::Sym { value } => format!("y:{}", value.as_str()),
            Literal::Regex { pattern, flags } => format!("r:{pattern:?}/{flags}"),
        }),
        ExprNode::Hash { entries, kwargs } => {
            let mut out = String::from(if *kwargs { "kw{" } else { "h{" });
            for (k, v) in entries {
                out.push_str(&literal_key(k)?);
                out.push_str("=>");
                out.push_str(&literal_key(v)?);
                out.push(',');
            }
            out.push('}');
            Some(out)
        }
        ExprNode::Array { elements, .. } => {
            let mut out = String::from("[");
            for el in elements {
                out.push_str(&literal_key(el)?);
                out.push(',');
            }
            out.push(']');
            Some(out)
        }
        // An already-hoisted constant is a literal for the purposes of
        // the call that takes it (a `+` chain over hoisted tags, below).
        ExprNode::Const { path } if path.len() == 1 && path[0].as_str().starts_with("HOISTED_") => {
            Some(format!("c:{}", path[0].as_str()))
        }
        ExprNode::Send { recv: Some(r), method, args, block: None, .. } if args.is_empty() => {
            let ExprNode::Const { path } = &*r.node else { return None };
            let [konst] = path.as_slice() else { return None };
            HOISTABLE_CONST_READERS
                .iter()
                .any(|(k, m)| *k == konst.as_str() && *m == method.as_str())
                .then(|| format!("{}.{}", konst.as_str(), method.as_str()))
        }
        _ => None,
    }
}

/// A `+` chain whose every leaf is a hoisted constant or a String
/// literal — the layout's `stylesheet_link_tag(…) + "\n" +
/// stylesheet_link_tag(…) + …` once each call is a constant. String `+`
/// is pure and answers a fresh String, so the WHOLE chain is one more
/// constant (its key is the chain's spelling); without this the tags
/// would be hoisted and the 27 concatenations still run per request.
/// Answers the key for a chain, `None` for anything else — including a
/// bare leaf, which is not a chain.
fn hoistable_concat_key(e: &Expr) -> Option<String> {
    fn leaf_or_chain(x: &Expr) -> Option<String> {
        match &*x.node {
            ExprNode::Lit { value: Literal::Str { .. } } => literal_key(x),
            ExprNode::Const { path } if path.len() == 1 && path[0].as_str().starts_with("HOISTED_") => {
                literal_key(x)
            }
            _ => hoistable_concat_key(x),
        }
    }
    let ExprNode::Send { recv: Some(l), method, args, block: None, .. } = &*e.node else {
        return None;
    };
    if method.as_str() != "+" || args.len() != 1 {
        return None;
    }
    Some(format!("plus({},{})", leaf_or_chain(l)?, leaf_or_chain(&args[0])?))
}

/// The dedupe key for a hoistable call, or `None` if this expression is
/// not one: `ActionView::ViewHelpers.<helper>(<literals>)`, no block.
fn hoistable_call_key(e: &Expr) -> Option<String> {
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node else {
        return None;
    };
    if !is_view_helpers_const(r) || !HOISTABLE_TAG_HELPERS.contains(&method.as_str()) {
        return None;
    }
    let mut key = format!("{}(", method.as_str());
    for a in args {
        key.push_str(&literal_key(a)?);
        key.push(',');
    }
    key.push(')');
    Some(key)
}

/// `image_tag("boost.svg", …)` -> `HOISTED_IMAGE_TAG_BOOST_SVG`. The first
/// String argument names it, because that is what a reader of the emitted
/// view is looking for; a suffix disambiguates two calls that differ only
/// in their options.
fn hoisted_const_name(
    method: &str,
    args: &[Expr],
    used: &mut crate::det::HashSet<String>,
) -> Symbol {
    let mut base = format!("HOISTED_{}", method.to_uppercase());
    if let Some(first) = args.iter().find_map(|a| match &*a.node {
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        _ => None,
    }) {
        let slug: String = first
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
            .collect();
        let slug = slug.trim_matches('_').to_string();
        if !slug.is_empty() {
            base = format!("{base}_{slug}");
        }
    }
    let mut name = base.clone();
    let mut n = 2;
    while !used.insert(name.clone()) {
        name = format!("{base}_{n}");
        n += 1;
    }
    Symbol::from(name.as_str())
}

/// Replace hoistable calls in one expression tree with references to
/// class constants, minting a constant the first time each distinct call
/// is seen. Children first, so a nested call is hoisted before its parent
/// is examined (a parent that contains one is not itself all-literal, so
/// the two can never both fire on the same node).
fn hoist_in_expr(
    e: &mut Expr,
    minted: &mut crate::det::HashMap<String, Symbol>,
    order: &mut Vec<(Symbol, Expr)>,
    used: &mut crate::det::HashSet<String>,
) {
    hoist_in_expr_at(e, false, minted, order, used)
}

/// `in_concat`: this node is an operand of a `+` the parent will fold
/// if the whole chain qualifies — so a qualifying `+` here is left for
/// the parent, and only the outermost `+` of a chain mints a constant.
fn hoist_in_expr_at(
    e: &mut Expr,
    in_concat: bool,
    minted: &mut crate::det::HashMap<String, Symbol>,
    order: &mut Vec<(Symbol, Expr)>,
    used: &mut crate::det::HashSet<String>,
) {
    let child_in_concat = matches!(&*e.node, ExprNode::Send { method, args, block: None, .. }
        if method.as_str() == "+" && args.len() == 1);
    e.node.for_each_child_mut(&mut |c| hoist_in_expr_at(c, child_in_concat, minted, order, used));
    let (key, label) = match hoistable_call_key(e) {
        Some(key) => (key, None),
        None => match hoistable_concat_key(e) {
            Some(key) => (key, Some("concat")),
            None => return,
        },
    };
    // A chain is hoisted at its TOP: a `+` whose parent is also a `+`
    // over the same kind of leaves is folded by the parent, so the
    // whole chain becomes one constant rather than one per operator.
    if label.is_some() && in_concat {
        return;
    }
    let name = match minted.get(&key) {
        Some(name) => name.clone(),
        None => {
            let ExprNode::Send { method, args, .. } = &*e.node else { unreachable!() };
            let name = match label {
                Some(label) => hoisted_const_name(label, &[], used),
                None => hoisted_const_name(method.as_str(), args, used),
            };
            minted.insert(key, name.clone());
            order.push((name.clone(), e.clone()));
            name
        }
    };
    let span = e.span;
    let ty = e.ty.clone();
    *e = Expr::new(span, ExprNode::Const { path: vec![name] });
    e.ty = ty;
}

/// Hoist a pure view-helper call whose arguments are all literals to a
/// class constant, so the string it always produces is built once at load
/// rather than on every call.
///
/// WHY THIS IS WORTH A PASS. campfire's `_actions.rb` calls `image_tag`
/// seven times per MESSAGE — about 200 times on a 40-message room page —
/// and each call allocates the options hash, the nested `aria:` hash,
/// another hash from `opts.to_h`, the materialised key/value pairs, then
/// an array and a string per attribute in `render_attrs`, to produce a
/// byte-identical `<img>` tag every time. Profiled on the spinel binary,
/// `image_tag` was 13.9% of the whole server's CPU on `/rooms/1`, and
/// `sp_PolyArray_new` was the single hottest leaf underneath it. Hoisting
/// those seven took the page from 206 to 224-238 req/s, byte-identical.
///
/// Hoisting rather than constant-FOLDING is deliberate: computing the
/// string in Rust here would make a second implementation of `image_tag`
/// that can drift from the runtime's, and the emitted constant keeps the
/// one implementation while paying it once per process.
///
/// NAMES ARE UNIQUE ACROSS THE WHOLE SLICE, not per class, and that is not
/// tidiness. campfire's `_actions.rb` and `_template.rb` are two
/// LibraryClasses that both reopen `Views::Messages`, and both ask for the
/// same icon — one with `size: 20` and one without. Named per class they
/// both minted `HOISTED_IMAGE_TAG_MENU_DOTS_HORIZONTAL_SVG`, whichever
/// file loaded second silently won, and every message rendered its
/// options menu with an `<img>` that had lost its `width` and `height`.
/// `scripts/campfire-compare` caught it against Rails; nothing else would
/// have. Dedupe stays PER CLASS (a class must define what it references,
/// since sibling files of one namespace have no load-order contract), so
/// two files hoisting the identical call get two constants with equal
/// values — redundant, and the only alternative that is safe.
fn hoist_pure_helper_constants(lcs: &mut [LibraryClass]) {
    // Seeded from every class, so a hoisted name can never collide with an
    // app constant defined in a sibling file of the same namespace either.
    let mut used: crate::det::HashSet<String> = lcs
        .iter()
        .flat_map(|lc| lc.constants.iter().map(|(n, _)| n.as_str().to_string()))
        .collect();
    for lc in lcs.iter_mut() {
        let mut minted: crate::det::HashMap<String, Symbol> =
            crate::det::HashMap::default();
        // Source order of first use, so the emitted constants read down the
        // file in the order the template reaches them.
        let mut order: Vec<(Symbol, Expr)> = Vec::new();
        for m in &mut lc.methods {
            hoist_in_expr(&mut m.body, &mut minted, &mut order, &mut used);
        }
        lc.constants.extend(order);
    }
}

/// Strip one trailing `.to_s` (the view walker's `coerce_to_s` wrap) so
/// the safe-call check sees the helper call itself.
fn strip_to_s(e: &Expr) -> &Expr {
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node {
        if method.as_str() == "to_s" && args.is_empty() {
            return r;
        }
    }
    e
}

thread_local! {
    /// What each app helper `(module, method)` returns, when the walk can
    /// read it (`plain_text_helpers`), set per `apply_helper_lowering` run.
    static PLAIN_HELPERS: std::cell::RefCell<HashMap<(String, String), HelperReturn>> = Default::default();
}

/// A helper's return as a template sees it.
#[derive(Clone, Debug, PartialEq)]
enum HelperReturn {
    /// Plain text on every path: Rails escapes it.
    Plain,
    /// Plain text or one of these positional arguments as given: safe
    /// exactly when each such argument is.
    Args(Vec<usize>),
}

/// The app helper methods whose every return is plain text — a String
/// literal, an interpolation, a number, or another such helper's
/// result — which Rails escapes in a template like any String, and those
/// that may instead hand back a required positional argument
/// (`def label(v) = v || "—"`), which Rails escapes unless the argument
/// was safe. Anything the walk cannot read stays out, keeping the safe
/// default below, and so does a method whose `.html_safe` was erased
/// (`App::html_safe_methods`).
fn plain_text_helpers(
    lcs: &[LibraryClass],
    helper_modules: &BTreeSet<ClassId>,
    html_safe: &BTreeSet<(String, String)>,
) -> HashMap<(String, String), HelperReturn> {
    fn tails<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
        match &*e.node {
            ExprNode::Seq { exprs } => match exprs.last() {
                Some(last) => tails(last, out),
                None => out.push(e),
            },
            ExprNode::If { then_branch, else_branch, .. } => {
                tails(then_branch, out);
                tails(else_branch, out);
            }
            ExprNode::Case { arms, .. } => {
                for arm in arms {
                    tails(&arm.body, out);
                }
                out.push(e);
            }
            ExprNode::Return { value } => tails(value, out),
            _ => out.push(e),
        }
    }
    fn returns<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
        e.node.for_each_child(&mut |c| returns(c, out));
        if let ExprNode::Return { value } = &*e.node {
            tails(value, out);
        }
    }
    let methods: Vec<(String, &crate::dialect::MethodDef)> = lcs
        .iter()
        .filter(|lc| helper_modules.contains(&lc.name))
        .flat_map(|lc| lc.methods.iter().map(move |m| (lc.name.0.as_str().to_string(), m)))
        .collect();
    let mut read: HashMap<(String, String), HelperReturn> = HashMap::default();
    loop {
        let before = read.len();
        for (module, m) in &methods {
            let key = (module.clone(), m.name.as_str().to_string());
            // Not one `lower::html_safe` or the tag builder recorded: its `.html_safe` mark was erased, not absent.
            if read.contains_key(&key) || html_safe.contains(&key) {
                continue;
            }
            let mut out = Vec::new();
            tails(&m.body, &mut out);
            returns(&m.body, &mut out);
            let is_plain = |e: &Expr| match &*e.node {
                ExprNode::Lit { value: Literal::Str { .. } | Literal::Int { .. } | Literal::Float { .. } } => true,
                ExprNode::StringInterp { .. } => true,
                ExprNode::Send { recv: None, method, .. } => methods.iter().any(|(md, other)| md == module && other.name == *method)
                    && read.get(&(module.clone(), method.as_str().to_string())) == Some(&HelperReturn::Plain),
                _ => false,
            };
            // An optional or rest positional shifts which argument binds which parameter by the call's arity.
            let fixed_arity = m.params.iter().all(|p| p.keyword || (p.default.is_none() && !p.rest && !p.forwarding));
            let positional = |e: &Expr| match &*e.node {
                ExprNode::Var { name, .. } if fixed_arity => {
                    m.params.iter().position(|p| p.name == *name && !p.keyword)
                }
                _ => None,
            };
            let arg_or_plain = |e: &Expr| -> Option<Option<usize>> {
                if is_plain(e) {
                    return Some(None);
                }
                if let Some(i) = positional(e) {
                    return Some(Some(i));
                }
                match &*e.node {
                    ExprNode::BoolOp { op: crate::expr::BoolOpKind::Or, left, right, .. } => {
                        match (positional(left), is_plain(right)) {
                            (Some(i), true) => Some(Some(i)),
                            _ => None,
                        }
                    }
                    _ => None,
                }
            };
            if out.is_empty() {
                continue;
            }
            let Some(each) = out.iter().map(|e| arg_or_plain(e)).collect::<Option<Vec<_>>>() else {
                continue;
            };
            let mut args: Vec<usize> = each.into_iter().flatten().collect();
            args.sort_unstable();
            args.dedup();
            read.insert(key, if args.is_empty() { HelperReturn::Plain } else { HelperReturn::Args(args) });
        }
        if read.len() == before {
            return read;
        }
    }
}

/// Calls whose result Rails treats as an html_safe buffer: tag-producing
/// framework helpers, `raw`, and app-helper module functions (which
/// compose those). The view walker's default `html_escape(<call>.to_s)`
/// wrap must NOT apply to these — escaping a safe buffer ships literal
/// `&lt;img&gt;` markup. Plain-string helpers (truncate,
/// time_ago_in_words) stay wrapped: their escape is Rails-correct.
/// Treating every app-helper as safe is a simplification (Rails escapes
/// an app helper that returns a plain string); the corpus' helpers all
/// return tag-helper compositions, and per-method safety inference can
/// refine this when a counterexample shows up.
fn is_html_safe_call(e: &Expr, index: &HashMap<Symbol, ClassId>) -> bool {
    let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node else {
        return false;
    };
    let ExprNode::Const { path } = &*r.node else {
        return false;
    };
    let joined =
        path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
    if joined.ends_with("ViewHelpers") {
        return matches!(
            method.as_str(),
            "raw" | "link_to" | "link_to_if" | "link_to_raw" | "button_to" | "mail_to" | "image_tag" | "content_tag"
                | "javascript_include_tag" | "label_tag" | "submit_tag" | "form_tag" | "form_with"
                // The rest of FormTagHelper — the BUILDER-LESS field
                // tags, which return an element exactly the way
                // `label_tag` and `submit_tag` beside them do. Listed
                // as a set rather than one at a time because the
                // dividing line is not which one an app happened to
                // reach: a helper that returns an ELEMENT is safe, a
                // helper that returns TEXT (`truncate`, `dom_id`,
                // `time_ago_in_words`) is not, and every name here is
                // on the element side. `hidden_field_tag` is how the
                // omission surfaced: campfire's quick boosts carry
                // their emoji in one, eight per message, and the
                // escape shipped `&lt;input …&gt;` — 320 inputs Rails
                // renders and the room page did not, with the forms
                // around them all present and correct.
                | "hidden_field_tag"
                | "text_field_tag"
                | "password_field_tag"
                | "text_area_tag"
                | "check_box_tag"
                | "radio_button_tag"
                | "select_tag"
                | "button_tag"
                | "field_set_tag"
                | "file_field_tag"
                | "email_field_tag"
                | "number_field_tag"
                | "search_field_tag"
                | "telephone_field_tag"
                | "url_field_tag"
                | "date_field_tag"
                | "color_field_tag"
                // The `<option>` builders, same rule.
                | "options_for_select"
                | "options_from_collection_for_select"
                | "grouped_options_for_select"
                | "time_zone_options_for_select"
        );
    }
    // Not every app helper: one whose every return is plain text is a String Rails escapes.
    if !index.values().any(|cid| cid.0.as_str() == joined) {
        return false;
    }
    match PLAIN_HELPERS.with(|p| p.borrow().get(&(joined.clone(), method.as_str().to_string())).cloned()) {
        None => true,
        Some(HelperReturn::Plain) => false,
        Some(HelperReturn::Args(idxs)) => idxs.iter().all(|&i| {
            args.get(i).is_some_and(|a| {
                let a = strip_to_s(a);
                // An interpolation may be a tag helper the walker inlined (`link_to` → `"<a href=…>"`).
                matches!(&*a.node, ExprNode::StringInterp { .. }) || is_html_safe_call(a, index)
            })
        }),
    }
}

/// `<Const ending in Base>.helpers` — the Rails idiom
/// `ActionController::Base.helpers.image_path(...)` used to reach view
/// helpers from a model. Collapses to the ViewHelpers module so the
/// trailing call resolves as `ActionView::ViewHelpers.image_path(...)`.
fn is_base_dot_helpers(node: &ExprNode) -> bool {
    if let ExprNode::Send { recv: Some(r), method, args, block, .. } = node {
        if method.as_str() == "helpers" && args.is_empty() && block.is_none() {
            if let ExprNode::Const { path } = &*r.node {
                return path.last().map(|s| s.as_str() == "Base").unwrap_or(false);
            }
        }
    }
    false
}

/// `Rails.application.routes.url_helpers` — the Rails idiom for reaching
/// route helpers from a model body (`Story#short_id_path` does
/// `...url_helpers.root_path + "s/#{short_id}"`). Collapses to the
/// generated `RouteHelpers` module so the trailing `.root_path` resolves
/// as `RouteHelpers.root_path`.
fn is_rails_url_helpers(node: &ExprNode) -> bool {
    let mut cur = node;
    for step in ["url_helpers", "routes", "application"] {
        let ExprNode::Send { recv: Some(r), method, args, block, .. } = cur else {
            return false;
        };
        if method.as_str() != step || !args.is_empty() || block.is_some() {
            return false;
        }
        cur = &r.node;
    }
    matches!(cur, ExprNode::Const { path }
        if path.last().map(|s| s.as_str() == "Rails").unwrap_or(false))
}

fn view_helpers_path() -> Vec<Symbol> {
    vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")]
}

/// Walk a method body and rewrite helper calls so they resolve against the
/// module-function surfaces. Four cases (children first, so nested calls
/// and the `.helpers` receiver are rewritten before their parent):
///   1. `<…Base>.helpers` → the `ActionView::ViewHelpers` constant.
///   2. `Rails.application.routes.url_helpers` → the `RouteHelpers` constant.
///   3. bare `name(args)` where `name` is an app helper → `<Module>.name(args)`.
///   4. bare `name(args)` where `name` is a framework view helper →
///      `ActionView::ViewHelpers.name(args)`.
/// Only receiver-less Sends are rewritten in (3)/(4): a call with a receiver
/// already resolves, and a bare identifier with no call shape is a local read.
fn rewrite_helper_calls(
    expr: &mut Expr,
    index: &HashMap<Symbol, ClassId>,
    route_helpers: &crate::det::HashSet<Symbol>,
    url_helper_classes: &crate::det::HashSet<Symbol>,
    rewrite_request: bool,
    own_methods: &crate::det::HashSet<Symbol>,
    own_params: &crate::det::HashSet<Symbol>,
    view_visible: &std::collections::BTreeSet<Symbol>,
) {
    expr.node.for_each_child_mut(&mut |c| {
        rewrite_helper_calls(
            c,
            index,
            route_helpers,
            url_helper_classes,
            rewrite_request,
            own_methods,
            own_params,
            view_visible,
        )
    });

    // Not left on `self` in a view or helper module: a helper reached as `self.x` is the bare `x` below, and the module function has no such method. A same-named param keeps `self`, which a bare call would read instead.
    if let ExprNode::Send { recv: Some(r), method, .. } = &*expr.node {
        if rewrite_request
            && matches!(&*r.node, ExprNode::SelfRef)
            && (index.contains_key(method) || view_visible.contains(method))
            && !own_methods.contains(method)
            && !own_params.contains(method)
        {
            let ExprNode::Send { recv, .. } = &mut *expr.node else { unreachable!() };
            *recv = None;
        }
    }

    // Destructive `<lvalue>.<m>!(args)` on a frozen string raises under
    // spinel (frozen literals) — lobsters' link_to_different_page does
    // `path.sub!(/…/, "")` on a route-helper literal (`active_path` → a
    // frozen "/active"). For the in-place string mutators whose bang form
    // modifies self and returns self-or-nil, the non-destructive
    // `<lvalue> = <lvalue>.<m>(args)` is equivalent when the bang return is
    // unused (statement position) and is frozen-safe. Assignable receiver
    // (Var/Ivar) only; `slice!` and friends (which return the removed part,
    // not the modified string) are deliberately excluded.
    let bang_rewrite: Option<Symbol> =
        if let ExprNode::Send { recv: Some(r), method, block: None, .. } = &*expr.node {
            // A reserved-word local (`class:`) or a numbered parameter
            // (`_1`) is not assignable, so it keeps the bang call.
            let is_lv = match &*r.node {
                ExprNode::Var { name, .. } => {
                    !crate::naming::is_reserved_local(name.as_str())
                        && !crate::naming::is_numbered_param(name.as_str())
                }
                ExprNode::Ivar { .. } => true,
                _ => false,
            };
            method
                .as_str()
                .strip_suffix('!')
                .filter(|base| {
                    is_lv
                        && matches!(
                            *base,
                            "sub" | "gsub" | "chomp" | "chop" | "strip" | "lstrip"
                                | "rstrip" | "squeeze" | "upcase" | "downcase"
                                | "capitalize" | "swapcase" | "tr" | "tr_s" | "delete"
                                | "reverse"
                        )
                })
                .map(Symbol::from)
        } else {
            None
        };
    if let Some(base) = bang_rewrite {
        let span = expr.span;
        let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
        let ExprNode::Send { recv, args, parenthesized, .. } = node else { unreachable!() };
        let r = recv.unwrap();
        let target = match &*r.node {
            ExprNode::Var { id, name } => LValue::Var { id: *id, name: name.clone() },
            ExprNode::Ivar { name } => LValue::Ivar { name: name.clone() },
            _ => unreachable!(),
        };
        let call = Expr::new(
            span,
            ExprNode::Send { recv: Some(r), method: base, args, block: None, parenthesized },
        );
        *expr.node = ExprNode::Assign { target, value: call };
        return;
    }

    // ActiveSupport `String#pluralize(count)` — count-aware inflection of
    // the string ITSELF (singular when count == 1, else the inflected
    // plural), distinct from the count-labeling `Inflector.pluralize(count,
    // word)` grounded below. Spinel can't reopen the built-in String, so
    // lower the String-receiver form to the ruby-family `Inflector.
    // pluralize_word` module function. String-typed receiver only, so a
    // model's own one-arg `pluralize` (should one exist) is left alone.
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*expr.node {
        if method.as_str() == "pluralize"
            && args.len() == 1
            && matches!(r.ty, Some(crate::ty::Ty::Str))
        {
            let span = expr.span;
            let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
            let ExprNode::Send { recv, args, .. } = node else { unreachable!() };
            let count = args.into_iter().next().unwrap();
            *expr.node = ExprNode::Send {
                recv: Some(Expr::new(
                    span,
                    ExprNode::Const { path: vec![Symbol::from("Inflector")] },
                )),
                method: Symbol::from("pluralize_word"),
                args: vec![recv.unwrap(), count],
                block: None,
                parenthesized: true,
            };
            return;
        }
    }

    // `X.<helper>` where X singleton-includes url_helpers (lobsters'
    // `Routes.user_url reparent_user`): a `<x>_path` retargets to the
    // generated RouteHelpers module; a `<x>_url` whose path sibling is
    // generated re-lands as the bare `<x>_url` form so the absolute-URL
    // grounding a few blocks down claims it in this same visit.
    if let ExprNode::Send { recv: Some(r), method, .. } = &*expr.node {
        if let ExprNode::Const { path } = &*r.node {
            if path.len() == 1 && url_helper_classes.contains(&path[0]) {
                let named_path = route_helpers.contains(method);
                let named_url = method
                    .as_str()
                    .strip_suffix("_url")
                    .is_some_and(|stem| {
                        route_helpers.contains(&Symbol::from(format!("{stem}_path")))
                    });
                if named_path {
                    let ExprNode::Send { recv, .. } = &mut *expr.node else { unreachable!() };
                    *recv = Some(Expr::new(
                        expr.span,
                        ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] },
                    ));
                } else if named_url {
                    let ExprNode::Send { recv, .. } = &mut *expr.node else { unreachable!() };
                    *recv = None;
                }
            }
        }
    }

    // Bare controller-context reads in a helper/view module body →
    // the per-dispatch parked objects (module functions have no
    // controller to delegate to): `request` →
    // `ActionController::Current.request`; `cookies`/`session`/`flash`
    // → `ActionController::Current.controller.<x>` (lobsters'
    // ApplicationHelper.filtered_tags reads the tag-filter cookie).
    if rewrite_request {
        // `helper_method :platform` — the app SAYING this controller
        // method is view-callable, for the marked methods the existing
        // path cannot serve.
        //
        // TWO ARMS OF ONE RULE, and the discriminator already exists.
        // `controller_helper_method_names` clones a marked method
        // CLASS-SIDE when its body is pure over its arguments, and the
        // view calls `DomainsController.caption_of_button(domain)` —
        // a static call, which is right because there is no per-request
        // state to reach. A marked method that READS REQUEST STATE
        // (campfire's `platform` is `@platform ||= ApplicationPlatform
        // .new(request.user_agent)`) cannot be cloned for exactly that
        // reason, and was left as honest residue. It routes through the
        // live controller instead.
        //
        // `index` is the "already served" test: a name in
        // `helper_method_index` has a module or a class-side clone
        // behind it, and this arm must not take it — doing so turned
        // lobsters' working static call into a dynamic one.
        //
        // Shadowed by the module's own methods and its params, because a
        // partial local spelled `platform` is that local (Rails mixes
        // helpers BENEATH a template's locals). Args are FORWARDED
        // rather than required to be empty: Rails allows a helper_method
        // to take them.
        if let ExprNode::Send { recv: None, method, args, block, .. } = &*expr.node {
            if view_visible.contains(method)
                && !index.contains_key(method)
                && !own_methods.contains(method)
                && !own_params.contains(method)
            {
                let span = expr.span;
                let controller = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(Expr::new(
                            span,
                            ExprNode::Const {
                                path: vec![
                                    Symbol::from("ActionController"),
                                    Symbol::from("Current"),
                                ],
                            },
                        )),
                        method: Symbol::from("controller"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                );
                *expr.node = ExprNode::Send {
                    recv: Some(controller),
                    method: method.clone(),
                    args: args.clone(),
                    block: block.clone(),
                    parenthesized: !args.is_empty(),
                };
                return;
            }
        }
        if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node {
            let m = method.as_str();
            if (m == "request" || m == "controller" || m == "cookies" || m == "session"
                || m == "flash" || m == "params")
                && args.is_empty()
                && (m != "controller"
                    || (!own_methods.contains(method) && !own_params.contains(method)))
            {
                let span = expr.span;
                let current = Expr::new(
                    span,
                    ExprNode::Const {
                        path: vec![
                            Symbol::from("ActionController"),
                            Symbol::from("Current"),
                        ],
                    },
                );
                let (recv, meth) = if m == "request" || m == "controller" {
                    (current, method.clone())
                } else {
                    (
                        Expr::new(
                            span,
                            ExprNode::Send {
                                recv: Some(current),
                                method: Symbol::from("controller"),
                                args: vec![],
                                block: None,
                                parenthesized: false,
                            },
                        ),
                        Symbol::from(m),
                    )
                };
                // The typed receiver is what arms the emitter's
                // string-key coercion (`is_string_keyed_hash`):
                // `@params` inside a controller is declared
                // `Hash[String, ParamValue]`, and a view's
                // `params[:join_code]` must not lose that fact on its
                // way through `Current.controller` — a Symbol key
                // against the string-keyed store reads nil, and
                // campfire's join form rendered `action="/join/"`.
                if m == "params" {
                    expr.ty = Some(crate::ty::Ty::Hash {
                        key: std::sync::Arc::new(crate::ty::Ty::Str),
                        value: std::sync::Arc::new(crate::ty::Ty::Class {
                            id: ClassId(Symbol::from("Roundhouse::ParamValue")),
                            args: vec![].into(),
                        }),
                    });
                }
                *expr.node = ExprNode::Send {
                    recv: Some(recv),
                    method: meth,
                    args: vec![],
                    block: None,
                    parenthesized: false,
                };
                return;
            }
        }
    }

    // Case 1: collapse `<…Base>.helpers` to the ViewHelpers module constant.
    if is_base_dot_helpers(&expr.node) {
        *expr.node = ExprNode::Const { path: view_helpers_path() };
        return;
    }

    // Case 1b: a controller's own `helpers.<m>(…)` — Rails' view-context
    // proxy (campfire's `helpers.dom_id(@boost.message, :boosts)`). An
    // app helper resolves to its module, anything else to ViewHelpers.
    if !own_methods.contains(&Symbol::from("helpers")) {
        if let ExprNode::Send { recv: Some(r), method, .. } = &mut *expr.node {
            let bare_helpers = matches!(&*r.node,
                ExprNode::Send { recv: None, method: h, args, block: None, .. }
                    if h.as_str() == "helpers" && args.is_empty());
            if bare_helpers {
                let path = match index.get(method) {
                    Some(module) => module.0.as_str().split("::").map(Symbol::from).collect(),
                    None => view_helpers_path(),
                };
                *r.node = ExprNode::Const { path };
            }
        }
    }

    // Case 2: collapse `Rails.application.routes.url_helpers` to RouteHelpers.
    if is_rails_url_helpers(&expr.node) {
        *expr.node = ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] };
        return;
    }

    // Bare `<x>_url` whose `<x>_path` sibling is generated — use the
    // shared view URL seam so request-derived origins get the same port
    // normalization as jbuilder. It still resolves Rails.application.domain,
    // preserving apps' canonical-domain overrides. Lobsters' hats page
    // links `request_hat_url` bare.
    //
    // Self wins here too: a class that defines `widgets_url` itself
    // calls that method (Rails mixes url helpers in beneath a class's
    // own methods, and a plain class has none mixed in at all).
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node
        && !own_methods.contains(method)
    {
        if let Some(stem) = method.as_str().strip_suffix("_url") {
            let path_name = Symbol::from(format!("{stem}_path"));
            if route_helpers.contains(&path_name) {
                *expr = if own_methods.contains(&Symbol::from("default_url_options")) {
                    url_for_own_options(stem, args.clone())
                } else {
                    crate::lower::view_to_library::absolute_url_interp(stem, args.clone())
                };
                return;
            }
        }
    }

    // Cases 3/4: a bare call resolving to an app or framework helper module.
    let path: Option<Vec<Symbol>> = match &*expr.node {
        ExprNode::Send { recv: None, method, args, block, .. } => {
            // A method's own PARAM shadows the helper index only as a BARE
            // reference (no args, no block) — that's a local read. `tag(x)`
            // or `tag { }` is a method call that still needs helper
            // qualification, so a same-named param must NOT capture it
            // (that dropped the qualification → latent NameError). Real
            // own-METHODS shadow at any arity (a self-dispatch).
            let param_shadow =
                own_params.contains(method) && args.is_empty() && block.is_none();
            if own_methods.contains(method) || param_shadow {
                // Self wins: a bare call naming a method the enclosing
                // class itself defines is a self-dispatch (Tag#to_param's
                // `tag` column reader), not a helper reference — Rails
                // helpers are mixed in beneath the receiver's own
                // methods, and models never see helpers at all. Without
                // this, an app helper that happens to share a model
                // accessor's name (lobsters' ApplicationHelper#tag
                // builder override vs Tag#tag) captures every read.
                None
            } else if let Some(module) = index.get(method) {
                Some(module.0.as_str().split("::").map(Symbol::from).collect())
            } else if method.as_str() == "pluralize" && args.len() == 2 {
                // Count-labeling `pluralize(count, word)` in a helper
                // body — the same home the view pipeline's classifier
                // already grounds to (`Inflector.pluralize`, the
                // spinel-blog convention), NOT a second ViewHelpers
                // impl. Two-arg form only: the optional plural-word /
                // locale variants aren't in the runtime's surface, so
                // they stay verbatim rather than mis-bind arity.
                Some(vec![Symbol::from("Inflector")])
            } else if is_framework_view_helper(method.as_str()) {
                Some(view_helpers_path())
            } else if route_helpers.contains(method) {
                Some(vec![Symbol::from("RouteHelpers")])
            } else {
                None
            }
        }
        _ => None,
    };
    if let Some(path) = path {
        let span = expr.span;
        let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
        let ExprNode::Send { method, mut args, block, .. } = node else { unreachable!() };
        if path.len() == 1 && path[0].as_str() == "Inflector"
            && method.as_str() == "pluralize" && args.len() == 2 && !index.contains_key(&method)
        {
            let word = args.pop().unwrap();
            *expr = crate::lower::view::pluralize_helper_call(args.pop().unwrap(), word);
            return;
        }
        // `link_to(37, url)` — Rails stringifies the text arg; the runtime
        // link_to is deliberately monomorphic (String text), so coercion
        // belongs here at the call boundary. Literal strings stay bare.
        if matches!(method.as_str(), "link_to" | "link_to_raw") {
            if let Some(text) = args.first_mut() {
                if !matches!(
                    &*text.node,
                    ExprNode::Lit { value: crate::expr::Literal::Str { .. } }
                ) {
                    let inner = std::mem::replace(
                        &mut *text.node,
                        ExprNode::Seq { exprs: vec![] },
                    );
                    *text.node = ExprNode::Send {
                        recv: Some(Expr::new(text.span, inner)),
                        method: Symbol::from("to_s"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    };
                }
            }
        }
        // Rails' `distance_of_time_in_words` takes seconds as readily as
        // Times — the authentication generator's reset mailer passes
        // `(0, @user.password_reset_token_expires_in)`. The runtime keeps
        // the two apart (one parameter type each; a Time-or-Integer slot
        // is poly on spinel), so a call whose first time is an Integer
        // goes to the seconds entry when the second is an Integer too or
        // has no type here (a mailer view's `user` parameter reaches emit
        // untyped). A Time in that second slot does not slip through:
        // spinel refuses `Time - Integer` there, and CRuby raises on it.
        let is_int = |e: &Expr| matches!(e.ty, Some(crate::ty::Ty::Int));
        let int_or_open = |e: &Expr| {
            matches!(e.ty, None | Some(crate::ty::Ty::Int) | Some(crate::ty::Ty::Var { .. }))
        };
        let method = if method.as_str() == "distance_of_time_in_words"
            && path == view_helpers_path()
            && args.len() >= 2
            && is_int(&args[0])
            && int_or_open(&args[1])
        {
            Symbol::from("distance_of_seconds_in_words")
        } else {
            method
        };
        *expr.node = ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path })),
            method,
            args,
            block,
            parenthesized: true,
        };
    }

    // `RouteHelpers.<x>_path(format: :rss)` → `RouteHelpers.<x>_path +
    // ".rss"`. Rails path helpers accept `format:` whether or not the
    // route spells `(.:format)`; the IR has no keyword params, so the
    // suffix moves to the call site — arity-independent, and helpers
    // without a format slot stay format-capable.
    let format_suffix = match &*expr.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str().ends_with("_path")
                && matches!(&*r.node, ExprNode::Const { path }
                    if path.last().map(|s| s.as_str() == "RouteHelpers").unwrap_or(false)) =>
        {
            match args.last().map(|a| &*a.node) {
                Some(ExprNode::Hash { entries, kwargs: true }) if entries.len() == 1 => {
                    match &*entries[0].0.node {
                        ExprNode::Lit { value: crate::expr::Literal::Sym { value } }
                            if value.as_str() == "format" =>
                        {
                            Some(entries[0].1.clone())
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        }
        _ => None,
    };
    if let Some(fmt) = format_suffix {
        let span = expr.span;
        let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
        let ExprNode::Send { recv, method, mut args, block, parenthesized } = node else {
            unreachable!()
        };
        args.pop();
        let call = Expr::new(span, ExprNode::Send { recv, method, args, block, parenthesized });
        let suffix = Expr::new(
            span,
            ExprNode::StringInterp {
                parts: vec![
                    crate::expr::InterpPart::Text { value: ".".to_string() },
                    crate::expr::InterpPart::Expr { expr: fmt },
                ],
            },
        );
        *expr.node = ExprNode::Send {
            recv: Some(call),
            method: Symbol::from("+"),
            args: vec![suffix],
            block: None,
            parenthesized: false,
        };
        return;
    }

    // `link_to(raw(x), …)` → `link_to_raw(x, …)`: Rails skips the label
    // escape for a safe buffer; with no safe-buffer type the exemption is
    // decided here. Children were rewritten first (and the bare-call
    // rewrite above may have just fired), so both calls are already in
    // their ViewHelpers.* form.
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &mut *expr.node {
        if method.as_str() == "link_to" && is_view_helpers_const(r) && !args.is_empty() {
            let raw_inner = match &*args[0].node {
                ExprNode::Send { recv: Some(r2), method: m2, args: a2, .. }
                    if m2.as_str() == "raw"
                        && a2.len() == 1
                        && is_view_helpers_const(r2) =>
                {
                    Some(a2[0].clone())
                }
                _ => None,
            };
            if let Some(inner) = raw_inner {
                *method = Symbol::from("link_to_raw");
                args[0] = inner;
            }
        }
    }

    // `sanitize(html, tags: T, attributes: A)` → `sanitize_with(html, T,
    // A)`: the runtime's `sanitize` takes the html alone (its arity is
    // fixed across targets), and the caller's allow-lists are their own
    // entry point. Both keywords, or the call is left as written.
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &mut *expr.node {
        if method.as_str() == "sanitize" && is_view_helpers_const(r) && args.len() == 2 {
            let lists = match &*args[1].node {
                ExprNode::Hash { entries, kwargs: true } if entries.len() == 2 => {
                    let get = |name: &str| {
                        entries.iter().find_map(|(k, v)| match &*k.node {
                            ExprNode::Lit { value: crate::expr::Literal::Sym { value } }
                                if value.as_str() == name =>
                            {
                                Some(v.clone())
                            }
                            _ => None,
                        })
                    };
                    get("tags").zip(get("attributes"))
                }
                _ => None,
            };
            if let Some((tags, attributes)) = lists {
                *method = Symbol::from("sanitize_with");
                args.truncate(1);
                args.push(tags);
                args.push(attributes);
            }
        }
    }

    // Unwrap the view walker's default `html_escape(<call>.to_s)` when
    // the call is html_safe (see is_html_safe_call) — Rails doesn't
    // escape safe buffers, and escaping them ships literal &lt;img&gt;.
    let unwrap: Option<Expr> = match &*expr.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "html_escape"
                && args.len() == 1
                && is_view_helpers_const(r)
                && is_html_safe_call(strip_to_s(&args[0]), index) =>
        {
            Some(args[0].clone())
        }
        _ => None,
    };
    if let Some(inner) = unwrap {
        *expr = inner;
    }
}

/// Ruby-emit-path pass: wrap each action's html render in the layout
/// call — `render(Views::X.y(...))` → `render(Views::Layouts.application(
/// Views::X.y(...), @<ivar>…, @flash…))`. Lives here (not the shared
/// controller lowering) because the wrap shape and the CRuby dispatch
/// contract move together: the overlay main.rb ships `controller.body`
/// verbatim, while other targets' dispatchers still wrap body-only.
/// The controller render seam is where a layout's ivar reads (@user,
/// @title) are statically in scope — the generic dispatch had no way to
/// pass them. Skipped renders: jbuilder json (`*_json` view call or a
/// `content_type:` kwarg), non-view renders (`render html:`/`plain:` —
/// Rails skips the layout for those too), and an already-wrapped
/// Layouts call (idempotence). No-op when the app has no
/// layouts/application view.
pub(crate) fn apply_layout_lowering(lcs: &mut [LibraryClass], app: &App) {
    // Cheap probe: no layouts/application view → nothing to wrap.
    let probe = Expr::new(
        crate::span::Span::synthetic(),
        ExprNode::Lit { value: crate::expr::Literal::Nil },
    );
    let closures = crate::lower::view_to_library::view_ivar_closures(&app.views, &app.controllers);
    if crate::lower::view_to_library::layout_wrap_expr_with(app, &closures, probe).is_none() {
        return;
    }
    for lc in lcs.iter_mut() {
        // `layout false` on the controller this class came from, scoped
        // by the `only:`/`except:` the declaration carried. Resolved
        // per METHOD because the scoping is per action: campfire's
        // `MessagesController` writes `layout false, only: :index`, so
        // `index` must render bare — it is a turbo-frame fragment
        // loaded into a page that already has a layout, and wrapping it
        // shipped a whole second `<html>` inside the first, 117 tags
        // Rails does not send — while `show` and `edit` keep theirs.
        let decl = app
            .controllers
            .iter()
            .find(|c| c.name == lc.name)
            .map(|c| c.layout.clone());
        for m in &mut lc.methods {
            if decl.as_ref().is_some_and(|d| d.suppresses(&m.name)) {
                continue;
            }
            rewrite_layout_wrap(&mut m.body, app, &closures);
        }
    }
}

fn rewrite_layout_wrap(
    expr: &mut Expr,
    app: &App,
    closures: &crate::det::HashMap<crate::lower::view_to_library::ViewKey, Vec<crate::ident::Symbol>>,
) {
    expr.node.for_each_child_mut(&mut |c| rewrite_layout_wrap(c, app, closures));
    // Post-lowering action bodies carry `render` as a SelfRef-receiver
    // Send (`self.render(...)` shape); accept the bare form too.
    let ExprNode::Send { recv, method, args, .. } = &mut *expr.node else {
        return;
    };
    let self_recv = match recv {
        None => true,
        Some(r) => matches!(&*r.node, ExprNode::SelfRef),
    };
    if !self_recv || method.as_str() != "render" || args.is_empty() {
        return;
    }
    // Extract-and-strip any `layout:` kwarg first: the shared lowering
    // keeps it as the wrap marker for body renders (`render html: X,
    // layout: "application"`), and the runtime `render(body, status:,
    // content_type:, location:)` doesn't accept it, so it must never
    // survive to the call.
    let mut layout_kwarg: Option<Expr> = None;
    for a in args.iter_mut().skip(1) {
        if let ExprNode::Hash { entries, .. } = &mut *a.node {
            entries.retain(|(k, v)| {
                let is_layout = matches!(
                    &*k.node,
                    ExprNode::Lit { value: crate::expr::Literal::Sym { value } }
                        if value.as_str() == "layout"
                );
                if is_layout {
                    layout_kwarg = Some(v.clone());
                }
                !is_layout
            });
        }
    }
    args.retain(|a| !matches!(&*a.node, ExprNode::Hash { entries, .. } if entries.is_empty()));
    if args.is_empty() {
        return;
    }
    // An explicit `layout: false` VETOES the wrap outright — the
    // controller-side partial-render rewrite plants it (Rails renders
    // partials without a layout), and a user's `render …, layout:
    // false` means the same thing.
    if layout_kwarg.as_ref().is_some_and(|v| {
        matches!(&*v.node, ExprNode::Lit { value: crate::expr::Literal::Bool { value: false } })
    }) {
        return;
    }
    // An explicit `layout: "application"` / `layout: true` wraps a body
    // render (non-Views literal html) the way Rails does. Other layout
    // names are left unwrapped — honest residue, only `application`
    // exists as an emitted layout.
    let layout_requested = layout_kwarg.as_ref().is_some_and(|v| match &*v.node {
        ExprNode::Lit { value: crate::expr::Literal::Str { value } } => value == "application",
        ExprNode::Lit { value: crate::expr::Literal::Sym { value } } => {
            value.as_str() == "application"
        }
        ExprNode::Lit { value: crate::expr::Literal::Bool { value } } => *value,
        _ => false,
    });
    // Trailing kwargs are fine (`status: :unprocessable_entity` renders
    // WITH layout in Rails) — except the jbuilder branch's
    // `content_type:`, which marks a non-html response.
    let has_content_type = args.iter().skip(1).any(|a| match &*a.node {
        ExprNode::Hash { entries, .. } => entries.iter().any(|(k, _)| {
            matches!(&*k.node, ExprNode::Lit { value: crate::expr::Literal::Sym { value } }
                if value.as_str() == "content_type")
        }),
        _ => false,
    });
    if has_content_type {
        return;
    }
    // An explicit `layout: "application"`/`true` wraps WHATEVER the
    // body is — `render html: content.html_safe, layout:
    // "application"` (lobsters /u serves a cached tree string) has a
    // Send body that isn't a Views call, and the Views-shape test
    // alone wrongly skipped it. Without an explicit request, only a
    // non-layout non-json Views call wraps (the implicit html render).
    let wrappable = layout_requested
        || match &*args[0].node {
            ExprNode::Send { recv: Some(r), method: vm, .. } => {
                !vm.as_str().ends_with("_json")
                    && matches!(&*r.node, ExprNode::Const { path }
                        if path.len() == 2
                            && path[0].as_str() == "Views"
                            && path[1].as_str() != "Layouts")
            }
            _ => false,
        };
    if !wrappable {
        return;
    }
    let inner = args[0].clone();
    if let Some(wrapped) = crate::lower::view_to_library::layout_wrap_expr_with(app, closures, inner) {
        args[0] = wrapped;
    }
}

// `record.update!(k: v, ...)` kwargs inlining moved to the shared
// post-analyze hook (`lower::apply_update_kwargs_inline`) — hook
// bodies arrive here already in writer-assign + save form, with
// unknown-receiver and impure-receiver sites on the residue ledger.

// `errors.add(:field, "msg")` grounding moved to the shared
// post-analyze hook (`lower::apply_errors_add_lowering`) — every hook
// body arrives here already rewritten to the `errors << "Field msg"`
// accumulator shape, with non-self receivers on the residue ledger.

/// Ground `Rails.application.routes.url_helpers.<x>_url(record?, host: H,
/// protocol: P, port: N)` → `"#{P}://#{H}:N#{RouteHelpers.<x>_path(record?)}"`.
/// The routing object graph behind `url_helpers` isn't modeled (and never
/// needs to be for this shape — an absolute URL is protocol + host + the
/// generated path helper). Leading positional args (the record in
/// `user_url(sender, host:)`) and any kwargs that are not host options
/// (real path params) are forwarded to the `<x>_path` helper; the record
/// rides whole so its custom `to_param` resolves. Non-matching url_helpers
/// uses are left alone. Applied to the Rails::Application reopen (whose
/// kwargs-only `root_url` is the original occurrence) and mod_note's
/// `user_url(sender, host:)`.
///
/// Rails' `url_for` keeps its host options for itself (`RESERVED_OPTIONS`,
/// see `lower::route_url_options`), so none of `host:`, `protocol:`,
/// `port:` or `only_path:` is ever passed on to the path helper:
///
///   * `only_path: true` answers the path alone. An app route's call
///     arrives here already renamed to `_path` by
///     `lower::route_url_options`; this arm is for the engine helpers
///     that pass does not know (Active Storage's `rails_blob_url`).
///   * `port:` follows the host as `:N`. A literal port that is the
///     scheme's own (80 for http, 443 for https) or `nil` is left out,
///     as Rails' `build_host_url` leaves it out.
/// `ActionView::ViewHelpers.url_for_options(RouteHelpers.<stem>_path(args), default_url_options)`:
/// a bare `<x>_url` in a class that defines its own `default_url_options`.
fn url_for_own_options(stem: &str, args: Vec<Expr>) -> Expr {
    let synth = |node: ExprNode| Expr::new(crate::span::Span::synthetic(), node);
    let path_call = synth(ExprNode::Send {
        recv: Some(synth(ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] })),
        method: Symbol::from(format!("{stem}_path")),
        args,
        block: None,
        parenthesized: true,
    });
    let options = synth(ExprNode::Send { recv: None, method: Symbol::from("default_url_options"), args: vec![], block: None, parenthesized: false });
    let mut call = synth(ExprNode::Send {
        recv: Some(synth(ExprNode::Const { path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")] })),
        method: Symbol::from("url_for_options"),
        args: vec![path_call, options],
        block: None,
        parenthesized: true,
    });
    call.ty = Some(crate::ty::Ty::Str);
    call
}

pub(crate) fn rewrite_url_helpers_absolute(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite_url_helpers_absolute);
    let matches = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(uh), method, args, block: None, .. }
            if method.as_str().ends_with("_url")
                && !args.is_empty()
                && matches!(&*args[args.len() - 1].node, ExprNode::Hash { .. })
                && is_url_helpers_chain(uh)
    );
    if !matches {
        return;
    }
    let span = expr.span;
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { method, mut args, .. } = node else { unreachable!() };
    // Split leading positional path args (e.g. the record in
    // `user_url(sender, host:)`) from the trailing kwargs hash.
    let trailing = args.pop().unwrap();
    let positional = args;
    let ExprNode::Hash { entries, .. } = &*trailing.node else { unreachable!() };
    let mut host: Option<Expr> = None;
    let mut protocol: Option<Expr> = None;
    let mut port: Option<Expr> = None;
    let mut only_path = false;
    let mut path_kwargs: Vec<(Expr, Expr)> = Vec::new();
    for (k, v) in entries {
        if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
            match value.as_str() {
                "host" => {
                    host = Some(v.clone());
                    continue;
                }
                "protocol" => {
                    protocol = Some(v.clone());
                    continue;
                }
                "port" => {
                    port = Some(v.clone());
                    continue;
                }
                "only_path" => {
                    only_path = matches!(&*v.node, ExprNode::Lit { value: Literal::Bool { value: true } });
                    continue;
                }
                _ => {}
            }
        }
        // Any other key is a real path parameter — forward it.
        path_kwargs.push((k.clone(), v.clone()));
    }
    // Forward leading positionals plus any surviving path kwargs to the
    // generated `<stem>_path` helper (the record rides whole, so custom
    // `to_param` — User=username — resolves inside the path helper).
    let mut path_args: Vec<Expr> = positional;
    if !path_kwargs.is_empty() {
        path_args.push(Expr::new(
            trailing.span,
            ExprNode::Hash { entries: path_kwargs, kwargs: true },
        ));
    }
    let stem = method.as_str().trim_end_matches("_url");
    let path_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] },
            )),
            method: Symbol::from(format!("{stem}_path")),
            args: path_args,
            block: None,
            parenthesized: true,
        },
    );
    if only_path {
        *expr = path_call;
        expr.ty = Some(crate::ty::Ty::Str);
        return;
    }
    let lit = |s: &str| crate::expr::InterpPart::Text { value: s.to_string() };
    let dyn_part = |e: Expr| crate::expr::InterpPart::Expr { expr: e };
    // The scheme's own port, when the protocol is known at compile time.
    let https = match protocol.as_ref().map(|p| &*p.node) {
        None => Some(false),
        Some(ExprNode::Lit { value: Literal::Str { value } }) => {
            Some(value.trim_end_matches("://") == "https")
        }
        Some(_) => None,
    };
    // A protocol known only at run time is read once, into a local the
    // port's run-time check below can read again.
    let protocol_var = Symbol::from("__url_protocol");
    let protocol_part = protocol.map(|p| match https {
        Some(_) => dyn_part(p),
        None => dyn_part(Expr::new(
            p.span,
            ExprNode::Assign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: protocol_var.clone() },
                value: p,
            },
        )),
    });
    let port_part = port.and_then(|p| match (&*p.node, https) {
        (ExprNode::Lit { value: Literal::Nil }, _) => None,
        (ExprNode::Lit { value: Literal::Int { value } }, Some(https)) => {
            let standard = if https { *value == 443 } else { *value == 80 };
            (!standard).then(|| lit(&format!(":{value}")))
        }
        (ExprNode::Lit { value: Literal::Str { value } }, Some(https)) => {
            let standard = if https { value == "443" } else { value == "80" };
            (!standard).then(|| lit(&format!(":{value}")))
        }
        // Known only at run time: decided there, as Rails decides it.
        _ => Some(dyn_part(runtime_port(p, https, &protocol_var))),
    });
    let mut parts: Vec<crate::expr::InterpPart> = Vec::new();
    match protocol_part {
        Some(p) => parts.push(p),
        None => parts.push(lit("http")),
    }
    parts.push(lit("://"));
    if let Some(h) = host {
        parts.push(dyn_part(h));
    }
    if let Some(p) = port_part {
        parts.push(p);
    }
    parts.push(dyn_part(path_call));
    *expr.node = ExprNode::StringInterp { parts };
    expr.ty = Some(crate::ty::Ty::Str);
}

/// `port:` known only at run time, read once:
///
/// ```ruby
/// __url_port = <port>
/// __url_port.nil? || __url_port.to_i == <default> ? "" : ":#{__url_port}"
/// ```
///
/// Rails' `build_host_url` leaves the port out when it is nil or the
/// scheme's own (`80` for http, `443` for https), comparing as Integers
/// (`"80"` is left out too). The default is a literal when the protocol
/// is, and otherwise read from the `__url_protocol` local the protocol
/// part assigned.
fn runtime_port(port: Expr, https: Option<bool>, protocol_var: &Symbol) -> Expr {
    let span = port.span;
    let mk = |node: ExprNode| Expr::new(span, node);
    let int = |v: i64| mk(ExprNode::Lit { value: Literal::Int { value: v } });
    let str_lit = |v: &str| mk(ExprNode::Lit { value: Literal::Str { value: v.to_string() } });
    let call = |recv: Expr, m: &str, args: Vec<Expr>| {
        mk(ExprNode::Send { recv: Some(recv), method: Symbol::from(m), args, block: None, parenthesized: true })
    };
    let port_var = Symbol::from("__url_port");
    let var = |name: &Symbol| mk(ExprNode::Var { id: crate::ident::VarId(0), name: name.clone() });
    let default = match https {
        Some(true) => int(443),
        Some(false) => int(80),
        None => mk(ExprNode::If {
            cond: call(call(var(protocol_var), "to_s", vec![]), "start_with?", vec![str_lit("https")]),
            then_branch: int(443),
            else_branch: int(80),
        }),
    };
    let leave_out = mk(ExprNode::BoolOp {
        op: crate::expr::BoolOpKind::Or,
        surface: Default::default(),
        left: call(var(&port_var), "nil?", vec![]),
        right: call(call(var(&port_var), "to_i", vec![]), "==", vec![default]),
    });
    let written = mk(ExprNode::StringInterp {
        parts: vec![
            crate::expr::InterpPart::Text { value: ":".to_string() },
            crate::expr::InterpPart::Expr { expr: var(&port_var) },
        ],
    });
    mk(ExprNode::Let {
        id: crate::ident::VarId(0),
        name: port_var.clone(),
        value: port,
        body: mk(ExprNode::If { cond: leave_out, then_branch: str_lit(""), else_branch: written }),
    })
}

fn is_url_helpers_chain(e: &Expr) -> bool {
    let ExprNode::Send { recv: Some(routes), method, .. } = &*e.node else { return false };
    if method.as_str() != "url_helpers" {
        return false;
    }
    let ExprNode::Send { recv: Some(rails_app), method: routes_m, .. } = &*routes.node else {
        return false;
    };
    if routes_m.as_str() != "routes" {
        return false;
    }
    matches!(&*rails_app.node, ExprNode::Send { recv: Some(r), method: app_m, .. }
        if app_m.as_str() == "application"
            && matches!(&*r.node, ExprNode::Const { path }
                if path.len() == 1 && path[0].as_str() == "Rails"))
}

// Mailer class-side wrappers (`def self.notify = new.notify(...)`)
// moved to the shared post-analyze hook
// (`lower::apply_mailer_class_side`) — mailer classes arrive here
// with the wrappers already synthesized, keyword/block-taking methods
// on the residue ledger.

// Block-form `create!/create do |kv| ... end` inlining moved to the
// shared post-analyze hook (`lower::apply_create_block_inline`) —
// every hook body arrives here with the factory block already inlined
// (kv = X.new; body; save-or-raise; kv).

/// View-pipeline vestige of the shared `Time.current` grounding
/// (`lower::apply_time_current_lowering`): the post-analyze hook skips
/// view bodies, so lowered view classes still take the rewrite here.
/// Delete when the view pipeline migrates to shared lowerings. Every
/// other body class arrives already grounded (re-running is an
/// idempotent no-op — `Time.current` no longer occurs).
pub(crate) fn apply_time_current_lowering(lcs: &mut [LibraryClass], app: &App) {
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            crate::lower::time_current::rewrite_time_current(&mut m.body, &app.time_formats);
        }
    }
}

/// View-pipeline vestige of the shared duration grounding
/// (`lower::apply_duration_lowering`): the post-analyze hook skips view
/// bodies, so lowered view classes still take the rewrite here
/// (lobsters' `_commentbox.html.erb` both subtracts a Duration from a
/// Time and calls `before?` on the result). Delete when the view
/// pipeline migrates to shared lowerings. Every other body class
/// arrives already grounded (re-running is an idempotent no-op — the
/// grounded forms no longer match).
///
/// The Time-vs-Duration comparison rule reads a receiver type the view
/// pipeline doesn't stamp, so in practice only the unit, predicate and
/// arithmetic rules bite here; running the full sequence keeps the one
/// order declaration (`lower::duration::apply_duration_rewrites`).
/// Ruby-family pre-emit pass: a fixture ACCESSOR is memoized, because
/// Rails' is.
///
/// `ActiveRecord::TestFixtures` keeps a `@fixture_cache` and clears it
/// between tests, so two `accounts(:signal)` calls in one test answer
/// the SAME object. The lowered accessor is `Account.find(1)` — a new
/// object every call — so campfire's `accounts(:signal).settings.
/// restrict_room_creation_to_administrators = true` followed by
/// `accounts(:signal).save!` is a mutation of one record and a save of
/// a DIFFERENT one. The write
/// vanished and campfire's room-creation restriction was never in
/// force. It is also what makes `.reload` mean anything: reloading a
/// throwaway instance is a no-op by construction.
///
/// RUBY-FAMILY, not the shared lowering, because the cache is CLASS
/// STATE on a module — `@__fx_<label>` inside a `def self.<label>`.
/// Rust, Crystal and Swift each build a fixture module too, and none of
/// them has a home for that slot; measured, all three dropped the
/// accessor entirely (`cannot find function 'one' in module
/// fixtures::articles`). Their accessors keep the per-call lookup,
/// which is the behaviour they have today.
///
/// The slot is cleared at the top of `_fixtures_load!` rather than by a
/// separate hook: the loader runs once per test, right after the schema
/// reset, so that IS the between-tests boundary.
///
/// Spelled `@slot = <lookup> if @slot.nil?` then a bare read, not
/// `@slot ||= <lookup>` — the python lowering declines `OpAssign`, and
/// the two-statement form leaves the method ending in a read.
pub(crate) fn apply_fixture_memoization(lcs: &mut [LibraryClass]) {
    for lc in lcs.iter_mut() {
        let labels: Vec<Symbol> = lc
            .methods
            .iter()
            .filter(|m| m.receiver == MethodReceiver::Class && is_fixture_lookup(&m.body))
            .map(|m| m.name.clone())
            .collect();
        if labels.is_empty() {
            continue;
        }
        for m in &mut lc.methods {
            if m.receiver != MethodReceiver::Class {
                continue;
            }
            if is_fixture_lookup(&m.body) {
                m.body = memoized_fixture_body(&m.name, &m.body);
            } else if m.name.as_str() == "_fixtures_load!" {
                let mut exprs: Vec<Expr> = labels
                    .iter()
                    .map(|label| {
                        Expr::new(
                            Span::synthetic(),
                            ExprNode::Assign {
                                target: LValue::Ivar { name: fixture_cache_ivar(label) },
                                value: Expr::new(
                                    Span::synthetic(),
                                    ExprNode::Lit { value: Literal::Nil },
                                ),
                            },
                        )
                    })
                    .collect();
                exprs.push(m.body.clone());
                m.body = Expr::new(Span::synthetic(), ExprNode::Seq { exprs });
            }
        }
    }
}

/// The lowered accessor shape — `<Model>.find(<literal id>)` and
/// nothing else. `by_label` (an if/elsif chain) and `_fixtures_load!`
/// (a Seq of inserts) both fail this, which is how the pass tells the
/// three kinds of method in a fixture module apart without knowing
/// their names.
fn is_fixture_lookup(body: &Expr) -> bool {
    matches!(&*body.node, ExprNode::Send { recv: Some(r), method, args, block: None, .. }
        if method.as_str() == "find"
            && args.len() == 1
            && matches!(&*args[0].node, ExprNode::Lit { value: Literal::Int { .. } })
            && matches!(&*r.node, ExprNode::Const { .. }))
}

/// `@__fx_<label>` — the cache slot. Prefixed so it cannot collide with
/// anything the fixture file names.
fn fixture_cache_ivar(label: &Symbol) -> Symbol {
    Symbol::from(format!("__fx_{}", label.as_str()))
}

fn memoized_fixture_body(label: &Symbol, lookup: &Expr) -> Expr {
    let slot = fixture_cache_ivar(label);
    let read = || {
        let mut e = Expr::new(Span::synthetic(), ExprNode::Ivar { name: slot.clone() });
        e.ty = lookup.ty.clone();
        e
    };
    let fill = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(read()),
                    method: Symbol::from("nil?"),
                    args: vec![],
                    block: None,
                    parenthesized: true,
                },
            ),
            then_branch: Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: LValue::Ivar { name: slot.clone() },
                    value: lookup.clone(),
                },
            ),
            else_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
        },
    );
    let mut seq = Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![fill, read()] });
    seq.ty = lookup.ty.clone();
    seq
}

pub(crate) fn apply_duration_lowering(lcs: &mut [LibraryClass], app: &App) {
    let temporal_predicates = !crate::lower::duration::app_defines_temporal_predicates(app);
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            crate::lower::duration::apply_duration_rewrites(&mut m.body, temporal_predicates, true);
        }
    }
}

/// A record handed to a path helper becomes its slug HERE, at the call
/// site, not inside the helper.
///
/// Rails puts `to_param` in the helper body and gets away with it: the
/// param is untyped, so the record arrives whole and `to_param`
/// dispatches to the model's override. Our helpers declare their
/// segments `String` (`routes_to_library::param_ty` — a slug segment
/// typed Int made every strict-target call site passing `short_id` a C
/// type error), and a declared type is a promise the caller has to
/// keep. Under AOT it IS kept: a `Tag` handed to a `String` slot is
/// coerced on the way in, so the helper body never saw a record at all
/// — `tag_path(tag)` rendered `/t/` with an EMPTY segment long before
/// `String#to_param` raised on it. The raise was the visible half of a
/// silent wrong-URL bug.
///
/// So the conversion moves to where the value's identity is still
/// known. Three signals, in order of how much they know:
///
///   1. the stamped type is a model that overrides `to_param` —
///      analyzed bodies (models, controllers) answer here;
///   2. the arg reads a SINGULAR association (`story.domain`,
///      `showing_user.invited_by_user`) whose target overrides it —
///      the `reference_targets` relation, rebuilt from `app.models`
///      because view bodies reach emit with `Ty::Var` args;
///   3. the arg is a bare name that IS a model name (`tag`, `@user`) —
///      the same convention the view lowerer's `ivar_ty` commits to.
///
/// Everything else is left alone, which is the whole reason the rule is
/// positive-signal-only: `tag.id`, `story.short_id`, `user.username`,
/// `user[:username]`, `@params["username"]` and an already-written
/// `story.to_param` all reach a path helper in this corpus, and every
/// one of them is already a slug. A rule phrased as "wrap unless it
/// looks scalar" would have to enumerate those instead, and would wrap
/// the next unfamiliar shape by default.
pub(crate) fn apply_route_param_lowering(lcs: &mut [LibraryClass], app: &App) {
    let all_models: crate::det::HashSet<String> =
        app.models.iter().map(|m| m.name.0.as_str().to_string()).collect();
    let slug_models = models_overriding_to_param(app);
    let assoc_targets = singular_association_targets(app);
    let owned_assocs = owned_singular_associations(app);
    let collection_targets = collection_association_targets(app);
    // `direct :name do |record, options| … end` helpers take their
    // arguments VERBATIM — Rails hands them to the block, and the block
    // decides what to read. campfire's
    // `direct :fresh_user_avatar do |user, options| route_for
    // :user_avatar, user.avatar_token, v: user.updated_at… end` wants
    // the RECORD; projecting it to `user.id` first handed the block an
    // Integer and every call died on `undefined method 'updated_at'`.
    // Skipping them by name is the one place this pass can tell the
    // difference: a generated resource helper's segment is a param, a
    // direct helper's argument is whatever its block says it is.
    let direct_helpers: crate::det::HashSet<String> = app
        .routes
        .direct_helpers
        .iter()
        .map(|h| format!("{}_path", h.name.as_str()))
        .collect();
    let class_method_targets = record_answering_class_methods(app);
    // The route helpers' OWN lowered signatures — read off the slice we
    // are rewriting, not re-derived from the route table. `param_ty`
    // decides a segment's type from CALL-SITE evidence (a String
    // reaching it makes it a slug), and this pass decides a record's
    // projection from whether the model overrides `to_param`. Those are
    // two different questions and campfire answers them differently for
    // the same helper, so the projection has to read what the signature
    // actually says.
    let (helper_param_tys, helper_keyword_tys) = route_helper_param_types(app);
    let integer_columns = integer_columns(app);
    let mut sig = RecordSignals {
        all_models: &all_models,
        slug_models: &slug_models,
        assoc_targets: &assoc_targets,
        owned_assocs: &owned_assocs,
        collection_targets: &collection_targets,
        class_method_targets: &class_method_targets,
        direct_helpers: &direct_helpers,
        helper_param_tys: &helper_param_tys,
        helper_keyword_tys: &helper_keyword_tys,
        integer_columns: &integer_columns,
        controller_names: app
            .controllers
            .iter()
            .filter_map(|c| c.name.0.as_str().rsplit("::").next().map(str::to_string))
            .collect(),
        self_returns: crate::det::HashMap::default(),
        local_records: crate::det::HashMap::default(),
        self_model: None,
        next_local: std::cell::Cell::new(0),
        taken_locals: std::cell::RefCell::new(crate::det::HashSet::default()),
    };
    // What each INSTANCE method in this slice answers, to a fixpoint.
    //
    // Across the whole slice, not per class, because the caller and the
    // method are routinely in different ones: campfire's
    // `WelcomeController#index` reads `last_room_visited`, which
    // `ApplicationController` declares. Walking an ancestor chain would
    // say the same thing for these two and less for a concern; the
    // uniqueness rule below is what keeps a slice-wide map honest, and
    // it is the same standard the association maps hold.
    //
    // Surveyed from the CONTROLLERS TOO, off `app.controllers` rather
    // than the slice: the slice being rewritten is routinely the
    // models/helpers or views one, and the method a helper names lives
    // in a controller — campfire's
    // `RoomsHelper.link_back_to_last_room_visited` reads
    // `Current.controller.last_room_visited`, declared on
    // `ApplicationController`. On the controllers slice the same
    // methods are surveyed twice and agree, which the uniqueness rule
    // absorbs.
    //
    // A FIXPOINT because the methods call each other:
    // `last_room_visited` is `…find_by(id: cookie) || default_room`,
    // and `default_room` has to be resolved first. Bounded by the
    // method count; two rounds in practice.
    {
        let controller_methods: Vec<(&str, &Expr)> = app
            .controllers
            .iter()
            .flat_map(|c| c.body.iter())
            .filter_map(|item| match item {
                crate::dialect::ControllerBodyItem::Action { action, .. } => {
                    Some((action.name.as_str(), &action.body))
                }
                _ => None,
            })
            .collect();
        let mut ambiguous: crate::det::HashSet<String> = crate::det::HashSet::default();
        for _ in 0..8 {
            let mut grew = false;
            let slice_methods = lcs.iter().flat_map(|lc| {
                lc.methods
                    .iter()
                    .filter(|m| m.receiver == MethodReceiver::Instance)
                    .map(|m| (m.name.as_str(), &m.body))
            });
            for (name, body) in slice_methods.chain(controller_methods.iter().copied()) {
                if ambiguous.contains(name) {
                    continue;
                }
                let Some(model) = arg_record_model(body_tail(body), &sig) else {
                    continue;
                };
                match sig.self_returns.get(name) {
                    Some(existing) if existing == &model => {}
                    Some(_) => {
                        ambiguous.insert(name.to_string());
                        sig.self_returns.remove(name);
                        grew = true;
                    }
                    None => {
                        sig.self_returns.insert(name.to_string(), model);
                        grew = true;
                    }
                }
            }
            if !grew {
                break;
            }
        }
    }
    for lc in lcs.iter_mut() {
        sig.self_model = Some(lc.name.0.as_str().to_string())
            .filter(|n| all_models.contains(n));
        for m in &mut lc.methods {
            sig.local_records.clear();
            sig.local_records = collect_local_records(&m.body, &sig);
            {
                let mut taken = sig.taken_locals.borrow_mut();
                taken.clear();
                taken.extend(m.params.iter().map(|p| p.name.clone()));
                crate::lower::model_to_library::markers::collect_local_names(&m.body, &mut taken);
            }
            rewrite_route_params(&mut m.body, &sig);
        }
    }
}

/// Locals a body binds to ONE resolvable record, by name — the map
/// behind `arg_record_model`'s local-variable signal. campfire's
///
/// ```text
/// if last_room = last_room_visited
///   link_back_to room_path(last_room)
/// ```
///
/// hands the helper a plain local; every signal the pass reads is
/// gone from the name itself, but the ASSIGNMENT still says what it
/// holds. Held to the standard every map here shares: a name every
/// assignment resolves to the same model answers that model; a name
/// any assignment does not resolve (or two disagree about) answers
/// nothing — including through `OpAssign`/`MultiAssign`, which this
/// pass cannot read.
fn collect_local_records(
    body: &Expr,
    sig: &RecordSignals<'_>,
) -> crate::det::HashMap<String, String> {
    fn walk(
        e: &Expr,
        sig: &RecordSignals<'_>,
        out: &mut crate::det::HashMap<String, String>,
        poisoned: &mut crate::det::HashSet<String>,
    ) {
        let unresolve = |name: &Symbol,
                             out: &mut crate::det::HashMap<String, String>,
                             poisoned: &mut crate::det::HashSet<String>| {
            out.remove(name.as_str());
            poisoned.insert(name.as_str().to_string());
        };
        match &*e.node {
            ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
                match arg_record_model(value, sig) {
                    Some(model) if !poisoned.contains(name.as_str()) => {
                        match out.get(name.as_str()) {
                            Some(existing) if existing == &model => {}
                            Some(_) => unresolve(name, out, poisoned),
                            None => {
                                out.insert(name.as_str().to_string(), model);
                            }
                        }
                    }
                    _ => unresolve(name, out, poisoned),
                }
            }
            ExprNode::OpAssign { target: LValue::Var { name, .. }, .. } => {
                unresolve(name, out, poisoned);
            }
            ExprNode::MultiAssign { targets, .. } => {
                for t in targets {
                    if let LValue::Var { name, .. } = t {
                        unresolve(name, out, poisoned);
                    }
                }
            }
            _ => {}
        }
        e.node.for_each_child(&mut |c| walk(c, sig, out, poisoned));
    }
    let mut out = crate::det::HashMap::default();
    let mut poisoned = crate::det::HashSet::default();
    walk(body, sig, &mut out, &mut poisoned);
    out
}

/// The last expression a body evaluates to — its return value for
/// every shape this pass reads. A `Seq`'s tail; anything else is
/// already the tail.
fn body_tail(body: &Expr) -> &Expr {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.last().unwrap_or(body),
        _ => body,
    }
}

/// Model class methods that answer ONE RECORD of their own model —
/// a body whose tail is `.first` / `.last`.
///
/// campfire's `Room.original` (`order(:created_at).first`) is the
/// caller: `default_room` reads `Current.user.rooms.original`, whose
/// receiver is a `has_many :through` and so carries no type at all.
/// The METHOD NAME carries it instead, on the same uniqueness rule the
/// association maps use — a name two models declare answers None.
fn record_answering_class_methods(app: &App) -> crate::det::HashMap<String, String> {
    let mut out: crate::det::HashMap<String, String> = crate::det::HashMap::default();
    let mut ambiguous: crate::det::HashSet<String> = crate::det::HashSet::default();
    // Read from `app.models`, not from the LibraryClass slice this pass
    // was handed: the pass runs over CONTROLLERS too, and the class
    // method a controller body names lives on a model that slice does
    // not contain.
    for model_def in &app.models {
        let model = model_def.name.0.as_str().to_string();
        for m in model_def.body.iter().filter_map(|item| match item {
            crate::dialect::ModelBodyItem::Method { method, .. } => Some(method),
            _ => None,
        }) {
            if m.receiver != MethodReceiver::Class {
                continue;
            }
            let tail = body_tail(&m.body);
            let answers_one = matches!(&*tail.node,
                ExprNode::Send { recv: Some(_), method, args, block: None, .. }
                    if args.is_empty() && matches!(method.as_str(), "first" | "last"));
            if !answers_one {
                continue;
            }
            let key = m.name.as_str().to_string();
            match out.get(&key) {
                Some(existing) if existing != &model => {
                    ambiguous.insert(key);
                }
                _ => {
                    out.insert(key, model.clone());
                }
            }
        }
    }
    for k in ambiguous {
        out.remove(&k);
    }
    out
}

/// Model names (`Tag`, `User`) whose class defines its own `to_param`.
fn models_overriding_to_param(app: &App) -> crate::det::HashSet<String> {
    app.models
        .iter()
        .filter(|m| {
            m.body.iter().any(|item| matches!(item, crate::dialect::ModelBodyItem::Method { method, .. }
                if method.name.as_str() == "to_param"))
        })
        .map(|m| m.name.0.as_str().to_string())
        .collect()
}

/// `(model, column)` for each integer column of a model's table: what
/// `comment.article_id` reads, where a view body still carries `Ty::Var`.
fn integer_columns(app: &App) -> crate::det::HashSet<(String, String)> {
    use crate::schema::ColumnType;
    let mut out = crate::det::HashSet::default();
    for m in &app.models {
        let Some(table) = app.schema.tables.get(&m.table.0) else { continue };
        for c in &table.columns {
            if matches!(c.col_type, ColumnType::Integer | ColumnType::BigInt) {
                out.insert((m.name.0.as_str().to_string(), c.name.as_str().to_string()));
            }
        }
    }
    out
}

/// Singular association reader → target model name. `belongs_to` and
/// `has_one` only: a `has_many` reader is a collection and never lands
/// in a path segment. An ambiguous name (two models naming the same
/// reader at different targets) is dropped rather than guessed.
/// has_many reader name → target model, when unambiguous across models.
///
/// Only consulted for `.first` / `.last` on such a read: those answer
/// ONE record of the collection's type, which is a record a path helper
/// needs converted. campfire's front door is
/// `redirect_to room_url(Current.user.rooms.last)` — a Send chain, so
/// none of the name-based signals see it, and it redirected to
/// `/rooms/#<Room:0x…>`. An ambiguous name is dropped rather than
/// guessed, exactly as the singular map does.
fn collection_association_targets(app: &App) -> crate::det::HashMap<String, String> {
    use crate::dialect::Association;
    let mut out: crate::det::HashMap<String, String> = crate::det::HashMap::default();
    let mut ambiguous: crate::det::HashSet<String> = crate::det::HashSet::default();
    for m in &app.models {
        for a in m.associations() {
            let Association::HasMany { name, target, .. } = a else { continue };
            let key = name.as_str().to_string();
            let val = target.0.as_str().to_string();
            match out.get(&key) {
                Some(existing) if existing != &val => {
                    ambiguous.insert(key);
                }
                _ => {
                    out.insert(key, val);
                }
            }
        }
    }
    for k in ambiguous {
        out.remove(&k);
    }
    out
}

fn singular_association_targets(app: &App) -> crate::det::HashMap<String, String> {
    use crate::dialect::Association;
    let mut out: crate::det::HashMap<String, String> = crate::det::HashMap::default();
    let mut ambiguous: crate::det::HashSet<String> = crate::det::HashSet::default();
    for m in &app.models {
        for a in m.associations() {
            let (name, target) = match a {
                Association::BelongsTo { name, target, .. }
                | Association::HasOne { name, target, .. } => (name, target),
                _ => continue,
            };
            let key = name.as_str().to_string();
            let val = target.0.as_str().to_string();
            match out.get(&key) {
                Some(existing) if existing != &val => {
                    ambiguous.insert(key);
                }
                _ => {
                    out.insert(key, val);
                }
            }
        }
    }
    for k in ambiguous {
        out.remove(&k);
    }
    out
}

fn owned_singular_associations(app: &App) -> crate::det::HashSet<(String, String)> {
    use crate::dialect::Association;
    let mut out = crate::det::HashSet::default();
    for m in &app.models {
        // Not the model's own declarations alone: an STI subclass (campfire's `Rooms::Open < Room`) answers its parent's associations.
        let mut current = Some(m);
        let mut seen = 0;
        while let Some(owner) = current {
            for a in owner.associations() {
                if let Association::BelongsTo { name, .. } | Association::HasOne { name, .. } = a {
                    out.insert((m.name.0.as_str().to_string(), name.as_str().to_string()));
                }
            }
            seen += 1;
            current = owner
                .parent
                .as_ref()
                .filter(|_| seen < 16)
                .and_then(|p| app.models.iter().find(|x| &x.name == p));
        }
    }
    out
}

/// Each route helper's declared query keywords: `<helper>_path` -> key
/// name -> declared type.
type HelperKeywordTys = crate::det::HashMap<String, crate::det::HashMap<String, crate::ty::Ty>>;

/// Everything `arg_record_model` reads, in one place — the signal set
/// grew past what a positional argument list explains.
///
/// `self_returns` is the one that varies per LibraryClass: it maps THIS
/// class's own method names to the model each one answers, so a bare
/// `room_path(last_room_visited)` can resolve through the method it
/// names. The other four are App-wide.
/// `<helper>_path` -> the declared type of each positional parameter,
/// and of each query keyword by name.
///
/// Read off the SAME builder the emit ships
/// (`lower_routes_to_library_functions`), not re-derived from the route
/// table: `param_ty`'s slug rule reads call-site evidence, and a second
/// copy of that reasoning here would drift the first time either moved.
/// The route helpers are `LibraryFunction`s and never appear in the
/// `lcs` slice this pass rewrites, so they have to be asked for.
///
/// Called BEFORE this pass mutates anything — the builder surveys the
/// IR to size each helper, which is the ordering constraint
/// `route_helper_receiver` already documents.
fn route_helper_param_types(
    app: &App,
) -> (crate::det::HashMap<String, Vec<crate::ty::Ty>>, HelperKeywordTys) {
    let mut out = crate::det::HashMap::default();
    let mut keywords: HelperKeywordTys = crate::det::HashMap::default();
    for f in crate::lower::routes_to_library::lower_routes_to_library_functions(app) {
        if f.module_path.last().map(|s| s.as_str()) != Some("RouteHelpers") {
            continue;
        }
        let Some(crate::ty::Ty::Fn { params, .. }) = &f.signature else {
            continue;
        };
        out.insert(
            f.name.as_str().to_string(),
            params.iter().map(|p| (*p.ty).clone()).collect(),
        );
        keywords.insert(
            f.name.as_str().to_string(),
            params
                .iter()
                .filter(|p| matches!(p.kind, crate::ty::ParamKind::Keyword { .. }))
                .map(|p| (p.name.as_str().to_string(), (*p.ty).clone()))
                .collect(),
        );
    }
    (out, keywords)
}

pub(crate) struct RecordSignals<'a> {
    all_models: &'a crate::det::HashSet<String>,
    slug_models: &'a crate::det::HashSet<String>,
    assoc_targets: &'a crate::det::HashMap<String, String>,
    /// `(model, name)` for each singular association a model declares.
    owned_assocs: &'a crate::det::HashSet<(String, String)>,
    collection_targets: &'a crate::det::HashMap<String, String>,
    /// A model CLASS METHOD whose body answers one record of that model
    /// — `Room.original` is `order(:created_at).first`. Unambiguous
    /// names only.
    class_method_targets: &'a crate::det::HashMap<String, String>,
    direct_helpers: &'a crate::det::HashSet<String>,
    /// `<helper>_path` -> declared type per positional parameter.
    helper_param_tys: &'a crate::det::HashMap<String, Vec<crate::ty::Ty>>,
    /// `<helper>_path` -> declared type per query keyword.
    helper_keyword_tys: &'a HelperKeywordTys,
    /// `(model, column)` for each integer column (`integer_columns`).
    integer_columns: &'a crate::det::HashSet<(String, String)>,
    /// Controller class names (last path segment), so a class-side
    /// respelling (`ApplicationController.last_room_visited`) is
    /// recognized as the controller-method read it is.
    controller_names: crate::det::HashSet<String>,
    self_returns: crate::det::HashMap<String, String>,
    /// Locals THE BODY BEING REWRITTEN binds to a resolvable record —
    /// campfire's `if last_room = last_room_visited` puts the record
    /// behind a name no other signal can read. Rebuilt per method;
    /// a name is held to the same standard as every map here: every
    /// assignment must resolve, and to the same model, or the name
    /// answers nothing.
    local_records: crate::det::HashMap<String, String>,
    /// The model whose methods are being rewritten, when the enclosing
    /// class is one — what a bare `self` names there.
    self_model: Option<String>,
    /// Numbers the `route_segment_<n>` locals an optional segment binds.
    next_local: std::cell::Cell<usize>,
    /// The names the method being rewritten binds (its params and locals),
    /// plus the `route_segment_<n>` taken so far. Rebuilt per method.
    taken_locals: std::cell::RefCell<crate::det::HashSet<Symbol>>,
}

fn rewrite_route_params(expr: &mut Expr, sig: &RecordSignals<'_>) {
    expr.node.for_each_child_mut(&mut |e| rewrite_route_params(e, sig));
    let is_helper_call = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(r), method, block: None, .. }
            if method.as_str().ends_with("_path")
                && !sig.direct_helpers.contains(method.as_str())
                && matches!(&*r.node, ExprNode::Const { path }
                    if path.last().map(|s| s.as_str() == "RouteHelpers").unwrap_or(false))
    );
    if !is_helper_call {
        return;
    }
    let ExprNode::Send { args, method: helper, .. } = &mut *expr.node else { unreachable!() };
    let declared = sig.helper_param_tys.get(helper.as_str()).cloned();
    for (idx, arg) in args.iter_mut().enumerate() {
        let wants_str_here = matches!(
            declared.as_ref().and_then(|tys| tys.get(idx)),
            Some(crate::ty::Ty::Str)
        );
        let Some(model) = arg_record_model(arg, sig) else {
            // The record may ALREADY be projected: the view lowerer's
            // `rewrite_path_arg` turns a local record into `<local>.id`
            // long before this pass runs, so by here the argument is a
            // bare `id` read and `arg_record_model` sees no record at
            // all. Same defect, later in the pipeline — coerce it the
            // same way.
            //
            // A Symbol too: Rails renders one with `to_param`, which is
            // `to_s`. campfire's own test writes `user_sidebar_url(:me)`
            // against a `scope defaults: { user_id: "me" }` segment,
            // and that segment is declared `String` after its default.
            if wants_str_here {
                if let ExprNode::Lit { value: crate::expr::Literal::Sym { value } } = &*arg.node {
                    let mut lit = Expr::new(
                        arg.span,
                        ExprNode::Lit {
                            value: crate::expr::Literal::Str { value: value.as_str().to_string() },
                        },
                    );
                    lit.ty = Some(crate::ty::Ty::Str);
                    *arg = lit;
                    continue;
                }
            }
            // Any other Integer too (roundhouse#1468): a segment one call
            // fills with a String is declared `String`, and a foreign key
            // another call passes (`comment.article_id`) is an Integer
            // there. Rails renders it with `to_param`, its `to_s`. A view
            // body's read still carries `Ty::Var`, so a record's integer
            // column counts as well.
            // An optional segment (`/top(/:length)`) is `String | nil`
            // (CodeRabbit on #1473). An Integer there is its `to_s` too, but
            // a `nil` has to stay `nil`: Rails drops the segment for it, and
            // `nil.to_s` would keep an empty one. A column read can be nil
            // whatever the schema says (a new record's key, a reader the
            // model overrides), so the argument is bound once and only a
            // value that isn't nil is converted.
            let wants_opt_str_here = matches!(
                declared.as_ref().and_then(|tys| tys.get(idx)),
                Some(crate::ty::Ty::Union { variants })
                    if variants.len() == 2
                        && variants.contains(&crate::ty::Ty::Str)
                        && variants.contains(&crate::ty::Ty::Nil)
            );
            // `Integer | nil` too (`flag ? n : nil`): the binding keeps the nil.
            let int_or_nil = matches!(&arg.ty, Some(crate::ty::Ty::Union { variants })
                if variants.len() == 2
                    && variants.contains(&crate::ty::Ty::Int)
                    && variants.contains(&crate::ty::Ty::Nil));
            if wants_opt_str_here
                && (matches!(arg.ty, Some(crate::ty::Ty::Int)) || int_or_nil || is_integer_column_read(arg, sig))
            {
                let value = std::mem::replace(arg, Expr::new(arg.span, ExprNode::Seq { exprs: vec![] }));
                *arg = to_s_keeping_nil(value, sig);
                continue;
            }
            if wants_str_here
                && (is_bare_id_read(arg)
                    || is_symbol_arg(arg)
                    || matches!(arg.ty, Some(crate::ty::Ty::Int))
                    || is_integer_column_read(arg, sig))
            {
                let span = arg.span;
                let inner =
                    std::mem::replace(arg, Expr::new(span, ExprNode::Seq { exprs: vec![] }));
                let mut to_s = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(inner),
                        method: Symbol::from("to_s"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                );
                to_s.ty = Some(crate::ty::Ty::Str);
                *arg = to_s;
            }
            continue;
        };
        // A model that overrides `to_param` answers its slug there and
        // the segment is declared `Str`. Every other model's `to_param`
        // IS `id`, and `param_ty` USUALLY declares that segment `Int` —
        // so call `id` rather than a `to_param` the emitted model
        // doesn't have.
        //
        // …unless the SIGNATURE says otherwise, which is a different
        // question from the one this pass asks. `param_ty` reads
        // CALL-SITE evidence: campfire's `direct :fresh_user_avatar`
        // hands `user.avatar_token` to `:user_avatar`, so that segment
        // is a slug and declared `Str` — while `User` overrides no
        // `to_param`, and the other call site is
        // `user_avatar_path(@user)`. A bare `id` there hands an Int to
        // a String slot. Rails renders a plain record with the DEFAULT
        // `to_param`, which IS `id.to_s`; emit that, and both call
        // sites agree on a text segment.
        //
        // This stayed invisible for as long as `@id` was boxed — a poly
        // interpolates into a path either way. Pinning it (da9c5d89) is
        // what made the contradiction reachable.
        project_record_arg(arg, &model, sig, wants_str_here);
    }
    // A POSITIONAL empty hash (`new_mod_reparent_path({}, id: …)`) is
    // Rails' empty options argument. The generated helper takes its
    // options as keywords only, so the `{}` is an extra positional
    // there — an ArgumentError under CRuby. It says nothing; drop it.
    if let ExprNode::Send { args, .. } = &mut *expr.node {
        args.retain(|a| !matches!(&*a.node,
            ExprNode::Hash { entries, kwargs: false } if entries.is_empty()));
    }
    // Query options (`new_mod_reparent_path({}, id: @reparent_user)`):
    // Rails renders a record there with `to_param` too. The helper's
    // query key is typed to match (routes_to_library::query_param_demand
    // types a record-valued key `String` for a slug model, else `Integer`).
    // A scalar under any other key (`page: page + 1`, `featured: true`,
    // `ratio: 1.5`) goes through `to_s`, as `project_route_helper_ids`
    // does in test bodies; one that can be nil keeps the nil, so the
    // helper leaves the key out as Rails does.
    if let ExprNode::Send { args, method: helper, .. } = &mut *expr.node {
        use crate::lower::controller_to_library::rewrites::{scalar_in_query_slot, scalar_value, to_s_of, QueryScalar};
        let keyword_tys = sig.helper_keyword_tys.get(helper.as_str());
        if let Some(last) = args.last_mut() {
            if let ExprNode::Hash { entries, kwargs: true } = &mut *last.node {
                for (k, v) in entries.iter_mut() {
                    if let Some(model) = arg_record_model(v, sig) {
                        project_record_arg(v, &model, sig, false);
                    } else {
                        // An `id` / `*_id` key is Integer by name, but the
                        // helper declares it `String?` once a slug record
                        // reaches it; a scalar there is text like any other.
                        let declared_text = matches!(&*k.node, ExprNode::Lit { value: crate::expr::Literal::Sym { value } }
                            if keyword_tys.and_then(|tys| tys.get(value.as_str())).map(Ty::peel_nilable) == Some(&Ty::Str));
                        let scalar = scalar_in_query_slot(k, v).or_else(|| if declared_text { scalar_value(v) } else { None });
                        match scalar {
                            Some(QueryScalar::Text) => {
                                let mut to_s = to_s_of(v.clone());
                                to_s.ty = Some(Ty::Str);
                                *v = to_s;
                            }
                            Some(QueryScalar::TextOrNil) => *v = to_s_keeping_nil(v.clone(), sig),
                            None => push_hash_query_option(helper, k, v),
                        }
                    }
                }
            }
        }
    }
}

/// A Hash under a route helper's query key. Rails writes it as nested
/// parameters (`filter: { a: 1 }` is `?filter%5Ba%5D=1`); the generated
/// helper types the key `String?` and writes the value's `to_s`. The
/// call is left as it is and ledgered, so the survey lists it.
fn push_hash_query_option(helper: &Symbol, k: &Expr, v: &Expr) {
    use crate::diagnostic::{Diagnostic, DiagnosticKind};
    if !matches!(v.ty.as_ref().map(crate::ty::Ty::peel_nilable), Some(crate::ty::Ty::Hash { .. })) {
        return;
    }
    let ExprNode::Lit { value: crate::expr::Literal::Sym { value: key } } = &*k.node else {
        return;
    };
    let kind = DiagnosticKind::LowerResidue {
        pass: Symbol::from("route_query_option"),
        construct: Symbol::from(format!("{}({}:)", helper.as_str(), key.as_str())),
        reason: Symbol::from("hash value"),
    };
    crate::emit::diagnostics::push(Diagnostic {
        span: v.span,
        severity: Diagnostic::default_severity(&kind),
        kind,
        message: format!(
            "`{}` query option `{}` is a Hash: Rails writes nested `{}[…]=` parameters, \
             the generated helper writes the Hash's `to_s`",
            helper.as_str(),
            key.as_str(),
            key.as_str()
        ),
    });
}

/// `value`'s `to_s`, or nil when it is nil: the value bound once to a
/// fresh `route_segment_<n>` local, then `nil? ? nil : to_s`. Rails
/// leaves a nil segment or query value out, and `nil.to_s` would keep
/// an empty one.
fn to_s_keeping_nil(value: Expr, sig: &RecordSignals<'_>) -> Expr {
    let span = value.span;
    // A name the method doesn't already bind: the `Let` assigns in
    // the method's own scope.
    let name = loop {
        let n = sig.next_local.get();
        sig.next_local.set(n + 1);
        let candidate = Symbol::from(format!("route_segment_{n}"));
        if sig.taken_locals.borrow_mut().insert(candidate.clone()) {
            break candidate;
        }
    };
    let value_ty = value.ty.clone().unwrap_or(crate::ty::Ty::Untyped);
    let typed = |node: ExprNode, ty: crate::ty::Ty| Expr { ty: Some(ty), ..Expr::new(span, node) };
    let read = || typed(ExprNode::Var { id: crate::ident::VarId(0), name: name.clone() }, value_ty.clone());
    let send = |method: &str, ty: crate::ty::Ty| {
        typed(
            ExprNode::Send { recv: Some(read()), method: Symbol::from(method), args: vec![], block: None, parenthesized: false },
            ty,
        )
    };
    let opt_str = crate::ty::Ty::Union { variants: vec![crate::ty::Ty::Str, crate::ty::Ty::Nil].into() };
    let check = typed(
        ExprNode::If {
            cond: send("nil?", crate::ty::Ty::Bool),
            then_branch: typed(ExprNode::Lit { value: crate::expr::Literal::Nil }, crate::ty::Ty::Nil),
            else_branch: send("to_s", crate::ty::Ty::Str),
        },
        opt_str.clone(),
    );
    typed(ExprNode::Let { id: crate::ident::VarId(0), name, value, body: check }, opt_str)
}

/// A record in a route helper's argument, as Rails renders it: the
/// model's own `to_param` (a slug model), else its id — as text when
/// the slot is declared `String`.
fn project_record_arg(arg: &mut Expr, model: &str, sig: &RecordSignals<'_>, wants_str: bool) {
    {
        let slug = sig.slug_models.contains(model);
        let span = arg.span;
        let record = std::mem::replace(arg, Expr::new(span, ExprNode::Seq { exprs: vec![] }));
        let (method, ty) = if slug {
            ("to_param", crate::ty::Ty::Str)
        } else {
            ("id", crate::ty::Ty::Int)
        };
        let mut projected = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(record),
                method: Symbol::from(method),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        projected.ty = Some(ty);
        if !slug && wants_str {
            let mut to_s = Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(projected),
                    method: Symbol::from("to_s"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            );
            to_s.ty = Some(crate::ty::Ty::Str);
            projected = to_s;
        }
        *arg = projected;
    }
}

/// A bare `<recv>.id` read with no arguments — what the view lowerer
/// leaves behind for a record handed to a path helper.
fn is_bare_id_read(arg: &Expr) -> bool {
    matches!(&*arg.node,
        ExprNode::Send { recv: Some(_), method, args, block: None, .. }
            if method.as_str() == "id" && args.is_empty())
}

/// `record.<column>` where the record resolves to a model and the column
/// is one of its integer columns.
fn is_integer_column_read(arg: &Expr, sig: &RecordSignals<'_>) -> bool {
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*arg.node else {
        return false;
    };
    args.is_empty()
        && arg_record_model(r, sig)
            .is_some_and(|model| sig.integer_columns.contains(&(model, method.as_str().to_string())))
}

fn is_symbol_arg(arg: &Expr) -> bool {
    matches!(&*arg.node, ExprNode::Lit { value: crate::expr::Literal::Sym { .. } })
        || matches!(arg.ty, Some(crate::ty::Ty::Sym))
}

fn arg_record_model(arg: &Expr, sig: &RecordSignals<'_>) -> Option<String> {
    let all_models = sig.all_models;
    let assoc_targets = sig.assoc_targets;
    let collection_targets = sig.collection_targets;
    // Not by name alone once a read's type says otherwise: lobsters' `tag.tag` is Tag's String column, named like a `belongs_to :tag`. A bare local keeps its name: its inferred type is the weaker signal there.
    if matches!(&*arg.node, ExprNode::Send { recv: Some(_), .. }) && matches!(
        arg.ty.as_ref().map(crate::ty::Ty::peel_nilable),
        Some(crate::ty::Ty::Str | crate::ty::Ty::Int | crate::ty::Ty::Float | crate::ty::Ty::Sym | crate::ty::Ty::Bool)
    ) {
        return None;
    }
    // (1) A stamped model type.
    if let Some(crate::ty::Ty::Class { id, .. }) =
        arg.ty.as_ref().map(crate::ty::Ty::peel_nilable)
    {
        if all_models.contains(id.0.as_str()) {
            return Some(id.0.as_str().to_string());
        }
    }
    // (5) `a || b` — Rails' "this one, else that one" (campfire's
    // `last_room_visited` is `…find_by(id: cookie) || default_room`).
    // BOTH sides must answer the SAME model: one side resolving proves
    // nothing about the value that actually arrives.
    if let ExprNode::BoolOp { op: crate::expr::BoolOpKind::Or, left, right, .. } = &*arg.node {
        let (l, r) = (arg_record_model(left, sig), arg_record_model(right, sig));
        return match (l, r) {
            (Some(a), Some(b)) if a == b => Some(a),
            _ => None,
        };
    }
    match &*arg.node {
        // (8) `self` inside a model — lobsters'
        // `comment_short_id_path(self)` in Comment#as_json.
        ExprNode::SelfRef => sig.self_model.clone(),
        // (2) A singular association read, or (4) `.first` / `.last` on
        // a has_many read — one record of the collection's type. Both
        // are receiver-ful zero-arg Sends, so they share an arm: a
        // separate `first`/`last` arm placed after this one would be
        // unreachable, since this pattern matches those method names too.
        ExprNode::Send { recv: Some(r), method, args, block: None, .. } if args.is_empty() => {
            // Not by name alone when the receiver's model is known: lobsters' `category.category` and `tag.tag` are columns, named like another model's `belongs_to`.
            let recv_model = match (r.ty.as_ref().map(crate::ty::Ty::peel_nilable), &*r.node) {
                (Some(crate::ty::Ty::Class { id, .. }), _) if all_models.contains(id.0.as_str()) => Some(id.0.as_str().to_string()),
                (_, ExprNode::Var { name, .. }) if all_models.contains(&crate::naming::camelize(name.as_str())) => {
                    Some(crate::naming::camelize(name.as_str()))
                }
                (_, ExprNode::Var { name, .. }) => sig.local_records.get(name.as_str()).cloned(),
                _ => None,
            };
            let owned = recv_model
                .as_ref()
                .is_none_or(|m| sig.owned_assocs.contains(&(m.clone(), method.as_str().to_string())));
            if let Some(target) = assoc_targets
                .get(method.as_str())
                .filter(|target| owned && all_models.contains(*target))
            {
                return Some(target.clone());
            }
            // (7b) `self.<method>` — the same self-call as (7) below,
            // with the receiver spelled out. campfire writes
            // `… || self.default_room`, and the emitted controller
            // keeps that spelling.
            if matches!(&*r.node, ExprNode::SelfRef) {
                if let Some(model) = sig.self_returns.get(method.as_str()) {
                    return Some(model.clone());
                }
            }
            // (7c) The helper-module spelling of (7): once a helper
            // method leaves its controller for a module function, the
            // helper lowering rewrites its bare controller-method
            // reads to `ActionController::Current.controller.<method>`
            // — the receiver is by construction the same controller
            // instance (7) resolves against, so the same map answers.
            if let ExprNode::Send { recv: Some(cr), method: cm, args: cargs, block: None, .. } =
                &*r.node
            {
                if cm.as_str() == "controller"
                    && cargs.is_empty()
                    && matches!(&*cr.node, ExprNode::Const { path }
                        if path.last().map(|s| s.as_str() == "Current").unwrap_or(false))
                {
                    if let Some(model) = sig.self_returns.get(method.as_str()) {
                        return Some(model.clone());
                    }
                }
            }
            // (7d) The other helper-module spelling: a state-free
            // controller method gets a class-side twin, and the helper
            // reads `ApplicationController.last_room_visited`. The
            // receiver must NAME a controller — a model's class-side
            // call stays with (6).
            if let ExprNode::Const { path } = &*r.node {
                if path.last().is_some_and(|s| sig.controller_names.contains(s.as_str())) {
                    if let Some(model) = sig.self_returns.get(method.as_str()) {
                        return Some(model.clone());
                    }
                }
            }
            // (6) A model CLASS METHOD that answers one record —
            // campfire's `Current.user.rooms.original`, where `Room
            // .original` is `order(:created_at).first`. The receiver's
            // own type is exactly what does not resolve here; the
            // METHOD NAME is what carries it, on the same uniqueness
            // rule the association maps use.
            if let Some(target) = sig
                .class_method_targets
                .get(method.as_str())
                .filter(|target| all_models.contains(*target))
            {
                return Some(target.clone());
            }
            if !matches!(method.as_str(), "first" | "last") {
                return None;
            }
            let ExprNode::Send { method: assoc, args: aargs, block: None, .. } = &*r.node else {
                return None;
            };
            if !aargs.is_empty() {
                return None;
            }
            collection_targets
                .get(assoc.as_str())
                .filter(|target| all_models.contains(*target))
                .cloned()
        }
        // (4b) `find_by`/`find`/`find_by!` on a has_many read — ONE
        // record of the collection's type, exactly like `.first` above
        // and split out only because those take no arguments. The
        // receiver stays a zero-arg association read: `@room.messages
        // .where(…).find_by(…)` is a chain this pass does not follow.
        ExprNode::Send { recv: Some(r), method, block: None, .. }
            if matches!(method.as_str(), "find" | "find_by" | "find_by!") =>
        {
            let ExprNode::Send { method: assoc, args: aargs, block: None, .. } = &*r.node else {
                return None;
            };
            if !aargs.is_empty() {
                return None;
            }
            collection_targets
                .get(assoc.as_str())
                .filter(|target| all_models.contains(*target))
                .cloned()
        }
        // (3) A bare name that IS a model name — including the
        // zero-arg receiver-less bareword form. A template local
        // (`render "users/user", user: u` → `user` in the partial) and a
        // helper's own reader (`Room::MessagePusher#room`) both parse as
        // a receiver-less Send, not a Var, because prism cannot prove
        // the name is a local inside an ERB-ingested body. Matching only
        // Var/Ivar left campfire's `account_user_path(user)` and
        // `room_path(room)` unconverted — the same owner-form note
        // `scope_chain::owner_model_from_name` carries.
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => {
            let camel = crate::naming::camelize(name.as_str());
            if all_models.contains(&camel) {
                return Some(camel);
            }
            // (3b) A local whose ASSIGNMENT resolved — campfire's
            // `if last_room = last_room_visited` binds the record
            // behind a name that is no model's. Locals only: an ivar's
            // writes live in other methods this body-scoped map never
            // saw.
            if matches!(&*arg.node, ExprNode::Var { .. }) {
                if let Some(model) = sig.local_records.get(name.as_str()) {
                    return Some(model.clone());
                }
            }
            // (3c) `<qualifier>_<model>` — lobsters' `@reparent_user`,
            // `parent_comment`: the name ends in a model's singular.
            // Longest suffix first, so `mod_mail` is ModMail, not Mail.
            let segs: Vec<&str> = name.as_str().trim_start_matches('@').split('_').collect();
            (1..segs.len())
                .map(|i| crate::naming::camelize(&segs[i..].join("_")))
                .find(|camel| all_models.contains(camel))
        }
        ExprNode::Send { recv: None, method, args, block: None, .. } if args.is_empty() => {
            let camel = crate::naming::camelize(method.as_str());
            if all_models.contains(&camel) {
                return Some(camel);
            }
            // (7) …else a call to one of THIS class's own methods,
            // resolved through what that method's body answers.
            // `redirect_to room_path(last_room_visited)` is the shape:
            // the name is not a model's, and nothing upstream typed it.
            sig.self_returns.get(method.as_str()).cloned()
        }
        _ => None,
    }
}

/// Ruby-family pre-emit pass for TRANSPILED-SHAPE classes only: models
/// whose accessors were hand-written in the source (`attr_accessor
/// :created_at`) and so never passed through the shared model lowering.
/// Schema-synthesized models don't need it — `schema::synth_attr_reader`
/// already splits storage (`@<col>_raw`, String) from access (a reader
/// parsing via `ActiveSupport.parse_db_time`), for every target.
///
/// For the hand-written shape (storage under `@<col>` itself), the
/// reader becomes `@col && ActiveSupport.parse_db_time(@col)` — short-
/// circuits a nullable column's `nil` without needing to know
/// nullability; `parse_db_time` (not bare `Time.parse`) treats a
/// zone-less column as UTC rather than the system's local zone (see
/// `active_support_time_parsing.rb`). The writer becomes
/// `@col = (value.respond_to?(:iso8601) ? value.iso8601 : value)` —
/// normalizing a `Time` passed by app code back to text so every write
/// lands on the same on-disk format. A strict no-op for apps with no
/// Date/DateTime/Time columns (the blog) and for schema-synthesized
/// models (the plain-ivar-read gate below).
pub(crate) fn apply_datetime_lowering(lcs: &mut [LibraryClass], app: &App) {
    for model in &app.models {
        let Some(table) = app.schema.tables.get(&model.table.0) else {
            continue;
        };
        let temporal: BTreeSet<Symbol> = table
            .columns
            .iter()
            .filter(|c| {
                matches!(
                    c.col_type,
                    crate::schema::ColumnType::Date
                        | crate::schema::ColumnType::DateTime
                        | crate::schema::ColumnType::Time
                )
            })
            .map(|c| c.name.clone())
            .collect();
        if temporal.is_empty() {
            continue;
        }
        let Some(lc) = lcs.iter_mut().find(|lc| lc.name == model.name) else {
            continue;
        };
        for m in &mut lc.methods {
            if m.receiver != MethodReceiver::Instance {
                continue;
            }
            match m.kind {
                // Only a PLAIN `@col`-read body — the hand-written
                // `attr_reader` shape from transpiled fixtures. A
                // schema-synthesized model's temporal reader already
                // parses its `@<col>_raw` storage ivar (see
                // `schema::synth_attr_reader`); overwriting it here
                // would re-point the read at a nonexistent `@<col>`.
                AccessorKind::AttributeReader
                    if temporal.contains(&m.name)
                        && is_plain_ivar_read(&m.body, &m.name) =>
                {
                    let column = table.columns.iter().find(|c| c.name == m.name).unwrap();
                    let (_, parser, _) = crate::lower::model_to_library::schema::temporal_seam(column);
                    m.body = temporal_reader_body(&m.name, parser);
                }
                // Hand-written temporal writers only, same reasoning:
                // synthesized models write storage via `<col>_raw=`
                // (never named after a temporal column), so any
                // `<col>=` writer matching the temporal set is the
                // transpiled-fixture shape that needs the Time→text
                // normalize.
                AccessorKind::AttributeWriter => {
                    let col = Symbol::from(m.name.as_str().trim_end_matches('='));
                    if temporal.contains(&col) {
                        if let Some(param) = m.params.first() {
                            let column = table.columns.iter().find(|c| c.name == col).unwrap();
                            let (ty, _, formatter) = crate::lower::model_to_library::schema::temporal_seam(column);
                            // Date uses the shared date-only formatter.
                            // Keep the pre-existing legacy timestamp
                            // iso8601 policy, not a Time storage refactor.
                            m.body = temporal_writer_body(&col, &param.name, (ty == Ty::Date).then_some(formatter));
                        }
                    }
                    // The synthesized `<col>_raw=` writer's own store
                    // is invalidated by the general walk below, along
                    // with every OTHER bare write to the same slot.
                }
                // Schema-synthesized temporal reader:
                // `ActiveSupport.parse_db_time(@<col>_raw)`. Profiling
                // the lobsters bench put Date._parse + its regexps at
                // ~4% of wall time — every `created_at` read re-parses
                // the same string. Memoize per instance:
                // `@__t_<col> ||= ActiveSupport.parse_db_time(@<col>_raw)`
                // (writer above invalidates). nil/"" raw re-evaluates
                // each read — parse_db_time's empty path is cheap.
                AccessorKind::Method | AccessorKind::AttributeReader
                    if temporal.contains(&m.name) && is_parse_db_time_body(&m.body) =>
                {
                    let memo = Expr::new(
                        Span::synthetic(),
                        ExprNode::OpAssign {
                            target: LValue::Ivar { name: parse_memo_ivar(&m.name) },
                            op: crate::expr::OpAssignOp::OrOr,
                            value: m.body.clone(),
                        },
                    );
                    // Not the memo as read: Rails converts an attribute into `Time.zone` on every read, and `use_zone` may have moved it.
                    m.body = Expr::new(
                        Span::synthetic(),
                        ExprNode::Send {
                            recv: Some(Expr::new(
                                Span::synthetic(),
                                ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
                            )),
                            method: Symbol::from("present_db"),
                            args: vec![memo],
                            block: None,
                            parenthesized: true,
                        },
                    );
                }
                _ => {}
            }
        }

        // EVERY bare `@<col>_raw = …` invalidates the memo, not just
        // the one inside the `<col>_raw=` writer.
        //
        // `_adapter_reload` is why this is a walk rather than a
        // special case on that writer: it re-reads the row and stores
        // each column with a plain ivar assign (it writes into `self`,
        // where `from_stmt` builds a fresh instance and can go through
        // the writers). The memo survived, so `record.reload.
        // updated_at` answered the value from BEFORE the reload —
        // invisible until something holds one instance across a write,
        // which is exactly what Rails' memoized fixture accessors make
        // routine.
        //
        // Statement positions only (a method body, a `Seq` element, an
        // `If` branch): the replacement is a two-statement `Seq`, and
        // a `Seq` spliced into an expression slot is not Ruby.
        for m in &mut lc.methods {
            if m.receiver != MethodReceiver::Instance {
                continue;
            }
            invalidate_parse_memo_on_raw_writes(&mut m.body, &temporal);
        }

        // The public `<col>=` writer (`self.<col>_raw =
        // ActiveSupport.format_db_time(value)`) is synthesized by the
        // shared model lowering (`schema::synth_temporal_writer`, kind
        // `Method` so the AttributeWriter arm above can't re-point it
        // at a nonexistent `@<col>`) — it arrives here already present,
        // and its raw-writer dispatch picks up the memo invalidation
        // installed above.
    }
}

/// Rewrite every STATEMENT-position `@<col>_raw = …` for a temporal
/// column into `@__t_<col> = nil; @<col>_raw = …`, so the parse memo
/// can never outlive the string it was parsed from.
fn invalidate_parse_memo_on_raw_writes(e: &mut Expr, temporal: &BTreeSet<Symbol>) {
    match &mut *e.node {
        ExprNode::Seq { exprs } => {
            for x in exprs.iter_mut() {
                invalidate_parse_memo_on_raw_writes(x, temporal);
            }
            return;
        }
        ExprNode::If { then_branch, else_branch, .. } => {
            invalidate_parse_memo_on_raw_writes(then_branch, temporal);
            invalidate_parse_memo_on_raw_writes(else_branch, temporal);
            return;
        }
        _ => {}
    }
    let ExprNode::Assign { target: LValue::Ivar { name }, .. } = &*e.node else {
        return;
    };
    let Some(base) = name.as_str().strip_suffix("_raw") else {
        return;
    };
    let base = Symbol::from(base);
    if !temporal.contains(&base) {
        return;
    }
    let span = e.span;
    let store = e.clone();
    *e = Expr::new(
        span,
        ExprNode::Seq {
            exprs: vec![
                Expr::new(
                    span,
                    ExprNode::Assign {
                        target: LValue::Ivar { name: parse_memo_ivar(&base) },
                        value: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
                    },
                ),
                store,
            ],
        },
    );
}

/// A rich-text record's `body` reader (`lower::rich_text`: `def body;
/// ActionText::Content.new(@body); end`) answers ONE Content per record,
/// as Rails' serialized attribute does: `@__content_body ||=
/// ActionText::Content.new(@body)`, reset before every statement-position
/// `@body = …` in the class (the writer and the adapter's reload alike).
///
/// A new Content per read threw away what the last one had worked out:
/// campfire reads each message's body four times a render, and its
/// plain-text conversion walked the markup every time (27% of a room
/// page). Ruby emit only: the memo is a second ivar, and the strict
/// targets declare fields from the schema.
/// Ruby-family pass: Action Text's `belongs_to :record, polymorphic:
/// true, touch: true`.
///
/// Rails' `has_rich_text` declares its `has_one` with `inverse_of:
/// :record`, so a rich text read or built through its record knows that
/// record object, and saving it (`message.body.update!(body: …)`)
/// touches it: `updated_at` moves, `after_touch` cascades (a message's
/// own `belongs_to :room, touch: true`), and the record's update commit
/// callbacks run — campfire's `Message::Searchable` rewrites the search
/// index from `after_update_commit`, which is how a body edited through
/// the rich text reaches the index.
///
/// Two edits:
///
///   1. every owner-side write of the rich-text slot (`rich_text_<attr>`,
///      `build_rich_text_<attr>`, `_preload_rich_text_<attr>`) hands the
///      record to it — `@__rich_text_body._adopt_touch_owner(self)` — the
///      inverse;
///   2. `ActionText::RichText`'s `after_save` and `after_destroy` end in
///      `_touch_owner` (connection.rb), which touches that record when
///      it is persisted.
///
/// A rich text loaded on its own (not through its record) has no owner
/// object and touches nothing: the polymorphic `record` lookup Rails
/// would make is not modeled. Ruby-family only — the owner slot is a
/// `Base` reference, which the strict targets' plain structs have no
/// supertype for.
pub(crate) fn apply_rich_text_record_touch(lcs: &mut [LibraryClass], app: &App) {
    let record = crate::lower::rich_text::record_class();
    let span = Span::synthetic();
    let syn = |n| Expr::new(span, n);
    let touch_call = || {
        syn(ExprNode::Send {
            recv: None,
            method: Symbol::from("_touch_owner"),
            args: vec![],
            block: None,
            parenthesized: false,
        })
    };
    for lc in lcs.iter_mut() {
        if lc.name == record {
            let Some(model) = app.models.iter().find(|m| m.name == record) else { continue };
            crate::lower::model_to_library::markers::fold_into_or_push(
                &mut lc.methods,
                model,
                "after_save",
                touch_call(),
            );
            crate::lower::model_to_library::markers::fold_into_or_push(
                &mut lc.methods,
                model,
                "after_destroy",
                touch_call(),
            );
            continue;
        }
        let Some(model) = app.models.iter().find(|m| m.name == lc.name) else { continue };
        for (_, attr) in crate::lower::rich_text::rich_text_attrs(model) {
            let slot = Symbol::from(format!("__rich_text_{}", attr.as_str()));
            let writers = [
                format!("rich_text_{}", attr.as_str()),
                format!("build_rich_text_{}", attr.as_str()),
                format!("_preload_rich_text_{}", attr.as_str()),
            ];
            for m in lc.methods.iter_mut() {
                if m.receiver != MethodReceiver::Instance || !writers.iter().any(|w| w == m.name.as_str()) {
                    continue;
                }
                adopt_after_slot_writes(&mut m.body, &slot);
            }
        }
    }
}

/// After each `@<slot> = …` statement in a sequence, insert
/// `@<slot>._adopt_touch_owner(self) if !@<slot>.nil?`.
fn adopt_after_slot_writes(e: &mut Expr, slot: &Symbol) {
    e.node.for_each_child_mut(&mut |c| adopt_after_slot_writes(c, slot));
    let ExprNode::Seq { exprs } = &mut *e.node else { return };
    let mut i = 0;
    while i < exprs.len() {
        let writes = matches!(&*exprs[i].node, ExprNode::Assign { target: LValue::Ivar { name }, .. } if name == slot);
        if writes {
            let span = exprs[i].span;
            let syn = |n| Expr::new(span, n);
            let ivar = || syn(ExprNode::Ivar { name: slot.clone() });
            let adopt = syn(ExprNode::If {
                cond: syn(ExprNode::Send {
                    recv: Some(syn(ExprNode::Send {
                        recv: Some(ivar()),
                        method: Symbol::from("nil?"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    })),
                    method: Symbol::from("!"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                }),
                then_branch: syn(ExprNode::Send {
                    recv: Some(ivar()),
                    method: Symbol::from("_adopt_touch_owner"),
                    args: vec![syn(ExprNode::SelfRef)],
                    block: None,
                    parenthesized: true,
                }),
                else_branch: syn(ExprNode::Lit { value: Literal::Nil }),
            });
            exprs.insert(i + 1, adopt);
            i += 2;
        } else {
            i += 1;
        }
    }
}

pub(crate) fn apply_rich_text_content_memo(lcs: &mut [LibraryClass]) {
    for lc in lcs.iter_mut() {
        let memoized: BTreeSet<Symbol> = lc
            .methods
            .iter()
            .filter(|m| m.receiver == MethodReceiver::Instance && m.params.is_empty())
            .filter_map(|m| content_new_of_ivar(&m.body).filter(|iv| *iv == m.name))
            .collect();
        if memoized.is_empty() {
            continue;
        }
        for m in &mut lc.methods {
            if m.receiver != MethodReceiver::Instance {
                continue;
            }
            if m.params.is_empty() && memoized.contains(&m.name) && content_new_of_ivar(&m.body).is_some() {
                m.body = Expr::new(
                    m.body.span,
                    ExprNode::OpAssign {
                        target: LValue::Ivar { name: content_memo_ivar(&m.name) },
                        op: crate::expr::OpAssignOp::OrOr,
                        value: m.body.clone(),
                    },
                );
                continue;
            }
            reset_content_memo_on_writes(&mut m.body, &memoized);
        }
    }
}

/// `@x` when `e` is exactly `ActionText::Content.new(@x)`.
fn content_new_of_ivar(e: &Expr) -> Option<Symbol> {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*e.node else {
        return None;
    };
    let ExprNode::Const { path } = &*recv.node else {
        return None;
    };
    if method.as_str() != "new"
        || path.len() != 2
        || path[0].as_str() != "ActionText"
        || path[1].as_str() != "Content"
    {
        return None;
    }
    match args.as_slice() {
        [arg] => match &*arg.node {
            ExprNode::Ivar { name } => Some(name.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn content_memo_ivar(col: &Symbol) -> Symbol {
    Symbol::from(format!("__content_{}", col.as_str()))
}

/// Statement-position `@x = …` for a memoized `x` becomes
/// `@__content_x = nil; @x = …` (the same walk the temporal memo takes).
fn reset_content_memo_on_writes(e: &mut Expr, memoized: &BTreeSet<Symbol>) {
    match &mut *e.node {
        ExprNode::Seq { exprs } => {
            for x in exprs.iter_mut() {
                reset_content_memo_on_writes(x, memoized);
            }
            return;
        }
        ExprNode::If { then_branch, else_branch, .. } => {
            reset_content_memo_on_writes(then_branch, memoized);
            reset_content_memo_on_writes(else_branch, memoized);
            return;
        }
        // `[]=`'s `case name when :body then @body = value&.to_s`.
        ExprNode::Case { arms, .. } => {
            for arm in arms.iter_mut() {
                reset_content_memo_on_writes(&mut arm.body, memoized);
            }
            return;
        }
        _ => {}
    }
    let ExprNode::Assign { target: LValue::Ivar { name }, .. } = &*e.node else {
        return;
    };
    if !memoized.contains(name) {
        return;
    }
    let span = e.span;
    let memo = content_memo_ivar(name);
    let store = e.clone();
    *e = Expr::new(
        span,
        ExprNode::Seq {
            exprs: vec![
                Expr::new(
                    span,
                    ExprNode::Assign {
                        target: LValue::Ivar { name: memo },
                        value: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
                    },
                ),
                store,
            ],
        },
    );
}

/// `@__t_<col>` — the per-instance memo slot for a parsed temporal
/// column. Underscore-prefixed so it can't collide with a real column.
fn parse_memo_ivar(col: &Symbol) -> Symbol {
    Symbol::from(format!("__t_{}", col.as_str()))
}

/// Is this body exactly `ActiveSupport.parse_db_time(<anything>)` —
/// the synthesized temporal-reader shape?
fn is_parse_db_time_body(body: &Expr) -> bool {
    matches!(&*body.node, ExprNode::Send { recv: Some(r), method, args, .. }
        if method.as_str() == "parse_db_time"
            && args.len() == 1
            && matches!(&*r.node, ExprNode::Const { path }
                if path.len() == 1 && path[0].as_str() == "ActiveSupport"))
}

/// Ruby-family pre-emit pass: SQL NULL survives hydration as real nil.
///
/// The shared `<Model>Row.from_raw` synthesis coerces every scalar slot
/// (`(row["col"] || 0).to_i`, `(row["col"]).to_s`) so strict targets get
/// non-nilable fields — but on the Ruby tree that turns NULL into 0/""
/// and breaks Rails semantics: `group_by(&:invited_by_user_id)[nil]`
/// finds no root users (the /u tree renders empty), `banned_at?` is
/// true for everyone. For NULLABLE, non-primary-key columns, rewrite
/// the slot assign to `row["col"].nil? ? nil : <original coercion>`.
///
/// The fk 0-sentinel convention stays (belongs_to writers store 0 for
/// nil); readers' `@fk == 0` guards are WIDENED to `@fk.nil? || @fk ==
/// 0` so both representations mean "no parent". CRuby-only by
/// placement: strict targets keep the defaulted non-nilable slots
/// until the nullable-column typing workstream lands.
pub(crate) fn apply_hydration_nil_lowering(lcs: &mut [LibraryClass], app: &App) {
    for model in &app.models {
        let Some(table) = app.schema.tables.get(&model.table.0) else {
            continue;
        };
        let nullable: BTreeSet<Symbol> = table
            .columns
            .iter()
            .filter(|c| c.nullable && !c.primary_key && c.name.as_str() != "id")
            .map(|c| c.name.clone())
            .collect();
        if nullable.is_empty() {
            continue;
        }
        let row_id = ClassId(Symbol::from(format!("{}Row", model.name.0.as_str())));
        if let Some(row_lc) = lcs.iter_mut().find(|lc| lc.name == row_id) {
            for m in &mut row_lc.methods {
                if m.name.as_str() == "from_raw" {
                    nil_guard_from_raw_slots(&mut m.body, &nullable);
                }
            }
        }
        let nullable_fks: BTreeSet<Symbol> = nullable
            .iter()
            .filter(|n| n.as_str().ends_with("_id"))
            .cloned()
            .collect();
        if nullable_fks.is_empty() {
            continue;
        }
        if let Some(lc) = lcs.iter_mut().find(|lc| lc.name == model.name) {
            for m in &mut lc.methods {
                widen_fk_zero_guards(&mut m.body, &nullable_fks);
                // Polymorphic readers dispatch on the type discriminator,
                // so they have no zero-sentinel guard to widen. Guard their
                // nullable FK here too, before any key-typed adapter call.
                if m.name_span.is_synthetic() {
                    for assoc in model.associations() {
                        if let crate::dialect::Association::BelongsTo {
                            name, foreign_key, polymorphic: true, ..
                        } = assoc {
                            if m.name == *name && nullable.contains(foreign_key) {
                                let cond = Expr::new(Span::synthetic(), ExprNode::Send {
                                    recv: Some(Expr::new(Span::synthetic(), ExprNode::Ivar { name: foreign_key.clone() })),
                                    method: Symbol::from("nil?"), args: vec![], block: None,
                                    parenthesized: false,
                                });
                                m.body = Expr::new(m.body.span, ExprNode::If {
                                    cond,
                                    then_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
                                    else_branch: m.body.clone(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Inside a `from_raw` body, rewrite `instance.<col> = <coercion>` for
/// nullable cols to `instance.<col> = <coercion-sans-|| default>`: the
/// `|| 0` an id-shaped column carries is stripped so NULL reaches the
/// coercion, and the coercion itself keeps it alive -- a Cast to a
/// nilable target off a pure read emits `row["col"]&.to_i` (see
/// `emit_cast`). The `if row["col"].nil? then nil else … end` this once
/// wrapped around that was a second guard on the same read, and the
/// `if` was the shape spinel could not type (spinel#4567). A BOOLEAN
/// column keeps the `if`: its cast is `!["0", "", "false"].include?(…)`,
/// not a method on the value, so `&.` has nothing to attach to and NULL
/// would read as false.
fn nil_guard_from_raw_slots(body: &mut Expr, nullable: &BTreeSet<Symbol>) {
    let ExprNode::Seq { exprs } = &mut *body.node else { return };
    for stmt in exprs.iter_mut() {
        let ExprNode::Send { method, args, .. } = &mut *stmt.node else { continue };
        let Some(col) = method.as_str().strip_suffix('=') else { continue };
        if !nullable.contains(&Symbol::from(col)) {
            continue;
        }
        let Some(value) = args.first_mut() else { continue };
        // Only Cast-wrapped scalars coerce; raw slots (bools) already
        // carry nil through.
        let ExprNode::Cast { value: inner, target_ty } = &*value.node else { continue };
        // Strip a `|| <default>` fallback (the id-shaped-column form) so
        // NULL isn't defaulted before the coercion sees it.
        let lookup = match &*inner.node {
            ExprNode::BoolOp { left, right, .. }
                if matches!(&*right.node, ExprNode::Lit { .. }) =>
            {
                left.clone()
            }
            _ => inner.clone(),
        };
        let cast = Expr::new(
            Span::synthetic(),
            ExprNode::Cast { value: lookup.clone(), target_ty: target_ty.clone() },
        );
        let is_bool = matches!(target_ty.peel_nilable(), crate::ty::Ty::Bool);
        *value = if is_bool {
            let nil_check = Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(lookup),
                    method: Symbol::from("nil?"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            );
            Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: nil_check,
                    then_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
                    else_branch: cast,
                },
            )
        } else {
            cast
        };
    }
}

/// `@<fk> == 0` → `@<fk>.nil? || @<fk> == 0` for nullable fks, walking
/// the whole method body (belongs_to reader guards, app-code sentinel
/// checks alike).
fn widen_fk_zero_guards(expr: &mut Expr, fks: &BTreeSet<Symbol>) {
    expr.node.for_each_child_mut(&mut |c| widen_fk_zero_guards(c, fks));
    let is_sentinel_eq = match &*expr.node {
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "==" && args.len() == 1 =>
        {
            matches!(&*r.node, ExprNode::Ivar { name } if fks.contains(name))
                && (matches!(&*args[0].node, ExprNode::Lit { value: Literal::Int { value: 0 } })
                    || matches!(&*args[0].node, ExprNode::Lit { value: Literal::Str { value } } if value.is_empty()))
        }
        _ => false,
    };
    if !is_sentinel_eq {
        return;
    }
    let ExprNode::Send { recv: Some(r), .. } = &*expr.node else { return };
    let nil_check = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(r.clone()),
            method: Symbol::from("nil?"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let original = expr.clone();
    *expr = Expr::new(
        expr.span,
        ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::Or,
            surface: crate::expr::BoolOpSurface::Symbol,
            left: nil_check,
            right: original,
        },
    );
}

/// `<time>.rfc2822` → `ActiveSupport.rfc2822(<time>)`.
///
/// `Time#rfc2822` is stdlib `time`, not core Ruby — `require "time"`
/// installs it. CRuby has it because the overlay requires that stdlib;
/// spinel has no `time` package, and the method cannot be supplied by
/// reopening `Time` (a reopened built-in loses its own method table for
/// self-calls, so even `self.strftime` inside the reopen is undefined).
/// So the call grounds to the module function every ruby-family tree
/// ships — the same posture `lower::duration` takes for `Integer#days`,
/// and for the same reason: no built-in reopening.
///
/// Both trees satisfy the one `active_support_time_parsing.rbs`
/// contract. The CRuby/JRuby overlay's implementation delegates
/// straight back to `t.rfc2822`, so grounding the call site cannot move
/// that lane's bytes; the spinel twin composes the same string from
/// strftime.
///
/// Zero-arg receiver sends only. Ruby-emit-path pass: the module
/// function it names is ruby-family runtime, so this must not run from
/// shared `lower/`.
pub(crate) fn apply_time_format_lowering(lcs: &mut [LibraryClass], app: &App) {
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            rewrite_rfc2822(&mut m.body);
            rewrite_time_parse(&mut m.body);
        }
    }
    apply_zoned_json_time_lowering(lcs, app);
}

/// Every `JsonBuilder.encode_datetime` in `lcs` grounds to the app's
/// zone (`rewrite_zoned_encode_datetime`) when that zone is not UTC; a
/// no-op otherwise. Runs with the other time-format rewrites (models'
/// `as_json` writers, controllers, html views) and on the lowered
/// jbuilder views, which take no other ruby-family pass.
pub(crate) fn apply_zoned_json_time_lowering(lcs: &mut [LibraryClass], app: &App) {
    if !app_presents_time_zoned(app) {
        return;
    }
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            rewrite_zoned_encode_datetime(&mut m.body);
        }
    }
}

/// Whether the app's `config.time_zone` makes Rails write a JSON
/// timestamp with an offset rather than `Z`.
///
/// `TimeWithZone#xmlschema` writes `Z` only when the zone itself is UTC
/// (its abbreviation is "UTC" or "UCT"): Rails' default, and the names
/// below. Every other zone writes the offset it has AT THAT INSTANT —
/// `Europe/Paris` is `+02:00` in October and `+01:00` in January, and
/// `London` is `+00:00`, not `Z` — so the offset cannot be fixed here.
/// Ingest lifts the zone into a `config_time_zone` method on the
/// `Rails::Application` reopen; an app with none presents UTC.
fn app_presents_time_zoned(app: &App) -> bool {
    const UTC_ZONES: [&str; 8] =
        ["UTC", "Etc/UTC", "Universal", "Etc/Universal", "Zulu", "Etc/Zulu", "UCT", "Etc/UCT"];
    let Some(application) = &app.rails_application else { return false };
    let Some(method) = application.methods.iter().find(|m| m.name.as_str() == "config_time_zone") else {
        return false;
    };
    let lit = match &*method.body.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
        _ => &method.body,
    };
    match &*lit.node {
        ExprNode::Lit { value: Literal::Str { value } } => !UTC_ZONES.contains(&value.as_str()),
        _ => false,
    }
}

/// `JsonBuilder.encode_datetime(raw)` →
/// `JsonBuilder.encode_value(ActiveSupport.json_time(raw))`, for an app
/// whose `config.time_zone` is not UTC (`app_presents_time_zoned`).
///
/// The jbuilder lowerer and the default `as_json` writer serialize a
/// datetime column through `encode_datetime`, a string reformat of the
/// stored UTC text that always ends in `Z`. Rails presents the column
/// as a `TimeWithZone` in the app's zone, so its JSON carries that
/// zone's offset. `ActiveSupport.json_time` is the runtime seam that
/// already renders it (`xmlschema(3)` of the hydrated value, which
/// `parse_db_time` lands in the app's zone; DST-correct, because the
/// offset is resolved per instant), and the one a model's own `as_json`
/// writer uses (`PairEncoding::ZonedTime`).
///
/// Ruby-emit-path pass, like `rewrite_rfc2822`: `json_time` ships in
/// the ruby-family trees (the CRuby/JRuby overlay and the spinel
/// subset), not on the strict targets, whose runtimes present UTC, so
/// the shared lowerer keeps emitting `encode_datetime`.
fn rewrite_zoned_encode_datetime(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut |c| rewrite_zoned_encode_datetime(c));
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &mut *expr.node else {
        return;
    };
    if method.as_str() != "encode_datetime" || args.len() != 1 {
        return;
    }
    if !matches!(&*r.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "JsonBuilder") {
        return;
    }
    let raw = args[0].clone();
    let mut zoned = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
            )),
            method: Symbol::from("json_time"),
            args: vec![raw],
            block: None,
            parenthesized: true,
        },
    );
    zoned.ty = Some(Ty::Union { variants: vec![Ty::Str, Ty::Nil].into() });
    *method = Symbol::from("encode_value");
    *args = vec![zoned];
}

// Not left as `Time.parse`: spinel ships no stdlib `time` (matz/spinel#1118), so it grounds like `rfc2822` above.
fn rewrite_time_parse(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut |c| rewrite_time_parse(c));
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &mut *expr.node else {
        return;
    };
    if method.as_str() != "parse" || args.len() != 1 {
        return;
    }
    let is_time = |e: &Expr| {
        matches!(&*e.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Time")
    };
    let target = if is_time(r) {
        "parse_time"
    } else if matches!(&*r.node,
        ExprNode::Send { recv: Some(z), method, args, block: None, .. }
            if method.as_str() == "zone" && args.is_empty() && is_time(z))
    {
        "zone_parse"
    } else {
        return;
    };
    let arg = args[0].clone();
    *expr.node = ExprNode::Send {
        recv: Some(Expr::new(
            Span::synthetic(),
            ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
        )),
        method: Symbol::from(target),
        args: vec![arg],
        block: None,
        parenthesized: true,
    };
}

fn rewrite_rfc2822(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut |c| rewrite_rfc2822(c));
    let ExprNode::Send { recv: Some(r), method, args, .. } = &mut *expr.node else {
        return;
    };
    if method.as_str() != "rfc2822" || !args.is_empty() {
        return;
    }
    let recv = r.clone();
    *expr.node = ExprNode::Send {
        recv: Some(Expr::new(
            Span::synthetic(),
            ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
        )),
        method: Symbol::from("rfc2822"),
        args: vec![recv],
        block: None,
        parenthesized: true,
    };
}

/// `::`-root a view body's constant references when the view's own
/// namespace shadows the constant.
///
/// A view emits as `module Views\n  module Stats`, so an unqualified
/// `Stats.get_cached_graph(:users)` inside it resolves LEXICALLY to
/// `Views::Stats` — the view module itself — and not to the top-level
/// `Stats` model the template meant. Ruby finds the inner constant
/// first and raises `NoMethodError: undefined method
/// 'get_cached_graph' for module Views::Stats`.
///
/// This is not a spinel-only concern: the CRuby target emits the same
/// unqualified reference and fails the same way at runtime (GET /stats
/// on lobsters). The AOT lane just surfaces it at compile time.
///
/// The rewrite is deliberately narrow — a head segment is rooted only
/// when it BOTH collides with a namespace visible in the view's lexical
/// scope AND names a real top-level app namespace. Emitted views always
/// spell sibling views fully from the root (`Views::Stories.similar(...)`),
/// so nothing here depends on lexical shorthand and the rooting cannot
/// retarget a reference that was already resolving correctly.
///
/// "Visible in the lexical scope" is the part that was wrong at first:
/// it read the view's OWN segments only. But every view is emitted
/// inside `module Views`, so the scope of ANY view body includes EVERY
/// OTHER view's namespace. campfire's `messages/_message` calls
/// `Users::AvatarsHelper.avatar_tag(...)`; `Views::Users` exists because
/// the app has `app/views/users/` templates, and Ruby stops there —
/// `uninitialized constant Views::Users::AvatarsHelper`, on the page
/// that renders every message.
/// Which shadows this pipeline roots. The view pipeline takes both —
/// every view sits under `module Views` beside every other view. The
/// LIBRARY pipeline takes only the runtime's, because an app class
/// referencing its OWN nested constant (`Sound::Image` inside `class
/// Sound`) already resolves, and rooting it would be churn for nothing.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum RootingScope {
    AppAndRuntime,
    RuntimeOnly,
}

/// Root method-body constant references that emission would otherwise shadow,
/// restricting candidate namespaces according to `scope`. A Data factory block
/// retains its enclosing owner's lexical scope, not the generated class's scope.
pub(crate) fn apply_constant_rooting(
    lcs: &mut [LibraryClass],
    app: &App,
    scope: RootingScope,
) {
    // …and "names a real top-level app namespace" has to mean the
    // NAMESPACE, not a class of exactly that name: campfire has no class
    // called `Users`, only `Users::AvatarsHelper` under it.
    //
    // A name the shared RUNTIME owns counts too, and for the same
    // reason a view's does: campfire's `Message::Broadcasts` concern is
    // where the broadcast lowering emits `Broadcasts.append(...)`, and
    // inside `module Broadcasts` that constant resolves to the concern
    // ITSELF. One authority for "is this the runtime's" —
    // `require_path_for_body_const`, which already had to know.
    let runtime_owned = |name: &str| {
        require_path_for_body_const(&[name.to_string()], app, &app.library_classes, "")
            .is_some_and(|p| p.starts_with("runtime/"))
    };
    let shadows = |name: &str| {
        if scope == RootingScope::RuntimeOnly {
            return runtime_owned(name);
        }
        let prefix = format!("{name}::");
        let under = |n: &str| n == name || n.starts_with(&prefix);
        app.models.iter().any(|m| under(m.name.0.as_str()))
            || app.library_classes.iter().any(|c| under(c.name.0.as_str()))
            || runtime_owned(name)
    };
    // Every `Views::<Seg>` in this emit, which is exactly the set of
    // names a view body can accidentally resolve to.
    let mut view_namespaces: Vec<String> = lcs
        .iter()
        .filter_map(|lc| lc.name.0.as_str().strip_prefix("Views::"))
        .filter_map(|rest| rest.split("::").next())
        // `Views` itself is never the shadow: a `Views::X` reference
        // inside `module Views` finds no `Views::Views` and lands on the
        // top-level one, which is the intended target.
        .filter(|seg| *seg != "Views" && shadows(seg))
        .map(|seg| seg.to_string())
        .collect();
    view_namespaces.sort();
    view_namespaces.dedup();

    // Every class name this emit defines, app and runtime alike — the
    // authority for "does the inner reading have a target?".
    let class_names: crate::det::HashSet<String> = app
        .models
        .iter()
        .map(|m| m.name.0.as_str().to_string())
        .chain(app.library_classes.iter().map(|c| c.name.0.as_str().to_string()))
        .chain(lcs.iter().map(|c| c.name.0.as_str().to_string()))
        .collect();
    let known = |n: &str| class_names.contains(n);
    for lc in lcs.iter_mut() {
        let name = if matches!(lc.origin, Some(LibraryClassOrigin::DataFactory { .. })) {
            lc.name.0.as_str().rsplit_once("::").unwrap().0
        } else {
            lc.name.0.as_str()
        };
        let mut shadowing: Vec<String> = name
            .split("::")
            .filter(|seg| shadows(seg))
            .map(|seg| seg.to_string())
            .collect();
        if name.starts_with("Views::") {
            shadowing.extend(view_namespaces.iter().cloned());
        }
        shadowing.sort();
        shadowing.dedup();
        // NESTING the emit introduced. A source file that writes the
        // COMPACT form (`module User::Bot`) has lexical scope
        // [User::Bot, Object] — `User`'s own constants are NOT in it.
        // The emit nests (`class User … module Bot`), which puts them
        // in, and campfire's `Bot::WebhookJob` stopped meaning the
        // top-level job and started meaning the concern itself.
        //
        // So: for every enclosing namespace, the constants it defines
        // are candidates for rooting — but only where the inner reading
        // has NO target for the FULL path. `User::Bot` exists, which is
        // the shadow; `User::Bot::WebhookJob` does not, which is what
        // proves the body meant the outer one.
        let prefixes: Vec<String> = {
            let segs: Vec<&str> = name.split("::").collect();
            (1..segs.len()).map(|i| segs[..i].join("::")).collect()
        };
        for m in &mut lc.methods {
            root_shadowed_constants(&mut m.body, name, &shadowing, &prefixes, &known);
        }
    }
}

fn root_shadowed_constants(
    expr: &mut Expr,
    own: &str,
    shadowing: &[String],
    prefixes: &[String],
    known: &dyn Fn(&str) -> bool,
) {
    expr.node
        .for_each_child_mut(&mut |c| root_shadowed_constants(c, own, shadowing, prefixes, known));
    let ExprNode::Const { path } = &mut *expr.node else {
        return;
    };
    let joined = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
    let Some(head) = path.first_mut() else { return };
    // Idempotent: a head already carrying the root marker is skipped, so
    // running the pass twice can't produce `::::Stats`.
    if head.as_str().starts_with("::") {
        return;
    }
    // The class's own constant is the innermost lexical scope, in the
    // source as in the emit, and binds the head before any enclosing
    // namespace's: `Catalog::Lifecycle::Entry` declares
    // `module Type` (`private_constant`) and reads `Type::NOTIFY_*` in a
    // `when`, while `Catalog::Type` is known and has no `NOTIFY_*`.
    // Rooted, that was `::Type`: uninitialized constant Type. A resolved
    // class reference is no exception: its absolute spelling would name
    // a private constant.
    //
    // A view has no such scope in the source: an ERB template is not
    // inside any module, so its `Views::…` nesting is all the emit's,
    // and its own module is one more enclosing namespace (below).
    // Inside `Views::Accounts`, campfire's `Users::ProfilesHelper` would
    // otherwise bind to the `Views::Accounts::Users` view module.
    let view = own.starts_with("Views::");
    if !view && known(&format!("{own}::{}", head.as_str())) {
        return;
    }
    // Emission nests compact declarations such as `class UI::Explicit`
    // under `module UI`. Rubydex already resolved the source reference.
    // Root its actual class when the new nesting would bind the head to
    // another class; a typed VALUE constant never carries this mark.
    if expr.decisions & crate::expr::RESOLVED_CLASS_REF != 0 {
        if let Some(crate::ty::Ty::Class { id, .. }) = &expr.ty {
            let resolved = id.0.as_str();
            for prefix in prefixes.iter().rev() {
                if known(&format!("{prefix}::{}", head.as_str())) {
                    if format!("{prefix}::{joined}") != resolved {
                        *path = resolved.split("::").map(Symbol::from).collect();
                        path[0] = Symbol::from(format!("::{}", path[0].as_str()));
                    }
                    return;
                }
            }
        }
    }
    // Either the name's OWN segments shadow it (the pre-existing rule:
    // `Views::Stats` referencing `Stats`, `Message::Broadcasts`
    // referencing the runtime's `Broadcasts`) …
    let by_own_segment = shadowing.iter().any(|s| s == head.as_str());
    // … or an ENCLOSING namespace defines it and the full path under
    // that namespace does not exist, which is the nesting the emit
    // introduced. See the prefix note at the call site.
    let by_enclosing = prefixes.iter().map(String::as_str).chain(view.then_some(own)).any(|p| {
        known(&format!("{p}::{}", head.as_str())) && !known(&format!("{p}::{joined}"))
    });
    if !by_own_segment && !by_enclosing {
        return;
    }
    // The path renders as `path.join("::")`, so a leading marker on the
    // head segment is all an absolute reference needs — there is no
    // separate "absolute" flag in the IR to set.
    *head = Symbol::from(format!("::{}", head.as_str()));
}

/// Collapse a controller's dynamic-render-options assignment to its bare
/// partial-name string: `@above = {partial: "stories/subnav", locals:
/// {…}}` → `@above = "stories/subnav"`. The matching view (`<%= render
/// @above %>`) dispatches over a `case @above when "stories/subnav" …`
/// pool of string names, so the runtime value must be the string. Upstream
/// lobsters uses both this hash form and the bare-string form (`@above =
/// "saved/subnav"`) — normalizing here lets the view stay string-uniform.
/// The `locals:` sub-hash is dropped HERE; the pool dispatch captures it
/// separately (see the DynPoolEntry / strict-locals kwargs path).
///
/// SCOPE: this is a Ruby-emit-path pass, but the `case @x when "…"` string
/// dispatch it feeds is emitted by the SHARED view lowerer. That's correct
/// only while lobsters (the sole dynamic-partial app) is ruby-only — a
/// strict target rendering `render @above` would see the un-collapsed hash
/// and never match a string arm. Revisit when another target grows dynamic
/// partials (move this collapse to the shared controller lowering then).
pub(crate) fn apply_dynamic_render_options_lowering(lcs: &mut [LibraryClass]) {
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            rewrite_render_options_assign(&mut m.body);
        }
    }
}

fn rewrite_render_options_assign(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut |c| rewrite_render_options_assign(c));
    let ExprNode::Assign { target: crate::expr::LValue::Ivar { name: ivar }, value } = &mut *expr.node
    else {
        return;
    };
    let ivar = ivar.clone();
    let ExprNode::Hash { entries, .. } = &*value.node else {
        return;
    };
    // Collapse ONLY a render-options hash: symbol keys drawn exactly from
    // {partial, locals}, with a String-literal `partial:`. Gating on the
    // full shape (not merely "contains a partial: key") avoids rewriting an
    // unrelated config hash like `@x = {partial: "y", scope: z}` app-wide —
    // `render @above` is the only consumer of this collapse.
    let mut partial_name: Option<String> = None;
    let mut computed: Vec<(Symbol, Expr)> = Vec::new();
    for (k, v) in entries.iter() {
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
            return; // non-symbol key → not a render-options hash
        };
        match key.as_str() {
            "partial" => match &*v.node {
                ExprNode::Lit { value: Literal::Str { value: s } } => {
                    partial_name = Some(s.clone())
                }
                _ => return, // dynamic partial name — leave the hash intact
            },
            "locals" => {
                if let ExprNode::Hash { entries: locals, .. } = &*v.node {
                    for (lk, lv) in locals {
                        if let ExprNode::Lit { value: Literal::Sym { value: local } } = &*lk.node
                            && !matches!(&*lv.node, ExprNode::Ivar { .. } | ExprNode::Lit { .. })
                        {
                            computed.push((local.clone(), lv.clone()));
                        }
                    }
                }
            }
            _ => return, // any foreign key → not a render-options hash
        }
    }
    if let Some(name) = partial_name {
        let span = value.span;
        *value = Expr::new(span, ExprNode::Lit { value: Literal::Str { value: name } });
        if computed.is_empty() {
            return;
        }
        // The view's dispatch arm reads each computed local back from this ivar (`dynamic_local_ivar`).
        let mut stmts: Vec<Expr> = computed
            .into_iter()
            .map(|(local, value)| {
                Expr::new(span, ExprNode::Assign {
                    target: crate::expr::LValue::Ivar {
                        name: crate::lower::view_to_library::dynamic_local_ivar(&ivar, &local),
                    },
                    value,
                })
            })
            .collect();
        stmts.push(expr.clone());
        *expr = Expr::new(expr.span, ExprNode::Seq { exprs: stmts });
    }
}

/// True when a reader body is exactly `@<name>` — the untouched
/// hand-written `attr_reader` shape (vs a synthesized parsing body).
fn is_plain_ivar_read(body: &Expr, name: &Symbol) -> bool {
    matches!(&*body.node, ExprNode::Ivar { name: n } if n == name)
}

fn datetime_ivar(col: &Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Ivar { name: col.clone() })
}

fn datetime_var(name: &Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: name.clone() })
}

/// `@col && ActiveSupport.parse_db_time(@col)`.
fn temporal_reader_body(col: &Symbol, parser: &str) -> Expr {
    // `ActiveSupport.parse_db_time` (not bare `Time.parse`) — a stored
    // column with no zone marker is always implicitly UTC (Rails/sqlite3
    // convention), but `Time.parse` defaults an absent zone to the
    // *system's local zone*. See `active_support_time_parsing.rb`.
    let parse_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
            )),
            method: Symbol::from(parser),
            args: vec![datetime_ivar(col)],
            block: None,
            parenthesized: true,
        },
    );
    Expr::new(
        Span::synthetic(),
        ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::And,
            surface: crate::expr::BoolOpSurface::Symbol,
            left: datetime_ivar(col),
            right: parse_call,
        },
    )
}

/// `@col = (value.respond_to?(:iso8601) ? value.iso8601 : value)`.
fn temporal_writer_body(col: &Symbol, value_param: &Symbol, date_formatter: Option<&str>) -> Expr {
    if let Some(formatter) = date_formatter {
        return Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Ivar { name: col.clone() },
                value: Expr::new(Span::synthetic(), ExprNode::Send {
                    recv: Some(Expr::new(Span::synthetic(), ExprNode::Const {
                        path: vec![Symbol::from("ActiveSupport")],
                    })),
                    method: Symbol::from(formatter),
                    args: vec![datetime_var(value_param)],
                    block: None,
                    parenthesized: true,
                }),
            },
        );
    }
    let responds = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(datetime_var(value_param)),
            method: Symbol::from("respond_to?"),
            args: vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit {
                    value: crate::expr::Literal::Sym { value: Symbol::from("iso8601") },
                },
            )],
            block: None,
            parenthesized: true,
        },
    );
    let iso_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(datetime_var(value_param)),
            method: Symbol::from("iso8601"),
            args: Vec::new(),
            block: None,
            parenthesized: true,
        },
    );
    let normalized = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: responds,
            then_branch: iso_call,
            else_branch: datetime_var(value_param),
        },
    );
    Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Ivar { name: col.clone() },
            value: normalized,
        },
    )
}

/// Emit both the `.rb` file and its `.rbs` sidecar for a single
/// LibraryClass. The sidecar carries the typed-signature view of the
/// same class shape — spinel reads it as an inference hint (see
/// project_rbs_emit_opportunity.md / spinel#571), and Steep/TypeProf
/// can consume it from the CRuby target.
pub(super) fn emit_library_class_pair(
    lc: &LibraryClass,
    app: &App,
    out_path: PathBuf,
) -> Vec<EmittedFile> {
    emit_library_class_pair_in(lc, app, &app.library_classes, &[], out_path)
}

fn emit_library_class_pair_in(
    lc: &LibraryClass,
    app: &App,
    classes: &[LibraryClass],
    factories: &[DataFactoryOwner],
    out_path: PathBuf,
) -> Vec<EmittedFile> {
    let rb = emit_library_class_decl_in(lc, app, classes, factories, out_path.clone(), &[], &[]);
    let rbs = super::rbs::emit_library_class_rbs_with_factories(lc, &out_path, classes, factories);
    vec![rb, rbs]
}

/// Pair variant for callers that pass synthesized sibling anchors.
pub(super) fn emit_library_class_pair_with_synthesized(
    lc: &LibraryClass,
    app: &App,
    out_path: PathBuf,
    synthesized_siblings: &[(String, String)],
) -> Vec<EmittedFile> {
    let rb = emit_library_class_decl_with_synthesized(
        lc,
        app,
        out_path.clone(),
        synthesized_siblings,
    );
    let rbs = super::rbs::emit_library_class_rbs_with_factories(lc, &out_path, &app.library_classes, &[]);
    vec![rb, rbs]
}

/// A lowered read owns its statement from prepare until finalize, including
/// argument serialization and row hydration. Release within that scope even
/// when the caller rescues inside a longer connection lease. This Ruby-only
/// pass leaves strict-target IR unchanged and covers reloads and preloads too.
fn read_statement_cleanup(lc: &LibraryClass) -> std::borrow::Cow<'_, LibraryClass> {
    fn db_send(expr: &Expr, name: &str) -> bool {
        matches!(&*expr.node, ExprNode::Send { recv: Some(recv), method, .. }
            if method.as_str() == name && matches!(&*recv.node,
                ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Db"))
    }
    fn prepare(expr: &Expr) -> bool {
        db_send(expr, "prepare") || db_send(expr, "prepare_uncached")
    }
    fn prepared_name(expr: &Expr) -> Option<&Symbol> {
        match &*expr.node {
            ExprNode::Assign { target: LValue::Var { name, .. }, value } if prepare(value) => Some(name),
            _ => None,
        }
    }
    fn finalize(expr: &Expr, name: &Symbol) -> bool {
        db_send(expr, "finalize") && matches!(&*expr.node,
            ExprNode::Send { args, .. } if args.len() == 1
                && matches!(&*args[0].node, ExprNode::Var { name: actual, .. } if actual == name))
    }
    fn needs_cleanup(expr: &Expr) -> bool {
        let mut found = prepared_name(expr).is_some();
        expr.node.for_each_child(&mut |child| found |= needs_cleanup(child));
        found
    }
    fn rewrite(expr: &mut Expr) {
        expr.node.for_each_child_mut(&mut rewrite);
        let ExprNode::Seq { exprs } = expr.node.as_mut() else { return };
        let mut start = 0;
        while start < exprs.len() {
            if let Some(name) = prepared_name(&exprs[start]) {
                if let Some(end) = (start + 1..exprs.len()).find(|&i| finalize(&exprs[i], name)) {
                    let mut work: Vec<_> = exprs.drain(start + 1..=end).collect();
                    let cleanup = work.pop().unwrap();
                    let body = Expr::new(Span::synthetic(), ExprNode::Seq { exprs: work });
                    exprs.insert(start + 1, Expr::new(Span::synthetic(), ExprNode::BeginRescue {
                        body,
                        rescues: vec![],
                        else_branch: None,
                        ensure: Some(cleanup),
                        implicit: false,
                    }));
                }
            }
            start += 1;
        }
    }
    if !lc.methods.iter().any(|m| needs_cleanup(&m.body)) {
        return std::borrow::Cow::Borrowed(lc);
    }
    let mut adapted = lc.clone();
    for method in &mut adapted.methods {
        rewrite(&mut method.body);
    }
    std::borrow::Cow::Owned(adapted)
}

/// Emit a group of LibraryFunctions sharing a `module_path` as a
/// single Ruby file. Mirrors `typescript::library::emit_module_file`
/// — converts the function group into a synthetic
/// `LibraryClass{is_module:true}` with class-method (`def self.X`)
/// declarations, then delegates to `emit_library_class_decl` so
/// require resolution, nested-module rendering, and method body
/// emission share one code path with class-shaped artifacts.
///
/// `module_function` would be the more idiomatic Ruby spelling,
/// but `def self.X` is what the existing spinel-blog hand-written
/// modules use AND what `emit_method` already produces — going
/// through that path keeps shapes byte-identical.
pub(super) fn emit_module_file(
    funcs: &[crate::dialect::LibraryFunction],
    app: &App,
    out_path: PathBuf,
) -> EmittedFile {
    if funcs.is_empty() {
        // No functions in the module — emit a placeholder file with
        // just the module wrapper. Callers can guard upstream by
        // checking the lowerer's output and not calling this when
        // they know the module would be empty.
        return EmittedFile { path: out_path, content: String::new() };
    }
    let lc = synthesize_module_lc(funcs);
    emit_library_class_decl(&lc, app, out_path)
}

/// Pair variant of `emit_module_file` — emits both `.rb` and `.rbs`.
pub(super) fn emit_module_file_pair(
    funcs: &[crate::dialect::LibraryFunction],
    app: &App,
    out_path: PathBuf,
) -> Vec<EmittedFile> {
    if funcs.is_empty() {
        return vec![EmittedFile { path: out_path, content: String::new() }];
    }
    let lc = synthesize_module_lc(funcs);
    let rb = emit_library_class_decl(&lc, app, out_path.clone());
    let rbs = super::rbs::emit_library_class_rbs(&lc, &out_path);
    vec![rb, rbs]
}

/// Emit only the `.rbs` sidecar for a `LibraryClass`. Used when the
/// `.rb` emit has bespoke post-processing the pair helpers can't
/// model (e.g. test files with autorun shim + preamble).
pub(super) fn emit_rbs_sidecar(lc: &LibraryClass, rb_path: &std::path::Path) -> EmittedFile {
    super::rbs::emit_library_class_rbs(lc, rb_path)
}

/// Emit only the `.rbs` sidecar derived from a `LibraryFunction` group.
/// Companion to `emit_rbs_sidecar` for module-shaped output whose `.rb`
/// emit flows through a bespoke path (e.g. `config/routes.rb`).
pub(super) fn emit_rbs_sidecar_from_funcs(
    funcs: &[crate::dialect::LibraryFunction],
    rb_path: &std::path::Path,
) -> EmittedFile {
    let lc = synthesize_module_lc(funcs);
    super::rbs::emit_library_class_rbs(&lc, rb_path)
}

fn synthesize_module_lc(
    funcs: &[crate::dialect::LibraryFunction],
) -> LibraryClass {
    use crate::dialect::{AccessorKind, MethodDef, MethodReceiver};
    use crate::ident::Symbol;

    let module_id = funcs
        .first()
        .map(|f| {
            ClassId(Symbol::from(
                f.module_path
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("::"),
            ))
        })
        .unwrap_or_else(|| ClassId(Symbol::from("")));
    let methods: Vec<MethodDef> = funcs
        .iter()
        .map(|f| MethodDef {
            unsupported_formals: f.unsupported_formals,
            has_anonymous_block: f.has_anonymous_block,
            name_span: crate::span::Span::synthetic(),
            name: f.name.clone(),
            receiver: MethodReceiver::Class,
            visibility: MethodVisibility::Public,
            params: f.params.clone(),
            body: f.body.clone(),
            signature: f.signature.clone(),
            effects: f.effects.clone(),
            enclosing_class: Some(module_id.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        })
        .collect();
    LibraryClass {
        name: module_id,
        is_module: true,
        parent: None,
        parent_span: Default::default(),
        includes: Vec::new(),
        include_spans: Vec::new(),
        methods,
        nullable_columns: Vec::new(),
        origin: None,
        constants: Vec::new(),
        conditional_definitions: Vec::new(),
        unknown_calls: Vec::new(),
        class_ivar_initializers: Vec::new(),
    }
}

#[test]
fn module_adapter_preserves_parameter_declaration_facts() {
    let source = crate::ingest::ingest_library_class(
        b"class Probe; def self.call((a,b)); 7; end; end",
        "probe.rb",
    ).unwrap().unwrap();
    let functions = crate::lower::view_to_library::flatten_lcs_to_functions(&[source]);
    let restored = synthesize_module_lc(&functions);
    assert_eq!(restored.methods[0].unsupported_formals,
        Some(crate::dialect::UnsupportedFormal::Destructured));
}

thread_local! {
    static RENDERED: std::cell::RefCell<Option<Vec<LibraryClass>>> = const { std::cell::RefCell::new(None) };
}

pub fn record_rendered<T>(f: impl FnOnce() -> T) -> (T, Vec<LibraryClass>) {
    let previous = RENDERED.with(|r| r.borrow_mut().replace(Vec::new()));
    let out = f();
    let rendered = RENDERED.with(|r| std::mem::replace(&mut *r.borrow_mut(), previous)).unwrap_or_default();
    (out, rendered)
}

/// Emit a single library-shape file. `out_path` is the project-root-relative
/// destination for the file; the require resolver computes paths relative to
/// `out_path`'s parent, so files emitted to `app/views/<plural>/` get
/// `../../../runtime/<x>` while files in `app/models/` get `../../runtime/<x>`.
pub(super) fn emit_library_class_decl(
    lc: &LibraryClass,
    app: &App,
    out_path: PathBuf,
) -> EmittedFile {
    emit_library_class_decl_with_synthesized(lc, app, out_path, &[])
}

fn emit_library_class_decl_in(
    lc: &LibraryClass,
    app: &App,
    classes: &[LibraryClass],
    factories: &[DataFactoryOwner],
    out_path: PathBuf,
    synthesized_siblings: &[(String, String)],
    nested: &[NestedBlock],
) -> EmittedFile {
    RENDERED.with(|rendered| {
        if let Some(rendered) = rendered.borrow_mut().as_mut() {
            rendered.push(lc.clone());
        }
    });
    let guarded = read_statement_cleanup(lc);
    let lc = guarded.as_ref();
    let path = out_path.clone();
    super::expr::with_emitted_file(&path, || {
        super::expr::with_core_class_reopen(is_core_class_name(lc.name.0.as_str()), || {
            emit_library_class_decl_inner(lc, app, classes, factories, out_path, synthesized_siblings, nested)
        })
    })
}

/// Variant that also accepts a list of (class_name, anchor) pairs for
/// synthesized siblings (e.g. `<Model>Row`, `<Resource>Params`) that
/// aren't in `app.library_classes` / `app.models`. Synthesized classes
/// have no separate require chain — nothing else loads them — so a
/// file that references one needs an explicit `require_relative`,
/// even when the target is in the same directory. Callers that don't
/// emit synthesized siblings pass an empty slice.
pub(super) fn emit_library_class_decl_with_synthesized(
    lc: &LibraryClass,
    app: &App,
    out_path: PathBuf,
    synthesized_siblings: &[(String, String)],
) -> EmittedFile {
    emit_library_class_decl_with_nested(lc, app, out_path, synthesized_siblings, &[])
}

fn emit_library_class_decl_with_nested(
    lc: &LibraryClass,
    app: &App,
    out_path: PathBuf,
    synthesized_siblings: &[(String, String)],
    nested: &[NestedBlock],
) -> EmittedFile {
    // The one chokepoint every library-shape file goes through, and the
    // only place that knows which class is being emitted — the send
    // emitter is a free function reached from a dozen callers. A reopen
    // of a core class needs its `self.` receivers kept (see the elision
    // in `expr.rs`); everything else is emitted exactly as before.
    emit_library_class_decl_in(lc, app, &app.library_classes, &[], out_path, synthesized_siblings, nested)
}

/// Render an indented method with named visibility directives that cannot leak
/// to neighboring definitions. Explicitly public constructor/copy hooks need a
/// directive too, because Ruby makes those instance methods private by default.
fn render_library_method(s: &mut String, m: &MethodDef, body_pad: &str) {
    for line in super::emit_method(m).lines() {
        if line.is_empty() {
            writeln!(s).unwrap();
        } else {
            writeln!(s, "{body_pad}{line}").unwrap();
        }
    }
    let implicit_private = m.receiver == MethodReceiver::Instance
        && matches!(m.name.as_str(), "initialize" | "initialize_copy" | "initialize_dup" | "initialize_clone");
    let directive = match (m.receiver, m.visibility) {
        (_, MethodVisibility::Public) if !implicit_private => None,
        (MethodReceiver::Instance, MethodVisibility::Public) => Some("public"),
        (MethodReceiver::Instance, MethodVisibility::Protected) => Some("protected"),
        (MethodReceiver::Instance, MethodVisibility::Private) => Some("private"),
        (MethodReceiver::Class, MethodVisibility::Private) => Some("private_class_method"),
        (MethodReceiver::Class, MethodVisibility::Public) => None,
        (MethodReceiver::Class, MethodVisibility::Protected) => {
            writeln!(s, "{body_pad}class << self").unwrap();
            writeln!(s, "{body_pad}  protected :{}", m.name).unwrap();
            writeln!(s, "{body_pad}end").unwrap();
            None
        }
    };
    if let Some(directive) = directive {
        writeln!(s, "{body_pad}{directive} :{}", m.name).unwrap();
    }
}

/// Assemble a library class's Ruby file with its requires and declarations.
/// Reattach lifted Data methods by declaration span, preserving the factory's
/// position and including dependencies from its methods and parameter defaults.
fn emit_library_class_decl_inner(
    lc: &LibraryClass,
    app: &App,
    classes: &[LibraryClass],
    factories: &[DataFactoryOwner],
    out_path: PathBuf,
    synthesized_siblings: &[(String, String)],
    nested: &[NestedBlock],
) -> EmittedFile {
    let name = lc.name.0.as_str();
    let out_dir = out_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(PathBuf::new);
    let self_anchor = out_path.with_extension("").to_string_lossy().into_owned();
    let mut s = String::new();

    let mut emitted_external_require = false;
    for included in &lc.includes {
        let detail = format!("{} includes unresolved {}", lc.name.0, included.0);
        if external_include_is_enabled(&detail) {
            for feature in super::expr::external_include_features(&detail) {
                writeln!(s, "require {feature:?}").unwrap();
                emitted_external_require = true;
            }
        }
    }
    if emitted_external_require {
        writeln!(s).unwrap();
    }

    // A constant whose initializer runs this class's own methods at load
    // time (see `partition_deferred_constants`) drags those method
    // bodies' constant refs into LOAD time with it — `Sound::BUILTIN`
    // calls `initialize`, which reads `Sound::Image`. Those refs would
    // otherwise be classified body-only and left to the aggregator.
    // Source class state can call methods and read constants between writes.
    // Keep those declarations in source order; synthesized framework classes
    // and mattr/cattr `@@` seeds retain their existing initialization policy.
    // Lowered controllers also have no origin, but Concern methods carry
    // spans from another file. Their offsets cannot order the controller's
    // macro calls: those initializers must run after all collected methods.
    // A `Data.define` block is source too: its `@ivar` writes and
    // constants run in the order that the block gives them.
    let replayed_in_place: Vec<&Expr> = lc
        .unknown_calls
        .iter()
        .filter(|call| {
            (call.decisions & crate::expr::REPLAYED_CLASS_BODY != 0
                || (call.decisions & crate::expr::BODY_STATEMENT != 0 && super::expr::carries_body_statements()))
                && !call.span.is_synthetic()
        })
        .collect();
    // Kept out of `replayed_in_place`: that list is what turns on
    // source-ordered bodies, and a constant write must not move includes
    // or a gem DSL. The call is span-sorted into the constant list below.
    let constant_calls: Vec<&Expr> = lc
        .unknown_calls
        .iter()
        .filter(|call| !call.span.is_synthetic() && constant_defining_class_call(app, classes, lc, call))
        .collect();
    let ordered_body = (lc.origin.is_none() || lc.data_define().is_some())
        && (lc.has_source_ivar_initializers() || !replayed_in_place.is_empty())
        && !app.controllers.iter().any(|controller| controller.name == lc.name);
    let (mut eager, mut deferred, initializers_call_self) = partition_deferred_constants(lc);
    let load_time_bodies = initializers_call_self || !deferred.is_empty();
    if ordered_body {
        eager = (0..lc.constants.len()).collect();
        deferred.clear();
    }

    // Parent + body-derived `require_relative` headers. Helpers return
    // project-root-anchored paths; we relpath each one against `out_dir`
    // so emit works correctly from any output directory.
    let mut requires: Vec<String> = Vec::new();
    // Ingest stored the parent as Rubydex resolved it.
    if let Some(parent) = lc.parent.as_ref() {
        if let Some(anchor) = require_path_for_parent(parent, app, classes) {
            if anchor != self_anchor {
                requires.push(relpath(&out_dir, &anchor));
            }
        }
    }
    // A namespaced file reopens its outer segments (`class Account`
    // around `module Joinable`), and the header names their superclass
    // to survive either load order — so this file needs THAT class's
    // parent loaded too, exactly as if it were its own.
    let outer_segments: Vec<&str> = name.split("::").collect();
    for i in 0..outer_segments.len().saturating_sub(1) {
        let qualified = outer_segments[..=i].join("::");
        let Some(outer_parent) = outer_class_parent(&qualified, app, classes) else { continue };
        if let Some(anchor) = require_path_for_parent(&outer_parent, app, classes) {
            if anchor != self_anchor {
                requires.push(relpath(&out_dir, &anchor));
            }
        }
    }
    // A reopen of a runtime framework class (lobsters' `module
    // ActiveRecord; class Base; def q ...`) must load the runtime's
    // definition first — under plain Ruby a bare reopen would otherwise
    // DEFINE an empty class, and under spinel the reopen's method
    // bodies reference runtime members. Parent-less and named into a
    // runtime namespace is the reopen signature.
    if lc.parent.is_none() {
        if let Some(anchor) = runtime_reopen_anchor(name) {
            requires.push(relpath(&out_dir, anchor));
        }
    }
    // The same rule for a namespace the runtime owns through a require
    // anchor rather than that table: the header below is the COMPOUND
    // `class Net::HTTP` whenever `Net` resolves to `runtime/net_http`,
    // and a compound header looks the outer constant up instead of
    // creating it. Nothing else loads that file first — boot.rb carries
    // no line for it, and lobsters' `extras/sponge.rb` reopen is loaded
    // through the models aggregator — so the file that needs the
    // constant requires the file that provides it.
    let outer_runtime_anchor = (outer_segments.len() > 1)
        .then(|| require_path_for_body_const(&[outer_segments[0].to_string()], app, classes, ""))
        .flatten()
        .filter(|p| p.starts_with("runtime/"));
    if let Some(anchor) = outer_runtime_anchor.as_deref() {
        let rel = relpath(&out_dir, anchor);
        if !requires.contains(&rel) {
            requires.push(rel);
        }
    }
    // `include`d modules must be LOADED before the `include` executes at
    // class-definition time — unlike body const-refs (request-time), so we
    // require them even when they're same-dir siblings (plain Ruby has no
    // Rails autoload). Resolve through the same model/library_class anchor.
    let mut nested_include_requires = BTreeSet::new();
    for inc in &lc.includes {
        // SPLIT on `::` — an include's ClassId is one Symbol holding the
        // whole path, and the const resolver keys on the ROOT segment.
        // Unsplit, `Turbo::Streams::StreamName::ClassMethods` was its
        // own root, matched nothing, and campfire's guarded channel
        // included a module with no require to load it.
        let path: Vec<String> = inc.0.as_str().split("::").map(str::to_string).collect();
        if let Some(anchor) = require_path_for_body_const(&path, app, classes, name) {
            if anchor != self_anchor {
                let nested_prefix = format!("{self_anchor}/");
                if anchor.starts_with(&nested_prefix) {
                    // Nested constants such as `Account::Joinable` reopen
                    // their containing class. Load them after that class is
                    // declared, otherwise the nested file runs too early.
                    nested_include_requires.insert(relpath(&out_dir, &anchor));
                } else {
                    requires.push(relpath(&out_dir, &anchor));
                }
            }
        }
    }
    // Require-edge classification: LOAD-time refs vs body-only refs.
    //
    // Class-body constant initializers execute while the file is being
    // required (lobsters' markdowner.rb interpolates
    // `User::VALID_USERNAME` into a class-body regexp constant), so
    // their targets need explicit requires — same footing as the
    // parent and `include`s above.
    //
    // Method-body refs resolve at request time, after boot completes.
    // Their `app/models/*` targets load through the `app/models.rb`
    // aggregator (required from main.rb / test_helper.rb before any
    // dispatch — see `apply_models_aggregator`), NOT through per-file
    // requires. Emitting requires for them is what broke the lobsters
    // boot: user.rb's body-only edge to markdowner.rb closed a cycle
    // against markdowner's genuine load-time need on user.rb, and
    // `require_relative`'s mid-load short-circuit left `User` undefined
    // where the class body read it. Non-aggregated anchors (runtime/*,
    // app/views, app/controllers/*, test/fixtures/*) keep their
    // requires — nothing else loads those.
    // Resolve one referenced constant path to the require it needs, or
    // None: no anchor, this file itself, or a body-only ref into
    // `app/models/*` (which the aggregator loads).
    let resolve = |path: &Vec<String>, load_time: bool| -> Option<String> {
        let first = path.first()?;
        // Synthesized siblings (`<Model>Row`, `<Resource>Params`,
        // `<Plural>Fixtures`) match by exact first-segment name; deeper
        // paths (`X::Y`) don't match here since synthesized classes are
        // flat. Their anchors flow through the same aggregator gate
        // below (model-dir synthesized classes are in `app/models.rb`;
        // fixture siblings keep explicit requires).
        let anchor = synthesized_siblings
            .iter()
            .find(|(n, _)| n == first)
            .map(|(_, a)| a.clone())
            .or_else(|| require_path_for_body_const(path, app, classes, name))?;
        if !load_time && !load_time_bodies && anchor.starts_with("app/models/") {
            return None;
        }
        // A view never requires the aggregator that loads it: views load
        // only through app/views.rb, so the require would come while that
        // file is still loading, a circular require that loads nothing
        // and warns under $VERBOSE.
        if anchor == "app/views" && self_anchor.starts_with("app/views/") {
            return None;
        }
        (anchor != self_anchor).then(|| relpath(&out_dir, &anchor))
    };
    let resolve_all = |e: &Expr, load_time: bool| -> BTreeSet<String> {
        let mut paths: BTreeSet<Vec<String>> = BTreeSet::new();
        walk_const_paths(e, &mut paths);
        paths.iter().filter_map(|p| resolve(p, load_time)).collect()
    };
    // Initializers and eager constants share the source's load-time order,
    // including when a dependency requires closing and reopening the class.
    enum BodyItem<'a> {
        Constant(usize, &'a Expr),
        Initializer(&'a Expr),
        Call(&'a Expr),
        Include(&'a ClassId),
        Method(&'a MethodDef),
    }
    // A model's body is its own file's. What a Concern's `included` block
    // runs there (a seed, a replayed call) carries the Concern's file, so
    // its offset says nothing about where it runs: at the `include`, ahead
    // of the model's own statements, in the order the splice gave it.
    let model_file = app
        .models
        .iter()
        .find(|m| m.name == lc.name && !m.span.is_synthetic())
        .map(|m| m.span.file);
    let key = |span: &crate::span::Span| match model_file {
        Some(file) if span.file != file => 0,
        _ => span.start,
    };
    let mut body: Vec<_> = eager.iter().map(|&i| {
        (lc.constants[i].1.span.start, BodyItem::Constant(i, &lc.constants[i].1))
    }).collect();
    body.extend(lc.class_ivar_initializers.iter().filter(|init| {
        // Source-ordered bodies interleave only real `@ivar` writes; keep
        // synthetic mattr/cattr `@@` seeds on the partitioned path below.
        !ordered_body || (!init.span.is_synthetic() && matches!(
            &*init.node,
            ExprNode::Assign { target: LValue::Ivar { .. }, .. }
                | ExprNode::OpAssign { target: LValue::Ivar { .. }, .. }
        ))
    }).map(|init| {
        (key(&init.span), BodyItem::Initializer(init))
    }));
    // Where the source has them: a method defined before the call is the macro's to override, one after it overrides the macro's.
    if ordered_body {
        body.extend(replayed_in_place.iter().map(|call| (key(&call.span), BodyItem::Initializer(call))));
    }
    body.extend(constant_calls.iter().filter(|call| {
        !(ordered_body && replayed_in_place.iter().any(|kept| std::ptr::eq(*kept, **call)))
    }).map(|call| (call.span.start, BodyItem::Call(call))));
    // Concern-spliced and other foreign-file methods keep foreign
    // `name_span` offsets; interleave only same-file declarations.
    let body_file = model_file.or_else(|| lc.class_ivar_initializers.iter().find_map(|init| {
        (!init.span.is_synthetic()
            && matches!(
                &*init.node,
                ExprNode::Assign { target: LValue::Ivar { .. }, .. }
                    | ExprNode::OpAssign { target: LValue::Ivar { .. }, .. }
            ))
        .then_some(init.span.file)
    })).or_else(|| replayed_in_place.first().map(|call| call.span.file))
        .or_else(|| constant_calls.first().map(|call| call.span.file));
    let same_file_method = |m: &MethodDef| {
        !ordered_body
            && !constant_calls.is_empty()
            && !m.name_span.is_synthetic()
            && constant_calls.first().is_some_and(|call| m.name_span.file == call.span.file)
    };
    let mut positioned: Vec<(usize, &ClassId)> = Vec::new();
    // An ordered body writes such an include where the source has it,
    // after the methods above it its hook calls on `base`.
    let mut ordered_includes: Vec<(u32, &ClassId)> = Vec::new();
    if !ordered_body {
        for inc in &lc.includes {
            let floor = positioned.last().map(|(at, _)| *at);
            if let Some(at) = hooked_include_position(lc, inc, app, classes).max(floor) {
                positioned.push((at, inc));
            }
        }
    } else {
        for (inc, span) in lc.includes.iter().zip(&lc.include_spans) {
            let in_body = !span.is_synthetic() && body_file.is_some_and(|file| span.file == file);
            let after_first = ordered_includes.last().is_some_and(|(start, _)| *start <= span.start);
            if in_body && (after_first || hooked_include_position(lc, inc, app, classes).is_some()) {
                ordered_includes.push((span.start, inc));
            }
        }
    }
    let (moved_methods, moved_includes) =
        methods_traveling_with_constant_call(lc, &positioned, same_file_method);
    let method_in_ordered_body = |m: &MethodDef| {
        ordered_body
            && !m.name_span.is_synthetic()
            && body_file.is_some_and(|file| m.name_span.file == file)
    };
    if ordered_body {
        body.extend(
            lc.methods
                .iter()
                .filter(|m| method_in_ordered_body(m))
                .map(|m| (m.name_span.start, BodyItem::Method(m))),
        );
    }
    for (slot, start) in &moved_includes {
        body.push((*start, BodyItem::Include(positioned[*slot].1)));
    }
    body.extend(ordered_includes.iter().map(|(start, inc)| (*start, BodyItem::Include(inc))));
    for index in &moved_methods {
        let method = &lc.methods[*index];
        body.push((method.name_span.start, BodyItem::Method(method)));
    }
    body.sort_by_key(|(start, _)| *start);
    // Every require the file needs at its TOP: the class-body calls and
    // deferred constants (both run at load), and the method bodies.
    let mut body_requires: BTreeSet<String> = BTreeSet::new();
    for call in &lc.unknown_calls {
        body_requires.extend(resolve_all(call, true));
    }
    for call in &replayed_in_place {
        let mut paths = BTreeSet::new();
        walk_load_time_paths(call, lc, &mut BTreeSet::new(), &mut paths);
        body_requires.extend(paths.iter().filter_map(|p| resolve(p, true)));
    }
    for &i in &deferred {
        body_requires.extend(resolve_all(&lc.constants[i].1, true));
    }
    let factory_methods: Vec<&MethodDef> = lc
        .constants
        .iter()
        .filter_map(|(name, _)| lifted_data_factory(lc, name.as_str(), classes, factories))
        .flat_map(|class| class.methods.iter())
        .collect();
    for m in &lc.methods {
        body_requires.extend(resolve_all(&m.body, false));
    }
    for m in factory_methods {
        body_requires.extend(resolve_all(&m.body, false));
        for default in m.params.iter().filter_map(|param| param.default.as_ref()) {
            body_requires.extend(resolve_all(default, false));
        }
    }
    // …and each eager initializer's own, in order, because WHERE those go
    // matters. A class body runs top to bottom, and a required file can
    // read this one's earlier constants — campfire's `ContentFilters`
    // defines `EDITOR_FORMATTING_TAGS`, then builds
    // `TextMessagePresentationFilters` from `SanitizeTags`, whose own
    // class body reads `ContentFilters::EDITOR_FORMATTING_TAGS`. Rails
    // autoloads `SanitizeTags` at that reference, after the constant
    // exists; a require at the top of the file ran it first, and
    // `require_relative`'s mid-load short-circuit left the constant
    // undefined. Each initializer needing a new file that reads an earlier
    // constant SPLITS the body: the file closes, requires that dependency,
    // and reopens. Later dependencies wait for their own use. Spinel splices
    // a `require_relative` where it stands, so both lanes read the
    // same order.
    //
    // Only when there IS such a reader: a split for a file that never
    // looks at this class's constants is churn (lobsters' `Markdowner`
    // requiring `user`), so the trigger is a required file whose own
    // load-time code reads one of the constants above it.
    let per_body: Vec<BTreeSet<String>> =
        body.iter().map(|(_, item)| match item {
            BodyItem::Constant(_, expr) | BodyItem::Initializer(expr) | BodyItem::Call(expr) => {
                resolve_all(expr, true)
            }
            BodyItem::Method(_) | BodyItem::Include(_) => BTreeSet::new(),
        }).collect();
    let readers = constant_readers(lc, app, classes, &resolve);
    let mut splits = std::collections::BTreeMap::new();
    for p in 1..per_body.len() {
        let mut before: BTreeSet<String> = body_requires.clone();
        before.extend(requires.iter().cloned());
        for q in &per_body[..p] {
            before.extend(q.iter().cloned());
        }
        let earlier: BTreeSet<&String> = body[..p]
            .iter()
            .filter_map(|(_, item)| match item {
                BodyItem::Constant(i, _) => readers.get(lc.constants[*i].0.as_str()),
                _ => None,
            })
            .flatten()
            .collect();
        if per_body[p].iter().any(|r| !before.contains(r) && earlier.contains(r)) {
            let needed: BTreeSet<String> = per_body[p]
                .iter()
                .filter(|r| !before.contains(*r))
                .cloned()
                .collect();
            splits.insert(p, needed);
        }
    }
    let deferred_requires: BTreeSet<&String> = splits.values().flatten().collect();
    for reqs in &per_body {
        for r in reqs {
            if !deferred_requires.contains(r) {
                body_requires.insert(r.clone());
            }
        }
    }
    body_requires.retain(|r| !requires.contains(r));
    requires.extend(body_requires);
    for r in &requires {
        writeln!(s, "require_relative {r:?}").unwrap();
    }
    if !requires.is_empty() {
        writeln!(s).unwrap();
    }

    // Compound names like `Views::Articles` emit as nested
    // `module Views\n  module Articles` rather than `module Views::Articles`.
    // Compound-form headers blow up at load time when the outer namespace
    // isn't already defined (Ruby looks up `Views` as a constant); nested
    // headers create the chain on the fly. Spinel-blog's hand-written
    // views use the nested form for the same reason.
    let mut segments: Vec<&str> = name.split("::").collect();
    // …UNLESS an outer segment belongs to the shared RUNTIME, where we
    // cannot know whether to reopen it as `class` or `module` and the
    // wrong guess is a TypeError at load. campfire extends Action Text
    // with `class ActionText::Attachment::OpengraphEmbed`;
    // `ActionText::Attachment` is a CLASS in runtime/action_text.rb, and
    // nesting emitted `module Attachment` around it.
    //
    // The COMPOUND header the source itself wrote needs no such
    // knowledge — it only requires the outer constants to already
    // exist, which the `require_relative` of that runtime file written
    // above guarantees. That is also why this is not the default:
    // `Views::Articles` has no owner but the nesting itself, and a
    // compound header there would look up a `Views` nothing created.
    if outer_runtime_anchor.is_some() {
        segments = vec![name];
    }
    let segments = segments;
    let depth = segments.len();
    let body_pad = "  ".repeat(depth);

    // An outer segment that names a known app CLASS must reopen as
    // `class`, not `module` — lobsters nests `CandidateId` inside the
    // `ShortId` model, and `module ShortId` after the model file loaded
    // is a TypeError under Ruby. (Aggregator order guarantees the owner
    // file loads first: `x.rb` sorts before `x/y.rb` because '.' < '/'.)
    // …and it must reopen with the SAME superclass, because the load
    // order the aggregator guarantees is not the only one: `account.rb`
    // requires `account/joinable` at its top so the `include` resolves,
    // which runs the nested file FIRST. A bare `class Account` there
    // creates it under Object, and the real `class Account <
    // ApplicationRecord` then dies with "superclass mismatch". Naming
    // the parent in both places makes the order irrelevant.
    //
    // Resolution is by QUALIFIED prefix, not bare segment:
    // `Opengraph::Fetch::RedirectDeniedError` nests inside the CLASS
    // `Opengraph::Fetch`, and looking up a bare `Fetch` finds nothing
    // ("Fetch is not a module" at load).
    let outer_header = |i: usize, seg: &str| {
        let qualified = segments[..=i].join("::");
        let parent = outer_class_parent(&qualified, app, classes);
        match (is_app_class(&qualified, app, classes), parent) {
            (true, Some(p)) => format!("class {seg} < {}", lexically_qualified(p.0.as_str(), &segments[..i], app, classes)),
            (true, None) => format!("class {seg}"),
            (false, _) => format!("module {seg}"),
        }
    };

    // A file-level `X = Data.define(…)` assigns the class once, inside
    // the namespace of `X`, and the body reopens it. A block would not
    // do: a `class` in a block body belongs to the enclosing scope, not
    // to the Data class.
    let data_factory = lc.data_define().map(|(members, _)| {
        let members: Vec<String> = members
            .iter()
            .map(|member| {
                super::emit_expr(&Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Sym { value: member.clone() } },
                ))
            })
            .collect();
        format!("Data.define({})", members.join(", "))
    });
    // A split reopens the class later in the file, without the factory.
    let open_header = |s: &mut String, factory: Option<&str>| {
        if lc.is_module {
            // Modules don't take a parent; ingest already enforces this.
            for (i, seg) in segments.iter().enumerate() {
                let header = if i < depth - 1 {
                    outer_header(i, seg)
                } else {
                    format!("module {seg}")
                };
                writeln!(s, "{}{header}", "  ".repeat(i)).unwrap();
            }
        } else {
            // Outer segments (if any) are namespace modules; the last is the class.
            for (i, seg) in segments.iter().take(depth - 1).enumerate() {
                writeln!(s, "{}{}", "  ".repeat(i), outer_header(i, seg)).unwrap();
            }
            let last = segments[depth - 1];
            let pad = "  ".repeat(depth - 1);
            if let Some(factory) = factory {
                writeln!(s, "{pad}{last} = {factory}\n").unwrap();
            }
            match lc.parent.as_ref() {
                Some(p) => {
                    let parent = lexically_qualified(p.0.as_str(), &segments[..depth - 1], app, classes);
                    writeln!(s, "{pad}class {last} < {parent}").unwrap()
                }
                None => writeln!(s, "{pad}class {last}").unwrap(),
            }
        }
    };
    // The source block has the top level as its lexical scope. Ruby puts
    // its constants on `Object`, and its methods read constants from
    // there, not through the class and its mixins. `::NAME` writes and
    // reads give the same result in the reopened class. A block itself
    // would not do here either: Spinel runs the statements of a block
    // with `self` as `main`, not as the class.
    let data_block = lc.data_define().is_some();
    let scope = if data_block { "::" } else { "" };
    let top_level = |expr: &Expr| {
        let mut expr = expr.clone();
        if data_block {
            read_constants_from_top_level(&mut expr);
        }
        expr
    };
    let include_name = |inc: &ClassId| {
        if data_block {
            format!("::{}", inc.0.as_str())
        } else {
            lexically_qualified(inc.0.as_str(), &segments, app, classes)
        }
    };
    // Plain `include X` normally; guarded only inside futamura's own
    // internal `ruby` copy, and only for an include the analyzer could
    // not resolve — the planner blocks just the routes that reach this
    // file, so the box must still load the rest. See `FUTAMURA_BOX`.
    let include_line = |inc: &ClassId| {
        let name = include_name(inc);
        if super::expr::in_futamura_box() && include_is_unresolved(lc, inc) {
            format!("include {name} if defined?({name})")
        } else {
            format!("include {name}")
        }
    };
    open_header(&mut s, data_factory.as_deref());
    let nested = std::cell::RefCell::new(NestedEmitter {
        pending: nested.iter().collect(),
        names: nested.iter().map(|block| block.name.clone()).collect(),
    });

    for require in &nested_include_requires {
        writeln!(s, "{body_pad}require_relative {require:?}").unwrap();
    }
    if !nested_include_requires.is_empty() {
        writeln!(s).unwrap();
    }

    // An include whose `self.included(base)` hook calls class methods of
    // this class, or names its methods by symbol, runs after them, as in
    // the source (see `hooked_include_position`); the rest open the body.
    // An include the source has after a positioned one is written no
    // earlier than it, so the includes keep their source order, and with
    // it the class's ancestors. An ordered body interleaves its methods
    // with ivar writes and keeps the other includes first.
    for inc in &lc.includes {
        if positioned.iter().any(|(_, p)| *p == inc) || ordered_includes.iter().any(|(_, p)| *p == inc) {
            continue;
        }
        nested.borrow_mut().write_include(&mut s, depth, lc, inc);
        writeln!(s, "{body_pad}{}", include_line(inc)).unwrap();
    }
    if lc.includes.len() > positioned.len() + ordered_includes.len() && !lc.methods.is_empty() {
        writeln!(s).unwrap();
    }

    // Class-level constants (`NAME = <expr>`), emitted before methods so
    // refs in method bodies resolve. A multi-line value (proc/array) keeps
    // its continuation lines indented to the class body.
    //
    // EXCEPT one whose initializer CALLS this class — campfire's
    // `Sound::BUILTIN = [ new(name: "56k", …), … ]` builds a table of
    // instances, and a class body runs top to bottom, so `new` there
    // reaches Object#initialize and raises. Those (and any constant
    // reading them) emit after the methods, which is where the source
    // had them. `emit_deferred_constants` renders that tail.
    let render_constants = |s: &mut String, which: &[usize]| {
        for &i in which {
            let (cname, value) = &lc.constants[i];
            nested.borrow_mut().write_expr(s, depth, lc, value);
            let rendered = super::emit_expr(&top_level(value));
            if let Some(factory) = lifted_data_factory(lc, cname.as_str(), classes, factories) {
                writeln!(s, "{body_pad}{} = {rendered} do", cname.as_str()).unwrap();
                for method in &factory.methods {
                    render_library_method(s, method, &format!("{body_pad}  "));
                }
                writeln!(s, "{body_pad}end").unwrap();
                continue;
            }
            // One of the class's `class << self` is its singleton
            // class's, written back there. Unindented: a heredoc's lines
            // are whitespace-significant.
            if value.decisions & crate::expr::SINGLETON_BODY != 0 && super::expr::carries_body_statements() {
                writeln!(s, "{body_pad}class << self").unwrap();
                writeln!(s, "{body_pad}  {} = {}", cname.as_str(), rendered.trim_end()).unwrap();
                writeln!(s, "{body_pad}end").unwrap();
                continue;
            }
            let mut lines = rendered.lines();
            match lines.next() {
                Some(first_line) => {
                    writeln!(s, "{body_pad}{scope}{} = {first_line}", cname.as_str()).unwrap();
                    for line in lines {
                        if line.is_empty() {
                            writeln!(s).unwrap();
                        } else {
                            writeln!(s, "{body_pad}{line}").unwrap();
                        }
                    }
                }
                None => writeln!(s, "{body_pad}{scope}{} = nil", cname.as_str()).unwrap(),
            }
        }
    };
    // A method a load-time conditional defines is written inside it: the
    // first of its members writes the whole statement where it stands,
    // each branch's methods in that branch, and the others are skipped.
    let written_conditionals: std::cell::RefCell<Vec<usize>> = Default::default();
    // A branch's class or module loads from its own file, here.
    let require_of = |name: &ClassId| -> String {
        let anchor = format!("app/models/{}", crate::naming::underscore(&file_owner(name.0.as_str(), app, classes)));
        format!("require_relative {:?}", relpath(&out_dir, &anchor))
    };
    fn write(
        s: &mut String,
        conditional: &crate::dialect::ConditionalDefinitions,
        lc: &LibraryClass,
        pad: &str,
        require_of: &dyn Fn(&ClassId) -> String,
    ) {
        let inner = format!("{pad}  ");
        for (i, arm) in conditional.arms.iter().enumerate() {
            match (&arm.condition, i) {
                (Some(cond), 0) => {
                    let keyword = if conditional.negated { "unless" } else { "if" };
                    writeln!(s, "{pad}{keyword} {}", super::emit_expr(cond)).unwrap();
                }
                (Some(cond), _) => writeln!(s, "{pad}elsif {}", super::emit_expr(cond)).unwrap(),
                (None, _) => writeln!(s, "{pad}else").unwrap(),
            }
            // The branch's methods and nested conditionals, in source order.
            enum Part<'a> {
                Method(&'a MethodDef),
                Nested(&'a crate::dialect::ConditionalDefinitions),
                Declared(&'a ClassId),
            }
            let mut parts: Vec<(u32, Part<'_>)> = Vec::new();
            for member in &arm.methods {
                if let Some(method) = lc.methods.iter().find(|m| member.is(m)) {
                    parts.push((member.name_span.start, Part::Method(method)));
                }
            }
            parts.extend(arm.nested.iter().map(|n| (n.span.start, Part::Nested(n))));
            parts.extend(arm.declarations.iter().map(|d| (d.start, Part::Declared(&d.name))));
            parts.sort_by_key(|(start, _)| *start);
            for (_, part) in parts {
                match part {
                    Part::Method(method) => render_library_method(s, method, &inner),
                    Part::Nested(nested) => write(s, nested, lc, &inner, require_of),
                    Part::Declared(name) => writeln!(s, "{inner}{}", require_of(name)).unwrap(),
                }
            }
        }
        writeln!(s, "{pad}end").unwrap();
    }
    let write_conditional = |s: &mut String, index: usize| {
        written_conditionals.borrow_mut().push(index);
        write(s, &lc.conditional_definitions[index], lc, &body_pad, &require_of);
    };
    let render_conditional = |s: &mut String, m: &MethodDef| -> bool {
        let Some(index) = lc.conditional_definitions.iter().position(|c| c.contains(m)) else { return false };
        if !written_conditionals.borrow().contains(&index) {
            write_conditional(s, index);
        }
        true
    };
    let render_method = |s: &mut String, m: &MethodDef| {
        if render_conditional(s, m) {
            return;
        }
        if data_block {
            let mut m = m.clone();
            m.body = top_level(&m.body);
            for param in m.params.iter_mut().chain(m.block_param.iter_mut()) {
                param.default = param.default.as_ref().map(&top_level);
            }
            render_library_method(s, &m, &body_pad);
        } else {
            render_library_method(s, m, &body_pad);
        }
    };
    let render_body = |s: &mut String, items: &[(u32, BodyItem<'_>)]| {
        for (_, item) in items {
            match item {
                BodyItem::Constant(i, _) => render_constants(s, &[*i]),
                BodyItem::Call(expr) => {
                    nested.borrow_mut().write_expr(s, depth, lc, expr);
                    for line in super::emit_expr(&top_level(expr)).lines() {
                        writeln!(s, "{body_pad}{line}").unwrap();
                    }
                }
                BodyItem::Initializer(expr) if ordered_body => {
                    nested.borrow_mut().write_expr(s, depth, lc, expr);
                    for line in in_singleton_scope(expr, super::emit_expr(&top_level(expr))).lines() {
                        writeln!(s, "{body_pad}{line}").unwrap();
                    }
                }
                // Keep initialization in the require plan even when the
                // lowered class renders it after its methods below.
                BodyItem::Initializer(_) => {}
                BodyItem::Include(inc) => {
                    nested.borrow_mut().write_include(s, depth, lc, inc);
                    writeln!(s, "{body_pad}{}", include_line(inc)).unwrap();
                }
                BodyItem::Method(method) => {
                    writeln!(s).unwrap();
                    render_method(s, method);
                    writeln!(s).unwrap();
                }
            }
        }
    };
    // Concern-spliced / synthetic-span methods cannot order against
    // source `@ivar` writes; emit them before the interleaved body.
    if ordered_body {
        let mut first = true;
        for m in lc.methods.iter().filter(|m| !method_in_ordered_body(m)) {
            if !first {
                writeln!(s).unwrap();
            }
            first = false;
            render_method(&mut s, m);
        }
        if lc.methods.iter().any(|m| !method_in_ordered_body(m)) && !body.is_empty() {
            writeln!(s).unwrap();
        }
    }

    let mut start = 0;
    for (at, reqs) in &splits {
        render_body(&mut s, &body[start..*at]);
        for i in (0..depth).rev() {
            writeln!(s, "{}end", "  ".repeat(i)).unwrap();
        }
        writeln!(s).unwrap();
        for r in reqs {
            writeln!(s, "require_relative {r:?}").unwrap();
        }
        writeln!(s).unwrap();
        open_header(&mut s, None);
        start = *at;
    }
    render_body(&mut s, &body[start..]);
    if !eager.is_empty() && !lc.methods.is_empty() {
        writeln!(s).unwrap();
    }

    // Class-body calls the ingest didn't model (`LibraryClass::
    // unknown_calls`), replayed verbatim when — and only when — the
    // class extends a base we don't model at all. See
    // `replays_foreign_class_body`. After the constants (a captured
    // call may reference one) and before the methods.
    //
    // A plain class (no superclass, or a built-in one) that `extend`s an app
    // module replays that `extend` and the calls to the module's methods
    // after it (`app_extension_calls`); every other call stays dropped.
    // Preserved external-gem `include`s are emitted separately and filtered
    // out of both the foreign replay list and the dropped-call reports.
    let in_body = |call: &&Expr| {
        constant_calls.iter().any(|kept| std::ptr::eq(*kept, *call))
            || (ordered_body && replayed_in_place.iter().any(|kept| std::ptr::eq(*kept, *call)))
    };
    let replayed: Vec<&Expr> = if replays_foreign_class_body(lc, app, classes) {
        lc.unknown_calls
            .iter()
            .filter(|call| !is_preserved_external_include(call) && !in_body(call))
            .collect()
    } else {
        let mut picked = app_extension_calls(lc, app, classes);
        picked.extend(gem_include_macro_calls(lc));
        // Source order, each call once: `span_method` prepends its wrapper,
        // and an app macro written after it may read the ancestors.
        let own: Vec<&Expr> =
            lc.unknown_calls.iter().filter(|call| picked.iter().any(|p| std::ptr::eq(*p, *call))).collect();
        let mut replayed = own.clone();
        replayed.retain(|call| !in_body(call));
        for call in &lc.unknown_calls {
            // A payload job's `queue_as` and `discard_on
            // ActiveJob::DeserializationError` are not dropped: the plan
            // carries them (`lower::job_payload`).
            if in_body(&call) {
                continue;
            }
            let listed = own.iter().any(|c| std::ptr::eq(*c, call))
                || is_preserved_external_include(call)
                || crate::lower::job_payload::modelled_class_body_call(app, lc, call)
                || crate::lower::sti_scope::models_class_body_setting(app, lc, call)
                || crate::ingest::inherited_alias::carried_by_copy(lc, call);
            if listed {
                continue;
            }
            report_dropped_class_body_call(lc, call);
        }
        replayed
    };
    for call in &replayed {
        nested.borrow_mut().write_expr(&mut s, depth, lc, call);
        for line in super::emit_expr(call).lines() {
            if line.is_empty() {
                writeln!(s).unwrap();
            } else {
                writeln!(s, "{body_pad}{line}").unwrap();
            }
        }
    }
    if !replayed.is_empty() && !lc.methods.is_empty() {
        writeln!(s).unwrap();
    }

    // Non-ordered classes keep the methods-then-initializers partition.
    // Ordered bodies already emitted foreign methods above and same-file
    // methods inside the interleaved body.
    if !ordered_body {
        let mut first = true;
        for (index, m) in lc.methods.iter().enumerate() {
            if !first {
                writeln!(s).unwrap();
            }
            first = false;
            for (slot, (_, inc)) in positioned.iter().enumerate().filter(|(_, (at, _))| *at == index) {
                if moved_includes.iter().any(|(moved, _)| *moved == slot) {
                    continue;
                }
                nested.borrow_mut().write_include(&mut s, depth, lc, inc);
                writeln!(s, "{body_pad}{}", include_line(inc)).unwrap();
                writeln!(s).unwrap();
            }
            if moved_methods.binary_search(&index).is_ok() {
                continue;
            }
            render_method(&mut s, m);
        }
        for (slot, (_, inc)) in positioned.iter().enumerate().filter(|(_, (at, _))| *at >= lc.methods.len()) {
            if moved_includes.iter().any(|(moved, _)| *moved == slot) {
                continue;
            }
            writeln!(s).unwrap();
            nested.borrow_mut().write_include(&mut s, depth, lc, inc);
            writeln!(s, "{body_pad}{}", include_line(inc)).unwrap();
        }
    }

    // Finite class-side initialization is lowered IR, not replay of a
    // framework DSL. Runs after class methods (a Concern macro may write
    // its `class_attribute`). Per-class `@ivar` seeds leave unset
    // subclasses absent; `mattr_*` / `cattr_*` seeds are `@@` and share
    // across the hierarchy by Ruby class-variable rules. Source-ordered
    // `@ivar` writes already interleaved above; remaining seeds still run.
    let remaining_inits = |init: &Expr| {
        !ordered_body
            || init.span.is_synthetic()
            || !matches!(
                &*init.node,
                ExprNode::Assign { target: LValue::Ivar { .. }, .. }
                    | ExprNode::OpAssign { target: LValue::Ivar { .. }, .. }
            )
    };
    if lc.class_ivar_initializers.iter().any(remaining_inits) && !lc.methods.is_empty() {
        writeln!(s).unwrap();
    }
    for init in lc.class_ivar_initializers.iter().filter(|init| remaining_inits(init)) {
        nested.borrow_mut().write_expr(&mut s, depth, lc, init);
        for line in super::emit_expr(&top_level(init)).lines() {
            writeln!(s, "{body_pad}{line}").unwrap();
        }
    }

    if !deferred.is_empty() {
        if !lc.methods.is_empty() {
            writeln!(s).unwrap();
        }
        render_constants(&mut s, &deferred);
    }

    // A conditional that defines no method (it only declares) has had no
    // member to stand at.
    for index in 0..lc.conditional_definitions.len() {
        if !written_conditionals.borrow().contains(&index) {
            writeln!(s).unwrap();
            write_conditional(&mut s, index);
        }
    }

    let remaining = nested.borrow().names.clone();
    nested.borrow_mut().write_names(&mut s, depth, &remaining);

    for i in (0..depth).rev() {
        writeln!(s, "{}end", "  ".repeat(i)).unwrap();
    }

    EmittedFile { path: out_path, content: s }
}

/// `text`, the emitted `expr`, inside a `class << self` when the source
/// wrote it in the class's own (`expr::SINGLETON_BODY`) and the tree
/// carries it there: Ruby runs it with the singleton class as `self`,
/// definee and lexical scope.
fn in_singleton_scope(expr: &Expr, text: String) -> String {
    if expr.decisions & crate::expr::SINGLETON_BODY == 0 || !super::expr::carries_body_statements() {
        return text;
    }
    let text = text.trim_end();
    let body = if text.contains("<<") { text.to_string() } else { super::shared::indent_lines(text, 1) };
    format!("class << self\n{body}\nend")
}

/// Same-file methods a constant-defining call must see, and the hooked
/// includes that sit between them.
///
/// Returns method indices (ascending) and `(slot, anchor)` into
/// `positioned`. The anchor is the include's own offset, and not
/// before the end of the method the hook calls. A call written on
/// either side of the include stays there. An include that cannot
/// move stays with the later method loop, and so does every method
/// that would otherwise pass it.
fn methods_traveling_with_constant_call<'a>(
    lc: &'a LibraryClass,
    positioned: &[(usize, &'a ClassId)],
    same_file: impl Fn(&MethodDef) -> bool,
) -> (Vec<usize>, Vec<(usize, u32)>) {
    let mut method_moves = vec![false; lc.methods.len()];
    let mut include_moves = vec![false; positioned.len()];
    for (index, method) in lc.methods.iter().enumerate() {
        let blocked = positioned
            .iter()
            .enumerate()
            .any(|(slot, (at, _))| *at <= index && !include_moves[slot]);
        if !same_file(method) || blocked {
            continue;
        }
        method_moves[index] = true;
        for (slot, (at, _)) in positioned.iter().enumerate() {
            if at.checked_sub(1) == Some(index) {
                include_moves[slot] = true;
            }
        }
    }
    let methods: Vec<usize> = method_moves
        .iter()
        .enumerate()
        .filter(|(_, moves)| **moves)
        .map(|(index, _)| index)
        .collect();
    let includes = include_moves
        .iter()
        .enumerate()
        .filter(|(_, moves)| **moves)
        .map(|(slot, _)| {
            let followed = &lc.methods[positioned[slot].0 - 1];
            let after_method = followed.name_span.end.max(followed.name_span.start.saturating_add(1));
            let source = lc
                .includes
                .iter()
                .zip(lc.include_spans.iter())
                .find_map(|(inc, span)| std::ptr::eq(inc, positioned[slot].1).then_some(span))
                .filter(|span| !span.is_synthetic())
                .map(|span| span.start);
            (slot, source.map(|start| start.max(after_method)).unwrap_or(after_method))
        })
        .collect();
    (methods, includes)
}

/// Where `include inc` must be written in `lc`'s body when the source's
/// order is load-bearing: the index into `lc.methods` to write it before.
///
/// The include bucket opens the emitted body, ahead of the constants and
/// methods. That is Ruby-equivalent for method lookup, but not for an
/// app module's `self.included(base)` hook, which runs at the include
/// and may call `base`'s own class methods. A class that defines `ATTRS`
/// and `def self.attrs`, then includes a module whose hook reads
/// `base.attrs`, raised NoMethodError at load when the include was
/// written first. Such an include goes right after the last class method
/// of `lc` its hook calls on `base` (the source must have had it below
/// that def), and so also after every eagerly emitted constant.
///
/// A hook may also look up one of `lc`'s methods by symbol in a call on
/// `base`: a class that defines `store`, then includes a module whose
/// hook runs `base.send(:alias_method, :store_without_validation,
/// :store)`. Written first, the include raised NameError (undefined
/// method 'store') at load. Such an include goes right after the last
/// method of `lc`, of either side, the hook names. `None` keeps it first.
fn hooked_include_position(lc: &LibraryClass, inc: &ClassId, app: &App, classes: &[LibraryClass]) -> Option<usize> {
    let bare = |c: &ClassId| c.0.as_str().trim_start_matches("::").to_string();
    let module = first_module_bare_named(&bare(inc), app, classes)?;
    let hook = module.methods.iter().find(|m| {
        m.receiver == MethodReceiver::Class && m.name.as_str() == "included" && m.params.len() == 1
    })?;
    let base = hook.params[0].name.clone();
    let mut called: Vec<Symbol> = Vec::new();
    let mut named: Vec<Symbol> = Vec::new();
    /// Methods a call on `base` looks up by name, so the method must
    /// already exist: the symbol arguments are the names looked up
    /// (`alias_method`'s second argument is the existing one). A call that
    /// defines its argument (`attr_accessor :x`, `validates :x`, `delegate
    /// :x`) names no method the class must have defined first: moving the
    /// include after a later `def x` would let the hook's definition
    /// clobber the class's own.
    fn looked_up(method: &str, args: &[Expr]) -> Vec<Symbol> {
        let syms = |args: &[Expr]| -> Vec<Symbol> {
            args.iter()
                .filter_map(|arg| match &*arg.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
                    _ => None,
                })
                .collect()
        };
        match method {
            "alias_method" => args.get(1).map(|a| syms(std::slice::from_ref(a))).unwrap_or_default(),
            "instance_method" | "method_defined?" | "public_method_defined?" | "private_method_defined?"
            | "protected_method_defined?" | "remove_method" | "undef_method" | "public" | "private"
            | "protected" | "module_function" | "public_instance_method" => syms(args),
            _ => Vec::new(),
        }
    }
    fn walk(e: &Expr, base: &Symbol, called: &mut Vec<Symbol>, named: &mut Vec<Symbol>) {
        if let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node {
            if matches!(&*r.node, ExprNode::Var { name, .. } if name == base) {
                if !called.contains(method) {
                    called.push(method.clone());
                }
                // `base.send(:alias_method, :a, :b)` is `base.alias_method :a, :b`.
                let forwarded = matches!(method.as_str(), "send" | "public_send" | "__send__");
                let lookup = match args.first().map(|a| &*a.node) {
                    Some(ExprNode::Lit { value: Literal::Sym { value } }) if forwarded => {
                        looked_up(value.as_str(), &args[1..])
                    }
                    _ => looked_up(method.as_str(), args),
                };
                for name in lookup {
                    if !named.contains(&name) {
                        named.push(name);
                    }
                }
            }
        }
        e.node.for_each_child(&mut |c| walk(c, base, called, named));
    }
    walk(&hook.body, &base, &mut called, &mut named);
    lc.methods
        .iter()
        .rposition(|m| (m.receiver == MethodReceiver::Class && called.contains(&m.name)) || named.contains(&m.name))
        .map(|i| i + 1)
}

/// For each of `lc`'s constants, the requires (as `resolve` spells them
/// from `lc`'s file) of the OTHER library classes whose load-time code —
/// constant initializers and class-body calls — reads it: qualified
/// (`ContentFilters::EDITOR_FORMATTING_TAGS`), or bare from a class
/// nested inside `lc` (which finds it lexically).
fn constant_readers(
    lc: &LibraryClass,
    app: &App,
    classes: &[LibraryClass],
    resolve: &dyn Fn(&Vec<String>, bool) -> Option<String>,
) -> crate::det::HashMap<String, BTreeSet<String>> {
    let own = lc.name.0.as_str();
    let own_segs: Vec<&str> = own.split("::").collect();
    let names: crate::det::HashSet<&str> =
        lc.constants.iter().map(|(n, _)| n.as_str()).collect();
    let mut out: crate::det::HashMap<String, BTreeSet<String>> =
        crate::det::HashMap::default();
    if let Some(index) = ConstantReaderIndex::for_slice(app, classes) {
        // The same reads as the walk below, looked up: a qualified
        // `Own::NAME` from any other class, or a bare `NAME` from a
        // class nested inside this one.
        for (cname, _) in &lc.constants {
            let mut qualified: Vec<String> = own_segs.iter().map(|s| s.to_string()).collect();
            qualified.push(cname.as_str().to_string());
            let bare = vec![cname.as_str().to_string()];
            let qualified_readers = index.get(&qualified).into_iter().flatten().map(|&j| (j, false));
            let bare_readers = index.get(&bare).into_iter().flatten().map(|&j| (j, true));
            for (j, needs_nesting) in qualified_readers.chain(bare_readers) {
                let other_name = classes[j].name.0.as_str();
                if other_name == own || (needs_nesting && !other_name.starts_with(&format!("{own}::"))) {
                    continue;
                }
                let other_path: Vec<String> = other_name.split("::").map(str::to_string).collect();
                if let Some(req) = resolve(&other_path, true) {
                    out.entry(cname.as_str().to_string()).or_default().insert(req);
                }
            }
        }
        return out;
    }
    for other in classes {
        #[cfg(test)]
        lookup_tests::READER_SCAN_VISITS.with(|n| n.set(n.get() + 1));
        let other_name = other.name.0.as_str();
        if other_name == own {
            continue;
        }
        let nested = other_name.starts_with(&format!("{own}::"));
        let mut paths: BTreeSet<Vec<String>> = BTreeSet::new();
        for (_, v) in &other.constants {
            walk_const_paths(v, &mut paths);
        }
        for call in other.unknown_calls.iter().chain(&other.class_ivar_initializers) {
            walk_const_paths(call, &mut paths);
        }
        for path in &paths {
            let read = match path.split_last() {
                Some((last, prefix)) if names.contains(last.as_str()) => {
                    prefix.iter().map(String::as_str).eq(own_segs.iter().copied())
                        || (nested && prefix.is_empty())
                }
                _ => false,
            };
            if !read {
                continue;
            }
            let other_path: Vec<String> = other_name.split("::").map(str::to_string).collect();
            if let Some(req) = resolve(&other_path, true) {
                out.entry(path.last().unwrap().clone()).or_default().insert(req);
            }
        }
    }
    out
}

/// Every constant path each library class's load-time code (constant
/// initializers and class-body calls) names, inverted: path → indices
/// into `app.library_classes` of the classes naming it. Built once per
/// `emit_library_class_decls` and dropped when it returns; outside that
/// scope (or for another `App`) `constant_readers` walks as before.
struct ConstantReaderIndex;

#[cfg(test)]
#[path = "lookup_tests.rs"]
mod lookup_tests;

type ReaderIndex = std::rc::Rc<HashMap<Vec<String>, Vec<usize>>>;

thread_local! {
    static CONSTANT_READER_INDEX: std::cell::RefCell<Option<(usize, usize, usize, ReaderIndex)>> =
        const { std::cell::RefCell::new(None) };
}

/// Restores the previous index when the emit that installed this one
/// returns. Borrows the `App` and the library-class slice for its whole
/// life, so neither address can be reused while the index is installed.
struct ConstantReaderIndexGuard<'a>(
    Option<(usize, usize, usize, ReaderIndex)>,
    std::marker::PhantomData<&'a App>,
    std::marker::PhantomData<&'a [LibraryClass]>,
);

impl Drop for ConstantReaderIndexGuard<'_> {
    fn drop(&mut self) {
        let previous = self.0.take();
        CONSTANT_READER_INDEX.with(|slot| *slot.borrow_mut() = previous);
    }
}

impl ConstantReaderIndex {
    fn install(app: &App) -> ConstantReaderIndexGuard<'_> {
        Self::install_from(app, &app.library_classes)
    }

    fn install_from<'a>(app: &'a App, classes: &'a [LibraryClass]) -> ConstantReaderIndexGuard<'a> {
        #[cfg(test)]
        lookup_tests::READER_BUILDS.with(|n| n.set(n.get() + 1));
        let mut index: HashMap<Vec<String>, Vec<usize>> = HashMap::default();
        for (j, other) in classes.iter().enumerate() {
            #[cfg(test)]
            lookup_tests::READER_CLASS_VISITS.with(|n| n.set(n.get() + 1));
            let mut paths: BTreeSet<Vec<String>> = BTreeSet::new();
            for (_, v) in &other.constants {
                walk_const_paths(v, &mut paths);
            }
            for call in other.unknown_calls.iter().chain(&other.class_ivar_initializers) {
                walk_const_paths(call, &mut paths);
            }
            for path in paths {
                index.entry(path).or_default().push(j);
            }
        }
        let key = app as *const App as usize;
        let previous = CONSTANT_READER_INDEX.with(|slot| {
            slot.borrow_mut().replace((key, classes.as_ptr() as usize, classes.len(), std::rc::Rc::new(index)))
        });
        ConstantReaderIndexGuard(previous, std::marker::PhantomData, std::marker::PhantomData)
    }

    /// The installed index, when it was built from this `app`.
    fn current(app: &App) -> Option<ReaderIndex> {
        #[cfg(test)]
        if lookup_tests::UNCACHED.get() { return None; }
        let key = app as *const App as usize;
        CONSTANT_READER_INDEX.with(|slot| {
            slot.borrow().as_ref().filter(|(k, _, _, _)| *k == key).map(|(_, _, _, index)| index.clone())
        })
    }

    /// The installed index when it was built from this exact class slice.
    fn for_slice(app: &App, classes: &[LibraryClass]) -> Option<ReaderIndex> {
        #[cfg(test)]
        if lookup_tests::UNCACHED.get() { return None; }
        let key = app as *const App as usize;
        let ptr = classes.as_ptr() as usize;
        let len = classes.len();
        CONSTANT_READER_INDEX.with(|slot| {
            slot.borrow().as_ref().and_then(|(k, source, n, index)| {
                (*k == key && *source == ptr && *n == len).then(|| index.clone())
            })
        })
    }
}

/// Both per-reference indexes, borrowing the App until this scope ends.
/// Nested emits restore the enclosing indexes on return.
pub(crate) struct LookupIndexes<'a> {
    _readers: ConstantReaderIndexGuard<'a>,
    _names: NameIndexGuard<'a>,
}

pub(crate) fn install_lookup_indexes(app: &App) -> LookupIndexes<'_> {
    LookupIndexes { _readers: ConstantReaderIndex::install(app), _names: NameIndex::install(app) }
}

fn install_lookup_indexes_for<'a>(app: &'a App, classes: &'a [LibraryClass]) -> LookupIndexes<'a> {
    LookupIndexes {
        _readers: ConstantReaderIndex::install_from(app, classes),
        _names: NameIndex::install_from(app, classes),
    }
}

/// The app's class names, for the per-reference lookups: model and
/// controller names, and each library class name's indices into
/// `app.library_classes` (a name can repeat: a reopen is its own entry).
/// Installed and keyed like `ConstantReaderIndex`; without it the
/// lookups scan as before.
struct NameIndex {
    models: crate::det::HashSet<String>,
    controllers: crate::det::HashSet<String>,
    library_classes: HashMap<String, Vec<usize>>,
    first_modules_bare: std::cell::OnceCell<HashMap<String, usize>>,
    constant_paths: std::cell::OnceCell<std::rc::Rc<crate::det::HashSet<String>>>,
    source_ptr: usize,
    source_len: usize,
}

thread_local! {
    static NAME_INDEX: std::cell::RefCell<Option<(usize, std::rc::Rc<NameIndex>)>> =
        const { std::cell::RefCell::new(None) };
}

struct NameIndexGuard<'a>(
    Option<(usize, std::rc::Rc<NameIndex>)>,
    std::marker::PhantomData<&'a App>,
    std::marker::PhantomData<&'a [LibraryClass]>,
);

impl Drop for NameIndexGuard<'_> {
    fn drop(&mut self) {
        let previous = self.0.take();
        NAME_INDEX.with(|slot| *slot.borrow_mut() = previous);
    }
}

impl NameIndex {
    fn install(app: &App) -> NameIndexGuard<'_> {
        Self::install_from(app, &app.library_classes)
    }

    fn install_from<'a>(app: &'a App, classes: &'a [LibraryClass]) -> NameIndexGuard<'a> {
        #[cfg(test)]
        lookup_tests::NAME_BUILDS.with(|n| n.set(n.get() + 1));
        let mut library_classes: HashMap<String, Vec<usize>> = HashMap::default();
        for (j, lc) in classes.iter().enumerate() {
            #[cfg(test)]
            lookup_tests::NAME_CLASS_VISITS.with(|n| n.set(n.get() + 1));
            library_classes.entry(lc.name.0.as_str().to_string()).or_default().push(j);
        }
        let index = NameIndex {
            models: app.models.iter().map(|m| m.name.0.as_str().to_string()).collect(),
            controllers: app.controllers.iter().map(|c| c.name.0.as_str().to_string()).collect(),
            library_classes,
            first_modules_bare: std::cell::OnceCell::new(),
            constant_paths: std::cell::OnceCell::new(),
            source_ptr: classes.as_ptr() as usize,
            source_len: classes.len(),
        };
        let key = app as *const App as usize;
        let previous = NAME_INDEX.with(|slot| slot.borrow_mut().replace((key, std::rc::Rc::new(index))));
        NameIndexGuard(previous, std::marker::PhantomData, std::marker::PhantomData)
    }

    fn covers(&self, classes: &[LibraryClass]) -> bool {
        self.source_ptr == classes.as_ptr() as usize && self.source_len == classes.len()
    }

    fn current(app: &App) -> Option<std::rc::Rc<NameIndex>> {
        #[cfg(test)]
        if lookup_tests::UNCACHED.get() { return None; }
        let key = app as *const App as usize;
        NAME_INDEX.with(|slot| slot.borrow().as_ref().filter(|(k, _)| *k == key).map(|(_, index)| index.clone()))
    }
}

/// Whether `name` is a model of this app.
fn is_model_named(name: &str, app: &App) -> bool {
    match NameIndex::current(app) {
        Some(index) => index.models.contains(name),
        None => app.models.iter().any(|m| m.name.0.as_str() == name),
    }
}

/// Whether `name` is a controller of this app.
fn is_controller_named(name: &str, app: &App) -> bool {
    match NameIndex::current(app) {
        Some(index) => index.controllers.contains(name),
        None => app.controllers.iter().any(|c| c.name.0.as_str() == name),
    }
}

/// The library classes named `name`, in `classes` order.
fn library_classes_named<'a>(name: &str, app: &'a App, classes: &'a [LibraryClass]) -> Vec<&'a LibraryClass> {
    match NameIndex::current(app).filter(|index| index.covers(classes)) {
        Some(index) => index
            .library_classes
            .get(name)
            .map(|js| js.iter().map(|&j| &classes[j]).collect())
            .unwrap_or_default(),
        None => classes.iter().filter(|lc| lc.name.0.as_str() == name).collect(),
    }
}

/// The first library module whose name, less any leading `::`, is
/// `target`.
fn first_module_bare_named<'a>(target: &str, app: &'a App, classes: &'a [LibraryClass]) -> Option<&'a LibraryClass> {
    let bare = |lc: &'a LibraryClass| lc.name.0.as_str().trim_start_matches("::");
    match NameIndex::current(app).filter(|index| index.covers(classes)) {
        Some(index) => {
            let first = index.first_modules_bare.get_or_init(|| {
                #[cfg(test)]
                lookup_tests::BARE_MODULE_BUILDS.with(|n| n.set(n.get() + 1));
                let mut first: HashMap<String, usize> = HashMap::default();
                for (j, lc) in classes.iter().enumerate() {
                    if lc.is_module {
                        first.entry(bare(lc).to_string()).or_insert(j);
                    }
                }
                first
            });
            first.get(target).map(|&j| &classes[j])
        }
        None => classes.iter().find(|lc| lc.is_module && bare(lc) == target),
    }
}

/// Whether `name` is a model or a library class of this app.
fn is_model_or_library_class(name: &str, app: &App, classes: &[LibraryClass]) -> bool {
    is_model_named(name, app) || !library_classes_named(name, app, classes).is_empty()
}

/// Split a class's constants into the ones that can be initialized
/// before its methods exist and the ones that can't. A constant defers
/// when its initializer dispatches to this class — a bare `new(…)` or a
/// receiverless call naming one of its own methods, or either of those
/// SPELLED WITH THE CLASS NAME (`Sound.new(…)`) — or when it reads a
/// constant that already deferred. A self-dispatch inside a STORED
/// closure doesn't count: that body runs on call, not on load. Returns
/// index lists so both groups keep source order, and whether class-ivar
/// initializers also reach method bodies at load time.
///
/// The class-name spelling is not hypothetical: `lower::class_body_new`
/// gives a class body's bare `new` its receiver (a receiverless call is
/// unresolvable on a strict target), and a rule that keyed on the bare
/// spelling alone stopped seeing campfire's `Sound::BUILTIN` the moment
/// it did. That emitted fifty-six constructor calls ABOVE the
/// `initialize` they call — `BasicObject#initialize: wrong number of
/// arguments (given 1, expected 0)` at load, on every one of the
/// suite's 52 files. The fact the rule is about is "does this
/// initializer dispatch to this class", and both spellings are that
/// fact.
fn partition_deferred_constants(lc: &LibraryClass) -> (Vec<usize>, Vec<usize>, bool) {
    fn is_own_class(recv: &Option<Expr>, class_name: &str) -> bool {
        let Some(r) = recv else { return false };
        if matches!(&*r.node, ExprNode::SelfRef) { return true; }
        let ExprNode::Const { path } = &*r.node else { return false };
        path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::") == class_name
    }
    /// Does this Const name a constant already marked deferred — bare
    /// `BUILTIN`, or `OwnClass::BUILTIN` (qualified by a later rewrite)?
    fn const_names_deferred(
        path: &[crate::ident::Symbol],
        deferred_names: &crate::det::HashSet<String>,
        class_name: &str,
    ) -> bool {
        if path.is_empty() {
            return false;
        }
        if path.len() == 1 {
            return deferred_names.contains(path[0].as_str());
        }
        let leaf = path[path.len() - 1].as_str();
        if !deferred_names.contains(leaf) {
            return false;
        }
        let prefix = path[..path.len() - 1]
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("::");
        prefix == class_name
    }
    fn calls_self(expr: &Expr, own: &crate::det::HashSet<&str>, deferred_names: &crate::det::HashSet<String>, class_name: &str) -> bool {
        match &*expr.node {
            // A closure that is STORED rather than run — `proc { … }`,
            // `lambda { … }`, `Proc.new { … }`, `->() { … }` — executes
            // its body when something CALLS it, not when the constant
            // initializes. lobsters' `CACHE_PAGE = proc { …
            // clear_session_cookie? }` otherwise reads as a self-dispatch,
            // and deferring it marks the whole class load-time-bodies,
            // which pulls every method-body const ref into the require
            // header: that closed a cycle (application_controller →
            // user → avatars_controller → application_controller) and
            // left the emitted tree unable to load at all.
            //
            // Blocks that DO run now (`map`, `each`) are descended into
            // at the Send arm below, so a Lambda reaching this arm is a
            // stored literal.
            ExprNode::Lambda { .. } => false,
            ExprNode::Send { recv, method, args, block, .. } => {
                if (recv.is_none() || is_own_class(recv, class_name))
                    && (method.as_str() == "new" || own.contains(method.as_str()))
                {
                    return true;
                }
                let stores_block = matches!(method.as_str(), "proc" | "lambda")
                    || (method.as_str() == "new"
                        && matches!(
                            recv.as_ref().map(|r| &*r.node),
                            Some(ExprNode::Const { path })
                                if path.len() == 1 && path[0].as_str() == "Proc"
                        ));
                recv.iter()
                    .chain(args.iter())
                    .any(|e| calls_self(e, own, deferred_names, class_name))
                    || (!stores_block
                        && block.as_ref().is_some_and(|b| match &*b.node {
                            ExprNode::Lambda { body, .. } => {
                                calls_self(body, own, deferred_names, class_name)
                            }
                            _ => calls_self(b, own, deferred_names, class_name),
                        }))
            }
            // Bare `BUILTIN` *and* `Sound::BUILTIN` — index_by grounding
            // (and similar) rewrites the receiver to the qualified form,
            // and a rule that only matched path.len() == 1 left INDEX
            // eager while BUILTIN deferred: `uninitialized constant
            // Sound::BUILTIN` at load (campfire models.rb → sound.rb).
            ExprNode::Const { path } if const_names_deferred(path, deferred_names, class_name) => {
                true
            }
            _ => {
                let mut found = false;
                expr.node.for_each_child(&mut |child| {
                    if !found && calls_self(child, own, deferred_names, class_name) {
                        found = true;
                    }
                });
                found
            }
        }
    }

    /// The constants a value writes as it runs (`ALL = [LOW = new(…)]`
    /// writes LOW): they exist only once that value has run. This walks every
    /// child, including blocks and branches the typing registry excludes.
    /// Deferring a constant that reads one of those is harmless: a later
    /// definition order is always valid, a missing one is not.
    fn written_constants(expr: &Expr, out: &mut crate::det::HashSet<String>) {
        if let ExprNode::Assign { target: crate::expr::LValue::Const { path }, .. } = &*expr.node {
            if let [name] = path.as_slice() {
                out.insert(name.as_str().to_string());
            }
        }
        expr.node.for_each_child(&mut |child| written_constants(child, out));
    }

    let own: crate::det::HashSet<&str> =
        lc.methods.iter().map(|m| m.name.as_str()).collect();
    let mut deferred_names: crate::det::HashSet<String> = crate::det::HashSet::default();
    let (mut eager, mut deferred) = (Vec::new(), Vec::new());
    for (i, (name, value)) in lc.constants.iter().enumerate() {
        if calls_self(value, &own, &deferred_names, lc.name.0.as_str()) {
            deferred_names.insert(name.as_str().to_string());
            written_constants(value, &mut deferred_names);
            deferred.push(i);
        } else {
            eager.push(i);
        }
    }
    let initializers_call_self = lc.class_ivar_initializers.iter()
        .any(|init| calls_self(init, &own, &deferred_names, lc.name.0.as_str()));
    (eager, deferred, initializers_call_self)
}

/// Writes every relative constant read in `expr` as a read from the top
/// level, `X` as `::X`. See `emit_library_class_decl_inner`.
fn read_constants_from_top_level(expr: &mut Expr) {
    if let ExprNode::Const { path } = &mut *expr.node {
        if path.first().is_some_and(|segment| !segment.as_str().is_empty()) {
            path.insert(0, Symbol::from(""));
        }
    }
    expr.node.for_each_child_mut(&mut |child| read_constants_from_top_level(child));
}

/// Project-root-anchored require target for a parent class, if one is needed.
/// `ActiveRecord::Base` lives in the runtime; same-dir parents
/// (ApplicationRecord, custom abstract bases) resolve to a sibling under
/// `app/models/`. Everything else returns `None` (assume the loader sees
/// the parent some other way).
/// The runtime stem a framework-class REOPEN must load first, if the
/// class lives in a runtime namespace this tree ships. App classes with
/// `::` names outside these namespaces (ShortId::CandidateId) get None.
fn runtime_reopen_anchor(name: &str) -> Option<&'static str> {
    for (prefix, anchor) in [
        ("ActiveRecord", "runtime/active_record"),
        ("ActionController", "runtime/action_controller"),
        ("ActionView", "runtime/action_view"),
        ("ActionDispatch", "runtime/action_dispatch"),
        ("ActionMailer", "runtime/action_mailer"),
        ("ActiveJob", "runtime/active_job"),
    ] {
        if name == prefix || name.strip_prefix(prefix).is_some_and(|r| r.starts_with("::")) {
            return Some(anchor);
        }
    }
    None
}

/// Whether this class's captured `unknown_calls` should be replayed
/// into the emitted class body.
///
/// The test is "does the class extend a base we model at all?".
/// `require_path_for_parent` already answers that — it resolves every
/// framework base (`ActiveJob::Base`, `ActionMailer::Base`, …) and
/// every app-local parent to a require target, and returns `None` only
/// for a superclass that is neither. `SearchParser < Parslet::Parser`
/// lands in that `None` case: the base comes from a gem, the gem is in
/// the app's bundle, and its class-body DSL is the class. Replaying is
/// the only way to emit it, and it is safe because the DSL's receiver
/// is the gem's own class.
///
/// Everything else deliberately stays dropped, because a call we don't
/// recognize on a base we DO model is already the lowering's business
/// and replaying it would be wrong twice over:
///
///   - a concern's `included do … end` is spliced into each includer by
///     `splice_concerns_into_models`; replaying it here would apply the
///     callbacks a second time, on a module the runtime gives no
///     `included` hook to,
///   - `queue_as` / `rescue_from` / `default from:` / `delegate` are
///     framework macros whose receiver is our own runtime base class.
///     Where the runtime doesn't define one, replaying turns a silent
///     modelling gap into a NoMethodError at class-definition time,
///     i.e. an app that won't boot.
///
/// A dropped call is no longer silent either way — see
/// `report_dropped_class_body_call`.
fn replays_foreign_class_body(lc: &LibraryClass, app: &App, classes: &[LibraryClass]) -> bool {
    if lc.is_module {
        return false;
    }
    lc.parent.as_ref().is_some_and(|p| {
        require_path_for_parent(p, app, classes).is_none() && !is_core_class_name(p.0.as_str())
    })
}

/// The class-body calls of a class whose body this tree already models
/// (one `replays_foreign_class_body` says is not a gem's — the caller
/// only reaches this function in that `else` branch) that only the
/// app's own code defines: `extend M`/`prepend M` of an app module and
/// the bare calls after it to that module's methods (`extend SymbolEnum`
/// then `symbol_enum :state, [:on, :off]`). No runtime base answers
/// these, so the module is the receiver's and replaying the calls is
/// what Ruby runs — whether or not the class also has an ordinary
/// app-local superclass: that superclass answers `replays_foreign_class_body`
/// on its own, and has no bearing on whether THIS class's own `extend`/
/// `prepend` of an unrelated module should replay. They are written
/// before the methods, like every replayed call, so a call to the
/// class's own `def self.` (defined below it in the emitted body) is
/// not one of them. Anything else stays dropped and reported
/// (`replays_foreign_class_body`).
fn app_extension_calls<'a>(lc: &'a LibraryClass, app: &App, classes: &'a [LibraryClass]) -> Vec<&'a Expr> {
    if lc.is_module {
        return Vec::new();
    }
    let app_module = |path: &[Symbol]| -> Option<&LibraryClass> {
        // `written_names` has already rewritten this path in place to
        // Rubydex's resolved answer (or left it as written when Rubydex
        // had none, or the call has no source span to resolve from —
        // a synthesized class body). Reading it back here keeps this
        // lookup from disagreeing with the analyzer's own
        // `fold_extended_modules`, which asks Rubydex for the same
        // span; the nested-under-this-class guess is kept only as a
        // fallback for an unresolved path (e.g. `extend SymbolEnum`
        // from a DSL this emitter itself expands, which never had a
        // source span `written_names` could resolve).
        // A rooted `::M` names the top-level M only: drop the empty
        // leading segment for the lookup and skip the nested guess.
        let rooted = path.first().is_some_and(|s| s.as_str().is_empty());
        let written = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
        let nested = if rooted { written.clone() } else { format!("{}::{written}", lc.name.0.as_str()) };
        match NameIndex::current(app).filter(|index| index.covers(classes)) {
            Some(index) => index.library_classes.get(&written).into_iter().flatten()
                .chain(index.library_classes.get(&nested).into_iter().flatten().filter(|_| !rooted))
                .copied()
                .filter(|&j| {
                    let c = &classes[j];
                    c.is_module && c.origin.is_none()
                })
                .min()
                .map(|j| &classes[j]),
            None => classes.iter().find(|c| {
                c.is_module && c.origin.is_none() && (c.name.0.as_str() == written || c.name.0.as_str() == nested)
            }),
        }
    };
    let mut extended: Vec<&LibraryClass> = Vec::new();
    let mut out = Vec::new();
    for call in &lc.unknown_calls {
        let ExprNode::Send { recv: None, method, args, .. } = &*call.node else { continue };
        // `prepend M` replays the same way `extend M` does — the
        // module is app-only and not otherwise modelled — but, unlike
        // `extend`, does not put M's methods on the class side, so it
        // does not feed the bare-macro-call follow-on below.
        if matches!(method.as_str(), "extend" | "prepend") {
            let modules: Option<Vec<&LibraryClass>> = args
                .iter()
                .map(|a| match &*a.node {
                    ExprNode::Const { path } => app_module(path),
                    _ => None,
                })
                .collect();
            if let Some(modules) = modules.filter(|m| !m.is_empty()) {
                if method.as_str() == "extend" {
                    extended.extend(modules);
                }
                out.push(call);
            }
            continue;
        }
        let defines = |c: &LibraryClass| {
            c.methods.iter().any(|m| m.name == *method && m.receiver == MethodReceiver::Instance)
        };
        if extended.iter().any(|m| defines(m)) {
            out.push(call);
        }
    }
    out
}

/// Class-body macros a module of a gem the Ruby family ships gives its
/// includer (`include ElasticAPM::SpanHelpers`, then `span_method :call`).
/// The gem is loaded on this tree, so the call is what Ruby runs. A target
/// that refuses the gem drops the include first, so nothing replays there.
const GEM_INCLUDE_MACROS: &[(&str, &[&str])] =
    &[("ElasticAPM::SpanHelpers", &["span_method", "span_class_method"])];

fn gem_include_macro_calls(lc: &LibraryClass) -> Vec<&Expr> {
    let macros: Vec<&str> = GEM_INCLUDE_MACROS
        .iter()
        .filter(|(module, _)| lc.includes.iter().any(|id| id.0.as_str() == *module))
        .flat_map(|(_, macros)| macros.iter().copied())
        .collect();
    lc.unknown_calls
        .iter()
        .filter(|call| matches!(&*call.node, ExprNode::Send { recv: None, method, .. } if macros.contains(&method.as_str())))
        .collect()
}

/// A receiverless class-body call that assigns a constant inside its block,
/// and whose method this class or an ancestor defines. Running the call is
/// what creates the constant; the ancestor check keeps a framework macro
/// (`queue_as`, `included`) from being replayed just because a block wrote
/// a name.
fn constant_defining_class_call(app: &App, classes: &[LibraryClass], lc: &LibraryClass, call: &Expr) -> bool {
    if lc.is_module {
        return false;
    }
    let ExprNode::Send { recv: None, method, .. } = &*call.node else { return false };
    block_assigns_bare_constant(call) && singleton_method_visible(app, classes, lc, method)
}

fn block_assigns_bare_constant(call: &Expr) -> bool {
    let ExprNode::Send { block: Some(block), .. } = &*call.node else { return false };
    let ExprNode::Lambda { body, .. } = &*block.node else { return false };
    let stmts: &[Expr] = match &*body.node {
        ExprNode::Seq { exprs } => exprs,
        _ => std::slice::from_ref(body),
    };
    stmts.iter().any(|stmt| match &*stmt.node {
        ExprNode::Assign { target: LValue::Const { path }, .. } => path.len() == 1,
        ExprNode::Send { .. } => block_assigns_bare_constant(stmt),
        _ => false,
    })
}

/// An ancestor's class method. This fragment's own `def self` is emitted
/// with the methods, after this call, so it is not loaded yet. Another
/// reopen of this class is a different file.
fn singleton_method_visible(app: &App, classes: &[LibraryClass], lc: &LibraryClass, method: &Symbol) -> bool {
    let defines = |m: &MethodDef| m.receiver == MethodReceiver::Class && m.name == *method;
    let mut current = lc.parent.clone();
    let mut seen = BTreeSet::new();
    seen.insert(lc.name.clone());
    while let Some(id) = current {
        if !seen.insert(id.clone()) {
            return false;
        }
        // The require loads the ancestor's anchor file, the first fragment.
        if let Some(class) = classes.iter().find(|class| class.name == id) {
            if class.methods.iter().any(defines) {
                return true;
            }
            current = class.parent.clone();
            continue;
        }
        if let Some(model) = app.models.iter().find(|model| model.name == id) {
            if model.methods().any(defines) {
                return true;
            }
            current = model.parent.clone();
            continue;
        }
        return false;
    }
    false
}

/// Ledger a class-body call we captured but chose not to replay. The
/// emit diagnostic sink is live here (ingest's is not — it runs before
/// any `diagnostics::scope`), which is why the judgment happens at emit
/// rather than where the call was captured.
fn report_dropped_class_body_call(lc: &LibraryClass, call: &Expr) {
    let name = match &*call.node {
        ExprNode::Send { method, .. } => method.as_str().to_string(),
        _ => "call".to_string(),
    };
    crate::emit::diagnostics::push(crate::lower::residue_diagnostic(
        "library_class_body",
        &name,
        call.span,
        "not modelled",
        format!(
            "`{}` in the body of `{}` is not modelled and is dropped; only a class \
             extending a base roundhouse does NOT model (a gem's, whose DSL is the \
             class) replays its class body verbatim",
            name,
            lc.name.0.as_str(),
        ),
    ));
}

fn is_preserved_external_include(call: &Expr) -> bool {
    matches!(
        &call.diagnostic(),
        Some(crate::diagnostic::DiagnosticKind::Unsupported {
            construct,
            detail,
            ..
        }) if construct.as_str() == "include"
            && external_include_is_enabled(detail)
            && !super::expr::external_include_features(detail).is_empty()
    )
}

fn external_include_is_enabled(detail: &str) -> bool {
    let Some(module) = detail.split(" includes unresolved ").nth(1) else {
        return false;
    };
    let namespace = module.split("::").next().unwrap_or(module);
    super::expr::external_gem_namespace_is_enabled(namespace)
}

/// Is `inc` one of `lc`'s includes the analyzer could not resolve? Scans
/// `unknown_calls` for the matching refusal `Analyzer::analyze` pushes
/// (`"{class} includes unresolved {module}"`, `analyze/mod.rs`). Only
/// consulted inside futamura's box (`FUTAMURA_BOX`) to decide whether
/// this one `include` line needs guarding; every other include stays
/// plain.
fn include_is_unresolved(lc: &LibraryClass, inc: &ClassId) -> bool {
    let detail = format!("{} includes unresolved {}", lc.name.0, inc.0);
    lc.unknown_calls.iter().any(|call| {
        matches!(&call.diagnostic(),
            Some(crate::diagnostic::DiagnosticKind::Unsupported { construct, detail: d, .. })
            if construct.as_str() == "include" && *d == detail)
    })
}

/// Is this name one of the app's own CLASSES (not a module)? Namespace
/// segments that name one must reopen as `class`, not `module`.
/// The file a class belongs in: the OUTERMOST enclosing app class, or
/// the class itself when no enclosing segment is one.
///
/// A nested class used to get a file of its own, which meant every
/// part-file re-opened the outer class and CLOSED it again — and
/// `TracePoint(:end)`, which is how a gem implements "end of class
/// body", fires on each of those closes. A gem that validates there
/// saw a body missing what the parent's own file sets. Naming the same
/// superclass in both places (see `outer_header`) makes the ORDER
/// irrelevant; it cannot make the COUNT of body-ends one.
///
/// Outermost rather than nearest: `A::B::C::D` with both `A::B` and
/// `A::B::C` classes belongs in `A::B`'s file, because that is the
/// body whose end must happen once.
pub(super) fn file_owner(name: &str, app: &App, classes: &[LibraryClass]) -> String {
    if library_classes_named(name, app, classes).iter().any(|class|
        matches!(class.origin, Some(LibraryClassOrigin::DataFactory { .. })))
    {
        return file_owner(name.rsplit_once("::").unwrap().0, app, classes);
    }
    let segments: Vec<&str> = name.split("::").collect();
    for i in 0..segments.len().saturating_sub(1) {
        let prefix = segments[..=i].join("::");
        if owns_a_file(&prefix, app, classes) {
            return prefix;
        }
    }
    name.to_string()
}

/// A class that can absorb its nested classes: a LIBRARY class, not a
/// model.
///
/// Models are emitted by a different pipeline, so there is nothing for
/// a nested class to be spliced into and it keeps its own file. The
/// emit already knew that; the path resolver did not, and answered
/// "same file, no require needed" for `Account::Joinable` nested in
/// the MODEL `Account` — whose file then referenced a constant nothing
/// had loaded. Both sides ask this now.
fn owns_a_file(name: &str, app: &App, classes: &[LibraryClass]) -> bool {
    library_classes_named(name, app, classes).iter().any(|c| !c.is_module)
}

/// Every constant path the app declares (models, controllers, library
/// classes and modules, test modules) and each namespace enclosing one.
fn app_constant_paths(app: &App, classes: &[LibraryClass]) -> std::rc::Rc<crate::det::HashSet<String>> {
    let build = || {
        #[cfg(test)]
        lookup_tests::CONSTANT_PATH_BUILDS.with(|n| n.set(n.get() + 1));
        let mut out: crate::det::HashSet<String> = crate::det::HashSet::default();
        let names = app.models.iter().map(|m| m.name.0.as_str())
            .chain(app.controllers.iter().map(|c| c.name.0.as_str()))
            .chain(classes.iter().map(|c| c.name.0.as_str()))
            .chain(app.test_modules.iter().map(|t| t.name.0.as_str()));
        for name in names {
            let name = name.trim_start_matches("::");
            let mut end = 0;
            while let Some(at) = name[end..].find("::") {
                out.insert(name[..end + at].to_string());
                end += at + 2;
            }
            out.insert(name.to_string());
        }
        std::rc::Rc::new(out)
    };
    match NameIndex::current(app).filter(|index| index.covers(classes)) {
        Some(index) => index.constant_paths.get_or_init(build).clone(),
        None => build(),
    }
}

/// Does the app declare the constant `path`, or a namespace enclosing a
/// declaration (models, controllers, library classes and modules, test
/// modules)?
fn declares_constant_path(path: &str, app: &App, classes: &[LibraryClass]) -> bool {
    if NameIndex::current(app).is_some_and(|index| index.covers(classes)) {
        return app_constant_paths(app, classes).contains(path);
    }
    let names = app.models.iter().map(|m| m.name.0.as_str())
        .chain(app.controllers.iter().map(|c| c.name.0.as_str()))
        .chain(classes.iter().map(|c| c.name.0.as_str()))
        .chain(app.test_modules.iter().map(|t| t.name.0.as_str()));
    names.into_iter().any(|name| {
        let name = name.trim_start_matches("::");
        name == path || name.strip_prefix(path).is_some_and(|rest| rest.starts_with("::"))
    })
}

/// An app constant `name` as a class header's superclass or a body's
/// mixin must spell it, given the lexical `scopes` (innermost last) Ruby
/// searches first: `::Api::Base` inside `module Admin; module Api`, where
/// a bare `Api` is `Admin::Api`. Ingest stores the absolute name Rubydex
/// resolved, without its `::`, and the emitted file nests every segment
/// of the class it declares, so a source class written compact (`class
/// Billing::Receipt < Record`, which is `::Record`) is emitted inside
/// `module Billing` where a bare `Record` would be `Billing::Record`. A
/// name the app does not declare (a runtime or gem class) stays as
/// written.
fn lexically_qualified(name: &str, scopes: &[&str], app: &App, classes: &[LibraryClass]) -> String {
    if name.starts_with("::") || scopes.is_empty() {
        return name.to_string();
    }
    let first = name.split("::").next().unwrap_or(name);
    let shadowed = (1..=scopes.len())
        .rev()
        .map(|i| scopes[..i].join("::"))
        .any(|scope| declares_constant_path(&format!("{scope}::{first}"), app, classes));
    if !shadowed || !declares_constant_path(name, app, classes) {
        return name.to_string();
    }
    format!("::{name}")
}

fn is_app_class(name: &str, app: &App, classes: &[LibraryClass]) -> bool {
    is_model_named(name, app)
        || is_controller_named(name, app)
        || library_classes_named(name, app, classes).iter().any(|c| !c.is_module)
}

/// The declared superclass of an app class named by a namespace
/// segment, so a nested file can reopen it with the same parent.
fn outer_class_parent(name: &str, app: &App, classes: &[LibraryClass]) -> Option<ClassId> {
    if is_model_named(name, app) {
        if let Some(m) = app.models.iter().find(|m| m.name.0.as_str() == name) {
            return m.parent.clone();
        }
    }
    if is_controller_named(name, app) {
        if let Some(c) = app.controllers.iter().find(|c| c.name.0.as_str() == name) {
            return c.parent.clone();
        }
    }
    library_classes_named(name, app, classes).into_iter()
        .find(|c| !c.is_module).and_then(|c| c.parent.clone())
}

fn require_path_for_parent(parent: &ClassId, app: &App, classes: &[LibraryClass]) -> Option<String> {
    let raw = parent.0.as_str();
    if raw == "ActiveRecord::Base" {
        // When the app REOPENS ActiveRecord::Base (lobsters' `q`
        // monkeypatch, emitted as a library class), route the parent
        // require through the reopen file — it requires the runtime
        // itself first, so subclasses see both the framework methods
        // and the app's additions. Without this the reopen dangles:
        // nothing else in the require graph names it.
        if !library_classes_named("ActiveRecord::Base", app, classes).is_empty()
        {
            return Some("app/models/active_record/base".to_string());
        }
        return Some("runtime/active_record".to_string());
    }
    if raw == "ActionController::Base" || raw == "ActionController::API" {
        return Some("runtime/action_controller".to_string());
    }
    if raw == "ActionMailer::Base" {
        return Some("runtime/action_mailer".to_string());
    }
    if raw == "ActiveJob::Base" {
        return Some("runtime/active_job".to_string());
    }
    // `ApplicationCable::Channel < ActionCable::Channel::Base` and its
    // Connection twin — the two roots of an ingested `app/channels/`
    // tree. Both are a SUPERCLASS reference, which is load-time, so a
    // missing require is not a lazy failure: the file does not define.
    if raw == "ActionCable::Channel::Base" || raw == "ActionCable::Connection::Base" {
        return Some("runtime/action_cable".to_string());
    }
    // `LogScrubbingFormatter < ::Logger::Formatter` — campfire's, and
    // the same load-time rule the two cable roots above state. The
    // leading `::` is part of the spelling an app writes, so both
    // forms resolve.
    if raw == "Logger::Formatter" || raw == "::Logger::Formatter" {
        return Some("runtime/logger".to_string());
    }
    if is_model_or_library_class(raw, app, classes)
    {
        // underscore: namespaced parents nest (see emit_library_class_decls).
        let owner = file_owner(raw, app, classes);
        return Some(format!("app/models/{}", crate::naming::underscore(&owner)));
    }
    if is_controller_named(raw, app) {
        return Some(format!("app/controllers/{}", crate::naming::underscore(raw)));
    }
    // A SUPERCLASS that is a gem's class — lobsters' `TimeSeries <
    // SVG::Graph::TimeSeries`, campfire's `ApplicationPlatform <
    // PlatformAgent`. Both used to emit no require at all and load only
    // because boot.rb happened to have pulled the gem in first; a body
    // reached from the test harness, which builds its own require
    // chain, saw an undefined superclass and the file did not define.
    // `PlatformAgent` is the same shape and no longer a façade:
    // `runtime/ruby/user_agent.rb` PORTS it, so the anchor is that file
    // rather than the façade host. It needs its own arm precisely
    // because this resolver reads `is_gem_facade_root` and nothing
    // else — dropping the pair from that list to port them took the
    // anchor with it, and campfire's ruby lane went 256/288 to 0/288 on
    // `uninitialized constant PlatformAgent` at load. The spinel lane
    // stayed green throughout: its `main.rb` requires every runtime
    // file eagerly, so the constant happened to exist by the time
    // `application_platform.rb` ran. Exactly the accident the comment
    // above says this arm exists to stop.
    if raw == "PlatformAgent" || raw == "UserAgent" {
        return Some("runtime/user_agent".to_string());
    }
    if let Some(root) = raw.split("::").next() {
        if is_gem_facade_root(root) {
            return Some("runtime/gem_facades".to_string());
        }
    }
    None
}

/// Roots hosted by `runtime/ruby/gem_facades.rb` — the one file that
/// stands in for every stubbed gem, so all their roots anchor there.
/// On a ruby-family tree `project.rs` rewrites that file into the
/// guarded-require block, and the same anchor loads the real gems.
///
/// `OpenTelemetry` is the one name here that is not a stubbed GEM — its
/// façade never stands aside for a real `opentelemetry-api`/`-sdk` (see
/// `runtime/ruby/open_telemetry_facade.rb`) — but it is still hosted off
/// this same anchor file (`gem_facades.rb`'s own `require_relative`, and
/// the per-flavor rewrite's matching one), so a body naming it resolves
/// the same way.
fn is_gem_facade_root(root: &str) -> bool {
    matches!(
        root,
        "Markly"
            | "Nokogiri"
            | "Mail"
            | "ROTP"
            | "BCrypt"
            | "RQRCode"
            | "SVG"
            | "OpenTelemetry"
    )
}

/// Core Ruby classes an app may reopen (monkeypatch) without them being
/// `app/models` files. Kept in sync with `rbs::is_builtin_class_name`.
fn is_core_class_name(name: &str) -> bool {
    matches!(
        name,
        "Integer"
            | "Float"
            | "String"
            | "Symbol"
            | "TrueClass"
            | "FalseClass"
            | "NilClass"
            | "Array"
            | "Hash"
            | "Object"
            | "Numeric"
            | "Comparable"
            | "Enumerable"
            | "Kernel"
    )
}

/// Project-root-anchored require target for a body-referenced constant.
/// `Views::<Plural>` resolves to `app/views/<plural>/_<singular>`; runtime
/// modules resolve to `runtime/<x>`. The caller relpaths the result against
/// the requirer's `out_dir`, so a single mapping serves every output kind.
/// Same-dir siblings (other models, library_classes) drop because Ruby's
/// load path covers them; unknowns drop silently.
fn require_path_for_body_const(
    path: &[String],
    app: &App,
    classes: &[LibraryClass],
    self_name: &str,
) -> Option<String> {
    // A ROOTED head (`::Broadcasts`, `::Sound::Image`) names the same
    // file as the bare one — the marker says where Ruby should look, not
    // what to load. Strip it before resolving, or rooting a reference
    // silently deletes its require and the constant is undefined at load
    // for a different reason than the one rooting fixed.
    // Ingest marks the root either way: as a `::X` head or as an empty
    // leading segment (`::Logger::Formatter` → ["", "Logger", "Formatter"]).
    let path = match path.split_first() {
        Some((head, rest)) if head.is_empty() => rest,
        _ => path,
    };
    let rooted;
    let path: &[String] = match path.first().and_then(|f| f.strip_prefix("::")) {
        Some(bare) => {
            rooted = std::iter::once(bare.to_string())
                .chain(path.iter().skip(1).cloned())
                .collect::<Vec<_>>();
            &rooted
        }
        None => path,
    };
    let first = path.first()?;
    // A captured boot store is a value on Object, whose builtin class
    // needs no require but whose carried initializer does.
    if first == "Object" && path.len() > 1 && self_name != "Object"
        && app.library_classes.iter().any(|class| class.name.0.as_str() == "Object"
            && class.constants.iter().any(|(name, _)| name.as_str() == path[1]))
    {
        return Some("app/models/object".to_string());
    }
    // A nested class of THIS class is still a different file:
    // `Sound::Image` lives at app/models/sound/image.rb even though the
    // reference's first segment is `Sound`. Resolve the full path before
    // the self-reference short-circuit below.
    let defined = |name: &str| is_model_or_library_class(name, app, classes);
    // A value constant (`Shop::Types::LABEL`) lives in the file of the
    // longest prefix that is a class or module (`Shop::Types`), which
    // neither the whole path nor its first segment (`Shop`, a bare
    // namespace) names.
    let joined = path.join("::");
    let holder = (1..=path.len())
        .rev()
        .map(|n| path[..n].join("::"))
        .find(|prefix| prefix.contains("::") && defined(prefix))
        .unwrap_or_else(|| joined.clone());
    if holder != self_name && defined(&holder) {
        // The file that DEFINES it, which for a nested class is its
        // parent's — and when that is the file being emitted, there is
        // nothing to require: the class is already in it.
        let owner = file_owner(&holder, app, classes);
        if owner == self_name {
            return None;
        }
        return Some(format!("app/models/{}", crate::naming::underscore(&owner)));
    }
    // A bare reference inside a namespace resolves LEXICALLY first:
    // campfire's `module ContentFilters` builds
    // `TextMessagePresentationFilters = …new(RemoveSoloUnfurledLinkText,
    // …)` at LOAD time, and that name is
    // `ContentFilters::RemoveSoloUnfurledLinkText` — a sibling file that
    // the aggregator requires AFTER this one (`x.rb` sorts before
    // `x/y.rb`). Without the require the constant is uninitialized and
    // the whole app fails to load, so resolve the qualified form before
    // giving up on the bare one.
    let lexical = format!("{self_name}::{joined}");
    if !self_name.is_empty() && is_model_or_library_class(&lexical, app, classes)
    {
        let owner = file_owner(&lexical, app, classes);
        if owner == self_name {
            return None;
        }
        return Some(format!("app/models/{}", crate::naming::underscore(&owner)));
    }
    if first == self_name {
        return None;
    }
    // A core Ruby class an app reopens (e.g. `class String` in
    // lib/monkey.rb) lands in `library_classes`, but it is NOT an
    // `app/models/<name>` file — every `String.new` would otherwise emit
    // a dangling `require_relative "app/models/string"`. The reference
    // resolves to the builtin; any monkeypatch is loaded via its own file
    // (lib/), not through this model anchor.
    if is_core_class_name(first) {
        return None;
    }
    if is_model_or_library_class(first, app, classes)
    {
        return Some(format!("app/models/{}", crate::naming::underscore(first)));
    }
    if is_controller_named(first, app) {
        return Some(format!("app/controllers/{}", crate::naming::underscore(first)));
    }
    // Qualified runtime names, resolved on the FULL path — `ActiveSupport`
    // is a namespace we ship several unrelated pieces of, so anchoring on
    // its root segment would over-require. A fixture loader is the first
    // body to need this: `created_at: <%= 1.hour.ago %>` grounds to
    // `ActiveSupport::Duration.hour(1)` and `test/fixtures/<x>.rb` is
    // reached from the test harness, not from main.rb's require chain.
    if joined == "ActiveSupport::SecurityUtils" {
        return Some("runtime/security_utils".to_string());
    }
    if joined == "ActiveSupport::Duration" {
        return Some("runtime/active_support_duration".to_string());
    }
    match first.as_str() {
        // `Views::*` refs always go through the per-app aggregator at
        // `app/views.rb` (spinel-blog convention; loads all view
        // modules so any `Views::X.method` resolves regardless of
        // which template the method lives in). Per-template requires
        // would be wrong because the same `Views::X` const can host
        // methods from multiple files (`_article.rb`, `index.rb`,
        // `show.rb` all re-open `module Views::Articles`).
        "Views" => Some("app/views".to_string()),
        // Runtime modules under `runtime/`. ViewHelpers still ships
        // hand-written; RouteHelpers is now generated into
        // `app/route_helpers.rb` from `app.routes` so consumers
        // resolve there. Add entries as lowerings introduce new ones;
        // unknown idents silently drop.
        "Broadcasts" => Some("runtime/broadcasts".to_string()),
        // `ActionCable.server.broadcast(stream, payload)` — the
        // low-level publish API, and the two bases `app/channels/*.rb`
        // subclasses. Sibling of Broadcasts, not a replacement: the
        // Turbo Stream family still goes through `Broadcasts.append`.
        "ActionCable" => Some("runtime/action_cable".to_string()),
        "Logger" => Some("runtime/logger".to_string()),
        // `ActionText::Content` / `ActionText::Attachment` — the value
        // half of Action Text. `ActionText::RichText` is NOT here: it
        // is a lowered model and resolves under `app/models/` like any
        // other, which is exactly the split this entry preserves.
        "ActionText" => Some("runtime/action_text".to_string()),
        // `ActiveStorage::Attached` — the value half, what a
        // `has_one_attached` reader returns. `ActiveStorage::Attachment`
        // is NOT here for the same reason `ActionText::RichText` is
        // not: it is a lowered model under `app/models/`.
        "ActiveStorage" => Some("runtime/active_storage".to_string()),
        // `Turbo::Streams::StreamName::ClassMethods`, INCLUDED by a
        // channel that guards its own stream (campfire's
        // `RoomMessagesChannel`). An include runs at class-definition
        // time, so a missing anchor here is a tree that does not boot.
        "Turbo" => Some("runtime/turbo_streams".to_string()),
        "Inflector" => Some("runtime/inflector".to_string()),
        // `IPAddr` — ported into `runtime/ruby/ipaddr.rb` because the
        // strict targets have no stdlib to reach for. Anchored for
        // every target, ruby family included: `ipaddr` is not in
        // `project::BUNDLED`, so nothing inserts a bare
        // `require "ipaddr"` that would reopen this class with the
        // stdlib's own `initialize` and leave the predicates reading a
        // slot nobody filled. One IPAddr per tree: ours on the strict
        // targets, and on the ruby family the stdlib's under the same
        // path with the port's `octets` reopened onto it
        // (`project::IPADDR_STDLIB`) — so runtime code reads octets
        // everywhere and never `to_i`.
        "IPAddr" => Some("runtime/ipaddr".to_string()),
        // `Rack::Utils` — rack's encoding negotiation, ported into
        // `runtime/ruby/rack_utils.rb` for spinel and the rack gem's own
        // on the ruby family (`project::ruby_runtime_files`). Anchored so
        // a file that names it loads the one its tree has.
        "Rack" => Some("runtime/rack_utils".to_string()),
        // `SQLite3::Database` — the gem on the ruby family, the FFI port
        // (`runtime/spinel/sqlite3_database.rb`) on spinel; one require
        // path either way.
        "SQLite3" => Some("runtime/sqlite3_database".to_string()),
        // `Resolv` — Ruby's resolver, ported into
        // `runtime/ruby/resolv.rb` for the targets that have none and
        // swapped for the stdlib on the ruby family. Anchored for the
        // same reason IPAddr is: `resolv` is not in `project::BUNDLED`,
        // so nothing inserts a bare `require "resolv"` of its own.
        "Resolv" => Some("runtime/resolv".to_string()),
        // `Tempfile` — anchored for the same reason IPAddr and Resolv
        // are, and it was MISSING: the port landed in `spinel_files`
        // with no entry here, so nothing in any emitted tree ever
        // required `runtime/tempfile` and the file shipped dead. The
        // interpreted lane hid it — under CRuby something in the bundle
        // loads the stdlib's `tempfile` on its own, so the constant
        // resolved without the anchor — while on spinel the name went
        // into inference unresolved, which is a failure far from its
        // cause rather than a missing require. matz/spinel#4811 added
        // `Tempfile` to spinel's own missing-require table and the
        // diagnostic named it at campfire's
        // `vips_loader_policy_test.rb:163`.
        "Tempfile" => Some("runtime/tempfile".to_string()),
        // `Net::HTTP` — a real client on both lanes (`project::BUNDLED`
        // writes the `require "net/http"`), REOPENED on spinel by
        // `runtime/spinel/net_http.rb` to add the block form of
        // `#request` and the stub seam `lower::webmock` fills. Anchored
        // so every file that names the client loads the reopen; the ruby
        // family's copy of the file is a bare `require "net/http"`.
        "Net" => Some("runtime/net_http".to_string()),
        // `WebPush` — the web-push gem. On spinel, runtime/spinel/
        // web_push.rb: the façade's stub slot plus a port of the gem's
        // delivery over the openssl package. On the ruby family the
        // file is the façade alone, which guarded-requires the real
        // gem. Anchored rather than left to load order: campfire's
        // `WebPush::Notification` names the module without requiring
        // anything, and the port is what defines `WebPush::Request`,
        // the class its initializer prepends the SSRF guard onto.
        "WebPush" => Some("runtime/web_push".to_string()),
        // `Surfguard` — basecamp/surfguard's address policy, ported into
        // `runtime/ruby/surfguard.rb`. Anchored for every target: the
        // gem is a GIT dependency, so no `gem install` and no bare
        // `require` reaches it on the ruby family either. campfire's
        // `RestrictedHTTP::PrivateNetworkGuard` is a two-line shim over
        // it, and `Push::Subscription` names `Surfguard::Unresolvable`
        // in a rescue clause — a NameError there took the whole suite
        // to 0 of 288, because fixtures save through the validation
        // that reaches it.
        "Surfguard" => Some("runtime/surfguard".to_string()),
        // `Zlib` — ported into `runtime/ruby/zlib.rb`. Anchored for
        // every target for the same reason IPAddr is: `zlib` is not in
        // `project::BUNDLED`, so nothing writes a bare `require "zlib"`
        // and the constant would resolve to nothing at all.
        "Zlib" => Some("runtime/zlib".to_string()),
        // `Timeout` — ported into `runtime/ruby/timeout.rb` for spinel,
        // swapped for Ruby's own on the ruby family (`require
        // "timeout"`). Anchored for the same reason Zlib is: on spinel
        // nothing else loads the port, and campfire's unfurl controller
        // names the constant without a require.
        "Timeout" => Some("runtime/timeout".to_string()),
        // `TypeID` — ported into `runtime/ruby/typeid.rb`, swapped for the
        // gem on the ruby family. Anchored so the model that names it
        // (lobsters' Token concern, spliced into 18 models) loads it.
        "TypeID" => Some("runtime/typeid".to_string()),
        // `Concurrent` — concurrent-ruby's pools and barrier, ported into
        // `runtime/spinel/concurrent.rb` over spinel's own threads and
        // swapped for the gem on the ruby family. Anchored for the same
        // reason Zlib is: nothing else writes a require for it.
        "Concurrent" => Some("runtime/concurrent".to_string()),
        // `Mime::Type` — actionpack's registry, ported into
        // `runtime/ruby/mime.rb`. Anchored for every target: the
        // constant is actionpack's, not the stdlib's, so no bare
        // `require` reaches it on the ruby family either.
        "Mime" => Some("runtime/mime".to_string()),
        // `I18n` — the i18n gem's locale accessors, in
        // `runtime/ruby/i18n_locale.rb` (not `i18n.rb`, which would shadow the
        // gem wherever `runtime/ruby` is on the load path). No Rails is
        // loaded on these trees, so nothing else defines the constant.
        "I18n" => Some("runtime/i18n_locale".to_string()),
        // `useragent` + `platform_agent`, PORTED into
        // `runtime/ruby/user_agent.rb` — they were façades that raised
        // until campfire's room page turned out to render all three PWA
        // partials, each of which asks `ApplicationPlatform` what
        // browser it is. Anchored for every target, and it has to be
        // anchored rather than lazily required: campfire's
        // `ApplicationPlatform < PlatformAgent` names its superclass at
        // LOAD time, so a missing anchor is not a lazy failure — the
        // tree does not boot.
        "UserAgent" | "UserAgentToken" | "PlatformAgent" => {
            Some("runtime/user_agent".to_string())
        }
        "ViewHelpers" => Some("runtime/action_view".to_string()),
        "RouteHelpers" => Some("app/route_helpers".to_string()),
        // Gem façades — typed raising stand-ins for write-path-only
        // gem surface (see runtime/ruby/gem_facades.rb). One file
        // hosts every stubbed gem, so all their roots anchor here.
        _ if is_gem_facade_root(first.as_str()) => Some("runtime/gem_facades".to_string()),
        _ => None,
    }
}

/// Compute a `require_relative`-style relative path from `from_dir` to
/// the project-root-anchored `to_anchor`. Both inputs are slash-separated;
/// the result has no `.rb` extension because `require_relative` doesn't
/// need one.
fn relpath(from_dir: &Path, to_anchor: &str) -> String {
    let from_parts: Vec<&str> = from_dir
        .to_str()
        .unwrap_or("")
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let to_parts: Vec<&str> = to_anchor.split('/').filter(|s| !s.is_empty()).collect();
    // The anchor's LAST segment is a file, never a shared directory:
    // `app/models/content_filters/sanitize_tags.rb` requiring its own
    // namespace module `app/models/content_filters` matched every
    // segment and wrote `require_relative ""` instead of
    // "../content_filters".
    let common = from_parts
        .iter()
        .zip(&to_parts[..to_parts.len().saturating_sub(1)])
        .take_while(|(a, b)| a == b)
        .count();
    let ups = from_parts.len() - common;
    let mut parts: Vec<&str> = std::iter::repeat("..").take(ups).collect();
    parts.extend(&to_parts[common..]);
    parts.join("/")
}

pub(super) fn walk_const_paths(e: &Expr, out: &mut BTreeSet<Vec<String>>) {
    if let ExprNode::Const { path } = &*e.node {
        out.insert(path.iter().map(|s| s.as_str().to_string()).collect());
    }
    e.node.for_each_child(&mut |child| walk_const_paths(child, out));
}

// has_secure_password synthesis moved to the shared model lowering
// (`lower::secure_password::push_secure_password_methods`) — every
// target's model classes now carry authenticate + the plaintext
// accessors, in the bcrypt gem's own contract shape
// (`BCrypt::Password.create/new`): the CRuby/JRuby trees load the
// real gem (guarded require in the overlay main.rb), and a future
// spinel-bcrypt spin package satisfies the same calls.

fn sp_expr(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

fn ivar_read(name: &Symbol) -> Expr {
    sp_expr(ExprNode::Ivar { name: name.clone() })
}

// Request-params key normalization moved to the Ruby expr emitter's
// type-directed index hook (`emit::ruby::expr`): a symbol/dynamic key on
// a statically string-keyed hash (`Hash[String, _]`) is coerced to a
// string at the single `[]` emit chokepoint. That gates on the receiver
// *type* rather than a `params` name heuristic, so it covers views and
// helpers (params flows there too) and never touches a genuine
// symbol-keyed `Hash[Symbol, _]` like `StoryRepository#@params`.

// typed_store accessor synthesis moved to the shared model lowering
// (`lower::typed_store::push_typed_store_methods`) — every target's
// model classes now carry the reader/predicate/writer methods; the
// `TypedStore` runtime module (YAML seam) still ships only on the
// CRuby/JRuby overlay trees.

// ── boolean-column cast lowering ─────────────────────────────────────

/// Ruby-family pre-emit pass: boolean-column readers and `<col>?`
/// predicates cast the stored value instead of returning it raw. The
/// CRuby sqlite adapter hydrates boolean columns as the Integers
/// SQLite stores (0/1) — and `0` is TRUTHY in Ruby, so a plain `@col`
/// read makes every `user.is_admin?` guard pass for non-admins.
/// Rewritten body: `@col == true || @col == 1` (handles both a
/// DB-hydrated Integer and an app-assigned true/false; nil/0/false →
/// false). An ordinary nullable getter keeps nil; its `?` predicate
/// still answers false. Strict targets hydrate native booleans and keep
/// the shared synthesized shape. Only plain `@col`-read bodies are rewritten
/// (idempotent; custom bodies win).
pub(crate) fn apply_boolean_lowering(lcs: &mut [LibraryClass], app: &App) {
    for model in &app.models {
        let Some(table) = app.schema.tables.get(&model.table.0) else {
            continue;
        };
        let bool_cols: HashMap<Symbol, bool> = table
            .columns
            .iter()
            .filter(|c| matches!(c.col_type, crate::schema::ColumnType::Boolean))
            .map(|c| (c.name.clone(), c.nullable && !c.primary_key))
            .collect();
        if bool_cols.is_empty() {
            continue;
        }
        let Some(lc) = lcs.iter_mut().find(|lc| lc.name == model.name) else {
            continue;
        };
        for m in &mut lc.methods {
            if m.receiver != MethodReceiver::Instance {
                continue;
            }
            let col = Symbol::from(m.name.as_str().trim_end_matches('?'));
            let Some(nullable) = bool_cols.get(&col) else {
                continue;
            };
            if is_plain_ivar_read(&m.body, &col) {
                let cast = boolean_cast_body(&col);
                m.body = if *nullable && !m.name.as_str().ends_with('?') {
                    sp_expr(ExprNode::If {
                        cond: sp_expr(ExprNode::Send {
                            recv: Some(ivar_read(&col)),
                            method: Symbol::from("nil?"),
                            args: vec![],
                            block: None,
                            parenthesized: false,
                        }),
                        then_branch: sp_expr(ExprNode::Lit { value: Literal::Nil }),
                        else_branch: cast,
                    })
                } else {
                    cast
                };
            }
        }
    }
}

/// Ruby-family pre-emit pass: the public writer of an integer, float,
/// decimal or boolean column casts what it is given, as ActiveModel's
/// typed attribute does (`record.min_favs = "5"` reads back `5`,
/// `= ""` reads nil, `active = "1"` true). Before, the writer stored the
/// value as given, so a request param reached every validator, comparison
/// and serializer as a String (#1158). `ActiveSupport.cast_integer` /
/// `cast_float` / `cast_decimal` / `cast_boolean` implement ActiveModel
/// 8.1.4's casts; a decimal column is a Float here, cast as `Type::Decimal`
/// casts (a Symbol is 0.0 there, where `Type::Float` raises). Hydration passes
/// values already of the column's type (`Db.column_int_opt`,
/// `row.x&.to_i`), on which each cast is the identity. Strict targets
/// type the writer's parameter and are not rewritten.
///
/// Only the synthesized writer is rewritten (`@col = <the value>`, perhaps
/// through `normalizes`, which then runs on the cast value as in Rails),
/// also when a model's own `col=` renamed it `_write_<col>_raw`, so its
/// `super` casts too. An enum column's writer maps labels and is left
/// alone, as is the primary key's.
pub(crate) fn apply_typed_writer_cast_lowering(lcs: &mut [LibraryClass], app: &App) {
    for model in &app.models {
        let table = app.schema.tables.get(&model.table.0);
        let mut casts: HashMap<Symbol, &'static str> = table
            .map(|table| {
                table
                    .columns
                    .iter()
                    // A stored array column's writer takes the Array; its
                    // elements are not cast (`lower::array_columns`).
                    .filter(|c| !c.primary_key && !c.array_stored && !model.enums.contains_key(&c.name))
                    .filter_map(|c| {
                        let cast = match c.col_type {
                            crate::schema::ColumnType::Integer | crate::schema::ColumnType::BigInt => "cast_integer",
                            crate::schema::ColumnType::Float => "cast_float",
                            crate::schema::ColumnType::Decimal { .. } => "cast_decimal",
                            crate::schema::ColumnType::Boolean => "cast_boolean",
                            _ => return None,
                        };
                        Some((c.name.clone(), cast))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // A model's `attribute :name, :type` writers cast as ActiveModel's
        // types do, as `ingest::active_model_attributes` casts a plain
        // class's: an `ActiveModel::Model` form's and an ActiveRecord
        // model's alike (Rails 8.1.4). Over a column, Rails' declared type
        // replaces the column's everywhere, but here only the writer would
        // follow it, so a column keeps its own cast unless the two agree
        // (`markers::attribute_matches_column`; the mismatch is reported
        // with the model DSL). An enum column keeps its mapping too: the
        // enum decorates the attribute (`"archived"` stays a label).
        // `:boolean` already casts inline (`markers::push_attribute_api_methods`).
        {
            for (name, ty) in crate::lower::model_to_library::markers::attribute_api_decls(&model.body) {
                if model.enums.contains_key(&name) {
                    continue;
                }
                if let Some(col) = table.and_then(|t| t.columns.iter().find(|c| c.name == name))
                    && !crate::lower::model_to_library::markers::attribute_matches_column(ty.as_str(), col)
                {
                    continue;
                }
                let cast = match ty.as_str() {
                    "string" => "cast_string",
                    "integer" => "cast_integer",
                    "float" => "cast_float",
                    "decimal" => "cast_decimal",
                    "datetime" => "cast_datetime",
                    _ => continue,
                };
                casts.insert(name, cast);
            }
        }
        if casts.is_empty() {
            continue;
        }
        let Some(lc) = lcs.iter_mut().find(|lc| lc.name == model.name) else {
            continue;
        };
        // The columns a `numericality:` validation checks: their writer
        // keeps what the user assigned, and the check reads it.
        let mut checked: BTreeSet<Symbol> = BTreeSet::new();
        for m in &lc.methods {
            collect_numericality_checks(&m.body, &mut checked);
        }
        // Columns only: an ActiveModel attribute has no `_before_type_cast`,
        // so its check reads the cast value (`"abc"` is 0, Rails 8.1.4).
        checked.retain(|c| casts.contains_key(c) && table.is_some_and(|t| t.columns.iter().any(|col| col.name == *c)));
        for m in &mut lc.methods {
            if m.receiver != MethodReceiver::Instance || m.kind != crate::dialect::AccessorKind::AttributeWriter {
                if !checked.is_empty() {
                    read_raw_in_numericality_checks(&mut m.body, &checked);
                    forget_raw_on_internal_writes(&mut m.body, &checked);
                }
                continue;
            }
            let name = m.name.as_str();
            let col = name
                .strip_prefix("_write_")
                .and_then(|n| n.strip_suffix("_raw"))
                .or_else(|| name.strip_suffix('='))
                .map(Symbol::from);
            let Some(col) = col else { continue };
            let Some(cast) = casts.get(&col) else { continue };
            let [param] = m.params.as_slice() else { continue };
            let param = param.name.clone();
            // A defaulted attribute's writer is `@col = value; @col_default_applied = true`.
            let assign = match &mut *m.body.node {
                ExprNode::Seq { exprs } => match exprs.first_mut() {
                    Some(first) => &mut *first.node,
                    None => continue,
                },
                node => node,
            };
            let ExprNode::Assign { target: crate::expr::LValue::Ivar { name: ivar }, value } = assign else {
                continue;
            };
            if *ivar != col {
                continue;
            }
            cast_param_reads(value, &param, cast);
            if checked.contains(&col) {
                let keep_raw = sp_expr(ExprNode::Assign {
                    target: crate::expr::LValue::Ivar { name: raw_ivar(&col) },
                    value: sp_expr(ExprNode::Var { id: crate::ident::VarId(0), name: param.clone() }),
                });
                let cast_write = std::mem::replace(&mut m.body, sp_expr(ExprNode::Lit { value: Literal::Nil }));
                m.body = sp_expr(ExprNode::Seq { exprs: vec![keep_raw, cast_write] });
            }
        }
    }
}

/// `@<col>_before_type_cast`: what the user assigned to `col`, nil when
/// the value did not come from the user (a load, an internal write).
fn raw_ivar(col: &Symbol) -> Symbol {
    Symbol::from(format!("{}_before_type_cast", col.as_str()))
}

/// The column a lowered `numericality:` check tests, when `e` is one:
/// `if @col.nil? || !@col.is_a?(Numeric)` (`inline_numericality_check`).
fn numericality_check_column(e: &Expr) -> Option<Symbol> {
    let ExprNode::If { cond, .. } = &*e.node else { return None };
    let ExprNode::BoolOp { op: crate::expr::BoolOpKind::Or, left, right, .. } = &*cond.node else {
        return None;
    };
    let ExprNode::Send { recv: Some(nil_recv), method: nil_m, args: nil_args, .. } = &*left.node else {
        return None;
    };
    let ExprNode::Ivar { name } = &*nil_recv.node else { return None };
    if nil_m.as_str() != "nil?" || !nil_args.is_empty() {
        return None;
    }
    let ExprNode::Send { recv: Some(not_recv), method: not_m, .. } = &*right.node else { return None };
    let ExprNode::Send { recv: Some(is_a_recv), method: is_a_m, args: is_a_args, .. } = &*not_recv.node else {
        return None;
    };
    let numeric = matches!(is_a_args.as_slice(), [a] if matches!(&*a.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Numeric"));
    let same = matches!(&*is_a_recv.node, ExprNode::Ivar { name: n } if n == name);
    (not_m.as_str() == "!" && is_a_m.as_str() == "is_a?" && numeric && same).then(|| name.clone())
}

fn collect_numericality_checks(e: &Expr, out: &mut BTreeSet<Symbol>) {
    if let Some(col) = numericality_check_column(e) {
        out.insert(col);
    }
    e.node.for_each_child(&mut |c| collect_numericality_checks(c, out));
}

/// Ruby-family pre-emit pass: a `numericality:` check on an attribute no
/// column types (an `attr_accessor` of an `ActiveModel::Model` form, fed
/// request Strings) reads the number Rails' validator parses from the value
/// as given (`ActiveSupport.numericality_number(nil, @attr)`): `"12"` and
/// `" 12 "` are numbers, `"12abc"` is not, and `only_integer` reads the raw
/// text, so `"7.0"` is not an integer. The attribute keeps the String. A
/// column's check is `apply_typed_writer_cast_lowering`'s.
pub(crate) fn parse_untyped_numericality_input(lcs: &mut [LibraryClass], app: &App) {
    use crate::dialect::{ModelBodyItem, ValidationRule};
    for model in &app.models {
        let columns: BTreeSet<Symbol> = app
            .schema
            .tables
            .get(&model.table.0)
            .map(|table| table.columns.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default();
        let untyped: BTreeSet<Symbol> = model
            .body
            .iter()
            .filter_map(|item| match item {
                ModelBodyItem::Validation { validation, .. } => Some(validation),
                _ => None,
            })
            .filter(|v| !columns.contains(&v.attribute))
            .filter(|v| v.rules.iter().any(|r| matches!(r, ValidationRule::Numericality { .. })))
            .map(|v| v.attribute.clone())
            .collect();
        if untyped.is_empty() {
            continue;
        }
        let Some(lc) = lcs.iter_mut().find(|lc| lc.name == model.name) else {
            continue;
        };
        for method in &mut lc.methods {
            read_parsed_in_numericality_checks(&mut method.body, &untyped);
        }
    }
}

fn read_parsed_in_numericality_checks(e: &mut Expr, untyped: &BTreeSet<Symbol>) {
    if let Some(attr) = numericality_check_column(e).filter(|a| untyped.contains(a)) {
        let number = sp_expr(ExprNode::Send {
            recv: Some(sp_expr(ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] })),
            method: Symbol::from("numericality_number"),
            args: vec![sp_expr(ExprNode::Lit { value: Literal::Nil }), ivar_read(&attr)],
            block: None,
            parenthesized: true,
        });
        replace_ivar_reads(e, &attr, &number);
        return;
    }
    e.node.for_each_child_mut(&mut |c| read_parsed_in_numericality_checks(c, untyped));
}

/// In each lowered `numericality:` check on a `checked` column, read the
/// number ActiveModel's validator reads: the user's raw value when there
/// is one, parsed as `parse_as_number` does
/// (`ActiveSupport.numericality_number`), instead of the cast value. The
/// `allow_nil`/`allow_blank` skip around it keeps the cast value, as
/// Rails' `EachValidator#validate` does.
fn read_raw_in_numericality_checks(e: &mut Expr, checked: &BTreeSet<Symbol>) {
    if let Some(col) = numericality_check_column(e).filter(|c| checked.contains(c)) {
        let number = sp_expr(ExprNode::Send {
            recv: Some(sp_expr(ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] })),
            method: Symbol::from("numericality_number"),
            args: vec![ivar_read(&raw_ivar(&col)), ivar_read(&col)],
            block: None,
            parenthesized: true,
        });
        replace_ivar_reads(e, &col, &number);
        return;
    }
    e.node.for_each_child_mut(&mut |c| read_raw_in_numericality_checks(c, checked));
}

fn replace_ivar_reads(e: &mut Expr, col: &Symbol, with: &Expr) {
    if matches!(&*e.node, ExprNode::Ivar { name } if name == col) {
        *e = with.clone();
        return;
    }
    e.node.for_each_child_mut(&mut |c| replace_ivar_reads(c, col, with));
}

/// A write to a checked column other than its writer (`[]=`, a counter)
/// is not the user's assignment: it clears the raw value, so the check
/// reads the column again.
fn forget_raw_on_internal_writes(e: &mut Expr, checked: &BTreeSet<Symbol>) {
    e.node.for_each_child_mut(&mut |c| forget_raw_on_internal_writes(c, checked));
    let col = match &*e.node {
        ExprNode::Assign { target: crate::expr::LValue::Ivar { name }, .. } if checked.contains(name) => name.clone(),
        _ => return,
    };
    let forget = sp_expr(ExprNode::Assign {
        target: crate::expr::LValue::Ivar { name: raw_ivar(&col) },
        value: sp_expr(ExprNode::Lit { value: Literal::Nil }),
    });
    let write = std::mem::replace(e, sp_expr(ExprNode::Lit { value: Literal::Nil }));
    *e = sp_expr(ExprNode::Seq { exprs: vec![forget, write] });
}

/// Replace each read of `param` in `e` with `ActiveSupport.<cast>(param)`.
fn cast_param_reads(e: &mut Expr, param: &Symbol, cast: &str) {
    if matches!(&*e.node, ExprNode::Var { name, .. } if name == param) {
        let read = std::mem::replace(e, sp_expr(ExprNode::Lit { value: Literal::Nil }));
        *e = sp_expr(ExprNode::Send {
            recv: Some(sp_expr(ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] })),
            method: Symbol::from(cast),
            args: vec![read],
            block: None,
            parenthesized: true,
        });
        return;
    }
    e.node.for_each_child_mut(&mut |c| cast_param_reads(c, param, cast));
}

/// `@col == true || @col == 1`.
fn boolean_cast_body(col: &Symbol) -> Expr {
    let eq = |rhs: Expr| {
        sp_expr(ExprNode::Send {
            recv: Some(ivar_read(col)),
            method: Symbol::from("=="),
            args: vec![rhs],
            block: None,
            parenthesized: false,
        })
    };
    sp_expr(ExprNode::BoolOp {
        op: crate::expr::BoolOpKind::Or,
        surface: Default::default(),
        left: eq(sp_expr(ExprNode::Lit {
            value: crate::expr::Literal::Bool { value: true },
        })),
        right: eq(sp_expr(ExprNode::Lit {
            value: crate::expr::Literal::Int { value: 1 },
        })),
    })
}

// ─── Runtime-Relation eager loading (issue #27 follow-up) ──────────────
//
// The static arel path already lowers `includes(:assoc)` into inline
// preload statements, but chains that reach the runtime
// `ActiveRecord::Relation` (scope chains, association relations) only
// RECORDED their `includes(...)` specs — `to_a` never executed them, so
// every association read was a lazy per-row query (the lobsters 2x
// query-count gap vs Rails, ~985 excess queries per benchmark pass:
// belongs_to singles ~870, has_many/through ~295).
//
// This pass synthesizes, per model, the statically-dispatched preload
// machinery `Relation#to_a` calls (`@model.preload_associations(records,
// @includes)` — Base supplies a no-op default):
//
//   def self.preload_associations(records, specs)   # spec walker
//   def self._preload_dispatch(records, name, nested)  # case-dispatch
//   def self._preload_batch_<assoc>(records)        # one batched IN load
//   def _preload_<belongs_to>(rec)                  # cache setter
//
// plus a cache guard prepended to each belongs_to reader (mirroring the
// has_many readers' `return @x_cache if @x_loaded` shape; the has_many
// setters/caches already exist from the static-path work).
//
// No method_missing, no send: nested specs (`story: :user`) recurse
// through the case arm's statically-named target class — the same
// case-dispatch shape as dynamic-partial rendering. Bodies are generated
// as Ruby source and parsed back through `runtime_src::parse_methods`
// (templates are fixed; identifiers come from assoc/table names).
//
// Two framework-owned associations batch the same way, under Rails'
// own names: `has_one_attached :<attr>` as `<attr>_attachment` (one
// join over the attachment and blob tables, installing a row-bearing
// `ActiveStorage::Attached` on each record) and `has_rich_text :<attr>`
// as `rich_text_<attr>` (one `IN` over `action_text_rich_texts`,
// installing the record through the owner's load-once setter). The
// `with_attached_*` / `with_rich_text_*` scopes preload these names, so
// campfire's `Message.with_attachment_details` costs the room page two
// queries where it cost eighty.
//
// Known gaps, deliberate: scoped / polymorphic has_one, direct has_many
// with a scope or polymorphic owner, and scope-carrying through-assocs
// (other than a plain `order("...")`) get no batch arm — the dispatch
// falls through and the lazy reader stays correct (just N+1, matching
// Rails, which also lazy-loads what `includes` doesn't name). Unscoped
// non-polymorphic has_one uses PreloadKind::HasOne (first-wins). Assigning
// a belongs_to (`c.story = s`) on a PRELOADED record does not refresh
// the cache (fresh records never have the loaded flag set, so the
// benchmark's build-then-render flows are unaffected).
/// Synthesize runtime batch loaders when includes hints occur, leaving
/// associations whose restrictions cannot be preserved to their lazy readers.
pub(crate) fn apply_preload_lowering(lcs: &mut [LibraryClass], app: &App) {
    use crate::dialect::Association;

    // A dynamic query can reach Relation without any named scope (for
    // example, a helper returning a where.not chain). Its includes hint
    // needs the same batch loaders. Apps with no includes remain untouched.
    // A has_many reader that replaces its target's default scope on the
    // key (`lower::default_scope::reader_replaces_scope`) answers other
    // rows than Rails' preload, which ANDs the scope: its batch loader
    // must exist for any `includes` that reaches the runtime, seen here
    // or not, rather than fall back to that reader.
    if !app_mentions_includes(app) && !app_has_scope_replacing_reader(app) {
        return;
    }

    for lc in lcs.iter_mut() {
        let Some(model) = app.models.iter().find(|m| m.name == lc.name) else { continue };

        // belongs_to readers gain the cache guard the has_many readers
        // already carry: `return @user_cache if @user_loaded`.
        for assoc in model.associations() {
            let Association::BelongsTo { name, .. } = assoc else { continue };
            let Some(m) = lc.methods.iter_mut().find(|m| {
                m.name == *name && m.receiver == MethodReceiver::Instance
            }) else {
                continue;
            };
            let old = std::mem::replace(
                &mut m.body,
                Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
            );
            m.body = Expr::new(
                Span::synthetic(),
                ExprNode::Seq { exprs: vec![preload_cache_guard(name), old] },
            );
        }

        let src = preload_methods_source(model, app);
        let methods = crate::runtime_src::parse_methods(&src).unwrap_or_else(|e| {
            panic!("apply_preload_lowering: generated source failed to parse: {e}\n{src}")
        });
        for mut m in methods {
            if lc.methods.iter().any(|existing| {
                existing.name == m.name && existing.receiver == m.receiver
            }) {
                continue; // user-defined names win
            }
            m.enclosing_class = Some(lc.name.0.clone());
            lc.methods.push(m);
        }
    }
}

/// `return @<name>_cache if @<name>_loaded` — the same guard shape the
/// has_many readers carry (`through_reader_body`), so preloaded and lazy
/// reads share one cache contract.
fn preload_cache_guard(name: &Symbol) -> Expr {
    let span = Span::synthetic;
    Expr::new(
        span(),
        ExprNode::If {
            cond: Expr::new(
                span(),
                ExprNode::Ivar { name: Symbol::from(format!("{}_loaded", name.as_str())) },
            ),
            then_branch: Expr::new(
                span(),
                ExprNode::Return {
                    value: Expr::new(
                        span(),
                        ExprNode::Ivar {
                            name: Symbol::from(format!("{}_cache", name.as_str())),
                        },
                    ),
                },
            ),
            else_branch: Expr::new(span(), ExprNode::Lit { value: Literal::Nil }),
        },
    )
}

/// True when any raw app body (controller actions, library-class
/// methods, model scopes/methods) sends `includes`/`preload`/
/// `eager_load` to a receiver. Over-approximates (a chain the arel pass
/// later consumes statically still counts) — the cost of a false
/// positive is inert synthesized methods, not wrong behavior.
fn app_has_scope_replacing_reader(app: &App) -> bool {
    app.models
        .iter()
        .any(|m| m.associations().any(|a| crate::lower::default_scope::reader_replaces_scope(&app.models, a)))
}

fn app_mentions_includes(app: &App) -> bool {
    use crate::dialect::{ControllerBodyItem, ModelBodyItem};
    let in_expr = expr_mentions_includes;
    app.controllers.iter().any(|c| {
        c.body.iter().any(|item| match item {
            ControllerBodyItem::Action { action, .. } => in_expr(&action.body),
            _ => false,
        })
    }) || app.library_classes.iter().any(|lc| lc.methods.iter().any(|m| in_expr(&m.body)))
        || app.models.iter().any(|m| {
            m.body.iter().any(|item| match item {
                ModelBodyItem::Scope { scope, .. } => in_expr(&scope.body),
                ModelBodyItem::Method { method, .. } => in_expr(&method.body),
                _ => false,
            })
        })
}

fn expr_mentions_includes(expr: &Expr) -> bool {
    let mut found = false;
    fn walk(e: &Expr, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv: Some(recv), method, .. } = &*e.node {
            if matches!(method.as_str(), "includes" | "preload" | "eager_load" | "preload_associations") {
                *found = true;
                return;
            }
            // `Preloader.new(...).call` lowers to `Preloading.records`
            // when the records are not one known model. That call is
            // the only mention, and the batch loaders have to exist
            // for it.
            if method.as_str() == "records"
                && matches!(&*recv.node, ExprNode::Const { path }
                    if path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).eq(["ActiveRecord", "Preloading"]))
            {
                *found = true;
                return;
            }
        }
        e.node.for_each_child(&mut |c| walk(c, found));
    }
    walk(expr, &mut found);
    found
}

/// Whether `model`'s primary key is a uuid / string column, which an
/// `IN (...)` lists quoted and a batch keys by its String (the integer
/// helper would `to_i` it to its leading digits and find nothing; cf.
/// #310 for `Relation#ids`).
fn string_primary_key(app: &App, model: &crate::dialect::Model) -> bool {
    app.schema
        .tables
        .get(&model.table.0)
        .and_then(|t| t.columns.iter().find(|c| c.primary_key))
        .is_some_and(|c| {
            matches!(
                c.col_type,
                crate::schema::ColumnType::Uuid
                    | crate::schema::ColumnType::String { .. }
                    | crate::schema::ColumnType::Text
            )
        })
}

/// The `Db` list helper and the row-key conversion for a batch keyed by
/// the owner's id.
fn key_helpers(string_key: bool) -> (&'static str, &'static str) {
    if string_key { ("escape_string_list", "to_s") } else { ("escape_int_list", "to_i") }
}

/// One preloadable association, resolved against the app's model set.
enum PreloadKind {
    /// (fk column on the owner, target class, target table, the `Db`
    /// helper that renders the target's key list for `IN (...)`)
    BelongsTo { fk: String, target: String, table: String, key_list: &'static str },
    /// (fk column on the target, target class)
    HasMany { fk: String, owner_key: String, target: String },
    /// `has_one` — same FK-on-target batch as has_many, installing one
    /// record (or nil) per owner through the single-record preload setter.
    HasOne { fk: String, owner_key: String, target: String },
    /// Batched form of the through-reader join:
    /// `SELECT <t>.*, <thr>.<thr_fk> AS __src FROM <t> JOIN <thr> ON
    /// <thr>.<src_fk> = <t>.id WHERE <thr>.<thr_fk> IN (...)`.
    Through { target: String, join: String, group_col: String, order: Option<String>, string_key: bool },
    /// `has_one_attached :<attr>`: one join over the attachment and blob
    /// tables for the whole record set, installing a row-bearing
    /// `ActiveStorage::Attached` on each record (`attr`, owner class).
    /// `variations` is the declared-variant list as Ruby source
    /// (`lower::attached::variations_ruby_source`), the proxy's fourth
    /// constructor argument.
    Attached { attr: String, owner: String, variations: String, string_key: bool },
    /// `has_many_attached :<attr>`: one join over the attachment and blob
    /// tables for the whole record set, installing a proxy per record
    /// that answers its rows (`AttachedMany#_preload_rows`) without
    /// asking again.
    AttachedMany { attr: String, owner: String, string_key: bool },
    /// `has_rich_text :<attr>`: one `IN` over `action_text_rich_texts`,
    /// installed through the owner's load-once setter.
    RichText { attr: String, owner: String },
    /// `has_markdown :<attr>`: one `IN` over `action_text_markdowns`,
    /// installed through the owner's load-once setter.
    PlainText { attr: String, owner: String },
}

/// Would `assoc`'s raw-SQL batch (belongs_to, `through:`) load rows of a
/// model whose `default_scope` the runtime Relation applies
/// (`lower::default_scope`): its target, or its `through:` model?
fn touches_default_scoped(model: &crate::dialect::Model, assoc: &crate::dialect::Association, app: &App) -> bool {
    use crate::dialect::Association;
    let scoped = |id: &ClassId| {
        app.models.iter().any(|m| &m.name == id && crate::lower::default_scope::default_scope(m).is_some())
    };
    let (target, through) = match assoc {
        Association::BelongsTo { target, .. } => (target, None),
        Association::HasMany { target, through, .. } | Association::HasOne { target, through, .. } => {
            (target, through.as_ref())
        }
        _ => return false,
    };
    // A direct has_many / has_one batch loads through
    // `Relation.new(<target>)`, which applies the scope itself.
    let Some(thr) = through else {
        return matches!(assoc, Association::BelongsTo { .. }) && scoped(target);
    };
    scoped(target) || {
        model.associations().any(|a| {
            a.name() == thr
                && matches!(a, Association::BelongsTo { target, .. } | Association::HasMany { target, .. } | Association::HasOne { target, .. } if scoped(target))
        })
    }
}

/// Select association shapes whose batch queries preserve the reader's filters,
/// resolving each against the app model registry and its table metadata.
fn preload_targets(model: &crate::dialect::Model, app: &App) -> Vec<(String, PreloadKind)> {
    use crate::dialect::Association;
    use crate::naming::pluralize_snake;

    let model_exists = |id: &ClassId| app.models.iter().any(|m| &m.name == id);
    // The owner's own key is a uuid / string: the batches keyed by it
    // (attachments, has_many :through) list it quoted and key their rows
    // by the String, as the belongs_to batch below does for its target.
    let owner_string_key = string_primary_key(app, model);
    let mut out = Vec::new();
    for assoc in model.associations() {
        // A refused association (`assoc_primary_key`) has no reader to
        // preload into.
        if crate::lower::assoc_primary_key::unsupported_reason(model, &app.models, assoc).is_some() {
            continue;
        }
        // A belongs_to or `through:` batch is raw SQL; one that loads a
        // default-scoped model keeps its lazy reader, whose Relation
        // applies the scope, rather than load the rows it hides.
        if touches_default_scoped(model, assoc, app) {
            continue;
        }
        match assoc {
            Association::BelongsTo { name, target, foreign_key, .. } => {
                let Some(target_model) = app.models.iter().find(|m| &m.name == target) else {
                    continue;
                };
                // A uuid / string key is a quoted literal in `IN (...)`;
                // the integer helper would `to_i` it to its leading
                // digits and find nothing (cf. #310 for `Relation#ids`).
                let string_key = string_primary_key(app, target_model);
                out.push((
                    name.as_str().to_string(),
                    PreloadKind::BelongsTo {
                        fk: foreign_key.as_str().to_string(),
                        target: target.0.as_str().to_string(),
                        table: target_model.table.0.as_str().to_string(),
                        key_list: if string_key { "escape_string_list" } else { "escape_int_list" },
                    },
                ));
            }
            // This loader only applies the foreign-key predicate. A scope
            // or polymorphic owner-type restriction must stay on the lazy
            // reader until the batch query can preserve it as well.
            Association::HasMany {
                name, target, foreign_key, through: None,
                scope: None, as_interface: None, ..
            } => {
                if !model_exists(target) {
                    continue;
                }
                out.push((
                    name.as_str().to_string(),
                    PreloadKind::HasMany {
                        fk: foreign_key.as_str().to_string(),
                        owner_key: assoc.owner_key().as_str().to_string(),
                        target: target.0.as_str().to_string(),
                    },
                ));
            }
            // Same restriction as has_many: FK-only batch. Scoped or
            // polymorphic has_one stays on the lazy reader until the
            // batch can preserve those predicates.
            Association::HasOne {
                name, target, foreign_key,
                scope: None, as_interface: None, ..
            } => {
                if !model_exists(target) {
                    continue;
                }
                out.push((
                    name.as_str().to_string(),
                    PreloadKind::HasOne {
                        fk: foreign_key.as_str().to_string(),
                        owner_key: assoc.owner_key().as_str().to_string(),
                        target: target.0.as_str().to_string(),
                    },
                ));
            }
            // Through: same two-hop resolution as
            // `apply_through_assoc_lowering`; assoc scopes other than a
            // plain `order("...")` (or none) don't batch — the lazy
            // reader keeps them correct.
            Association::HasMany {
                name, target, through: Some(thr_name), scope, ..
            } => {
                if !model_exists(target) {
                    continue;
                }
                let order = match scope {
                    None => None,
                    Some(s) => match order_literal(s) {
                        Some(o) => Some(o),
                        None => continue,
                    },
                };
                let Some(Association::HasMany { target: thr_target, as_interface, .. }) =
                    model.associations().find(|a| {
                        matches!(a, Association::HasMany { name, .. } if name == thr_name)
                    })
                else {
                    continue;
                };
                // A missing join model is not batched through the convention fallback.
                if !app.models.iter().any(|m| &m.name == thr_target) {
                    continue;
                }
                let crate::lower::model_to_library::ThroughWriterJoin::Resolved(_, owner_fk, src_fk, _) =
                    crate::lower::model_to_library::through_writer_join(model, &app.models, name)
                else {
                    continue;
                };
                let thr_table = pluralize_snake(thr_target.0.as_str());
                let target_table = pluralize_snake(target.0.as_str());
                let owner_type = through_owner_type_predicate(&thr_table, as_interface, &model.name);
                out.push((
                    name.as_str().to_string(),
                    PreloadKind::Through {
                        target: target.0.as_str().to_string(),
                        join: format!(
                            "INNER JOIN {thr_table} ON {thr_table}.{src_fk} = {target_table}.id{owner_type}"
                        ),
                        group_col: format!("{thr_table}.{owner_fk}"),
                        order: order.map(|o| o.to_string()),
                        string_key: owner_string_key,
                    },
                ));
            }
            _ => {}
        }
    }
    // The framework-owned pairs, under Rails' own association names.
    for (_span, attr) in crate::lower::attached::attached_attrs(model) {
        out.push((
            crate::lower::attached::attachment_assoc_name(&attr).as_str().to_string(),
            PreloadKind::Attached {
                attr: attr.as_str().to_string(),
                owner: model.name.0.as_str().to_string(),
                variations: crate::lower::attached::variations_ruby_source(model, &attr),
                string_key: owner_string_key,
            },
        ));
    }
    for (_span, attr) in crate::lower::attached::many_attached_attrs(model) {
        out.push((
            crate::lower::attached::many_attachments_assoc_name(&attr)
                .as_str()
                .to_string(),
            PreloadKind::AttachedMany {
                attr: attr.as_str().to_string(),
                owner: model.name.0.as_str().to_string(),
                string_key: owner_string_key,
            },
        ));
    }
    if model_exists(&crate::lower::rich_text::record_class()) {
        for (_span, attr) in crate::lower::rich_text::rich_text_attrs(model) {
            out.push((
                format!("rich_text_{}", attr.as_str()),
                PreloadKind::RichText {
                    attr: attr.as_str().to_string(),
                    owner: model.name.0.as_str().to_string(),
                },
            ));
        }
    }
    if model_exists(&crate::lower::plain_text_attr::record_class()) {
        for (_span, attr) in crate::lower::plain_text_attr::plain_text_attrs(model) {
            out.push((
                format!("markdown_{}", attr.as_str()),
                PreloadKind::PlainText {
                    attr: attr.as_str().to_string(),
                    owner: model.name.0.as_str().to_string(),
                },
            ));
        }
    }
    out
}

/// The statements gathering the owners' keys into `ids` for a batch
/// preload. `id` is never nil; any other `primary_key:` column can be,
/// and a nil key matches no child (Rails skips it too), so it is
/// skipped rather than sent as `IS NULL`.
fn collect_owner_keys(owner_key: &str) -> String {
    if owner_key == "id" {
        "    ids << r.id".to_string()
    } else {
        format!("    v = r.{owner_key}\n    ids << v unless v.nil?")
    }
}

/// Extract the string literal from an assoc-scope lambda body of the
/// exact shape `order("...")` (lobsters `has_many :tags, -> { order
///('tags.is_media desc, tags.tag') }, through: :taggings`).
fn order_literal(scope_body: &Expr) -> Option<&str> {
    let ExprNode::Send { recv: None, method, args, .. } = &*scope_body.node else {
        return None;
    };
    if method.as_str() != "order" || args.len() != 1 {
        return None;
    }
    let ExprNode::Lit { value: Literal::Str { value } } = &*args[0].node else {
        return None;
    };
    Some(value.as_str())
}

/// Generate the per-model preload methods as Ruby source (fed back
/// through `runtime_src::parse_methods`). Templates stay boring on
/// purpose: statement-level assigns and explicit nil-guards round-trip
/// through every walker; no `||=`-on-index, no ternaries.
fn preload_methods_source(model: &crate::dialect::Model, app: &App) -> String {
    let targets = preload_targets(model, app);
    let mut src = String::new();

    // Batch loaders + belongs_to cache setters.
    for (name, kind) in &targets {
        match kind {
            PreloadKind::BelongsTo { fk, target, table, key_list } => {
                let _ = write!(
                    src,
                    r#"
def self._preload_batch_{name}(records)
  ids = []
  records.each do |r|
    v = r.{fk}
    ids << v unless v.nil? || v == 0
  end
  ids.uniq!
  by_id = {{}}
  if ids.length > 0
    {target}._hydrate_all("SELECT " + {target}._columns_sql + " FROM {table} WHERE {table}.id IN (" + Db.{key_list}(ids) + ")").each do |rec|
      by_id[rec.id] = rec
    end
  end
  records.each do |r|
    r._preload_{name}(by_id[r.{fk}])
  end
  by_id.values
end

def _preload_{name}(rec)
  @{name}_cache = rec
  @{name}_loaded = true
  nil
end
"#
                );
            }
            PreloadKind::HasMany { fk, owner_key, target } => {
                let collect_keys = collect_owner_keys(owner_key);
                let _ = write!(
                    src,
                    r#"
def self._preload_batch_{name}(records)
  ids = []
  records.each do |r|
{collect_keys}
  end
  loaded = []
  if ids.length > 0
    loaded = ActiveRecord::Relation.new({target}).where({fk}: ids).to_a
  end
  grouped = {{}}
  loaded.each do |rec|
    k = rec.{fk}
    grouped[k] = [] if grouped[k].nil?
    grouped[k] << rec
  end
  records.each do |r|
    r._preload_{name}(grouped[r.{owner_key}] || [])
  end
  loaded
end
"#
                );
            }
            PreloadKind::HasOne { fk, owner_key, target } => {
                let collect_keys = collect_owner_keys(owner_key);
                let _ = write!(
                    src,
                    r#"
def self._preload_batch_{name}(records)
  ids = []
  records.each do |r|
{collect_keys}
  end
  by_id = {{}}
  loaded = []
  if ids.length > 0
    loaded = ActiveRecord::Relation.new({target}).where({fk}: ids).to_a
  end
  loaded.each do |rec|
    k = rec.{fk}
    by_id[k] = rec if by_id[k].nil?
  end
  records.each do |r|
    r._preload_{name}(by_id[r.{owner_key}])
  end
  loaded
end
"#
                );
            }
            PreloadKind::Through { target, join, group_col, order, string_key } => {
                let (key_list, to_key) = key_helpers(*string_key);
                let table = crate::naming::pluralize_snake(target.as_str());
                let order_sql = match order {
                    Some(o) => format!(" ORDER BY {o}"),
                    None => String::new(),
                };
                let _ = write!(
                    src,
                    r#"
def self._preload_batch_{name}(records)
  ids = []
  records.each do |r|
    ids << r.id
  end
  grouped = {{}}
  loaded = []
  if ids.length > 0
    adapter = ActiveRecord.adapter
    adapter.each_row("SELECT {table}.*, {group_col} AS __src FROM {table} {join} WHERE {group_col} IN (" + Db.{key_list}(ids) + "){order_sql}") do |row|
      rec = {target}.instantiate(row)
      loaded << rec
      k = row["__src"].{to_key}
      grouped[k] = [] if grouped[k].nil?
      grouped[k] << rec
    end
  end
  records.each do |r|
    r._preload_{name}(grouped[r.id] || [])
  end
  loaded
end
"#
                );
            }
            // One join for every record's attachment row; a record the
            // query did not name gets a proxy that already knows it has
            // nothing (id 0), so its `attached?` is a field read too.
            // The blob columns are `ActiveStorage::Blob.columns`, so
            // this query and the proxy's own cannot disagree about the
            // row shape; two typed hashes (attachment id, blob) rather
            // than one hash of rows so every value keeps its own type.
            PreloadKind::Attached { attr, owner, variations, string_key } => {
                let (key_list, to_key) = key_helpers(*string_key);
                let _ = write!(
                    src,
                    r#"
def self._preload_batch_{name}(records)
  ids = []
  records.each do |r|
    ids << r.id
  end
  att_ids = {{}}
  blobs = {{}}
  if ids.length > 0
    ActiveRecord.adapter.select_rows("SELECT a.record_id AS record_id, a.id AS attachment_id, " + ActiveStorage::Blob.columns("b") + " FROM active_storage_attachments a JOIN active_storage_blobs b ON b.id = a.blob_id WHERE a.record_type = '{owner}' AND a.name = '{attr}' AND a.record_id IN (" + Db.{key_list}(ids) + ")").each do |row|
      rid = row["record_id"].{to_key}
      att_ids[rid] = row["attachment_id"].to_i
      blobs[rid] = ActiveStorage::Blob.from_row(row)
    end
  end
  records.each do |r|
    att = ActiveStorage::Attached.new("{owner}", r.id, "{attr}", {variations})
    found = att_ids[r.id]
    if found.nil?
      att._preload_row(0, nil)
    else
      att._preload_row(found, blobs[r.id])
    end
    r._preload_{name}(att)
  end
  []
end
"#
                );
            }
            PreloadKind::AttachedMany { attr, owner, string_key } => {
                let (key_list, to_key) = key_helpers(*string_key);
                let _ = write!(
                    src,
                    r#"
def self._preload_batch_{name}(records)
  ids = []
  records.each do |r|
    ids << r.id
  end
  rows_by = {{}}
  if ids.length > 0
    ActiveRecord.adapter.select_rows("SELECT a.record_id AS record_id, a.id AS attachment_id, " + ActiveStorage::Blob.columns("b") + " FROM active_storage_attachments a JOIN active_storage_blobs b ON b.id = a.blob_id WHERE a.record_type = '{owner}' AND a.name = '{attr}' AND a.record_id IN (" + Db.{key_list}(ids) + ") ORDER BY a.id").each do |row|
      rid = row["record_id"].{to_key}
      list = rows_by[rid]
      if list.nil?
        list = []
        rows_by[rid] = list
      end
      list.push(ActiveStorage::ManyAttachment.new(row["attachment_id"].to_i, ActiveStorage::Blob.from_row(row)))
    end
  end
  records.each do |r|
    many = ActiveStorage::AttachedMany.new("{owner}", r.id, "{attr}")
    found = rows_by[r.id]
    many._preload_rows(found.nil? ? [] : found)
    r._preload_{name}(many)
  end
  []
end
"#
                );
            }
            // One IN over the rich-text table; a record with no row is
            // told so, which is the state the load-once reader must not
            // re-query.
            PreloadKind::RichText { attr, owner } => {
                let _ = write!(
                    src,
                    r#"
def self._preload_batch_{name}(records)
  ids = []
  records.each do |r|
    ids << r.id
  end
  by_id = {{}}
  loaded = []
  if ids.length > 0
    loaded = ActiveRecord::Relation.new(ActionText::RichText).where(record_type: "{owner}", name: "{attr}", record_id: ids).to_a
  end
  loaded.each do |rec|
    by_id[rec.record_id] = rec
  end
  records.each do |r|
    r._preload_{name}(by_id[r.id])
  end
  loaded
end
"#
                );
            }
            PreloadKind::PlainText { attr, owner } => {
                let _ = write!(
                    src,
                    r#"
def self._preload_batch_{name}(records)
  ids = []
  records.each do |r|
    ids << r.id
  end
  by_id = {{}}
  loaded = []
  if ids.length > 0
    loaded = ActiveRecord::Relation.new(ActionText::Markdown).where(record_type: "{owner}", name: "{attr}", record_id: ids).to_a
  end
  loaded.each do |rec|
    by_id[rec.record_id] = rec
  end
  records.each do |r|
    r._preload_{name}(by_id[r.id])
  end
  loaded
end
"#
                );
            }
        }
    }

    // Dispatch: one case arm per preloadable assoc; unknown names fall
    // through silently (lazy readers stay correct — mirrors what Rails
    // does for anything `includes` didn't name).
    src.push_str("\ndef self._preload_dispatch(records, name, nested)\n");
    if targets.is_empty() {
        src.push_str("  nil\nend\n");
    } else {
        src.push_str("  case name\n");
        for (name, kind) in &targets {
            let target = match kind {
                PreloadKind::BelongsTo { target, .. } => Some(target.as_str()),
                PreloadKind::HasMany { target, .. } => Some(target.as_str()),
                PreloadKind::HasOne { target, .. } => Some(target.as_str()),
                PreloadKind::Through { target, .. } => Some(target.as_str()),
                PreloadKind::RichText { .. } => Some("ActionText::RichText"),
                PreloadKind::PlainText { .. } => Some("ActionText::Markdown"),
                // `includes(logo_attachment: :blob)`: the blob is already
                // in the row the loader fetched; there is no model to
                // recurse into. Many installs the proxy only.
                PreloadKind::Attached { .. } | PreloadKind::AttachedMany { .. } => None,
            };
            match target {
                Some(target) => {
                    let _ = write!(
                        src,
                        "  when :{name}\n    loaded = _preload_batch_{name}(records)\n    {target}.preload_associations(loaded, [nested]) unless nested.nil?\n"
                    );
                }
                None => {
                    let _ = write!(src, "  when :{name}\n    _preload_batch_{name}(records)\n");
                }
            }
        }
        src.push_str("  end\n  nil\nend\n");
    }

    // An eager load's association (`lower::eager_load`): the same IN load
    // with the chain's conditions and order on the association's table.
    src.push_str("\ndef self._eager_dispatch(records, name, filter)\n");
    let eager = crate::lower::eager_load::eager_loadable(model);
    if eager.is_empty() {
        src.push_str("  nil\nend\n");
    } else {
        src.push_str("  case name\n");
        for (name, target, fk) in &eager {
            let target = target.0.as_str();
            let _ = write!(
                src,
                "  when :{name}\n    ids = []\n    records.each do |r| ids << r.id end\n    loaded = []\n    if ids.length > 0\n      rel = ActiveRecord::Relation.new({target}).where({fk}: ids)\n      filter.conditions.each do |c| rel = rel.where(c) end\n      filter.negations.each do |c| rel = rel.where.not(c) end\n      filter.orders.each do |o| rel = rel.order(o) end\n      loaded = rel.to_a\n    end\n    grouped = {{}}\n    loaded.each do |rec|\n      k = rec.{fk}\n      grouped[k] = [] if grouped[k].nil?\n      grouped[k] << rec\n    end\n    records.each do |r| r._preload_{name}(grouped[r.id] || []) end\n    {target}.preload_associations(loaded, [filter.nested]) unless filter.nested.nil?\n"
            );
        }
        src.push_str("  end\n  nil\nend\n");
    }

    // Spec walker — the entry point `Relation#to_a` calls. Specs are
    // Symbols, Hashes (nested: `story: :user`), or Arrays of either.
    src.push_str(
        r#"
def self.preload_associations(records, specs)
  return nil if records.length == 0
  specs.each do |spec|
    if spec.is_a?(Hash)
      spec.each do |name, nested|
        if nested.is_a?(ActiveRecord::EagerFilter)
          _eager_dispatch(records, name, nested)
        else
          _preload_dispatch(records, name, nested)
        end
      end
    elsif spec.is_a?(Array)
      preload_associations(records, spec)
    elsif !spec.nil?
      _preload_dispatch(records, spec, nil)
    end
  end
  nil
end
"#,
    );

    src
}

/// Monomorphize an app helper on a `raw(...)` argument.
///
/// Rails carries "this string is safe" in the VALUE (SafeBuffer), so a
/// safe label can ride a plain parameter into a helper and out to
/// `link_to`, which then declines to escape it. Nothing survives that
/// boundary here: the emit-time unwrap (`is_html_safe_call`) only sees
/// producers it can name at the call site, and the AOT tree has no
/// safe-string type at all. lobsters' layout hits it once and it shows
/// on every page with a header —
///
///   link_to_different_page raw("#{user}&nbsp;<span class='karma'>…"), settings_path
///
/// rendered `michell_wiegand&amp;nbsp;&lt;span…` on the spinel tree, a
/// +24B diff against Rails on THIRTEEN of the 49 benchmark routes.
///
/// So safety propagates statically instead: a call passing `raw(x)`
/// retargets to a `<name>_raw` clone of the helper in which the safe
/// parameter's escaping consumers are neutralized. This is the same
/// exemption the `link_to(raw(x))` → `link_to_raw(x)` rewrite makes one
/// level down, lifted through one user-defined frame.
///
/// Deliberately narrow. The clone rewrites only what it can prove
/// consumes THAT parameter — `link_to` on it becomes `link_to_raw`, and
/// an `html_escape` of it collapses — and a helper whose safe argument
/// reaches neither is emitted as an ordinary clone rather than guessed
/// at. Widening this is a typed-safe-string project, not a bigger match
/// arm.
pub(crate) fn apply_raw_helper_monomorphization(lcs: &mut [LibraryClass], app: &App) {
    let sites = raw_helper_sites(app);
    if sites.is_empty() {
        return;
    }
    // Definition side: synthesize the `_raw` clone next to the original.
    for lc in lcs.iter_mut() {
        let mut synthesized: Vec<crate::dialect::MethodDef> = Vec::new();
        for m in &lc.methods {
            for (name, idx) in &sites {
                if &m.name != name {
                    continue;
                }
                let Some(param) = m.params.get(*idx) else { continue };
                let mut clone = m.clone();
                clone.name = Symbol::from(format!("{}_raw", name.as_str()));
                let pname = param.name.clone();
                propagate_raw_param(&mut clone.body, &pname);
                synthesized.push(clone);
            }
        }
        lc.methods.extend(synthesized);
    }
    // Call side: `M.helper(raw(x), …)` → `M.helper_raw(x, …)`.
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            rewrite_raw_helper_calls(&mut m.body, &sites);
        }
    }
}

/// `(helper name, index of the argument passed as `raw(...)`)` for every
/// app-helper call site that passes one. Read from `app.views` — i.e.
/// BEFORE lowering, where both the helper call and `raw` are still bare
/// sends — because the two sides of this rewrite are emitted in
/// different LC groups and each needs the same answer.
///
/// Views only: `raw` is a view-layer marker, and a controller reaching
/// for one would be building markup in the wrong place. Extend to
/// controller bodies if a corpus ever does it.
fn raw_helper_sites(app: &App) -> BTreeSet<(Symbol, usize)> {
    let mut out = BTreeSet::new();
    if app.helper_method_index.is_empty() {
        return out;
    }
    let mut scan = |e: &Expr| {
        collect_raw_helper_sites(e, &app.helper_method_index, &mut out);
    };
    for v in &app.views {
        scan(&v.body);
    }
    out
}

fn collect_raw_helper_sites(
    e: &Expr,
    index: &crate::det::HashMap<Symbol, ClassId>,
    out: &mut BTreeSet<(Symbol, usize)>,
) {
    e.node.for_each_child(&mut |c| collect_raw_helper_sites(c, index, out));
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*e.node else {
        return;
    };
    if !index.contains_key(method) {
        return;
    }
    for (i, a) in args.iter().enumerate() {
        let is_raw = matches!(&*a.node,
            ExprNode::Send { recv: None, method: m, args: ra, .. }
                if m.as_str() == "raw" && ra.len() == 1);
        if is_raw {
            out.insert((method.clone(), i));
        }
    }
}

/// Inside the `_raw` clone: the named parameter is already safe, so the
/// escaping it would otherwise receive comes off.
fn propagate_raw_param(body: &mut Expr, pname: &Symbol) {
    body.node.for_each_child_mut(&mut |c| propagate_raw_param(c, pname));
    // `ViewHelpers.link_to(<uses param>, …)` → `link_to_raw`.
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &mut *expr_node_mut(body) {
        if method.as_str() == "link_to"
            && is_view_helpers_const(r)
            && args.first().is_some_and(|a| mentions_var(a, pname))
        {
            *method = Symbol::from("link_to_raw");
            return;
        }
    }
    // `ViewHelpers.html_escape(<param>)` → `<param>`.
    let collapse = match &*body.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "html_escape"
                && args.len() == 1
                && is_view_helpers_const(r)
                && mentions_var(&args[0], pname) =>
        {
            Some(args[0].clone())
        }
        _ => None,
    };
    if let Some(inner) = collapse {
        *body = inner;
    }
}

fn expr_node_mut(e: &mut Expr) -> &mut ExprNode {
    &mut e.node
}

/// Does `e` read `name` (directly, or through a `.to_s` the emitter
/// added)?
fn mentions_var(e: &Expr, name: &Symbol) -> bool {
    match &*e.node {
        ExprNode::Var { name: n, .. } => n == name,
        ExprNode::Send { recv: Some(r), args, .. } if args.is_empty() => mentions_var(r, name),
        _ => false,
    }
}

fn rewrite_raw_helper_calls(e: &mut Expr, sites: &BTreeSet<(Symbol, usize)>) {
    e.node.for_each_child_mut(&mut |c| rewrite_raw_helper_calls(c, sites));
    let ExprNode::Send { recv: Some(_), method, args, block: None, .. } = &mut *e.node else {
        return;
    };
    let Some((_, idx)) = sites.iter().find(|(n, i)| n == method && args.len() > *i) else {
        return;
    };
    let inner = match &*args[*idx].node {
        ExprNode::Send { recv: Some(r2), method: m2, args: a2, .. }
            if m2.as_str() == "raw" && a2.len() == 1 && is_view_helpers_const(r2) =>
        {
            Some(a2[0].clone())
        }
        _ => None,
    };
    if let Some(inner) = inner {
        let i = *idx;
        *method = Symbol::from(format!("{}_raw", method.as_str()));
        args[i] = inner;
    }
}

#[cfg(test)]
mod insert_rel_param_tests {
    use super::*;
    use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, MethodVisibility, Param};
    use crate::expr::{Expr, ExprNode, Literal};
    use crate::ty::ParamKind;

    fn bare_method(params: Vec<Param>, signature: Option<Ty>) -> MethodDef {
        MethodDef {
            visibility: MethodVisibility::Public,
            unsupported_formals: None,
            has_anonymous_block: false,
            name_span: crate::span::Span::synthetic(),
            name: Symbol::from("those_tagged"),
            receiver: MethodReceiver::Class,
            params,
            body: Expr::new(crate::span::Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
            signature,
            effects: crate::effect::EffectSet::default(),
            enclosing_class: Some(Symbol::from("Widget")),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        }
    }

    /// The decided fix, at the type level: a rest-param method's `__rel`
    /// is a KEYWORD in `m.params` (a default, not a positional default —
    /// the only legal ordering after a splat), and in the `Ty::Fn`
    /// signature it is `ParamKind::Keyword { required: false }`, typed
    /// as the owning model's `Relation` — NOT `Ty::Untyped` — so a
    /// target whose `.rbs` DOES read the signature (unlike this
    /// fixture's own, which the general stamping pass declines to give
    /// one; see `tests/relation_rest_param_class_method.rs`) writes
    /// `?__rel: ActiveRecord::Relation`, matching every other typed slot
    /// on the method instead of losing its type to the one param this
    /// fix touches.
    #[test]
    fn rest_param_method_gets_rel_as_a_typed_keyword_in_params_and_signature() {
        let owner = ClassId(Symbol::from("Widget"));
        let params = vec![Param::rest(Symbol::from("tags"))];
        let sig = Ty::Fn {
            params: vec![crate::ty::Param {
                name: Symbol::from("tags"),
                ty: Ty::Untyped.into(),
                kind: ParamKind::Rest,
            }].into(),
            block: None,
            ret: std::sync::Arc::new(Ty::Untyped),
            effects: crate::effect::EffectSet::pure(),
        };
        let mut m = bare_method(params, Some(sig));
        let rel_param = Symbol::from("__rel");
        assert!(insert_rel_param(&mut m, &rel_param, &owner));

        let rel = m.params.iter().find(|p| p.name == rel_param).expect("__rel param");
        assert!(rel.keyword, "a keyword, not a positional, after a splat: {rel:?}");
        assert!(!rel.rest);
        assert!(rel.default.is_some(), "optional: carries a default");

        let Some(Ty::Fn { params, .. }) = &m.signature else {
            panic!("the signature must not be dropped");
        };
        let rel_ty_param = params.iter().find(|p| p.name == rel_param).expect("__rel in signature");
        assert_eq!(rel_ty_param.kind, ParamKind::Keyword { required: false });
        assert_eq!(*rel_ty_param.ty, Ty::Relation { of: owner });
    }

    /// The untouched case: no rest param, `__rel` stays exactly what it
    /// always was — a positional with a default, inserted before any
    /// keywords, typed `Untyped` in the signature (unchanged from
    /// before this fix).
    #[test]
    fn non_rest_method_keeps_the_untyped_positional_default() {
        let owner = ClassId(Symbol::from("Widget"));
        let params = vec![Param::positional(Symbol::from("user"))];
        let sig = Ty::Fn {
            params: vec![crate::ty::Param {
                name: Symbol::from("user"),
                ty: Ty::Untyped.into(),
                kind: ParamKind::Required,
            }].into(),
            block: None,
            ret: std::sync::Arc::new(Ty::Untyped),
            effects: crate::effect::EffectSet::pure(),
        };
        let mut m = bare_method(params, Some(sig));
        let rel_param = Symbol::from("__rel");
        assert!(insert_rel_param(&mut m, &rel_param, &owner));

        let rel = m.params.iter().find(|p| p.name == rel_param).expect("__rel param");
        assert!(!rel.keyword, "unchanged: still a positional-with-default: {rel:?}");
        assert!(rel.default.is_some());

        let Some(Ty::Fn { params, .. }) = &m.signature else {
            panic!("the signature must not be dropped");
        };
        let rel_ty_param = params.iter().find(|p| p.name == rel_param).expect("__rel in signature");
        assert_eq!(rel_ty_param.kind, ParamKind::Optional);
        assert_eq!(*rel_ty_param.ty, Ty::Untyped, "unchanged: still Untyped, not Relation");
    }

    /// `def f(*tags, **opts)`: no keyword may follow `**opts`, so `__rel:`
    /// goes ahead of it, in the params and in the signature.
    #[test]
    fn rest_and_kwrest_method_gets_rel_ahead_of_the_kwrest() {
        let owner = ClassId(Symbol::from("Widget"));
        let kwrest = Param { keyword: true, ..Param::rest(Symbol::from("opts")) };
        let params = vec![Param::rest(Symbol::from("tags")), kwrest];
        let sig = Ty::Fn {
            params: vec![
                crate::ty::Param { name: Symbol::from("tags"), ty: Ty::Untyped.into(), kind: ParamKind::Rest },
                crate::ty::Param { name: Symbol::from("opts"), ty: Ty::Untyped.into(), kind: ParamKind::KeywordRest },
            ].into(),
            block: None,
            ret: std::sync::Arc::new(Ty::Untyped),
            effects: crate::effect::EffectSet::pure(),
        };
        let mut m = bare_method(params, Some(sig));
        let rel_param = Symbol::from("__rel");
        assert!(insert_rel_param(&mut m, &rel_param, &owner));

        let names: Vec<&str> = m.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["tags", "__rel", "opts"]);
        let Some(Ty::Fn { params, .. }) = &m.signature else {
            panic!("the signature must not be dropped");
        };
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["tags", "__rel", "opts"]);
    }
}

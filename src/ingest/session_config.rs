//! `config.session_store` and the Rails default session cookie name,
//! lifted onto the `Rails::Application` reopen as `session_cookie_key`.
//!
//! Rails always has a session cookie name. When the app declares none,
//! `ActionDispatch::Session::CookieStore` derives one from the
//! application class (`CwtRails::Application` → `_cwt_rails_session`).
//! A literal `config.session_store :cookie_store, key: "..."` in
//! `config/application.rb`, an environment file, or any initializer
//! overrides that default; later files win, and `if`/`unless` around a
//! declaration are kept so the runtime env picks the same branch Rails
//! would.
//!
//! THE DECLARATIONS ARE REPLAYED, NOT EVALUATED. One step IR walks each
//! config body (class/module/`configure`/`if`/`unless`). Synthesize
//! emits sequential `key = …` assignments in boot order; the ledger's
//! `reads` accepts a statement only when every leaf of that IR is a
//! store call, so a mixed conditional that also carries an unread
//! sibling stays Unconsumed.
//!
//! What this cannot type is refused (ingest error in strict mode, a
//! survey gap otherwise): a non-literal `key:`, a `session_store` that
//! is not at the top of a config body or of an `if`/`unless` inside one
//! (a loop, a method), and an options splat.

use std::path::{Path, PathBuf};

use ruby_prism::{Node, Visit};

use super::survey;
use super::util::{
    constant_id_str, constant_path_of, flatten_statements, string_value, symbol_value,
};
use super::{IngestError, IngestResult};
use crate::vfs::Vfs;

/// One statement that can change the cookie key, in execution order.
#[derive(Clone, Debug)]
enum Step {
    /// `config.session_store …, key: "…"` (or the middleware default
    /// `"_session_id"` when the store is set without a key).
    Set(Result<String, String>),
    /// An `if`/`unless` whose branches set the key.
    If {
        cond: String,
        then: Vec<Step>,
        other: Vec<Step>,
    },
}

fn store_call(node: &Node<'_>) -> bool {
    let Some(call) = node.as_call_node() else {
        return false;
    };
    constant_id_str(&call.name()) == "session_store"
        && call
            .receiver()
            .and_then(|r| r.as_call_node())
            .is_some_and(|r| constant_id_str(&r.name()) == "config" && r.arguments().is_none())
}

fn text(node: &Node<'_>, src: &str) -> String {
    let loc = node.location();
    src[loc.start_offset()..loc.end_offset()].to_string()
}

fn store_key(node: &Node<'_>) -> Result<String, String> {
    let call = node
        .as_call_node()
        .ok_or_else(|| "session_store is not a call".to_string())?;
    let args = call
        .arguments()
        .ok_or("session_store has no store argument")?;
    let mut key = "_session_id".to_string();
    for arg in args.arguments().iter() {
        let Some(kwargs) = arg.as_keyword_hash_node() else {
            continue;
        };
        for element in kwargs.elements().iter() {
            let pair = element
                .as_assoc_node()
                .ok_or("a session_store options splat")?;
            if symbol_value(&pair.key()).as_deref() == Some("key") {
                key = string_value(&pair.value())
                    .ok_or("session_store key is not a literal String")?;
            }
        }
    }
    Ok(key)
}

/// Steps among `stmts`. When `strict`, any statement that is not a
/// supported store call or a container of only such calls is refusal —
/// the ledger needs that so mixed siblings stay Unconsumed. Synthesize
/// walks leniently and skips unread siblings the way Rails ignores them
/// for this one setting.
fn steps(stmts: Vec<Node<'_>>, src: &str, strict: bool) -> Result<Vec<Step>, String> {
    let mut out = Vec::new();
    for stmt in stmts {
        if let Some(class) = stmt.as_class_node() {
            out.extend(steps(
                class.body().map(flatten_statements).unwrap_or_default(),
                src,
                strict,
            )?);
            continue;
        }
        if let Some(module) = stmt.as_module_node() {
            out.extend(steps(
                module.body().map(flatten_statements).unwrap_or_default(),
                src,
                strict,
            )?);
            continue;
        }
        if let Some(call) = stmt.as_call_node() {
            if constant_id_str(&call.name()) == "configure" {
                if let Some(body) = call
                    .block()
                    .and_then(|b| b.as_block_node())
                    .and_then(|b| b.body())
                {
                    out.extend(steps(flatten_statements(body), src, strict)?);
                    continue;
                }
            }
        }
        if let Some(step) = conditional(&stmt, src, strict)? {
            out.push(step);
            continue;
        }
        if store_call(&stmt) {
            out.push(Step::Set(store_key(&stmt)));
            continue;
        }
        if strict {
            return Err(
                "session_store shares its statement with something this lift does not read"
                    .to_string(),
            );
        }
    }
    Ok(out)
}

fn conditional(stmt: &Node<'_>, src: &str, strict: bool) -> Result<Option<Step>, String> {
    let body = |n: Option<ruby_prism::StatementsNode<'_>>| -> Result<Vec<Step>, String> {
        match n {
            Some(s) => steps(s.body().iter().collect(), src, strict),
            None => Ok(Vec::new()),
        }
    };
    let (cond, then, other) = if let Some(node) = stmt.as_if_node() {
        let other = match node.subsequent() {
            None => Vec::new(),
            Some(next) => match next.as_else_node() {
                Some(e) => body(e.statements())?,
                None => conditional(&next, src, strict)?.into_iter().collect(),
            },
        };
        (
            text(&node.predicate(), src),
            body(node.statements())?,
            other,
        )
    } else if let Some(node) = stmt.as_unless_node() {
        let other = match node.else_clause() {
            Some(e) => body(e.statements())?,
            None => Vec::new(),
        };
        (
            text(&node.predicate(), src),
            body(node.statements())?,
            other,
        )
    } else {
        return Ok(None);
    };
    if then.is_empty() && other.is_empty() {
        return Ok(None);
    }
    let cond = if stmt.as_unless_node().is_some() {
        format!("unless {cond}")
    } else {
        format!("if {cond}")
    };
    Ok(Some(Step::If { cond, then, other }))
}

fn count(steps: &[Step]) -> usize {
    steps
        .iter()
        .map(|s| match s {
            Step::Set(_) => 1,
            Step::If { then, other, .. } => count(then) + count(other),
        })
        .sum()
}

fn only_sets(steps: &[Step]) -> bool {
    steps.iter().all(|s| match s {
        Step::Set(Ok(_)) => true,
        Step::Set(Err(_)) => false,
        Step::If { then, other, .. } => only_sets(then) && only_sets(other),
    })
}

/// Ruby that assigns `key` in execution order.
fn replay(steps: &[Step], indent: &str) -> Result<String, String> {
    let mut out = String::new();
    for step in steps {
        match step {
            Step::Set(Ok(key)) => out.push_str(&format!("{indent}key = {key:?}\n")),
            Step::Set(Err(why)) => return Err(why.clone()),
            Step::If { cond, then, other } => {
                let inner = format!("{indent}  ");
                let then_code = replay(then, &inner)?;
                let other_code = replay(other, &inner)?;
                // `cond` is already `if …` or `unless …`.
                out.push_str(&format!("{indent}{cond}\n{then_code}"));
                out.push_str(&format!("{indent}else\n{other_code}"));
                out.push_str(&format!("{indent}end\n"));
            }
        }
    }
    Ok(out)
}

fn read_file<V: Vfs + ?Sized>(vfs: &V, path: &Path) -> IngestResult<Vec<Step>> {
    let Ok(source) = vfs.read(path) else {
        return Ok(Vec::new());
    };
    let file = path.display().to_string();
    let result = super::prism::parse(&source, &file);
    let src = String::from_utf8_lossy(&source).into_owned();
    let stmts = result
        .node()
        .as_program_node()
        .map(|p| p.statements().body().iter().collect::<Vec<_>>())
        .unwrap_or_default();
    let found = match steps(stmts, &src, false) {
        Ok(found) => found,
        Err(message) => {
            survey::continue_or_fail(IngestError::Unsupported {
                file: file.clone(),
                message,
            })?;
            return Ok(Vec::new());
        }
    };
    let mentioned = {
        struct Calls(usize);
        impl<'pr> ruby_prism::Visit<'pr> for Calls {
            fn visit_call_node(&mut self, call: &ruby_prism::CallNode<'pr>) {
                if store_call(&call.as_node()) {
                    self.0 += 1;
                }
                ruby_prism::visit_call_node(self, call);
            }
        }
        let mut calls = Calls(0);
        calls.visit(&result.node());
        calls.0
    };
    let refused = if mentioned > count(&found) {
        Some(
            "session_store is outside a supported config body or conditional".to_string(),
        )
    } else {
        replay(&found, "").err()
    };
    if let Some(message) = refused {
        survey::continue_or_fail(IngestError::Unsupported { file, message })?;
        return Ok(Vec::new());
    }
    Ok(found)
}

/// True when `source` (one ledger statement) is only supported
/// `session_store` declarations — possibly wrapped in `configure` /
/// `if` / `unless`.
pub(super) fn reads(source: &[u8]) -> bool {
    let parsed = ruby_prism::parse(source);
    let src = String::from_utf8_lossy(source);
    let stmts = parsed
        .node()
        .as_program_node()
        .map(|p| p.statements().body().iter().collect::<Vec<_>>())
        .unwrap_or_else(|| flatten_statements(parsed.node()));
    match steps(stmts, &src, true) {
        Ok(found) => !found.is_empty() && only_sets(&found),
        Err(_) => false,
    }
}

fn application_name(nodes: Vec<Node<'_>>, prefix: &[String]) -> Option<String> {
    for node in nodes {
        if let Some(module) = node.as_module_node() {
            let mut path = prefix.to_vec();
            path.extend(constant_path_of(&module.constant_path())?);
            if let Some(name) = application_name(
                module.body().map(flatten_statements).unwrap_or_default(),
                &path,
            ) {
                return Some(name);
            }
        } else if let Some(class) = node.as_class_node() {
            let mut path = prefix.to_vec();
            path.extend(constant_path_of(&class.constant_path())?);
            if class
                .superclass()
                .and_then(|n| constant_path_of(&n))
                .is_some_and(|p| p == ["Rails", "Application"])
            {
                return Some(path.join("::"));
            }
            if let Some(name) = application_name(
                class.body().map(flatten_statements).unwrap_or_default(),
                &path,
            ) {
                return Some(name);
            }
        }
    }
    None
}

fn default_key(name: &str) -> String {
    let underscored = crate::naming::underscore_words(name);
    format!(
        "_{}_session",
        underscored
            .strip_suffix("_application")
            .unwrap_or(&underscored)
    )
}

/// Ruby source for the reopen's `session_cookie_key`, None when the app
/// has no Application class and no store declarations (the runtime
/// default in `rails.rb` stands).
pub(super) fn synthesize<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
) -> IngestResult<Option<String>> {
    let app_path = dir.join("config/application.rb");
    let app_bytes = vfs.read(&app_path).unwrap_or_default();
    let app_parsed = ruby_prism::parse(&app_bytes);
    let name = application_name(
        app_parsed
            .node()
            .as_program_node()
            .map(|p| p.statements().body().iter().collect())
            .unwrap_or_default(),
        &[],
    );
    let application = read_file(vfs, &app_path)?;
    let env_dir = dir.join("config/environments");
    let mut env_files: Vec<PathBuf> = if vfs.is_dir(&env_dir) {
        vfs.read_dir(&env_dir)
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "rb"))
            .collect()
    } else {
        Vec::new()
    };
    env_files.sort();
    let mut environments: Vec<(String, Vec<Step>)> = Vec::new();
    for path in &env_files {
        let Some(env) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let found = read_file(vfs, path)?;
        if !found.is_empty() {
            environments.push((env.to_string(), found));
        }
    }
    let init_dir = dir.join("config/initializers");
    let mut initializers = Vec::new();
    if vfs.is_dir(&init_dir) {
        for path in super::app::read_rb_files(vfs, &init_dir)? {
            initializers.extend(read_file(vfs, &path)?);
        }
    }
    if name.is_none() && application.is_empty() && environments.is_empty() && initializers.is_empty()
    {
        return Ok(None);
    }
    let key = name
        .as_deref()
        .map(default_key)
        .unwrap_or_else(|| "_session".to_string());
    let code = |found: &[Step]| {
        replay(found, "").map_err(|message| IngestError::Unsupported {
            file: "config/ (config.session_store)".to_string(),
            message,
        })
    };
    let mut src = format!("def session_cookie_key\nkey = {key:?}\n");
    src.push_str(&code(&application)?);
    for (env, found) in &environments {
        let body = code(found)?;
        src.push_str(&format!("if Rails.env.to_s == {env:?}\n{body}end\n"));
    }
    src.push_str(&code(&initializers)?);
    src.push_str("key\nend\n");
    Ok(Some(src))
}

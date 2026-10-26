//! Action Cable's request-origin settings, lifted onto the
//! `Rails::Application` reopen.
//!
//! Rails checks every `/cable` handshake with
//! `ActionCable::Connection::Base#allow_request_origin?` (actioncable
//! 8.1.4):
//!
//! ```ruby
//! return true if server.config.disable_request_forgery_protection
//! proto = Rack::Request.new(env).ssl? ? "https" : "http"
//! if server.config.allow_same_origin_as_host && env["HTTP_ORIGIN"] == "#{proto}://#{env['HTTP_HOST']}"
//!   true
//! elsif Array(server.config.allowed_request_origins).any? { |allowed_origin| allowed_origin === env["HTTP_ORIGIN"] }
//!   true
//! else
//!   false
//! end
//! ```
//!
//! and the three settings come from `config.action_cable.*`, which an app
//! assigns in `config/application.rb`, in `config/environments/<env>.rb`
//! or in an initializer. The engine's `action_cable.set_configs`
//! initializer fills one default between the environment file and the
//! app's initializers: `allowed_request_origins ||= /https?:\/\/localhost:\d+/`
//! in development only. `allow_same_origin_as_host` defaults to true and
//! `disable_request_forgery_protection` to false.
//!
//! The values are app code (an origin is often read from `ENV` or from an
//! app constant), so they are not evaluated here. Each becomes a method
//! on the reopen, as the `config.x` lift does, with one branch per
//! environment that assigns something: `action_cable_origin_listed?`
//! compares the Origin with `===` against each listed value, in order,
//! and the two flags answer a Boolean. The runtime defaults for all three
//! live in `runtime/ruby/rails.rb`.
//!
//! An assignment this cannot read faithfully is refused (an ingest error
//! in strict mode, a survey gap otherwise) rather than dropped: a value
//! that is neither a literal nor an Array literal (it may be an Array at
//! run time, which Rails would flatten through `Array()`), a splat in the
//! list, and any mention of a key that is not a plain top-level
//! assignment (`<<`, an assignment under an `if`).

use std::path::{Path, PathBuf};

use ruby_prism::Node;

use super::survey;
use super::util::{constant_id_str, flatten_statements};
use super::{IngestError, IngestResult};
use crate::vfs::Vfs;

use crate::ingest::prism_api::PrismLocation;
const ORIGINS: &str = "allowed_request_origins";
const SAME_ORIGIN: &str = "allow_same_origin_as_host";
const DISABLE: &str = "disable_request_forgery_protection";
const KEYS: [&str; 3] = [ORIGINS, SAME_ORIGIN, DISABLE];

/// Rails' development default for `allowed_request_origins`.
const DEVELOPMENT_ORIGINS: &str = r"/https?:\/\/localhost:\d+/";

/// What one assignment sets.
#[derive(Clone, Debug, PartialEq)]
enum Value {
    /// `allowed_request_origins`: the source of each value Rails'
    /// `Array(...)` would yield, in order. `nil` is an empty list that the
    /// development default still replaces (`||=`).
    Origins { listed: Vec<String>, nil: bool },
    /// A flag's expression, as written.
    Flag(String),
}

/// The key and value node of a `config.action_cable.<key> = <value>`
/// statement (`config` bare, as in the Application class body and an
/// environment file, or `Rails.application.config`, as in an
/// initializer). None for anything else.
pub(super) fn assignment<'pr>(node: &Node<'pr>) -> Option<(&'static str, Node<'pr>)> {
    let call = node.as_call_node()?;
    let name = constant_id_str(&call.name());
    let key = KEYS.iter().find(|k| name.strip_suffix('=') == Some(**k))?;
    let section = call.receiver()?;
    let section = section.as_call_node()?;
    if constant_id_str(&section.name()) != "action_cable" || section.arguments().is_some() {
        return None;
    }
    let config = section.receiver()?;
    let config = config.as_call_node()?;
    if constant_id_str(&config.name()) != "config" || config.arguments().is_some() {
        return None;
    }
    let anchored = match config.receiver() {
        None => true,
        Some(inner) => inner
            .as_call_node()
            .is_some_and(|c| constant_id_str(&c.name()) == "application"),
    };
    if !anchored {
        return None;
    }
    let args = call.arguments()?;
    let mut values = args.arguments().iter();
    let value = values.next()?;
    values.next().is_none().then_some((key, value))
}

/// Statements of one file in the containers Rails' config lines live in:
/// the top level, class and module bodies, and `configure do ... end`
/// blocks. The same discipline as `app::extract_config_assignments`.
fn assignments_in(source: &[u8], file: &str) -> (Vec<(&'static str, Result<Value, String>)>, usize) {
    fn walk<'pr>(
        stmts: Vec<Node<'pr>>,
        src: &str,
        out: &mut Vec<(&'static str, Result<Value, String>)>,
    ) {
        for stmt in stmts {
            if let Some(class) = stmt.as_class_node() {
                walk(class.body().map(flatten_statements).unwrap_or_default(), src, out);
                continue;
            }
            if let Some(module) = stmt.as_module_node() {
                walk(module.body().map(flatten_statements).unwrap_or_default(), src, out);
                continue;
            }
            if let Some(call) = stmt.as_call_node() {
                if constant_id_str(&call.name()) == "configure" {
                    if let Some(body) =
                        call.block().and_then(|b| b.as_block_node()).and_then(|b| b.body())
                    {
                        walk(flatten_statements(body), src, out);
                        continue;
                    }
                }
            }
            let Some((key, value)) = assignment(&stmt) else { continue };
            let read = if key == ORIGINS { origins(&value, src) } else { Ok(Value::Flag(text(&value, src))) };
            out.push((key, read));
        }
    }
    let result = super::prism::parse(source, file);
    let src = String::from_utf8_lossy(source).into_owned();
    let stmts = result
        .node()
        .as_program_node()
        .map(|p| p.statements().body().iter().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut out = Vec::new();
    walk(stmts, &src, &mut out);
    (out, mentions(&src))
}

fn text(node: &Node<'_>, src: &str) -> String {
    let loc = node.location();
    src[loc.start_offset()..loc.end_offset()].to_string()
}

/// `Array(value)`'s elements, when the source says what they are.
fn origins(value: &Node<'_>, src: &str) -> Result<Value, String> {
    if value.as_nil_node().is_some() {
        return Ok(Value::Origins { listed: Vec::new(), nil: true });
    }
    if let Some(array) = value.as_array_node() {
        let mut listed = Vec::new();
        for element in array.elements().iter() {
            if element.as_splat_node().is_some() {
                return Err("a splat in `config.action_cable.allowed_request_origins`".to_string());
            }
            listed.push(text(&element, src));
        }
        return Ok(Value::Origins { listed, nil: false });
    }
    if is_scalar_literal(value) {
        return Ok(Value::Origins { listed: vec![text(value, src)], nil: false });
    }
    Err(format!(
        "`config.action_cable.allowed_request_origins = {}` is neither a literal nor an Array literal, \
         and Rails reads it through `Array()`",
        text(value, src)
    ))
}

/// A String or Regexp literal: `Array()` of one is a list of one.
fn is_scalar_literal(node: &Node<'_>) -> bool {
    node.as_string_node().is_some()
        || node.as_interpolated_string_node().is_some()
        || node.as_regular_expression_node().is_some()
        || node.as_interpolated_regular_expression_node().is_some()
}

/// How many times the code (comments aside) names one of the keys under
/// `action_cable`, assignment or not.
fn mentions(src: &str) -> usize {
    src.lines()
        .map(|line| line.split('#').next().unwrap_or(""))
        .map(|code| KEYS.iter().map(|k| code.matches(&format!("action_cable.{k}")).count()).sum::<usize>())
        .sum()
}

/// One config file's assignments, or a refusal naming the file.
fn read_file<V: Vfs + ?Sized>(
    vfs: &V,
    path: &Path,
) -> IngestResult<Vec<(&'static str, Value)>> {
    let Ok(source) = vfs.read(path) else { return Ok(Vec::new()) };
    let file = path.display().to_string();
    let (found, mentioned) = assignments_in(&source, &file);
    let mut out = Vec::new();
    let mut refused = None;
    for (key, value) in &found {
        match value {
            Ok(v) => out.push((*key, v.clone())),
            Err(why) => refused = Some(why.clone()),
        }
    }
    if refused.is_none() && mentioned > found.len() {
        refused = Some(
            "a `config.action_cable` origin setting that is not a top-level assignment \
             (Rails' handshake check reads it; this lift reads only `config.action_cable.<key> = <value>`)"
                .to_string(),
        );
    }
    if let Some(message) = refused {
        survey::continue_or_fail(IngestError::Unsupported { file, message })?;
        // Survey mode: the file's settings stay unread, so the runtime
        // defaults stand for them and the gap is on the ledger.
        return Ok(Vec::new());
    }
    Ok(out)
}

fn last<'a>(assigned: &'a [(&'static str, Value)], key: &str) -> Option<&'a Value> {
    assigned.iter().rev().find(|(k, _)| *k == key).map(|(_, v)| v)
}

/// The effective value of `key` in one environment, in Rails' order:
/// `config/application.rb`, then the environment file, then the engine's
/// development default for the origin list, then the app's initializers.
fn effective(
    key: &str,
    application: &[(&'static str, Value)],
    environment: &[(&'static str, Value)],
    initializers: &[(&'static str, Value)],
    development: bool,
) -> Option<Value> {
    let mut value = last(environment, key).or_else(|| last(application, key)).cloned();
    if key == ORIGINS && development {
        let unset = matches!(value, None | Some(Value::Origins { nil: true, .. }));
        if unset {
            value = Some(Value::Origins { listed: vec![DEVELOPMENT_ORIGINS.to_string()], nil: false });
        }
    }
    last(initializers, key).cloned().or(value)
}

/// The method body's answer for one environment.
fn answer(key: &str, value: Option<&Value>, indent: &str) -> String {
    match (key, value) {
        (ORIGINS, Some(Value::Origins { listed, .. })) => {
            let mut out = String::new();
            for allowed in listed {
                out.push_str(&format!("{indent}return true if ({allowed}) === origin\n"));
            }
            out.push_str(&format!("{indent}return false\n"));
            out
        }
        (ORIGINS, _) => format!("{indent}return false\n"),
        (_, Some(Value::Flag(expr))) => format!("{indent}return ({expr}) ? true : false\n"),
        (SAME_ORIGIN, _) => format!("{indent}return true\n"),
        _ => format!("{indent}return false\n"),
    }
}

/// Ruby source for the reopen's overrides, one method per key the app
/// assigns anywhere; None when it assigns none (the runtime defaults are
/// Rails' then).
pub(super) fn synthesize<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Option<String>> {
    let application = read_file(vfs, &dir.join("config/application.rb"))?;
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
    let mut environments: Vec<(String, Vec<(&'static str, Value)>)> = Vec::new();
    for path in &env_files {
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        environments.push((name.to_string(), read_file(vfs, path)?));
    }
    if !environments.iter().any(|(name, _)| name == "development") {
        environments.push(("development".to_string(), Vec::new()));
    }
    let init_dir = dir.join("config/initializers");
    let mut initializers = Vec::new();
    if vfs.is_dir(&init_dir) {
        for path in super::app::read_rb_files(vfs, &init_dir)? {
            initializers.extend(read_file(vfs, &path)?);
        }
    }

    let assigned = |key: &str| {
        application.iter().chain(initializers.iter()).any(|(k, _)| *k == key)
            || environments.iter().any(|(_, a)| a.iter().any(|(k, _)| *k == key))
    };
    let mut src = String::new();
    for key in KEYS {
        if !assigned(key) {
            continue;
        }
        let (name, param) = match key {
            ORIGINS => ("action_cable_origin_listed?", "(origin)"),
            SAME_ORIGIN => ("action_cable_allow_same_origin_as_host", ""),
            _ => ("action_cable_disable_request_forgery_protection", ""),
        };
        src.push_str(&format!("def {name}{param}\n  env = Rails.env.to_s\n"));
        for (env, settings) in &environments {
            let value = effective(key, &application, settings, &initializers, env == "development");
            src.push_str(&format!("  if env == {env:?}\n{}  end\n", answer(key, value.as_ref(), "    ")));
        }
        let other = effective(key, &application, &[], &initializers, false);
        src.push_str(&answer(key, other.as_ref(), "  "));
        src.push_str("end\n");
    }
    Ok((!src.is_empty()).then_some(src))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MapVfs;
    use std::collections::HashMap;

    fn synth(files: &[(&str, &str)]) -> IngestResult<Option<String>> {
        let map: HashMap<PathBuf, Vec<u8>> =
            files.iter().map(|(p, s)| (PathBuf::from("/app").join(p), s.as_bytes().to_vec())).collect();
        synthesize(&MapVfs::new(map), Path::new("/app"))
    }

    #[test]
    fn nothing_assigned_keeps_the_runtime_defaults() {
        let out = synth(&[(
            "config/application.rb",
            "module Blog\n  class Application < Rails::Application\n    config.load_defaults 8.1\n  end\nend\n",
        )])
        .unwrap();
        assert_eq!(out, None);
    }

    #[test]
    fn each_environment_reads_its_own_list_and_development_keeps_rails_default_when_unset() {
        let out = synth(&[
            ("config/application.rb", "module Blog\n  class Application < Rails::Application\n  end\nend\n"),
            (
                "config/environments/production.rb",
                "Rails.application.configure do\n  config.action_cable.allowed_request_origins = [\n    AppOrigin.url,\n    %r{\\Ahttps://[\\w.-]+\\.ts\\.net\\z}\n  ]\nend\n",
            ),
            ("config/environments/test.rb", "Rails.application.configure do\nend\n"),
        ])
        .unwrap()
        .expect("an override");
        assert!(out.starts_with("def action_cable_origin_listed?(origin)\n  env = Rails.env.to_s\n"), "{out}");
        assert!(
            out.contains("  if env == \"production\"\n    return true if (AppOrigin.url) === origin\n    return true if (%r{\\Ahttps://[\\w.-]+\\.ts\\.net\\z}) === origin\n    return false\n  end\n"),
            "{out}"
        );
        assert!(out.contains("  if env == \"test\"\n    return false\n  end\n"), "{out}");
        assert!(
            out.contains(&format!("  if env == \"development\"\n    return true if ({DEVELOPMENT_ORIGINS}) === origin\n")),
            "{out}"
        );
        assert!(out.ends_with("  return false\nend\n"), "{out}");
        assert!(!out.contains("allow_same_origin_as_host"), "{out}");
    }

    #[test]
    fn an_initializer_wins_and_flags_keep_their_expression() {
        let out = synth(&[
            (
                "config/application.rb",
                "module Blog\n  class Application < Rails::Application\n    config.action_cable.allowed_request_origins = \"https://a.example\"\n    config.action_cable.allow_same_origin_as_host = false\n  end\nend\n",
            ),
            ("config/environments/development.rb", "Rails.application.configure do\n  config.action_cable.disable_request_forgery_protection = true\nend\n"),
            ("config/initializers/cable.rb", "Rails.application.config.action_cable.allowed_request_origins = nil\n"),
        ])
        .unwrap()
        .expect("an override");
        assert!(!out.contains("a.example"), "the initializer's nil replaces the list: {out}");
        assert!(out.contains("def action_cable_allow_same_origin_as_host\n"), "{out}");
        assert!(out.contains("    return (false) ? true : false\n"), "{out}");
        assert!(
            out.contains("def action_cable_disable_request_forgery_protection\n  env = Rails.env.to_s\n  if env == \"development\"\n    return (true) ? true : false\n  end\n  return false\nend\n"),
            "{out}"
        );
    }

    #[test]
    fn a_value_rails_would_flatten_or_an_unread_mention_is_refused() {
        for (path, source) in [
            ("config/environments/production.rb", "Rails.application.configure do\n  config.action_cable.allowed_request_origins = ENV.fetch(\"ORIGINS\").split(\",\")\nend\n"),
            ("config/environments/production.rb", "Rails.application.configure do\n  config.action_cable.allowed_request_origins = [*EXTRA]\nend\n"),
            ("config/environments/production.rb", "Rails.application.configure do\n  if ENV[\"X\"]\n    config.action_cable.allowed_request_origins = [\"https://a\"]\n  end\nend\n"),
            ("config/initializers/cable.rb", "Rails.application.config.action_cable.allowed_request_origins << \"https://a\"\n"),
        ] {
            let err = synth(&[(path, source)]).expect_err(source);
            assert!(err.to_string().contains("unsupported construct"), "{err}");
        }
    }
}

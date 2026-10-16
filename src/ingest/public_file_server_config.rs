//! `config.public_file_server.headers`, lifted onto the
//! `Rails::Application` reopen.
//!
//! Rails serves `public/` (and the compiled `public/assets/`) through
//! `ActionDispatch::Static`, which hands every file response the headers
//! this setting names (railties 8.1.4 `default_middleware_stack`:
//! `ActionDispatch::Static, paths["public"].first, index:, headers:
//! config.public_file_server.headers`). Its default is unset, which sends
//! no extra header; the generated `production.rb` sets a far-future
//! `Cache-Control`. That header is what lets a browser keep a stylesheet,
//! and what lets a cache in front of the app (Thruster, in campfire's own
//! Dockerfile) answer `/assets/*` without asking the app again. Without
//! it, the emitted tree served the same bytes with no `Cache-Control` at
//! all.
//!
//! The value is app code (campfire writes `"public, max-age=#{30.days.to_i}"`),
//! so it is not evaluated here, apart from the generator's own
//! `#{N.days.to_i}` spelling, folded to seconds (`fold_durations` says
//! why). It becomes `public_file_server_headers` on the reopen, returning
//! the app's own Hash expression. The runtime default (`{}`) lives in
//! `runtime/ruby/rails.rb`.
//!
//! THE ASSIGNMENTS ARE REPLAYED, NOT EVALUATED. Rails runs
//! `config/application.rb`, then the environment's file, then the
//! initializers, and the last assignment to execute wins. Generated
//! `development.rb` makes one conditionally (`if Rails.root.join(
//! "tmp/caching-dev.txt").exist?`), and campfire's `production.rb` makes
//! two in a row, a per-path lambda and then a plain Hash that replaces
//! it. So the method is those assignments in REVERSE execution order,
//! each one a `return`, each `if`/`unless` kept with its own condition:
//! the first `return` reached is the last assignment Rails would have
//! run, and an assignment shadowed by a later unconditional one is never
//! generated, never typed and never refused (campfire's lambda).
//!
//! What this cannot type is refused (an ingest error in strict mode, a
//! survey gap otherwise) rather than dropped, but only when it is
//! reachable: a value that is not a Hash literal, a Hash value that is a
//! lambda or proc (Rails calls it per path; the runtime's headers are one
//! `Hash[String, String]`), and any mention of the key that is not an
//! assignment at the top level of a config body or of an `if`/`unless`
//! inside one (`headers["X"] = …`, an assignment in a loop or a method).

use std::path::{Path, PathBuf};

use ruby_prism::Node;

use super::survey;
use super::util::{constant_id_str, flatten_statements};
use super::{IngestError, IngestResult};
use crate::vfs::Vfs;

/// One statement that can change the setting, in execution order.
#[derive(Clone, Debug)]
enum Step {
    /// `config.public_file_server.headers = <Hash>`: the Hash's source,
    /// or why it cannot be typed.
    Set(Result<String, String>),
    /// An `if`/`unless` whose branches assign: the condition as written
    /// (negated for `unless`), the steps of each branch.
    If { cond: String, then: Vec<Step>, other: Vec<Step> },
}

/// The `config.public_file_server.headers = <value>` value node
/// (`config` bare, as in the Application class body and an environment
/// file, or `Rails.application.config`, as in an initializer). None for
/// anything else.
fn assignment<'pr>(node: &Node<'pr>) -> Option<Node<'pr>> {
    let call = node.as_call_node()?;
    if constant_id_str(&call.name()) != "headers=" {
        return None;
    }
    let section = call.receiver()?;
    let section = section.as_call_node()?;
    if constant_id_str(&section.name()) != "public_file_server" || section.arguments().is_some() {
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
    values.next().is_none().then_some(value)
}

fn text(node: &Node<'_>, src: &str) -> String {
    let loc = node.location();
    src[loc.start_offset()..loc.end_offset()].to_string()
}

/// The Hash expression's source, when it is one the runtime can type.
fn headers(value: &Node<'_>, src: &str) -> Result<String, String> {
    let Some(hash) = value.as_hash_node() else {
        return Err(format!(
            "`config.public_file_server.headers = {}` is not a Hash literal",
            text(value, src)
        ));
    };
    for element in hash.elements().iter() {
        let Some(pair) = element.as_assoc_node() else {
            return Err("a splat in `config.public_file_server.headers`".to_string());
        };
        let v = pair.value();
        if v.as_lambda_node().is_some()
            || v.as_call_node()
                .is_some_and(|c| matches!(constant_id_str(&c.name()), "lambda" | "proc") && c.block().is_some())
        {
            return Err(format!(
                "`config.public_file_server.headers` gives `{}` a lambda, which Rails calls per path; \
                 the emitted static server sends one fixed set of headers",
                text(&pair.key(), src)
            ));
        }
    }
    Ok(fold_durations(&text(value, src)))
}

/// `#{30.days.to_i}` and the like, folded to the seconds Rails computes
/// for them (activesupport 8.1.4 `ActiveSupport::Duration::PARTS_IN_SECONDS`:
/// a month is 1/12 of a 365.2425-day year). Every generated environment
/// file and campfire spell the max-age this way, and the strict targets
/// ship no Duration, so left as written the lift would turn real-blog's
/// generated config into an error diagnostic on every one of them. An
/// interpolation of any other shape is left as written.
fn fold_durations(text: &str) -> String {
    const UNITS: [(&str, u64); 7] = [
        ("second", 1),
        ("minute", 60),
        ("hour", 3_600),
        ("day", 86_400),
        ("week", 604_800),
        ("month", 2_629_746),
        ("year", 31_556_952),
    ];
    let seconds = |inner: &str| -> Option<u64> {
        let rest = inner.trim().strip_suffix(".to_i")?;
        let (count, unit) = rest.split_once('.')?;
        let count: u64 = count.replace('_', "").parse().ok()?;
        let unit = unit.strip_suffix('s').unwrap_or(unit);
        let (_, per) = UNITS.iter().find(|(name, _)| *name == unit)?;
        count.checked_mul(*per)
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("#{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}').and_then(|end| seconds(&after[..end]).map(|n| (end, n))) {
            Some((end, n)) => {
                out.push_str(&n.to_string());
                rest = &after[end + 1..];
            }
            None => {
                out.push_str("#{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The steps among `stmts`, recursing into the containers Rails' config
/// lines live in (class and module bodies, `configure do ... end`) and
/// into `if`/`unless` branches.
fn steps(stmts: Vec<Node<'_>>, src: &str) -> Vec<Step> {
    let mut out = Vec::new();
    for stmt in stmts {
        if let Some(class) = stmt.as_class_node() {
            out.extend(steps(class.body().map(flatten_statements).unwrap_or_default(), src));
            continue;
        }
        if let Some(module) = stmt.as_module_node() {
            out.extend(steps(module.body().map(flatten_statements).unwrap_or_default(), src));
            continue;
        }
        if let Some(call) = stmt.as_call_node() {
            if constant_id_str(&call.name()) == "configure" {
                if let Some(body) = call.block().and_then(|b| b.as_block_node()).and_then(|b| b.body()) {
                    out.extend(steps(flatten_statements(body), src));
                    continue;
                }
            }
        }
        if let Some(step) = conditional(&stmt, src) {
            out.push(step);
            continue;
        }
        if let Some(value) = assignment(&stmt) {
            out.push(Step::Set(headers(&value, src)));
        }
    }
    out
}

/// An `if`/`elsif`/`else` or `unless`/`else` statement whose branches
/// assign the setting; None when no branch does.
fn conditional(stmt: &Node<'_>, src: &str) -> Option<Step> {
    let body = |n: Option<ruby_prism::StatementsNode<'_>>| -> Vec<Step> {
        n.map(|s| steps(s.body().iter().collect(), src)).unwrap_or_default()
    };
    let (cond, then, other) = if let Some(node) = stmt.as_if_node() {
        let other = match node.subsequent() {
            None => Vec::new(),
            Some(next) => match next.as_else_node() {
                Some(e) => body(e.statements()),
                // `elsif`: a nested if, as Ruby reads it.
                None => conditional(&next, src).into_iter().collect(),
            },
        };
        (format!("({})", text(&node.predicate(), src)), body(node.statements()), other)
    } else if let Some(node) = stmt.as_unless_node() {
        let other = node.else_clause().map(|e| body(e.statements())).unwrap_or_default();
        (format!("!({})", text(&node.predicate(), src)), body(node.statements()), other)
    } else {
        return None;
    };
    (!then.is_empty() || !other.is_empty()).then_some(Step::If { cond, then, other })
}

/// How many assignments the steps hold, reachable or not.
fn count(steps: &[Step]) -> usize {
    steps
        .iter()
        .map(|s| match s {
            Step::Set(_) => 1,
            Step::If { then, other, .. } => count(then) + count(other),
        })
        .sum()
}

/// Ruby for `steps` in reverse execution order: the first `return`
/// reached is the last assignment to run. Returns the code and whether
/// it always returns (an unconditional assignment was reached, so what
/// would follow it is dead). A reachable assignment that cannot be typed
/// is the error.
fn replay(steps: &[Step], indent: &str) -> Result<(String, bool), String> {
    let mut out = String::new();
    for step in steps.iter().rev() {
        match step {
            Step::Set(Ok(expr)) => {
                out.push_str(&format!("{indent}return ({expr})\n"));
                return Ok((out, true));
            }
            Step::Set(Err(why)) => return Err(why.clone()),
            Step::If { cond, then, other } => {
                let inner = format!("{indent}  ");
                let (then_code, then_done) = replay(then, &inner)?;
                let (other_code, other_done) = replay(other, &inner)?;
                out.push_str(&format!("{indent}if {cond}\n{then_code}"));
                if !other_code.is_empty() {
                    out.push_str(&format!("{indent}else\n{other_code}"));
                }
                out.push_str(&format!("{indent}end\n"));
                if then_done && other_done {
                    return Ok((out, true));
                }
            }
        }
    }
    Ok((out, false))
}

/// One config file's steps, or a refusal naming the file.
fn read_file<V: Vfs + ?Sized>(vfs: &V, path: &Path) -> IngestResult<Vec<Step>> {
    let Ok(source) = vfs.read(path) else { return Ok(Vec::new()) };
    let file = path.display().to_string();
    let result = super::prism::parse(&source, &file);
    let src = String::from_utf8_lossy(&source).into_owned();
    let stmts = result
        .node()
        .as_program_node()
        .map(|p| p.statements().body().iter().collect::<Vec<_>>())
        .unwrap_or_default();
    let found = steps(stmts, &src);
    let mentioned: usize = src
        .lines()
        .map(|line| line.split('#').next().unwrap_or(""))
        .map(|code| code.matches("public_file_server.headers").count())
        .sum();
    let refused = if mentioned > count(&found) {
        Some(
            "a `config.public_file_server.headers` setting that is not an assignment at the top \
             level of a config body or of an `if`/`unless` inside one (Rails' static file server \
             reads it; this lift reads only `config.public_file_server.headers = <Hash>`)"
                .to_string(),
        )
    } else {
        replay(&found, "").err()
    };
    if let Some(message) = refused {
        survey::continue_or_fail(IngestError::Unsupported { file, message })?;
        // Survey mode: the file's setting stays unread, so the runtime
        // default stands for it and the gap is on the ledger.
        return Ok(Vec::new());
    }
    Ok(found)
}

/// Ruby source for the reopen's `public_file_server_headers`, None when
/// the app assigns the key nowhere (the runtime default is Rails' then).
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
    let mut environments: Vec<(String, Vec<Step>)> = Vec::new();
    for path in &env_files {
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        let found = read_file(vfs, path)?;
        if !found.is_empty() {
            environments.push((name.to_string(), found));
        }
    }
    let init_dir = dir.join("config/initializers");
    let mut initializers = Vec::new();
    if vfs.is_dir(&init_dir) {
        for path in super::app::read_rb_files(vfs, &init_dir)? {
            initializers.extend(read_file(vfs, &path)?);
        }
    }
    if application.is_empty() && initializers.is_empty() && environments.is_empty() {
        return Ok(None);
    }
    // Each file was checked as it was read; a combined replay only
    // reaches a subset of what those checks did.
    let code = |steps: &[Step], indent: &str| replay(steps, indent).map_err(|message| {
        IngestError::Unsupported { file: "config/ (config.public_file_server.headers)".to_string(), message }
    });
    let mut src = String::from("def public_file_server_headers\n");
    let (init_code, init_done) = code(&initializers, "  ")?;
    src.push_str(&init_code);
    if !init_done {
        if !environments.is_empty() {
            src.push_str("  env = Rails.env.to_s\n");
        }
        for (env, found) in &environments {
            let (env_code, _) = code(found, "    ")?;
            src.push_str(&format!("  if env == {env:?}\n{env_code}  end\n"));
        }
        let (app_code, app_done) = code(&application, "  ")?;
        src.push_str(&app_code);
        if !app_done {
            src.push_str("  return {}\n");
        }
    }
    src.push_str("end\n");
    Ok(Some(src))
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
    fn nothing_assigned_keeps_the_runtime_default() {
        let out = synth(&[(
            "config/application.rb",
            "module Blog\n  class Application < Rails::Application\n    config.load_defaults 8.1\n  end\nend\n",
        )])
        .unwrap();
        assert_eq!(out, None);
    }

    #[test]
    fn the_last_assignment_wins_and_a_shadowed_lambda_is_never_read() {
        // campfire's production.rb: a per-path lambda, then a Hash that
        // replaces it.
        let out = synth(&[(
            "config/environments/production.rb",
            "Rails.application.configure do\n  config.public_file_server.headers = {\n    \"cache-control\" => lambda do |path, _|\n      \"public\"\n    end\n  }\n  config.public_file_server.headers = {\n    \"Cache-Control\" => \"public, max-age=#{30.days.to_i}\"\n  }\nend\n",
        )])
        .unwrap()
        .expect("an override");
        assert_eq!(
            out,
            "def public_file_server_headers\n  env = Rails.env.to_s\n  if env == \"production\"\n    return ({\n    \"Cache-Control\" => \"public, max-age=2592000\"\n  })\n  end\n  return {}\nend\n"
        );
    }

    #[test]
    fn a_conditional_assignment_keeps_its_condition() {
        // Generated development.rb: the caching toggle.
        let out = synth(&[(
            "config/environments/development.rb",
            "Rails.application.configure do\n  if Rails.root.join(\"tmp/caching-dev.txt\").exist?\n    config.action_controller.perform_caching = true\n    config.public_file_server.headers = { \"cache-control\" => \"public, max-age=#{2.days.to_i}\" }\n  else\n    config.action_controller.perform_caching = false\n  end\nend\n",
        )])
        .unwrap()
        .expect("an override");
        assert_eq!(
            out,
            "def public_file_server_headers\n  env = Rails.env.to_s\n  if env == \"development\"\n    if (Rails.root.join(\"tmp/caching-dev.txt\").exist?)\n      return ({ \"cache-control\" => \"public, max-age=172800\" })\n    end\n  end\n  return {}\nend\n"
        );
        // `unless` and `elsif` read as Ruby reads them.
        let out = synth(&[(
            "config/environments/production.rb",
            "Rails.application.configure do\n  unless ENV[\"A\"]\n    config.public_file_server.headers = { \"X\" => \"1\" }\n  end\n  if ENV[\"B\"]\n    x = 1\n  elsif ENV[\"C\"]\n    config.public_file_server.headers = { \"X\" => \"2\" }\n  end\nend\n",
        )])
        .unwrap()
        .expect("an override");
        assert!(
            out.contains("    if (ENV[\"B\"])\n    else\n      if (ENV[\"C\"])\n        return ({ \"X\" => \"2\" })\n      end\n    end\n    if !(ENV[\"A\"])\n      return ({ \"X\" => \"1\" })\n    end\n"),
            "{out}"
        );
    }

    #[test]
    fn generator_durations_fold_to_rails_seconds_and_other_code_stays() {
        assert_eq!(
            fold_durations("\"public, max-age=#{1.year.to_i}\" #{2.days.to_i} #{ 1_000.seconds.to_i } #{1.month.to_i}"),
            "\"public, max-age=31556952\" 172800 1000 2629746"
        );
        assert_eq!(fold_durations("#{TTL.to_i} #{2.fortnights.to_i} #{3.days} #{x"), "#{TTL.to_i} #{2.fortnights.to_i} #{3.days} #{x");
    }

    #[test]
    fn application_rb_is_the_fallback_and_an_initializer_wins() {
        let app = "module Blog\n  class Application < Rails::Application\n    config.public_file_server.headers = { \"X-A\" => \"1\" }\n  end\nend\n";
        let out = synth(&[("config/application.rb", app)]).unwrap().expect("an override");
        assert_eq!(out, "def public_file_server_headers\n  return ({ \"X-A\" => \"1\" })\nend\n");
        let out = synth(&[
            ("config/application.rb", app),
            ("config/initializers/static.rb", "Rails.application.config.public_file_server.headers = { \"X-B\" => \"2\" }\n"),
        ])
        .unwrap()
        .expect("an override");
        assert_eq!(out, "def public_file_server_headers\n  return ({ \"X-B\" => \"2\" })\nend\n");
    }

    #[test]
    fn a_reachable_lambda_or_an_unread_mention_is_refused() {
        for source in [
            "Rails.application.configure do\n  config.public_file_server.headers = { \"cache-control\" => ->(path, _) { \"public\" } }\nend\n",
            "Rails.application.configure do\n  config.public_file_server.headers = HEADERS\nend\n",
            // Behind a condition, the earlier lambda can still be the one that runs.
            "Rails.application.configure do\n  config.public_file_server.headers = { \"c\" => lambda { |p, _| \"x\" } }\n  if ENV[\"X\"]\n    config.public_file_server.headers = { \"X-A\" => \"1\" }\n  end\nend\n",
            "Rails.application.configure do\n  config.public_file_server.headers[\"X-A\"] = \"1\"\nend\n",
            "Rails.application.configure do\n  [1].each { config.public_file_server.headers = { \"X-A\" => \"1\" } }\nend\n",
        ] {
            let err = synth(&[("config/environments/production.rb", source)]).expect_err(source);
            assert!(err.to_string().contains("unsupported construct"), "{err}");
        }
    }
}

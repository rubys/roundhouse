//! `config.action_controller.perform_caching` and
//! `config.action_controller.page_cache_directory`, lifted onto the
//! `Rails::Application` reopen.
//!
//! Both gate what a controller writes to a cache. `perform_caching`
//! (actionpack 8.1.4 `AbstractController::Caching`, default true) decides
//! whether fragment and page caching do anything at all; the generated
//! `development.rb` turns it on only while `tmp/caching-dev.txt` exists,
//! and `test.rb` turns it off. `page_cache_directory` is where
//! actionpack-page_caching 1.2.5 writes a cached page; its Railtie fills
//! it with the app's `public/` when nothing assigned it
//! (`page_cache_directory ||= app.config.paths["public"].first`).
//!
//! The assignments are replayed per environment, conditions kept, the
//! way `public_file_server_config` replays the static file headers. Each
//! becomes a method on the reopen; the runtime defaults live in
//! `runtime/ruby/rails.rb`.
//!
//! What this cannot type is refused rather than dropped: a
//! `perform_caching` that is not `true` or `false`, and a
//! `page_cache_directory` given as a Proc, lambda or Symbol (Rails
//! resolves those per request on the controller; the runtime's directory
//! is one String).

use std::path::Path;

use ruby_prism::Node;

use super::public_file_server_config::{synthesize_setting, text, Setting};
use super::IngestResult;
use crate::vfs::Vfs;

fn perform_caching(value: &Node<'_>, src: &str) -> Result<String, String> {
    if value.as_true_node().is_some() || value.as_false_node().is_some() {
        return Ok(text(value, src));
    }
    Err(format!(
        "`config.action_controller.perform_caching = {}` is not `true` or `false`",
        text(value, src)
    ))
}

fn page_cache_directory(value: &Node<'_>, src: &str) -> Result<String, String> {
    let dynamic = value.as_lambda_node().is_some()
        || value.as_symbol_node().is_some()
        || value.as_call_node().is_some_and(|c| {
            matches!(super::util::constant_id_str(&c.name()), "lambda" | "proc") && c.block().is_some()
        });
    if dynamic {
        return Err(format!(
            "`config.action_controller.page_cache_directory = {}` is resolved per request on the \
             controller in Rails; the emitted runtime takes one directory",
            text(value, src)
        ));
    }
    Ok(format!("({}).to_s", text(value, src)))
}

const PERFORM_CACHING: Setting = Setting {
    section: "action_controller",
    key: "perform_caching",
    method: "action_controller_perform_caching",
    default: "true",
    read: perform_caching,
};

const PAGE_CACHE_DIRECTORY: Setting = Setting {
    section: "action_controller",
    key: "page_cache_directory",
    method: "action_controller_page_cache_directory",
    default: "Rails.public_path.to_s",
    read: page_cache_directory,
};

/// Ruby source for the reopen's overrides, one method per setting the app
/// assigns anywhere; None when it assigns neither.
pub(super) fn synthesize<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Option<String>> {
    let mut src = String::new();
    for setting in [&PERFORM_CACHING, &PAGE_CACHE_DIRECTORY] {
        if let Some(method) = synthesize_setting(vfs, dir, setting)? {
            src.push_str(&method);
        }
    }
    Ok((!src.is_empty()).then_some(src))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::{FileTree, MapVfs};
    use std::path::PathBuf;

    fn synth(files: &[(&str, &str)]) -> IngestResult<Option<String>> {
        let map: FileTree =
            files.iter().map(|(p, s)| (PathBuf::from("/app").join(p), s.as_bytes().to_vec())).collect();
        synthesize(&MapVfs::new(map), Path::new("/app"))
    }

    #[test]
    fn the_generated_development_toggle_and_a_page_cache_directory_replay() {
        // lobsters' development.rb and production.rb.
        let out = synth(&[
            (
                "config/environments/development.rb",
                "Rails.application.configure do\n  if Rails.root.join(\"tmp/caching-dev.txt\").exist?\n    config.action_controller.perform_caching = true\n    config.action_controller.page_cache_directory = Rails.public_path.join(\"cache\").to_s\n  else\n    config.action_controller.perform_caching = false\n  end\n  config.action_mailer.perform_caching = false\nend\n",
            ),
            (
                "config/environments/production.rb",
                "Rails.application.configure do\n  config.action_controller.perform_caching = true\n  config.action_controller.page_cache_directory = Rails.public_path.join(\"cache\").to_s\nend\n",
            ),
        ])
        .unwrap()
        .expect("overrides");
        assert_eq!(
            out,
            "def action_controller_perform_caching\n  env = Rails.env.to_s\n  if env == \"development\"\n    if (Rails.root.join(\"tmp/caching-dev.txt\").exist?)\n      return (true)\n    else\n      return (false)\n    end\n  end\n  if env == \"production\"\n    return (true)\n  end\n  return true\nend\n\
             def action_controller_page_cache_directory\n  env = Rails.env.to_s\n  if env == \"development\"\n    if (Rails.root.join(\"tmp/caching-dev.txt\").exist?)\n      return ((Rails.public_path.join(\"cache\").to_s).to_s)\n    end\n  end\n  if env == \"production\"\n    return ((Rails.public_path.join(\"cache\").to_s).to_s)\n  end\n  return Rails.public_path.to_s\nend\n"
        );
    }

    #[test]
    fn nothing_assigned_keeps_the_runtime_defaults() {
        assert_eq!(synth(&[("config/environments/production.rb", "Rails.application.configure do\nend\n")]).unwrap(), None);
    }

    #[test]
    fn a_computed_flag_or_a_per_request_directory_is_refused() {
        for source in [
            "Rails.application.configure do\n  config.action_controller.perform_caching = ENV[\"CACHE\"].present?\nend\n",
            "Rails.application.configure do\n  config.action_controller.page_cache_directory = -> { \"tmp\" }\nend\n",
            "Rails.application.configure do\n  config.action_controller.page_cache_directory = :cache_dir\nend\n",
        ] {
            let err = synth(&[("config/environments/production.rb", source)]).expect_err(source);
            assert!(err.to_string().contains("unsupported construct"), "{err}");
        }
    }
}

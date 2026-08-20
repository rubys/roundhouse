//! A constant reference the EMIT's nesting changed the meaning of.
//!
//! A Rails file that writes the COMPACT form — `module User::Bot`, which
//! is how every concern under `app/models/user/` is written — has
//! lexical scope `[User::Bot, Object]`. `User`'s own constants are NOT
//! in it, so `Bot::WebhookJob` there is the top-level job.
//!
//! The emit NESTS (`class User < ApplicationRecord … module Bot`),
//! which puts `User`'s constants in scope, and `Bot` stops meaning the
//! job's namespace and starts meaning the concern itself:
//! `uninitialized constant User::Bot::WebhookJob`. Same for the test
//! class beside it, which nests the same way.
//!
//! The guard is what makes this safe to do app-wide: root only where
//! the INNER reading has no target. `User::Bot` exists, which is the
//! shadow; `User::Bot::WebhookJob` does not, which is what proves the
//! body meant the outer one. A body that really does mean the nested
//! constant is untouched.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn emit() -> Vec<(String, String)> {
    let app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#,
        ),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  include Bot\nend\n",
        ),
        (
            // The COMPACT form, whose lexical scope excludes `User`.
            "app/models/user/bot.rb",
            r#"module User::Bot
  def deliver_webhook_later(message)
    Bot::WebhookJob.perform_later(self, message)
  end

  def own_helper
    Bot::Helper.call
  end
end
"#,
        ),
        (
            "app/jobs/bot/webhook_job.rb",
            "class Bot::WebhookJob < ApplicationJob\n  def perform(user, message)\n    nil\n  end\nend\n",
        ),
        (
            // The shadowing reading DOES have a target here, so the
            // reference must be left exactly as written.
            "app/models/user/bot/helper.rb",
            "class User::Bot::Helper\n  def self.call\n    1\n  end\nend\n",
        ),
    ]))
    .expect("ingest nested-constant app");
    ruby::emit_library(&app)
        .into_iter()
        .map(|f| (f.path.to_string_lossy().to_string(), f.content))
        .collect()
}

fn file_ending(suffix: &str) -> String {
    let files = emit();
    files
        .iter()
        .find(|(p, _)| p.ends_with(suffix))
        .map(|(_, c)| c.clone())
        .unwrap_or_else(|| {
            panic!(
                "no file ending in {suffix}; got {:?}",
                files.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>()
            )
        })
}

/// The shadowed reference is rooted, because the nested reading has no
/// `WebhookJob` to find.
#[test]
fn a_shadowed_constant_is_rooted() {
    let src = file_ending("user/bot.rb");
    assert!(
        src.contains("::Bot::WebhookJob"),
        "the top-level job must be reached absolutely:\n{src}"
    );
}

/// …and the reference whose nested reading DOES resolve is untouched:
/// `User::Bot::Helper` is a real class, so `Bot::Helper` means it.
#[test]
fn a_constant_the_nesting_really_owns_is_left_alone() {
    let src = file_ending("user/bot.rb");
    let helper_line = src
        .lines()
        .find(|l| l.contains("Helper"))
        .unwrap_or_else(|| panic!("no Helper reference:\n{src}"));
    assert!(
        !helper_line.contains("::Bot::Helper"),
        "the nested reading has a target — leave it:\n{helper_line}"
    );
}

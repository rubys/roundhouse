//! `config.active_job.queue_adapter` as `active_job_queue_adapter`; not refused when computed, since only whether it names solid_queue is read.

use std::path::Path;

use ruby_prism::Node;

use super::public_file_server_config::{synthesize_setting, text, Setting};
use super::IngestResult;
use crate::vfs::Vfs;

fn queue_adapter(value: &Node<'_>, src: &str) -> Result<String, String> {
    Ok(format!("({}).to_s", text(value, src)))
}

const QUEUE_ADAPTER: Setting = Setting {
    section: "active_job",
    key: "queue_adapter",
    method: "active_job_queue_adapter",
    default: "\"async\"",
    read: queue_adapter,
};

/// Ruby source for the reopen's override; None when the app assigns none.
pub(super) fn synthesize<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Option<String>> {
    synthesize_setting(vfs, dir, &QUEUE_ADAPTER)
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
    fn the_adapter_is_replayed_per_environment() {
        // lobsters' development.rb and production.rb.
        let out = synth(&[
            ("config/environments/development.rb", "Rails.application.configure do\n  config.active_job.queue_adapter = :solid_queue\nend\n"),
            ("config/environments/production.rb", "Rails.application.configure do\n  config.active_job.queue_adapter = :solid_queue\nend\n"),
        ])
        .unwrap()
        .expect("override");
        assert!(out.starts_with("def active_job_queue_adapter\n  env = Rails.env.to_s\n"), "{out}");
        assert!(out.contains("if env == \"development\"\n    return ((:solid_queue).to_s)\n"), "{out}");
        assert!(out.ends_with("  return \"async\"\nend\n"), "{out}");
    }

    #[test]
    fn nothing_assigned_keeps_the_runtime_default() {
        assert_eq!(synth(&[("config/environments/production.rb", "Rails.application.configure do\nend\n")]).unwrap(), None);
    }
}

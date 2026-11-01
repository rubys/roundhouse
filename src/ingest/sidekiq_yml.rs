//! `config/sidekiq.yml`: the worker's defaults, read at compile time.
//!
//! Sidekiq 8.1's CLI (`Sidekiq::CLI#parse_config`) runs the file through
//! ERB, loads the YAML, symbolizes its keys and merges the current
//! environment's section over the top level:
//!
//! ```ruby
//! opts = YAML.safe_load(erb.result, permitted_classes: [Symbol], aliases: true) || {}
//! opts.deep_symbolize_keys!
//! opts = opts.merge(opts.delete(environment.to_sym) || {})
//! ```
//!
//! The command line then wins key by key, and `-q` replaces the file's
//! queues whole. The emitted worker keeps that order: its `-c`, `-q` and
//! `-t` win over what this reads, and this wins over `RAILS_MAX_THREADS`.
//!
//! An emitted tree runs in production, so the `production:` section is
//! the one merged. Read, with or without the leading colon: `concurrency`,
//! `timeout` and `queues`; the retry limit `max_retries`; the dead set's
//! `dead_max_jobs` and `dead_timeout_in_seconds`; and the poller's
//! `poll_interval_average` and `average_scheduled_poll_interval`. A queue
//! is a name, `"name,weight"`, or `[name, weight]`, as `Capsule#queues=`
//! reads them. A value that is ERB, a key the worker has no use for
//! (`capsules`, `logfile`, ...), and a value in another shape are each a
//! warning; the setting keeps Sidekiq's default, or what the command
//! line gives.

use std::path::Path;

use serde_yaml_ng::Value;

use crate::app::SidekiqWorkerDefaults;
use crate::vfs::Vfs;

/// Stands in for an ERB tag while the YAML is read, so a value that was
/// ERB can be named in its warning rather than silently read as `""`.
const ERB: &str = "__roundhouse_erb__";

/// The defaults `config/sidekiq.yml` gives, and a warning for each part
/// of it that isn't read. Empty when the file is absent; a file that is
/// there but can't be read is a warning.
pub fn read<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> (SidekiqWorkerDefaults, Vec<String>) {
    match vfs.read(&dir.join("config/sidekiq.yml")) {
        Ok(bytes) => parse(&String::from_utf8_lossy(&bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(e) => (Default::default(), vec![format!("config/sidekiq.yml is not read: {e}")]),
    }
}

/// The keys the worker reads (and `strict`, which it accepts).
const WORKER_KEYS: [&str; 9] = [
    "concurrency",
    "timeout",
    "queues",
    "strict",
    "max_retries",
    "dead_max_jobs",
    "dead_timeout_in_seconds",
    "poll_interval_average",
    "average_scheduled_poll_interval",
];

pub fn parse(text: &str) -> (SidekiqWorkerDefaults, Vec<String>) {
    let mut defaults = SidekiqWorkerDefaults::default();
    let mut warnings = Vec::new();
    let (text, logic) = mark_erb(text);
    // Sidekiq runs the ERB before it reads the YAML, so a setting under
    // `<% if ... %>` may not be there at all: none of the file is read.
    if logic {
        warnings.push("config/sidekiq.yml has ERB that isn't an output tag (`<% ... %>`), so it is not read; give the worker's settings on its command line".to_string());
        return (defaults, warnings);
    }
    let mut doc = match serde_yaml_ng::from_str::<Value>(&text) {
        Ok(doc) => doc,
        Err(e) => {
            warnings.push(format!("config/sidekiq.yml is not read: {e}"));
            return (defaults, warnings);
        }
    };
    if doc.apply_merge().is_err() {
        warnings.push("config/sidekiq.yml's merge keys don't resolve; it is not read".to_string());
        return (defaults, warnings);
    }
    let Value::Mapping(top) = doc else { return (defaults, warnings) };
    let mut entries: Vec<(String, Value)> = Vec::new();
    let mut production = None;
    for (key, value) in top {
        let Some(key) = key_name(&key) else { continue };
        if key == "production" {
            production = Some(value);
        } else if matches!(value, Value::Mapping(_)) && key != "capsules" && !WORKER_KEYS.contains(&key.as_str()) {
            // Another environment's section.
        } else {
            entries.push((key, value));
        }
    }
    if let Some(Value::Mapping(section)) = production {
        for (key, value) in section {
            let Some(key) = key_name(&key) else { continue };
            entries.retain(|(k, _)| k != &key);
            entries.push((key, value));
        }
    }
    for (key, value) in entries {
        if mentions_erb(&value) {
            warnings.push(format!(
                "`:{key}` in config/sidekiq.yml is ERB, so it is not read; give it on the worker's command line"
            ));
            continue;
        }
        match key.as_str() {
            "concurrency" => match value.as_i64() {
                Some(n) if n > 0 => defaults.concurrency = Some(n),
                _ => warnings.push(format!("`:concurrency: {}` in config/sidekiq.yml is not a positive Integer, so it is not read", show(&value))),
            },
            "timeout" => match value.as_i64() {
                Some(n) if n >= 0 => defaults.timeout = Some(n),
                _ => warnings.push(format!("`:timeout: {}` in config/sidekiq.yml is not an Integer, so it is not read", show(&value))),
            },
            "queues" => match queues(&value) {
                Some(q) => defaults.queues = q,
                None => warnings.push(format!(
                    "`:queues:` in config/sidekiq.yml is not a list of names, `name,weight` or `[name, weight]`, so it is not read"
                )),
            },
            "max_retries" => match value.as_i64() {
                Some(n) if n >= 0 => defaults.max_retries = Some(n),
                _ => warnings.push(format!("`:max_retries: {}` in config/sidekiq.yml is not an Integer, so it is not read", show(&value))),
            },
            "dead_max_jobs" => match value.as_i64() {
                Some(n) if n > 0 => defaults.dead_max_jobs = Some(n),
                _ => warnings.push(format!("`:dead_max_jobs: {}` in config/sidekiq.yml is not a positive Integer, so it is not read", show(&value))),
            },
            "dead_timeout_in_seconds" => match value.as_i64() {
                Some(n) if n > 0 => defaults.dead_timeout_in_seconds = Some(n),
                _ => warnings.push(format!(
                    "`:dead_timeout_in_seconds: {}` in config/sidekiq.yml is not a positive Integer, so it is not read",
                    show(&value)
                )),
            },
            "poll_interval_average" | "average_scheduled_poll_interval" => match value.as_f64() {
                Some(n) if n > 0.0 => {
                    if key == "poll_interval_average" {
                        defaults.poll_interval_average = Some(n);
                    } else {
                        defaults.average_scheduled_poll_interval = Some(n);
                    }
                }
                _ => warnings.push(format!("`:{key}: {}` in config/sidekiq.yml is not a positive number, so it is not read", show(&value))),
            },
            "strict" => {}
            other => warnings.push(format!("`:{other}` in config/sidekiq.yml is not read by the Roundhouse worker")),
        }
    }
    (defaults, warnings)
}

/// `concurrency`, `:concurrency` and the symbol `:concurrency` alike.
fn key_name(key: &Value) -> Option<String> {
    let Value::String(s) = key else { return None };
    Some(s.strip_prefix(':').unwrap_or(s).to_string())
}

fn queues(value: &Value) -> Option<Vec<(String, i64)>> {
    let list = match value {
        Value::Sequence(list) => list.clone(),
        Value::String(_) => vec![value.clone()],
        _ => return None,
    };
    let mut out = Vec::new();
    for item in list {
        match item {
            Value::String(s) => {
                let mut parts = s.splitn(2, ',');
                let name = parts.next()?.trim().to_string();
                let weight = match parts.next() {
                    Some(w) => w.trim().parse::<i64>().ok()?,
                    None => 0,
                };
                if !queue_name(&name) {
                    return None;
                }
                out.push((name, weight));
            }
            Value::Sequence(pair) => {
                if pair.len() > 2 {
                    return None;
                }
                let name = match pair.first()? {
                    Value::String(s) => s.clone(),
                    _ => return None,
                };
                let weight = match pair.get(1) {
                    None => 0,
                    Some(w) => w.as_i64()?,
                };
                if !queue_name(&name) {
                    return None;
                }
                out.push((name, weight));
            }
            _ => return None,
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// A name the worker's `-q name,weight` list can carry: no space and no
/// comma, which would split it there.
fn queue_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(|c: char| c.is_whitespace() || c == ',')
}

fn mentions_erb(value: &Value) -> bool {
    match value {
        Value::String(s) => s.contains(ERB),
        Value::Sequence(list) => list.iter().any(mentions_erb),
        Value::Mapping(map) => map.iter().any(|(k, v)| mentions_erb(k) || mentions_erb(v)),
        _ => false,
    }
}

fn show(value: &Value) -> String {
    serde_yaml_ng::to_string(value).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Each `<%= ... %>` becomes the ERB marker, and each other tag
/// nothing. The second answer says whether there were any of those.
fn mark_erb(text: &str) -> (String, bool) {
    let mut out = String::with_capacity(text.len());
    let mut logic = false;
    let mut rest = text;
    while let Some(start) = rest.find("<%") {
        out.push_str(&rest[..start]);
        let tag = &rest[start..];
        match tag.find("%>") {
            Some(end) => {
                if tag.starts_with("<%=") {
                    out.push_str(ERB);
                } else if !tag.starts_with("<%#") {
                    logic = true;
                }
                rest = &tag[end + 2..];
            }
            None => rest = "",
        }
    }
    out.push_str(rest);
    (out, logic)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn symbol_keys_and_the_production_section_merge_over_the_top_level() {
        let (d, w) = parse(
            ":concurrency: 5\n:timeout: 8\n:queues:\n  - critical\n  - [default, 2]\n  - low,1\nproduction:\n  :concurrency: 10\ndevelopment:\n  :concurrency: 1\n",
        );
        assert!(w.is_empty(), "{w:#?}");
        assert_eq!(d.concurrency, Some(10));
        assert_eq!(d.timeout, Some(8));
        assert_eq!(d.queues, vec![("critical".into(), 0), ("default".into(), 2), ("low".into(), 1)]);
    }

    #[test]
    fn plain_keys_read_too() {
        let (d, w) = parse("concurrency: 3\nqueues: [default]\n");
        assert!(w.is_empty(), "{w:#?}");
        assert_eq!(d.concurrency, Some(3));
        assert_eq!(d.queues, vec![("default".into(), 0)]);
    }

    #[test]
    fn erb_and_unknown_keys_warn_and_are_not_read() {
        let (d, w) = parse(
            ":concurrency: <%= ENV.fetch(\"SIDEKIQ_CONCURRENCY\", 5) %>\n:logfile: ./log/sidekiq.log\n:queues:\n  - default\n",
        );
        assert_eq!(d.concurrency, None);
        assert_eq!(d.queues, vec![("default".into(), 0)]);
        assert_eq!(w.len(), 2, "{w:#?}");
        assert!(w[0].contains(":concurrency") && w[0].contains("ERB"), "{w:#?}");
        assert!(w[1].contains(":logfile"), "{w:#?}");
    }

    #[test]
    fn the_retry_dead_set_and_poller_settings_read() {
        let (d, w) = parse(
            ":max_retries: 3\n:dead_max_jobs: 500\n:dead_timeout_in_seconds: 86400\n:average_scheduled_poll_interval: 2\nproduction:\n  :poll_interval_average: 0.5\n",
        );
        assert!(w.is_empty(), "{w:#?}");
        assert_eq!(d.max_retries, Some(3));
        assert_eq!(d.dead_max_jobs, Some(500));
        assert_eq!(d.dead_timeout_in_seconds, Some(86400));
        assert_eq!(d.average_scheduled_poll_interval, Some(2.0));
        assert_eq!(d.poll_interval_average, Some(0.5));
    }

    #[test]
    fn retry_dead_set_and_poller_settings_in_another_shape_warn() {
        let (d, w) = parse(
            ":max_retries: -1\n:dead_max_jobs: 0\n:dead_timeout_in_seconds: forever\n:poll_interval_average: 0\n:average_scheduled_poll_interval: soon\n",
        );
        assert_eq!(d, Default::default());
        assert_eq!(w.len(), 5, "{w:#?}");
    }

    #[test]
    fn a_queue_name_with_a_space_or_a_comma_is_not_read() {
        for text in [":queues:\n  - \"my queue\"\n", ":queues:\n  - [\"a,b\", 2]\n", ":queues:\n  - [\"a b\", 2]\n"] {
            let (d, w) = parse(text);
            assert!(d.queues.is_empty(), "{text}: {:?}", d.queues);
            assert_eq!(w.len(), 1, "{text}: {w:#?}");
        }
    }

    #[test]
    fn erb_logic_leaves_the_whole_file_unread() {
        let (d, w) = parse(":concurrency: 5\n<% if ENV[\"FAST\"] %>\n:timeout: 1\n<% end %>\n");
        assert_eq!(d, Default::default());
        assert_eq!(w.len(), 1, "{w:#?}");
    }

    #[test]
    fn a_worker_key_with_a_mapping_or_a_long_pair_warns() {
        for text in [":queues:\n  critical: 2\n", ":queues:\n  - [default, 2, 3]\n", ":concurrency:\n  n: 5\n"] {
            let (d, w) = parse(text);
            assert_eq!(d, Default::default(), "{text}");
            assert_eq!(w.len(), 1, "{text}: {w:#?}");
        }
    }

    #[test]
    fn a_bad_shape_warns() {
        let (d, w) = parse(":concurrency: lots\n:queues: 3\n");
        assert_eq!(d, Default::default());
        assert_eq!(w.len(), 2, "{w:#?}");
    }
}

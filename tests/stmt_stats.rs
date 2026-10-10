//! Native diagnostic contract; no fixture, server, network or database file.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

struct ScratchDir(PathBuf);
impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn counters(line: &str) -> HashMap<&str, usize> {
    line.split_whitespace()
        .filter_map(|field| {
            let (key, value) = field.split_once('=')?;
            Some((key, value.parse().ok()?))
        })
        .collect()
}

#[test]
#[ignore = "requires Spinel (SPINEL=/path/to/spinel)"]
fn statement_stats_are_opt_in_and_count_cached_transient_and_bound_reads() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = std::env::temp_dir().join(format!("roundhouse-stmt-stats-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = ScratchDir(dir.clone());
    for name in ["db.rb", "active_support_time_parsing.rb"] {
        std::fs::copy(root.join("runtime/spinel").join(name), dir.join(name)).unwrap();
    }
    std::fs::write(dir.join("probe.rb"), include_str!("stmt_stats.rb")).unwrap();
    let compile = Command::new(std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into()))
        .args(["probe.rb", "-o", "probe"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );
    for setting in [None, Some(""), Some("0"), Some("1"), Some("stats.txt")] {
        let mut command = Command::new(dir.join("probe"));
        command.current_dir(&dir).env("DATABASE_POOL_SIZE", "1");
        if let Some(value) = setting {
            command.env("RH_STMT_STATS", value);
        } else {
            command.env_remove("RH_STMT_STATS");
        }
        let out = command.output().unwrap();
        assert!(
            out.status.success(),
            "{setting:?}: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "stmt stats probe passed\n"
        );
        if matches!(setting, None | Some("") | Some("0")) {
            assert!(out.stderr.is_empty());
            assert!(!dir.join("stats.txt").exists());
            continue;
        }
        let stats = if setting == Some("1") {
            String::from_utf8(out.stderr).unwrap()
        } else {
            assert!(out.stderr.is_empty());
            std::fs::read_to_string(dir.join("stats.txt")).unwrap()
        };
        let lines: Vec<_> = stats.lines().collect();
        assert_eq!(lines.len(), 10, "{stats}");
        let phases = [
            "empty",
            "plain",
            "bound",
            "transient",
            "live",
            "released",
            "failed",
            "reset_failed",
            "trimmed",
            "close",
        ];
        for (line, phase) in lines.iter().zip(phases) {
            assert!(line
                .split_whitespace()
                .any(|field| field == format!("phase={phase}")));
            assert!(line
                .split_whitespace()
                .any(|field| field == "connection=0:0"));
        }
        let values: Vec<_> = lines.iter().map(|line| counters(line)).collect();
        assert_eq!(values[0]["misses"], 0);
        assert_eq!(values[0]["executions"], 0);
        assert_eq!(values[1]["misses"], 1);
        assert_eq!(values[1]["executions"], 1);
        assert_eq!(values[1]["qc_bypasses"], 1);
        // Five bound reads bypass SQL-only replay, even with equal values.
        assert_eq!(values[2]["qc_hits"], 1);
        assert_eq!(values[2]["qc_misses"], 1);
        assert_eq!(values[2]["qc_bypasses"], 6);
        assert_eq!(values[2]["executions"], 7);
        assert!(!values[2].contains_key("qc_bound_hits"));
        assert!(!values[2].contains_key("qc_bound_misses"));
        assert_eq!(values[3]["transient_prepares"], 4);
        assert_eq!(values[3]["qc_hits"], 2);
        assert_eq!(values[3]["qc_misses"], 2);
        assert_eq!(values[3]["qc_bypasses"], 8);
        assert_eq!(values[3]["executions"], 11);
        assert_eq!(values[3]["finalizations"], 4);
        assert_eq!(values[4]["transient_prepares"], 5);
        assert_eq!(values[4]["executions"], 12);
        assert_eq!(values[4]["finalizations"], 4);
        assert_eq!(values[5]["executions"], 12);
        assert_eq!(values[5]["finalizations"], 5);
        assert_eq!(values[6]["executions"], 13);
        assert_eq!(values[6]["finalizations"], 5);
        assert_eq!(values[6]["evictions"], 0);
        assert_eq!(values[7]["executions"], 14);
        assert_eq!(values[7]["finalizations"], 6);
        assert_eq!(values[7]["evictions"], 1);
        assert_eq!(values[8]["entries"], 128);
        assert_eq!(values[8]["trims"], 1);
        assert_eq!(values[8]["prepare_failures"], 1);
        assert_eq!(values[8]["executions"], 144);
        assert_eq!(values[9]["entries"], 0);
        assert_eq!(values[9]["executions"], 144);
        assert_eq!(values[9]["finalizations"] - values[8]["finalizations"], 128);
        assert_eq!(values[9]["misses"], values[9]["finalizations"]);
    }
}

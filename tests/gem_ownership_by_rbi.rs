//! A gem whose RBI was read has said which classes it defines, so the
//! first-segment guess (`acme-core` -> `Acme`, `shopify-adt` ->
//! `Shopify`) is not made for a constant that RBI does not account
//! for. A gem whose RBI was not read still gets the guess.

use roundhouse::gem_boundary::GemBoundary;
use roundhouse::gems::{gem_owning_constant_with, GemCensus, Lockfile};
use roundhouse::ingest::rbi::load_gem_boundary;
use roundhouse::vfs::Vfs;
use std::collections::HashMap;
use std::path::PathBuf;

const LOCK: &str = "\
GEM
  remote: https://rubygems.org/
  specs:
    acme-core (2.1.0)
    dark-thing (1.0.0)

PLATFORMS
  ruby

DEPENDENCIES
  acme-core
  dark-thing
";

const RBI: &str = "module Acme\n  class Client\n    def get; end\n  end\nend\n";

fn boundary() -> GemBoundary {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("sorbet/rbi/gems/acme-core@2.1.0.rbi", RBI),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let lock = Lockfile::parse(LOCK);
    load_gem_boundary(&tree_vfs(tree), std::path::Path::new(""), &lock, &Default::default())
}

fn tree_vfs(tree: HashMap<PathBuf, Vec<u8>>) -> impl Vfs {
    roundhouse::vfs::MapVfs::new(tree)
}

#[test]
fn a_gem_that_declared_its_classes_does_not_own_a_sibling_constant() {
    let census = GemCensus::of(&Lockfile::parse(LOCK));
    let b = boundary();
    let declares = |gem: &str, path: &str| b.declares_path(gem, path);
    // `module Acme` is only a container in the RBI: `Acme::Config` is
    // somebody else's.
    assert_eq!(gem_owning_constant_with(&census, "Acme::Config", &declares), None);
    assert_eq!(gem_owning_constant_with(&census, "Acme::Client", &declares), Some("acme-core"));
    assert_eq!(gem_owning_constant_with(&census, "Acme::Client::Response", &declares), Some("acme-core"));
}

#[test]
fn a_gem_without_a_read_rbi_keeps_the_first_segment_guess() {
    let census = GemCensus::of(&Lockfile::parse(LOCK));
    let b = boundary();
    let declares = |gem: &str, path: &str| b.declares_path(gem, path);
    assert_eq!(gem_owning_constant_with(&census, "Dark::Widget", &declares), Some("dark-thing"));
}

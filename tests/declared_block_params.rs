//! A method that declares its block, `{ (Writer) -> void }` in RBS or
//! `T.proc.params(writer: Writer).void` in a sig, yields `Writer` to the
//! block. The block's own parameter list is the signature, so a block
//! with ONE parameter binds that parameter's type, not the whole
//! `^(Writer) -> void` function type (every call on the block variable
//! was then a dispatch on a proc).

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};
use roundhouse::ingest::ingest_app_from_tree;

fn receivers(registry: &str, action: &str) -> Vec<String> {
    let controller = format!(
        "class CartController < ApplicationController\n  def show\n    {action}\n  end\nend\n"
    );
    let files: [(&str, &str); 5] = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        (
            "app/services/registry.rb",
            registry,
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("app/controllers/cart_controller.rb", &controller),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    diagnose(&app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::SendDispatchFailed { recv_ty, .. } => Some(format!("{recv_ty:?}")),
            _ => None,
        })
        .collect()
}

fn writer_class(recv: &str) -> bool {
    recv.contains("Writer") && !recv.contains("Fn")
}

#[test]
fn an_rbs_block_with_one_parameter_yields_that_parameter() {
    let r = receivers(
        "class Writer\nend\n\nclass Registry\n  #: () { (Writer) -> void } -> void\n  def self.with; end\nend\n",
        "Registry.with { |writer| writer.bogus }",
    );
    assert_eq!(r.len(), 1, "{r:?}");
    assert!(writer_class(&r[0]), "{r:?}");
}

#[test]
fn a_sorbet_proc_block_with_one_parameter_yields_that_parameter() {
    let r = receivers(
        "class Writer\nend\n\nclass Registry\n  extend T::Sig\n\n  sig { params(blk: T.proc.params(writer: Writer).void).void }\n  def self.with(&blk); end\nend\n",
        "Registry.with { |writer| writer.bogus }",
    );
    assert_eq!(r.len(), 1, "{r:?}");
    assert!(writer_class(&r[0]), "{r:?}");
}

#[test]
fn a_block_that_yields_nothing_binds_nothing() {
    let r = receivers(
        "class Registry\n  #: () { () -> void } -> void\n  def self.with; end\nend\n",
        "Registry.with { 1 }",
    );
    assert!(r.is_empty(), "{r:?}");
}

#[test]
fn an_instance_method_without_a_block_does_not_steal_the_class_side_block() {
    // Same name on both sides: instance `with` has no block; class `with`
    // yields Writer. An instance call must not bind from the class contract.
    let r = receivers(
        "class Writer\nend\n\nclass Registry\n  #: () { (Writer) -> void } -> void\n  def self.with; end\n\n  #: () -> void\n  def with; end\nend\n",
        "Registry.new.with { |writer| writer.bogus }",
    );
    assert!(r.is_empty(), "stole class-side block: {r:?}");
}

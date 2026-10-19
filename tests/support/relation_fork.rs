//! One contract for relations derived from a shared base, used by the
//! interpreted (CRuby overlay) and native (spinel, over HTTP) lanes.
//!
//! In Rails every query method answers a new relation and leaves its
//! receiver as it was: `base = Article.where.not(...)`, then
//! `base.where(...)`, `base.order(...)`, `base.limit(...)`, `base.none`,
//! `base.merge(...)` and friends each answer their own relation, and
//! `base` still counts what it counted before. A loaded relation keeps
//! its rows when a relation derived from it narrows or reorders them.
//!
//! The expected lines are CRuby 4.0.5 + Active Record 8.1.4's output for
//! the same model (topics/relation-immutable/contract_probe.rb in the
//! workspace that wrote this).

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog().write("app/models/relation_fork_report.rb", REPORT)
}

/// The overlay plus a route that renders the report, for the native lane.
pub fn served_overlay() -> super::emit_and_run::Overlay {
    overlay()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/relation-fork-report\", to: \"relation_fork_reports#show\"\n",
        )
        .write(
            "app/controllers/relation_fork_reports_controller.rb",
            "class RelationForkReportsController < ApplicationController\n  def show\n    render plain: RelationForkReport.run\n  end\nend\n",
        )
}

const REPORT: &str = r##"class RelationForkReport
  def self.run
    Article.create!(title: "Alpha", body: "first body text")
    Article.create!(title: "Beta", body: "second body text")
    Article.create!(title: "Gamma", body: "third body text")
    Article.create!(title: "Draft", body: "unpublished body")
    lines = []
    base = Article.where.not(title: "Draft")
    a = base.where(title: "Beta")
    b = base.order(title: :desc)
    c = base.limit(1)
    d = base.offset(1).order(:title).limit(5)
    lines << base.count.to_s
    lines << a.map(&:title).join(",")
    lines << b.map(&:title).join(",")
    lines << c.to_a.size.to_s
    lines << d.map(&:title).join(",")
    lines << base.order(:id).map(&:title).join(",")
    e = b.reorder(:title)
    lines << b.first.title
    lines << e.first.title
    lines << base.none.count.to_s
    lines << base.count.to_s
    lines << base.merge(Article.where(title: "Gamma")).count.to_s
    lines << base.count.to_s
    lines << a.unscope(where: :title).count.to_s
    lines << a.count.to_s
    lines << base.distinct.count.to_s
    lines << base.select(:id, :title).order(:title).map(&:title).join(",")
    loaded = base.order(:title).load
    lines << loaded.order(title: :desc).map(&:title).join(",")
    lines << loaded.map(&:title).join(",")
    lines << loaded.where(title: "Alpha").to_a.size.to_s
    lines << loaded.to_a.size.to_s
    lines << base.where(title: "Alpha").or(base.where(title: "Beta")).count.to_s
    lines << base.count.to_s
    lines.join("\n") + "\n"
  end
end
"##;

pub const SCRIPT: &str = "print RelationForkReport.run\n";

pub const EXPECTED: &str = "3\nBeta\nGamma,Beta,Alpha\n1\nBeta,Gamma\nAlpha,Beta,Gamma\nGamma\nAlpha\n0\n3\n1\n3\n4\n1\n3\nAlpha,Beta,Gamma\nAlpha,Beta,Gamma\nAlpha,Beta,Gamma\n1\n3\n2\n3\n";

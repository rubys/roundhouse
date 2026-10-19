//! App classes nested under a class (`Importer::TooBig`) and under a
//! module (`Billing::Invoice::Line`), and a third class reaching both
//! from another file.

use super::emit_and_run::{self, Overlay};

pub const IMPORTER: &str = r#"class Importer
  class TooBig < StandardError; end

  def self.check(n)
    raise TooBig, "too big: #{n}" if n > 3
    "ok #{n}"
  end

  def self.safe(n)
    check(n)
  rescue TooBig => e
    e.message
  end
end
"#;

pub const INVOICE: &str = r#"module Billing
  class Invoice
    class Line
      def initialize(amount)
        @amount = amount
      end

      def amount
        @amount
      end
    end

    def self.first_amount(x)
      Line.new(x).amount
    end
  end
end
"#;

pub const REPORT: &str = r#"class Report
  def self.summary
    [Importer.safe(2), Importer.safe(5), Importer::TooBig.new("direct").message,
     Billing::Invoice.first_amount(7), Billing::Invoice::Line.new(3).amount].join(",")
  end
end
"#;

/// What `Report.summary` answers under CRuby.
#[allow(dead_code)]
pub const SUMMARY: &str = "ok 2,too big: 5,direct,7,3";

pub fn overlay() -> Overlay {
    emit_and_run::real_blog()
        .write("app/models/importer.rb", IMPORTER)
        .write("app/models/billing/invoice.rb", INVOICE)
        .write("app/models/report.rb", REPORT)
}

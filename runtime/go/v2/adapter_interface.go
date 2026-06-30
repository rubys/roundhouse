// Roundhouse Go v2 — abstract ActiveRecord adapter contract.
//
// Hand-written, copied verbatim into the go2 overlay's `app/v2/`
// output. The 9-method contract that `runtime/ruby/active_record/
// base.rb` calls against `ActiveRecord.adapter`. Every concrete
// adapter (production sqlite, in-memory framework-test, future
// libsql/D1) implements it.
//
// Mirrors `runtime/rust/active_record_adapter.rs` 1:1 by method.
// Method names PascalCase so transpiled call sites
// (`adapter.Find(...)`) line up with the go2 expr walker's
// `go2_method_ident` pascalization. Row + condition + attribute
// shapes use `any` because the abstract slot is polymorphic —
// concrete adapters produce concrete row types; transpiled
// `instantiate(row)` is where per-model decoding happens.

package v2

// Row is the abstract row shape every adapter produces. Keyed by
// column name; values are the untyped tree the transpiled
// `ActiveRecord::Base` methods feed through `instantiate(row)`
// for per-model decoding. A nil Row signals "not found" from the
// Find / first-of-Where paths — Go's nil map compares equal to
// nil literally, so `if row == nil` is the natural Ruby
// `row.nil?` analog.
type Row = map[string]any

// ActiveRecordAdapterInterface — the 9-method contract every
// concrete adapter satisfies. Name mirrors the RBS-declared
// `ActiveRecord::AdapterInterface` phantom class so transpiled
// `ActiveRecord.adapter` slot signatures (`go_ty_stub` of
// `Ty::Class { id: "ActiveRecord::AdapterInterface" }`) line up.
// Method signatures mirror the Rust trait.
type ActiveRecordAdapterInterface interface {
	All(tableName string) []Row
	Find(tableName string, id int64) Row
	Where(tableName string, conditions map[string]any) []Row
	Count(tableName string) int64
	ExistsPred(tableName string, id int64) bool
	Insert(tableName string, attributes map[string]any) int64
	Update(tableName string, id int64, attributes map[string]any)
	Delete(tableName string, id int64)
	Truncate(tableName string)
}

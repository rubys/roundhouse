// Roundhouse go2 Modeler interface — the polymorphic-dispatch
// back-pointer through which `ActiveRecordBase` methods call into
// subclass-specific implementations.
//
// Q1 (project_go2_session_arc.md): Go has no inheritance, only
// embedding. When `(*ActiveRecordBase).FillTimestamps` calls
// `self.class.schema_columns` (Ruby) or `self.OpSet(...)`, those
// dispatches need to land on the concrete subclass (Article,
// Comment), not on Base's panic-stubs. Embedding alone doesn't
// help — Base's method receives `*ActiveRecordBase` and has no
// path back up to the outer Article.
//
// The back-pointer: every AR::Base instance carries a `Self
// Modeler` field; the OUTER subclass constructor wires
// `instance.Self = instance`. Inside Base methods, the
// `self.class.X` and `self.OpSet` shapes emit as
// `self.Self.X()` — interface dispatch resolves to the concrete
// subclass's implementation.
//
// Members listed here are exactly the polymorphic-dispatched
// surface Base needs. Add new ones as new shapes surface; each
// addition requires a matching method shim on every AR
// subclass.

package v2

type Modeler interface {
	SchemaColumns() []string
	OpGet(name string) interface{}
	OpSet(name string, value interface{}) interface{}
}

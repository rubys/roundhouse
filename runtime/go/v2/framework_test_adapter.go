// Roundhouse Go v2 — in-memory framework-test adapter.
//
// Hand-written, copied verbatim into the go2 overlay's `app/v2/`
// output. Mirrors `runtime/rust/framework_test_adapter.rs` and the
// pure-Ruby module version in `runtime/ruby/test/test_helper.rb`.
//
// Framework tests under `runtime/ruby/test/` use it as both the
// abstract adapter slot (`ActiveRecord.adapter = adapter`) and as
// a direct receiver for the test-helper API (`create_table`,
// `drop_table`, `reset_all!`, `schema`).

package v2

import "sync"

// TestSchema records the columns + foreign keys declared via
// `CreateTable`. Mirrors the rust + crystal + ts twins.
type TestSchema struct {
	Columns     []string
	ForeignKeys []string
}

// FrameworkTestAdapter — goroutine-safe in-memory store.
// Per-table id auto-increment with explicit-id override (tests
// pre-assign ids: `insert("stubs", id: 7)`). Full CRUD against
// maps protected by a single mutex; concurrency is bounded
// (single test goroutine) so the coarse lock is fine.
type FrameworkTestAdapter struct {
	mu      sync.Mutex
	tables  map[string]map[int64]Row
	nextIDs map[string]int64
	schemas map[string]TestSchema
}

func NewFrameworkTestAdapter() *FrameworkTestAdapter {
	return &FrameworkTestAdapter{
		tables:  map[string]map[int64]Row{},
		nextIDs: map[string]int64{},
		schemas: map[string]TestSchema{},
	}
}

func (a *FrameworkTestAdapter) ResetAll() {
	a.mu.Lock()
	defer a.mu.Unlock()
	a.tables = map[string]map[int64]Row{}
	a.nextIDs = map[string]int64{}
	a.schemas = map[string]TestSchema{}
}

func (a *FrameworkTestAdapter) CreateTable(name string, columns, foreignKeys []string) {
	a.mu.Lock()
	defer a.mu.Unlock()
	a.tables[name] = map[int64]Row{}
	a.nextIDs[name] = 0
	a.schemas[name] = TestSchema{Columns: columns, ForeignKeys: foreignKeys}
}

func (a *FrameworkTestAdapter) DropTable(name string) {
	a.mu.Lock()
	defer a.mu.Unlock()
	delete(a.tables, name)
	delete(a.nextIDs, name)
	delete(a.schemas, name)
}

// ActiveRecordAdapter implementation -------------------------------

func (a *FrameworkTestAdapter) All(tableName string) []Row {
	a.mu.Lock()
	defer a.mu.Unlock()
	t, ok := a.tables[tableName]
	if !ok {
		return nil
	}
	out := make([]Row, 0, len(t))
	for _, row := range t {
		out = append(out, copyRow(row))
	}
	return out
}

func (a *FrameworkTestAdapter) Find(tableName string, id int64) Row {
	a.mu.Lock()
	defer a.mu.Unlock()
	t, ok := a.tables[tableName]
	if !ok {
		return nil
	}
	row, ok := t[id]
	if !ok {
		return nil
	}
	return copyRow(row)
}

func (a *FrameworkTestAdapter) Where(tableName string, conditions map[string]any) []Row {
	a.mu.Lock()
	defer a.mu.Unlock()
	t, ok := a.tables[tableName]
	if !ok {
		return nil
	}
	out := []Row{}
	for _, row := range t {
		match := true
		for k, v := range conditions {
			rv, ok := row[k]
			if !ok || rv != v {
				match = false
				break
			}
		}
		if match {
			out = append(out, copyRow(row))
		}
	}
	return out
}

func (a *FrameworkTestAdapter) Count(tableName string) int64 {
	a.mu.Lock()
	defer a.mu.Unlock()
	t, ok := a.tables[tableName]
	if !ok {
		return 0
	}
	return int64(len(t))
}

func (a *FrameworkTestAdapter) Exists(tableName string, id int64) bool {
	a.mu.Lock()
	defer a.mu.Unlock()
	t, ok := a.tables[tableName]
	if !ok {
		return false
	}
	_, exists := t[id]
	return exists
}

func (a *FrameworkTestAdapter) Insert(tableName string, attributes map[string]any) int64 {
	a.mu.Lock()
	defer a.mu.Unlock()
	if _, ok := a.tables[tableName]; !ok {
		panic("table " + tableName + " not created")
	}
	explicit := int64(0)
	if v, ok := attributes["id"]; ok {
		if i, ok := toInt64(v); ok {
			explicit = i
		}
	}
	current := a.nextIDs[tableName]
	var id int64
	if explicit != 0 {
		id = explicit
	} else {
		id = current + 1
	}
	if id > current {
		a.nextIDs[tableName] = id
	}
	row := copyRow(attributes)
	row["id"] = id
	a.tables[tableName][id] = row
	return id
}

func (a *FrameworkTestAdapter) Update(tableName string, id int64, attributes map[string]any) {
	a.mu.Lock()
	defer a.mu.Unlock()
	t, ok := a.tables[tableName]
	if !ok {
		return
	}
	existing, ok := t[id]
	if !ok {
		return
	}
	for k, v := range attributes {
		existing[k] = v
	}
	existing["id"] = id
}

func (a *FrameworkTestAdapter) Delete(tableName string, id int64) {
	a.mu.Lock()
	defer a.mu.Unlock()
	if t, ok := a.tables[tableName]; ok {
		delete(t, id)
	}
}

func (a *FrameworkTestAdapter) Truncate(tableName string) {
	a.mu.Lock()
	defer a.mu.Unlock()
	a.tables[tableName] = map[int64]Row{}
	a.nextIDs[tableName] = 0
}

func copyRow(r map[string]any) Row {
	out := make(Row, len(r))
	for k, v := range r {
		out[k] = v
	}
	return out
}

// toInt64 coerces the common numeric shapes test fixtures hand in
// (`int`/`int64`/`int32`/`float64`) to `int64`. Anything else
// returns (0, false) so the caller treats it as no-explicit-id.
func toInt64(v any) (int64, bool) {
	switch x := v.(type) {
	case int64:
		return x, true
	case int:
		return int64(x), true
	case int32:
		return int64(x), true
	case float64:
		return int64(x), true
	}
	return 0, false
}

// Roundhouse go2 DB shim — bare-fn surface for the lowered
// `Db.prepare(sql)` / `Db.step?(stmt)` / `Db.column_*` /
// `Db.finalize(stmt)` calls the per-model adapter_emit lowerer
// produces (see `src/lower/model_to_library/adapter_emit.rs`).
//
// Mirrors `runtime/rust/db.rs`'s Db namespace member-for-member,
// translated to Go's `database/sql` interface. Function naming
// follows the go2 module-singleton convention — `Db.step?` → emitted
// as `Db_step_p(stmt)`, so the function lives here under that name
// without a per-call peephole bridge.
//
// Statement state is kept in a goroutine-shared `statements` table.
// rows materialize on `Db_prepare`; subsequent step/column calls
// index into the pre-fetched per-stmt slice. Matches the rust2
// runtime's eat-the-allocation-up-front pattern for the same
// reason — the `*sql.Rows` chain is awkward to thread through an
// opaque integer handle.

package v2

import (
	"database/sql"
	"os"
	"runtime"
	"strconv"
	"strings"
	"sync"

	_ "modernc.org/sqlite"
)

// Process-wide connection. Tests call SetupTestDB to reinstall a
// fresh in-memory connection per test; production wires this once
// in main.go via OpenProductionDB.
var (
	// dbMu guards the `db` POINTER (swapped by SetupTestDB /
	// OpenProductionDB), not query execution — `*sql.DB` is already a
	// concurrency-safe connection pool. Read paths take RLock so many
	// queries run in parallel across the pool (the point of WAL +
	// SetMaxOpenConns); only a connection swap takes the exclusive
	// Lock. Holding an exclusive lock around `db.Query` would serialize
	// every read and cap throughput at single-connection work no matter
	// how large the pool is.
	dbMu sync.RWMutex
	db   *sql.DB
)

// Per-statement materialized rows + cursor position. Mirrors
// rust/db.rs's StmtEntry — pre-fetch every row at prepare time so
// the opaque int64 handle can be indexed without holding a
// *sql.Rows borrow chain. The current snapshot is the most
// recently `Db_step_p`'d row.
type stmtEntry struct {
	cols    []string
	rows    [][]any
	pos     int
	current []any
}

var (
	stmtMu      sync.Mutex
	statements  = map[int64]*stmtEntry{}
	nextStmtID  int64
	lastRowID   int64
)

// SetupTestDB installs a fresh :memory: sqlite connection and runs
// the schema DDL. Called by emitted tests at the top of each body
// via setupTest(); replaces any previous connection so test state
// doesn't bleed across.
func SetupTestDB(schemaSQL string) {
	dbMu.Lock()
	defer dbMu.Unlock()
	if db != nil {
		db.Close()
	}
	conn, err := sql.Open("sqlite", ":memory:")
	if err != nil {
		panic("open :memory: sqlite: " + err.Error())
	}
	for _, stmt := range strings.Split(schemaSQL, ";\n") {
		stmt = strings.TrimSpace(stmt)
		if stmt == "" {
			continue
		}
		if _, err := conn.Exec(stmt); err != nil {
			panic("schema: " + err.Error())
		}
	}
	db = conn
	resetStmts()
}

// OpenProductionDB opens a file-backed sqlite connection and applies
// the schema DDL when the DB has no user tables yet. Skipping the
// schema apply on a populated DB preserves a compare-tool-staged
// seed without clobbering it.
func OpenProductionDB(path, schemaSQL string) {
	dbMu.Lock()
	defer dbMu.Unlock()
	if db != nil {
		db.Close()
	}
	// WAL + a sized connection pool (#17). The two are joint: WAL lets
	// readers proceed concurrently, but that concurrency has nowhere to
	// go unless the pool can hand out more than one connection at a
	// time. SQLite PRAGMAs are per-connection, so set them in the DSN —
	// every connection database/sql opens for the pool picks them up
	// (a one-shot `Exec("PRAGMA …")` would only configure whichever
	// single connection it happened to land on). journal_mode=WAL
	// persists in the file header; synchronous/busy_timeout are
	// per-connection and must ride the DSN each open.
	dsn := "file:" + path +
		"?_pragma=journal_mode(WAL)&_pragma=synchronous(NORMAL)&_pragma=busy_timeout(5000)"
	conn, err := sql.Open("sqlite", dsn)
	if err != nil {
		panic("open sqlite: " + err.Error())
	}
	conn.SetMaxOpenConns(prodPoolSize())
	var count int
	row := conn.QueryRow(
		"SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
	)
	if err := row.Scan(&count); err != nil {
		panic("probe schema: " + err.Error())
	}
	if count == 0 {
		for _, stmt := range strings.Split(schemaSQL, ";\n") {
			stmt = strings.TrimSpace(stmt)
			if stmt == "" {
				continue
			}
			if _, err := conn.Exec(stmt); err != nil {
				panic("schema: " + err.Error())
			}
		}
	}
	db = conn
	resetStmts()
}

// prodPoolSize picks the production pool's max-open-connections.
// Honors DATABASE_POOL_SIZE (the same env knob rust/spinel read) as an
// override, but defaults to runtime.NumCPU() rather than the request
// concurrency: pure-Go modernc sqlite has heavy internal locking and
// its throughput peaks at a small pool (~cores), then collapses when
// oversized — measured ~13.8k req/s at pool=4 vs ~6.8k at pool=64 on
// /articles. The bench harness therefore does NOT pass a
// wrk-concurrency-sized value for go (unlike rust/spinel). Falls back
// to 4 if NumCPU is unavailable. Tests use SetupTestDB's `:memory:`
// connection and don't go through here.
func prodPoolSize() int {
	if s := os.Getenv("DATABASE_POOL_SIZE"); s != "" {
		if n, err := strconv.Atoi(s); err == nil && n > 0 {
			return n
		}
	}
	if n := runtime.NumCPU(); n > 0 {
		return n
	}
	return 4
}

func resetStmts() {
	stmtMu.Lock()
	statements = map[int64]*stmtEntry{}
	nextStmtID = 0
	lastRowID = 0
	stmtMu.Unlock()
}

// Db_exec runs a one-shot DDL/INSERT/UPDATE/DELETE. Captures
// last_insert_rowid so the subsequent Db_last_insert_rowid call
// returns the freshly-inserted id (the typical
// `Db.exec(insert_sql); id = Db.last_insert_rowid` shape in lowered
// persistence).
func Db_exec(query string) {
	dbMu.Lock()
	defer dbMu.Unlock()
	if db == nil {
		panic("db not initialized; call SetupTestDB or OpenProductionDB first")
	}
	res, err := db.Exec(query)
	if err != nil {
		panic("Db_exec: " + err.Error())
	}
	if id, err := res.LastInsertId(); err == nil {
		stmtMu.Lock()
		lastRowID = id
		stmtMu.Unlock()
	}
}

// Db_prepare runs a SELECT and materializes every row up front,
// returning an opaque stmt id. Subsequent Db_step_p / Db_column_*
// / Db_finalize calls take the id by value. Mirrors rust/db.rs's
// pre-fetch strategy — sidesteps the *sql.Rows borrow chain.
func Db_prepare(query string) int64 {
	dbMu.RLock()
	if db == nil {
		dbMu.RUnlock()
		panic("db not initialized; call SetupTestDB or OpenProductionDB first")
	}
	rows, err := db.Query(query)
	dbMu.RUnlock()
	if err != nil {
		panic("Db_prepare: " + err.Error())
	}
	defer rows.Close()
	cols, err := rows.Columns()
	if err != nil {
		panic("Db_prepare columns: " + err.Error())
	}
	entry := &stmtEntry{cols: cols}
	for rows.Next() {
		raw := make([]any, len(cols))
		ptrs := make([]any, len(cols))
		for i := range raw {
			ptrs[i] = &raw[i]
		}
		if err := rows.Scan(ptrs...); err != nil {
			panic("Db_prepare scan: " + err.Error())
		}
		entry.rows = append(entry.rows, raw)
	}
	if err := rows.Err(); err != nil {
		panic("Db_prepare rows: " + err.Error())
	}
	stmtMu.Lock()
	defer stmtMu.Unlock()
	nextStmtID++
	id := nextStmtID
	statements[id] = entry
	return id
}

// Db_step_p advances the cursor on a prepared statement, snapshotting
// the next row. Returns false when exhausted or on an unknown id
// (idempotent). The "_p" suffix is the go2 emitter's mapping of
// Ruby's `?` predicate-method marker.
func Db_step_p(stmtID int64) bool {
	stmtMu.Lock()
	defer stmtMu.Unlock()
	entry, ok := statements[stmtID]
	if !ok {
		return false
	}
	if entry.pos < len(entry.rows) {
		entry.current = entry.rows[entry.pos]
		entry.pos++
		return true
	}
	entry.current = nil
	return false
}

// Db_column_int reads an integer column from the row most recently
// stepped. NULL → 0; numeric/text variants best-effort coerce
// (matches the rust/crystal/ts shims).
func Db_column_int(stmtID int64, i int64) int64 {
	stmtMu.Lock()
	defer stmtMu.Unlock()
	entry, ok := statements[stmtID]
	if !ok || entry.current == nil {
		return 0
	}
	if int(i) >= len(entry.current) {
		return 0
	}
	switch v := entry.current[i].(type) {
	case int64:
		return v
	case int:
		return int64(v)
	case float64:
		return int64(v)
	case string:
		n, _ := strconv.ParseInt(v, 10, 64)
		return n
	case []byte:
		n, _ := strconv.ParseInt(string(v), 10, 64)
		return n
	default:
		return 0
	}
}

// Db_column_text reads a text column from the most recently stepped
// row. NULL → ""; numeric variants stringify.
func Db_column_text(stmtID int64, i int64) string {
	stmtMu.Lock()
	defer stmtMu.Unlock()
	entry, ok := statements[stmtID]
	if !ok || entry.current == nil {
		return ""
	}
	if int(i) >= len(entry.current) {
		return ""
	}
	switch v := entry.current[i].(type) {
	case string:
		return v
	case []byte:
		return string(v)
	case int64:
		return strconv.FormatInt(v, 10)
	case float64:
		return strconv.FormatFloat(v, 'f', -1, 64)
	default:
		return ""
	}
}

// Db_finalize drops the stmt-table entry. Idempotent on unknown ids.
func Db_finalize(stmtID int64) {
	stmtMu.Lock()
	defer stmtMu.Unlock()
	delete(statements, stmtID)
}

// Db_last_insert_rowid returns the last-row-id from the most recent
// Db_exec. SQLite-specific.
func Db_last_insert_rowid() int64 {
	stmtMu.Lock()
	defer stmtMu.Unlock()
	return lastRowID
}

// Db_escape_string SQL-quotes a string literal per SQLite's escape
// rule (single quotes doubled). No other byte transforms — the lowered
// IR inlines values into SQL strings rather than binding parameters.
func Db_escape_string(s string) string {
	var b strings.Builder
	b.Grow(len(s) + 2)
	b.WriteByte('\'')
	for _, ch := range s {
		if ch == '\'' {
			b.WriteString("''")
		} else {
			b.WriteRune(ch)
		}
	}
	b.WriteByte('\'')
	return b.String()
}

// Db_escape_int renders an integer for SQL inlining.
func Db_escape_int(n int64) string {
	return strconv.FormatInt(n, 10)
}

// Roundhouse Go DB runtime.
//
// Hand-written helpers the Go emitter copies verbatim into each
// generated project as `app/db.go`. Owns the `*sql.DB` handle and
// hides `database/sql` boilerplate from the generated save/destroy/
// count/find methods.
//
// Uses modernc.org/sqlite — a pure-Go SQLite port — so generated
// projects don't require a C toolchain. Driver registration happens
// via the blank import.

package app

import (
	"database/sql"
	"strings"

	_ "modernc.org/sqlite"
)

var testDB *sql.DB

// SetupTestDB opens a fresh :memory: SQLite connection on the current
// goroutine, runs the schema DDL, and installs it in the package-
// level slot. Each generated test calls this through `setupTest()`
// at the top of its body. Replaces any previously-open connection.
//
// The DDL is split on `;\n` boundaries so our multi-statement
// CREATE_TABLES constant runs one statement at a time —
// database/sql's Exec interface only accepts single statements.
func SetupTestDB(schemaSQL string) {
	if testDB != nil {
		testDB.Close()
	}
	db, err := sql.Open("sqlite", ":memory:")
	if err != nil {
		panic("open :memory: sqlite: " + err.Error())
	}
	for _, stmt := range strings.Split(schemaSQL, ";\n") {
		stmt = strings.TrimSpace(stmt)
		if stmt == "" {
			continue
		}
		if _, err := db.Exec(stmt); err != nil {
			panic("schema: " + err.Error())
		}
	}
	testDB = db
}

// Conn returns the current test connection. Panics if SetupTestDB
// has not been called yet — only happens if a generated test bypassed
// the fixture harness.
func Conn() *sql.DB {
	if testDB == nil {
		panic("test db not initialized; call SetupTestDB first")
	}
	return testDB
}

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

// OpenProductionDB opens a file-backed SQLite connection and
// applies the schema DDL when the target DB has no tables yet.
// Skipping the schema apply on a populated DB preserves a
// compare-tool-staged seed without clobbering it.
func OpenProductionDB(path, schemaSQL string) {
	if testDB != nil {
		testDB.Close()
	}
	db, err := sql.Open("sqlite", path)
	if err != nil {
		panic("open sqlite: " + err.Error())
	}
	// Probe for existing tables before running schema. `sqlite_`-
	// prefixed names are sqlite-internal; the fixture DB always
	// has some user tables when pre-seeded.
	var count int
	row := db.QueryRow(
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
			if _, err := db.Exec(stmt); err != nil {
				panic("schema: " + err.Error())
			}
		}
	}
	testDB = db
}

package forkworker

import (
	"database/sql"
	"errors"

	"github.com/ente/stacktrace"
	"github.com/golang-migrate/migrate/v4"
	"github.com/golang-migrate/migrate/v4/database/postgres"
	_ "github.com/golang-migrate/migrate/v4/source/file"
)

// MigrationsTable tracks the fork's own migrations, so that upstream's
// schema_migrations numbering never collides with ours.
const MigrationsTable = "fork_schema_migrations"

// MigrateUp applies the fork migrations in migrationsDir (for example
// "file://migrations/fork"). Run it after upstream's migrations.
func MigrateUp(db *sql.DB, migrationsDir string) error {
	driver, err := postgres.WithInstance(db, &postgres.Config{MigrationsTable: MigrationsTable})
	if err != nil {
		return stacktrace.Propagate(err, "fork migrations: driver")
	}
	m, err := migrate.NewWithDatabaseInstance(migrationsDir, "postgres", driver)
	if err != nil {
		return stacktrace.Propagate(err, "fork migrations: load")
	}
	if err := m.Up(); err != nil && !errors.Is(err, migrate.ErrNoChange) {
		return stacktrace.Propagate(err, "fork migrations: up")
	}
	return nil
}

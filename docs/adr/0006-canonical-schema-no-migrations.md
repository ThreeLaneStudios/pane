# ADR-0006: Canonical Schema, No Migration Chain (Pre-Launch)

**Status:** Superseded by ADR-0019
**Date:** 2026-04-18
**Author:** Matt Carano

## Context

Pane accumulated 26+ migrations over months of rapid development. Migration chains create ordering dependencies, partial-apply bugs, and "works on my machine but not fresh install" failures. Three consecutive attempts to add migration 26 (canonical schema) failed on Matt's real database due to crsqlite conflicts, backup table collisions, and column count mismatches.

Matt's directive: "Zero customers = freedom. Stop treating pre-launch like we have users to protect. Clean schema > migration safety."

## Decision

Pre-launch, Pane uses a single canonical schema definition (`init_schema()`) instead of a migration chain. On startup:
1. Check if the DB schema matches the canonical definition
2. If mismatch: delete the DB and recreate from canonical schema
3. Flat files survive DB deletion and can repopulate content via integrity check

Post-launch, a proper migration system will be built (#492) - but only when we actually have users whose data must be preserved.

## Consequences

### Positive
- **No migration bugs** - One schema definition, no ordering dependencies, no partial-apply failures.
- **Fresh install = existing install** - Same schema everywhere, always.
- **Fast iteration** - Schema changes are a single file edit, not a new migration + testing the chain.
- **Clean slate** - Removes 26 migration files worth of accumulated complexity.

### Negative
- **Data loss on schema change** - Changing the schema means the DB gets deleted. Acceptable pre-launch (no users), unacceptable post-launch.
- **No rollback** - Can't "undo" a schema change by reverting a migration. Must restore from backup.
- **Temporary approach** - Must build real migrations before launch (#492).

### Risks
- If we forget to build migrations before launch, early users lose data on updates.
- Matt's personal data gets deleted during development when schema changes. Mitigated by automated backups.

## Alternatives Considered

**Keep migration chain** - Failed 3 times on real data. Migration 26 couldn't be applied without breaking crsqlite CRR state. Chain complexity was the problem.

**Versioned schema snapshots** - Export current schema, diff against target, auto-generate ALTER statements. Too complex for pre-launch, good for post-launch.

**Manual migrations** - Write ALTER TABLE by hand for each change. Error-prone, doesn't handle crsqlite CRR registration.

## References

- Canonical schema doc: `~/Shared/ExampleApp/docs/canonical-schema.md` (42 tables, 20 CRR-enabled)
- Post-launch migration infra: #492
- v0.60.0 failures: 3 attempts, all failed on real DB
- Matt quote: "Zero customers = freedom"

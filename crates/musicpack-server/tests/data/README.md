# `library-c-reference.db` — C-created reference database

Byte-verbatim copy of the legacy C server's own library database
(`docker/library/library.db` in the reference MusicPack repository, created
by the vendored-SQLite C implementation of `server/src/db.c` + `schema.c`).

Properties asserted by `tests/db_compat.rs`:

- `schema_version` = **10** (all ten forward-only migrations applied)
- WAL journal mode (persistent in the database header)
- the full stage-10 object set: 15 tables, 14 named indexes (+ SQLite
  auto-indexes), no triggers/views
- `sqlite_master` DDL text identical to what the Rust migration SQL
  produces

The copy is **never modified**: tests work on a temporary copy, and the
committed file must keep checksumming to the source. It contains the legacy
repository's two-package demo library data, which is what makes it a
realistic compatibility probe.

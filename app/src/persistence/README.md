# Persistence and migrations

NeoWarp uses a separate SQLite namespace. Keep ordinary terminal history, blocks, settings and window/tab/pane restoration when removing retired feature state.

## Database boundaries

Work from the repository root and use a fresh review database under `.work/`. Never run migration commands against original Warp data or an existing runtime profile. Editing an already-applied baseline does not update an existing development database.

Before the first NeoWarp release, reduce the local-terminal baseline together with its consumers, models and schema. Once a NeoWarp baseline has shipped, preserve it and add forward migrations for supported NeoWarp data.

## Updating the schema

`crates/persistence/src/lib.rs` embeds `crates/persistence/migrations/`. Keep each migration's `up.sql` and `down.sql` consistent. Use the installed Diesel CLI compatible with the repository; avoid unrelated bootstrap or toolchain changes.

Choose an unused database path, then run:

```sh
diesel migration run --migration-dir crates/persistence/migrations --database-url .work/schema-review.sqlite
diesel print-schema --database-url .work/schema-review.sqlite
```

The root `diesel.toml` configures generation of `crates/persistence/src/schema.rs` and limits query combinations to related tables. Generate the schema through Diesel, then update `model.rs` and actual readers/writers. Do not retain unused table declarations or feature-only types.

## Verification

Check the fresh schema, affected existing SQLite round trips and any changed save/restore consumers. Test rollback SQL only on a disposable database created for that check. Do not erase or migrate user data as cleanup.

Run Rust checks with the root build environment, `--locked` and the configured job limit. Keep databases, probes and logs in `.work/`; retain useful evidence and move obsolete temporary material to `.work/archived/`.

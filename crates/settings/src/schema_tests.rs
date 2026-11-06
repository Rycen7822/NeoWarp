use std::collections::HashSet;

use schemars::SchemaGenerator;

use crate::schema::SettingSchemaEntry;

fn entries() -> Vec<&'static SettingSchemaEntry> {
    inventory::iter::<SettingSchemaEntry>.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Invariant tests (run over all registered entries)
// ---------------------------------------------------------------------------

#[test]
fn at_least_one_entry_exists() {
    assert!(
        !entries().is_empty(),
        "Expected at least one SettingSchemaEntry to be registered"
    );
}

#[test]
fn no_empty_storage_keys() {
    for entry in entries() {
        assert!(
            !entry.storage_key.is_empty(),
            "Found entry with empty storage_key"
        );
    }
}

#[test]
fn all_defaults_produce_valid_json() {
    for entry in entries() {
        let json = (entry.default_value_fn)();
        assert!(
            serde_json::from_str::<serde_json::Value>(&json).is_ok(),
            "default_value_fn for '{}' produced invalid JSON: {json}",
            entry.storage_key
        );
    }
}

#[test]
fn all_schemas_are_serializable() {
    let mut schema_gen = SchemaGenerator::default();
    for entry in entries() {
        let schema = (entry.schema_fn)(&mut schema_gen);
        assert!(
            serde_json::to_string(&schema).is_ok(),
            "schema_fn for '{}' produced a non-serializable schema",
            entry.storage_key
        );
    }
}

#[test]
fn all_supported_platforms_fn_succeed() {
    for entry in entries() {
        // Just call it to ensure it doesn't panic.
        let _ = (entry.supported_platforms_fn)();
    }
}

#[test]
fn hierarchy_values_are_well_formed() {
    for entry in entries() {
        if let Some(h) = entry.hierarchy {
            assert!(
                !h.starts_with('.') && !h.ends_with('.'),
                "Hierarchy for '{}' has leading/trailing dots: '{h}'",
                entry.storage_key
            );
            assert!(
                !h.contains(".."),
                "Hierarchy for '{}' contains consecutive dots: '{h}'",
                entry.storage_key
            );
        }
    }
}

// ---------------------------------------------------------------------------
// $ref resolution test
// ---------------------------------------------------------------------------

#[test]
fn all_refs_resolve_to_definitions() {
    let mut schema_gen = SchemaGenerator::default();

    // Process all entries through the shared generator
    for entry in entries() {
        let _schema = (entry.schema_fn)(&mut schema_gen);
    }

    // Collect all $ref pointers from the schemas
    fn collect_refs(value: &serde_json::Value, refs: &mut HashSet<String>) {
        match value {
            serde_json::Value::Object(map) => {
                if let Some(serde_json::Value::String(r)) = map.get("$ref") {
                    refs.insert(r.clone());
                }
                for v in map.values() {
                    collect_refs(v, refs);
                }
            }
            serde_json::Value::Array(arr) => {
                for v in arr {
                    collect_refs(v, refs);
                }
            }
            _ => {}
        }
    }

    // Re-process to collect refs from the output
    let mut schema_gen2 = SchemaGenerator::default();
    let mut all_refs = HashSet::new();
    for entry in entries() {
        let schema = (entry.schema_fn)(&mut schema_gen2);
        let value = serde_json::to_value(&schema).unwrap();
        collect_refs(&value, &mut all_refs);
    }

    let defs = schema_gen2.definitions();
    for r in &all_refs {
        // schemars 1.x uses "#/$defs/TypeName"
        if let Some(type_name) = r.strip_prefix("#/$defs/") {
            assert!(
                defs.contains_key(type_name),
                "$ref '{r}' does not resolve to any definition. Available: {:?}",
                defs.keys().collect::<Vec<_>>()
            );
        }
    }
}

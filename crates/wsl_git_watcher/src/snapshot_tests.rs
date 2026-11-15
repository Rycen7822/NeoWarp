use super::*;

fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    source_git(directory.path(), &["init", "--quiet", "--template="]).unwrap();
    directory
}

#[test]
fn captures_ignored_shell_changes_without_touching_source_index_or_filters() {
    let root = repository();
    let cache = tempfile::tempdir().unwrap();
    fs::write(root.path().join(".gitignore"), "ignored/\n").unwrap();
    fs::write(
        root.path().join(".gitattributes"),
        "*.txt filter=must-not-run\n",
    )
    .unwrap();
    source_git(
        root.path(),
        &[
            "config",
            "filter.must-not-run.clean",
            "neowarp-nonexistent-filter",
        ],
    )
    .unwrap();
    source_git(
        root.path(),
        &["config", "filter.must-not-run.required", "true"],
    )
    .unwrap();
    fs::write(root.path().join("staged.rs"), "staged\n").unwrap();
    source_git(root.path(), &["add", "staged.rs"]).unwrap();
    let source_index = fs::read(root.path().join(".git/index")).unwrap();
    let source_refs = source_git(root.path(), &["show-ref", "--head"]).unwrap_or_default();
    fs::create_dir(root.path().join("ignored")).unwrap();
    let text = "x".repeat(8191);
    let after = format!("{text}after!\n");
    fs::write(root.path().join("ignored/old.txt"), format!("{text}\r\n")).unwrap();
    fs::write(root.path().join("removed.txt"), "deleted\rcontent\r\n").unwrap();
    fs::write(root.path().join("binary.bin"), [0u8; 256]).unwrap();
    fs::write(root.path().join("transition.txt"), "before\n").unwrap();
    fs::create_dir(root.path().join("nested")).unwrap();
    source_git(
        &root.path().join("nested"),
        &["init", "--quiet", "--template="],
    )
    .unwrap();
    // Larger text uses Git's ordinary clean/write path rather than the bounded preload buffer.
    let large = "y".repeat(1024 * 1024);
    fs::write(
        root.path().join("nested/space 代码.txt"),
        format!("{large}\r\n"),
    )
    .unwrap();
    let mut store = Store::new(root.path(), cache.path()).unwrap();
    let cancelled = AtomicBool::new(false);
    store.capture("before", &cancelled).unwrap();
    // File writes represent the observable effects of arbitrary shell/tools.
    fs::write(root.path().join("ignored/old.txt"), &after).unwrap();
    fs::write(root.path().join("ignored/new.txt"), "created\n").unwrap();
    fs::remove_file(root.path().join("removed.txt")).unwrap();
    fs::write(root.path().join("nested/space 代码.txt"), "nested after\n").unwrap();
    fs::write(root.path().join("transition.txt"), [0u8; 256]).unwrap();
    store.capture("after", &cancelled).unwrap();
    assert_eq!(
        store
            .read("before", Path::new("removed.txt"), 1024)
            .unwrap(),
        "deleted\rcontent\r\n"
    );
    let changes: Vec<_> = store
        .changes("before", "after")
        .unwrap()
        .into_iter()
        .map(|change| (change.path, change.kind))
        .collect();
    assert_eq!(
        store
            .read(
                "before",
                Path::new("nested/space 代码.txt"),
                2 * 1024 * 1024
            )
            .unwrap(),
        format!("{large}\n")
    );
    assert_eq!(
        changes,
        [
            ("ignored/new.txt".into(), ChangeKind::Added),
            ("ignored/old.txt".into(), ChangeKind::Modified),
            ("nested/space 代码.txt".into(), ChangeKind::Modified),
            ("removed.txt".into(), ChangeKind::Deleted),
            ("transition.txt".into(), ChangeKind::Binary),
        ]
    );
    assert_eq!(
        store
            .read("before", Path::new("ignored/old.txt"), 16384)
            .unwrap(),
        format!("{text}\n")
    );
    assert_eq!(
        store
            .read("after", Path::new("ignored/old.txt"), 16384)
            .unwrap(),
        after
    );
    assert_eq!(
        fs::read(root.path().join(".git/index")).unwrap(),
        source_index
    );
    assert_eq!(
        source_git(root.path(), &["show-ref", "--head"]).unwrap_or_default(),
        source_refs
    );
    let path = store.directory().to_path_buf();
    drop(store);
    assert!(
        !path.exists(),
        "session disposal must remove its private index and objects"
    );
}

#[test]
fn failed_capture_preserves_baseline_and_release_reclaims_old_objects() {
    let root = repository();
    let cache = tempfile::tempdir().unwrap();
    fs::write(root.path().join("old.txt"), "unique old content\n").unwrap();
    let mut store = Store::new(root.path(), cache.path()).unwrap();
    assert!(
        store
            .capture("initial-cancelled", &AtomicBool::new(true))
            .is_err()
    );
    let baseline = store.capture("baseline", &AtomicBool::new(false)).unwrap();
    #[cfg(windows)]
    {
        fs::write(
            root.path().join("raced.txt"),
            "changed between preload and index\n",
        )
        .unwrap();
        store
            .git(&["update-index", "--add", "--info-only", "raced.txt"], None)
            .unwrap();
        assert!(store.git(&["cat-file", "-e", ":raced.txt"], None).is_err());
        let mut remaining = NamedTempFile::new_in(store.directory()).unwrap();
        store.finish_initial_index(&mut remaining).unwrap();
        assert_eq!(
            store
                .git(&["cat-file", "blob", ":raced.txt"], None)
                .unwrap(),
            "changed between preload and index\n"
        );
    }
    #[cfg(windows)]
    assert!(
        store
            .import_objects(|_| {
                File::open(root.path().join("missing.txt"))?;
                Ok(())
            })
            .is_err()
    );
    let blob = store
        .git(&["rev-parse", &format!("{}:old.txt", baseline.tree)], None)
        .unwrap();
    fs::write(root.path().join("old.txt"), "replacement\n").unwrap();
    assert!(store.capture("cancelled", &AtomicBool::new(true)).is_err());
    assert!(store.snapshot("cancelled").is_none());
    assert_eq!(
        store.read("baseline", Path::new("old.txt"), 1024).unwrap(),
        "unique old content\n"
    );
    store.capture("preview", &AtomicBool::new(false)).unwrap();
    store.release("preview").unwrap();
    let current = store.capture("current", &AtomicBool::new(false)).unwrap();
    store.release("baseline").unwrap();
    assert!(
        store.git(&["cat-file", "-e", blob.trim()], None).is_err(),
        "released old blob must be pruned"
    );
    assert_eq!(
        store.read("current", Path::new("old.txt"), 1024).unwrap(),
        "replacement\n"
    );
    store.release("current").unwrap();
    assert!(
        store
            .git(
                &["cat-file", "-e", &format!("{}:old.txt", current.tree)],
                None
            )
            .is_err()
    );
    store.capture("abandoned", &AtomicBool::new(false)).unwrap();
    let Store {
        directory,
        _lock: owner_lock,
        ..
    } = store;
    let abandoned = directory.keep();
    drop(owner_lock);
    let _recovered = Store::new(root.path(), cache.path()).unwrap();
    assert!(
        !abandoned.exists(),
        "unlocked cache left by an exited owner must be reclaimed"
    );
}

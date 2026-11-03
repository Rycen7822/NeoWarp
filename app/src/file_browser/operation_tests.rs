use super::{Operation, apply};

#[test]
fn creates_renames_and_deletes_sidebar_entries() {
    futures_lite::future::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        assert_eq!(
            apply(root.clone(), Operation::NewFolder, "folder".into())
                .await
                .unwrap(),
            root
        );
        let folder = root.join("folder");
        apply(folder.clone(), Operation::NewFile, "first.txt".into())
            .await
            .unwrap();
        std::fs::write(folder.join("first.txt"), "keep contents").unwrap();
        assert_eq!(
            apply(
                folder.join("first.txt"),
                Operation::Rename,
                "renamed.txt".into()
            )
            .await
            .unwrap(),
            folder
        );
        assert!(!folder.join("first.txt").exists());
        assert_eq!(
            std::fs::read_to_string(folder.join("renamed.txt")).unwrap(),
            "keep contents"
        );
        apply(folder.clone(), Operation::Rename, "renamed-folder".into())
            .await
            .unwrap();
        let renamed = root.join("renamed-folder");
        assert!(renamed.join("renamed.txt").is_file());
        apply(
            renamed.join("renamed.txt"),
            Operation::Delete,
            String::new(),
        )
        .await
        .unwrap();
        assert!(!renamed.join("renamed.txt").exists());
        apply(renamed.clone(), Operation::NewFile, "child.txt".into())
            .await
            .unwrap();
        assert_eq!(
            apply(renamed.clone(), Operation::Delete, String::new())
                .await
                .unwrap(),
            root
        );
        assert!(!renamed.exists());
    });
}

#[test]
fn rejects_path_traversal_and_existing_destinations_without_changing_contents() {
    futures_lite::future::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        for name in [
            "",
            "..",
            "../outside.txt",
            "nested/file.txt",
            "nested\\file.txt",
        ] {
            assert!(
                apply(root.clone(), Operation::NewFile, name.into())
                    .await
                    .is_err()
            );
        }
        let source = root.join("source.txt");
        let target = root.join("target.txt");
        std::fs::write(&source, "source").unwrap();
        std::fs::write(&target, "target").unwrap();
        assert!(
            apply(root.clone(), Operation::NewFile, "target.txt".into())
                .await
                .is_err()
        );
        assert!(
            apply(source.clone(), Operation::Rename, "target.txt".into())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(source).unwrap(), "source");
        assert_eq!(std::fs::read_to_string(target).unwrap(), "target");
    });
}

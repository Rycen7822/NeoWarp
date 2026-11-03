use warp_util::git::run_git_command;

use super::*;

#[test]
fn toolbar_folding_releases_bodies_and_reopens_lazily() {
    use repo_metadata::watcher::DirectoryWatcher;
    use warp_util::standardized_path::StandardizedPath;
    use warpui::App;
    use warpui::platform::WindowStyle;

    App::test((), |mut app| async move {
        crate::test_util::settings::initialize_settings_for_tests(&mut app);
        app.add_singleton_model(|_| Appearance::mock());
        app.add_singleton_model(|_| crate::workspace::sync_inputs::SyncedInputState::mock());
        app.add_singleton_model(|_| crate::vim_registers::VimRegisters::new());
        app.add_singleton_model(|_| {
            crate::settings_view::keybindings::KeybindingChangedNotifier::mock()
        });
        app.add_singleton_model(|_| crate::workspace::ActiveSession::default());
        let directory = tempfile::tempdir().unwrap();
        let watcher = app.add_singleton_model(DirectoryWatcher::new_for_testing);
        let repository = watcher.update(&mut app, |watcher, ctx| {
            watcher
                .add_directory(
                    StandardizedPath::from_local_canonicalized(directory.path()).unwrap(),
                    ctx,
                )
                .unwrap()
        });
        let status =
            app.add_model(|ctx| GitRepoStatusModel::new_local_for_test(repository, None, ctx));
        let (_, review) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            GitReviewView::new(directory.path().to_owned(), status, ctx)
        });
        review.update(&mut app, |review, ctx| {
            review.set_files(
                vec![
                    FileChangeEntry {
                        path: "a.txt".into(),
                        additions: 1,
                        deletions: 0,
                    },
                    FileChangeEntry {
                        path: "b.txt".into(),
                        additions: 1,
                        deletions: 0,
                    },
                ],
                ctx,
            );
            let editor = ctx.add_view(|ctx| {
                CodeEditorView::new(
                    None,
                    None,
                    CodeEditorRenderOptions::new(VerticalExpansionBehavior::GrowToMaxHeight),
                    ctx,
                )
            });
            let weak_editor = editor.downgrade();
            editor.update(ctx, |editor, ctx| {
                editor.set_review_content("old", "new", ctx)
            });
            review.files[0].editor = Some(editor);
            review.files[0].base = Some(Arc::new("old".into()));
            review.handle_action(&GitReviewAction::Toggle("b.txt".into()), ctx);
            review.handle_action(&GitReviewAction::ToggleAll, ctx);
            assert!(
                review
                    .files
                    .iter()
                    .all(|card| !card.expanded && card.editor.is_none() && card.base.is_none())
            );
            assert!(weak_editor.upgrade(ctx).is_none());
            assert!(review.files[0].editor_state.is_some());
            review.handle_action(&GitReviewAction::ToggleWrapping, ctx);
            review.handle_action(&GitReviewAction::ToggleAll, ctx);
            assert!(review.wrap_lines);
            assert!(
                review
                    .files
                    .iter()
                    .all(|card| card.expanded && card.dirty && card.editor.is_none())
            );
            assert!(
                review.loading.is_none(),
                "Expanding must defer body reads until cards are visible"
            );
        });
    });
}

async fn fixture() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Test"],
        vec!["config", "commit.gpgsign", "false"],
        vec!["config", "core.autocrlf", "false"],
    ] {
        run_git_command(root, &args).await.unwrap();
    }
    std::fs::write(root.join("file.txt"), "base\n").unwrap();
    run_git_command(root, &["add", "file.txt"]).await.unwrap();
    run_git_command(root, &["commit", "-m", "base"])
        .await
        .unwrap();
    run_git_command(root, &["checkout", "-b", "feature"])
        .await
        .unwrap();
    std::fs::write(root.join("file.txt"), "base\ncommitted\n").unwrap();
    run_git_command(root, &["commit", "-am", "feature"])
        .await
        .unwrap();
    std::fs::write(root.join("file.txt"), "base\ncommitted\nedited\n").unwrap();
    std::fs::write(root.join("untracked.txt"), "new\n").unwrap();
    directory
}

#[tokio::test]
async fn comparison_counts_and_visible_content_follow_the_selected_base_after_editing() {
    let directory = fixture().await;
    let root = directory.path().to_path_buf();
    let head = super::super::diff_state::diff_metadata_against_head(&root)
        .await
        .unwrap();
    assert_eq!(head.aggregate_stats.files_changed, 2);
    assert_eq!(head.aggregate_stats.total_additions, 2);
    let head_file = load_file(root.clone(), "file.txt".into(), DiffMode::Head, None, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*head_file.0, "base\ncommitted\n");

    let (base, files) = load_comparison(root.clone(), DiffMode::MainBranch, None)
        .await
        .unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files.iter().map(|file| file.additions).sum::<usize>(), 3);
    assert_eq!(files.iter().map(|file| file.deletions).sum::<usize>(), 0);
    let branch_file = load_file(
        root.clone(),
        "file.txt".into(),
        DiffMode::MainBranch,
        Some(base.clone()),
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(*branch_file.0, "base\n");
    assert_eq!(branch_file.1, head_file.1);

    // An external editor changes the working tree without running a terminal command.
    std::fs::write(root.join("file.txt"), "base\ncommitted\nedited\nnext\n").unwrap();
    let (_, refreshed) = load_comparison(root.clone(), DiffMode::MainBranch, Some(base.clone()))
        .await
        .unwrap();
    assert_eq!(
        refreshed.iter().map(|file| file.additions).sum::<usize>(),
        4
    );
    let updated = load_file(
        root.clone(),
        "file.txt".into(),
        DiffMode::MainBranch,
        Some(base),
        Some(branch_file.0.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(Arc::ptr_eq(&updated.0, &branch_file.0));
    assert_eq!(updated.1, "base\ncommitted\nedited\nnext\n");
    let head_again = load_file(root.clone(), "file.txt".into(), DiffMode::Head, None, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*head_again.0, "base\ncommitted\n");
    assert_eq!(head_again.1, updated.1);
    assert_eq!(
        crate::util::git::detect_current_branch(&root)
            .await
            .unwrap(),
        "feature"
    );
}

#[tokio::test]
async fn missing_comparison_branch_is_an_error_instead_of_an_empty_success() {
    let directory = fixture().await;
    let result = load_comparison(
        directory.path().to_path_buf(),
        DiffMode::OtherBranch("missing-branch".into()),
        None,
    )
    .await;
    assert!(result.is_err());
}

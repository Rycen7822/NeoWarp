use std::iter;

use warp_util::path::CleanPathResult;

use super::super::TerminalView;
use super::{GridHighlightedLink, path_without_trailing_sentence_punctuation};
use crate::terminal::model::grid::grid_handler::PossiblePath;
use crate::terminal::model::index::Point;
use crate::terminal::model::terminal_model::WithinModel;

#[cfg(windows)]
#[test]
fn file_urls_resolve_windows_and_wsl_paths_without_changing_the_target_host() {
    let shell = crate::terminal::ShellLaunchData::WSL {
        distro: "NeoWarp-Link-Test".into(),
    };
    let resolve = |uri| {
        super::file_url_path(
            &url::Url::parse(uri).unwrap(),
            Some(&shell),
            Some("linux-box"),
        )
    };
    assert_eq!(
        resolve("file:///home/tester/%E5%9B%BE%20%231%25.png"),
        Some(r"\\WSL$\NeoWarp-Link-Test\home\tester\图 #1%.png".into())
    );
    assert_eq!(
        resolve("file://LINUX-BOX/home/tester/image.png"),
        Some(r"\\WSL$\NeoWarp-Link-Test\home\tester\image.png".into())
    );
    assert_eq!(
        resolve("file:///mnt/d/NeoWarp-Link-Test/image.png"),
        Some(r"D:\NeoWarp-Link-Test\image.png".into())
    );
    assert_eq!(
        resolve("file:///D:/NeoWarp-Link-Test/image.png"),
        Some(r"D:\NeoWarp-Link-Test\image.png".into())
    );
    assert_eq!(
        resolve("file://wsl.localhost/Other/home/tester/image.png"),
        Some(r"\\wsl.localhost\Other\home\tester\image.png".into())
    );
}

#[cfg(windows)]
#[test]
fn invalid_file_urls_do_not_fall_back_to_local_windows_files() {
    let shell = crate::terminal::ShellLaunchData::WSL {
        distro: "NeoWarp-Link-Test".into(),
    };
    for uri in [
        "file:///home/tester/image%00.png",
        "file:///home/tester/%FF.png",
    ] {
        let url = url::Url::parse(uri).unwrap();
        assert_eq!(super::file_url_path(&url, Some(&shell), None), None);
    }
    let url = url::Url::parse("file:///home/tester/image.png").unwrap();
    assert_eq!(super::file_url_path(&url, None, None), None);
}

#[test]
fn strips_only_sentence_periods() {
    // A trailing period after a real file name is sentence punctuation.
    assert_eq!(
        path_without_trailing_sentence_punctuation("notes/README.md.").map(|trimmed| trimmed.path),
        Some("notes/README.md")
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation(".gitignore.").map(|trimmed| trimmed.path),
        Some(".gitignore")
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation("C:/Users/c/warp-md-test.md.")
            .map(|trimmed| trimmed.path),
        Some("C:/Users/c/warp-md-test.md")
    );

    // No trailing period -> nothing to trim.
    assert_eq!(
        path_without_trailing_sentence_punctuation("notes/README.md").map(|trimmed| trimmed.path),
        None
    );

    // `.`/`..` path components must be preserved, not treated as punctuation.
    assert_eq!(
        path_without_trailing_sentence_punctuation(".").map(|trimmed| trimmed.path),
        None
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation("..").map(|trimmed| trimmed.path),
        None
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation("foo/.").map(|trimmed| trimmed.path),
        None
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation("foo/..").map(|trimmed| trimmed.path),
        None
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation("foo..").map(|trimmed| trimmed.path),
        None
    );
}

#[test]
fn strips_trailing_fullwidth_sentence_punctuation() {
    let trimmed = path_without_trailing_sentence_punctuation("notes/README.md，")
        .expect("fullwidth comma should be stripped");
    assert_eq!(trimmed.path, "notes/README.md");
    assert_eq!(trimmed.removed_width, 2);

    let trimmed = path_without_trailing_sentence_punctuation("notes/README.md。！？")
        .expect("CJK sentence punctuation should be stripped");
    assert_eq!(trimmed.path, "notes/README.md");
    assert_eq!(trimmed.removed_width, 6);
}

// Regression test for https://github.com/warpdotdev/warp/issues/11477:
// a `.md` path at the end of a sentence captured the trailing period, so the
// resolved file and the highlight range ended in `.md.` and the file failed
// markdown classification. The trailing period must be excluded from both.
#[test]
fn compute_valid_paths_excludes_trailing_sentence_period() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("warp-md-test.md");
    std::fs::write(&file, "# Hello\n").unwrap();

    // The captured token as it would appear in `Drafted at <abs path>.`
    let token = format!("{}.", file.to_string_lossy());
    let end_col = token.chars().count() - 1;
    let candidate = WithinModel::AltScreen(PossiblePath {
        path: CleanPathResult {
            path: token,
            line_and_column_num: None,
        },
        range: Point { row: 0, col: 0 }..=Point {
            row: 0,
            col: end_col,
        },
    });

    let shell = cfg!(windows).then(|| crate::terminal::ShellLaunchData::WSL {
        distro: "NeoWarp-Link-Test".into(),
    });
    let link = TerminalView::compute_valid_paths(
        dir.path().to_str().unwrap(),
        iter::once(candidate),
        1000,
        shell,
    )
    .expect("the markdown file should be detected as a link");

    let GridHighlightedLink::File(file_link) = link else {
        panic!("expected a file link");
    };
    let file_link = file_link.get_inner();

    // The resolved file excludes the trailing period (so it classifies as `.md`)...
    assert_eq!(
        file_link.absolute_path.file_name().unwrap(),
        "warp-md-test.md"
    );
    // ...and the highlighted range stops before the trailing period.
    assert_eq!(
        *file_link.link.range().end(),
        Point {
            row: 0,
            col: end_col - 1,
        }
    );
}

#[test]
fn compute_valid_paths_excludes_trailing_fullwidth_sentence_punctuation() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("warp-md-test.md");
    std::fs::write(&file, "# Hello\n").unwrap();

    let token = format!("{}，", file.to_string_lossy());
    let punctuation_width = 2;
    let end_col = token.chars().count();
    let candidate = WithinModel::AltScreen(PossiblePath {
        path: CleanPathResult {
            path: token,
            line_and_column_num: None,
        },
        range: Point { row: 0, col: 0 }..=Point {
            row: 0,
            col: end_col,
        },
    });

    let link = TerminalView::compute_valid_paths(
        dir.path().to_str().unwrap(),
        iter::once(candidate),
        1000,
        None,
    )
    .expect("the markdown file should be detected as a link");

    let GridHighlightedLink::File(file_link) = link else {
        panic!("expected a file link");
    };
    let file_link = file_link.get_inner();

    assert_eq!(
        file_link.absolute_path.file_name().unwrap(),
        "warp-md-test.md"
    );
    assert_eq!(
        *file_link.link.range().end(),
        Point {
            row: 0,
            col: end_col - punctuation_width,
        }
    );
}

#[test]
fn compute_valid_paths_keeps_trailing_fullwidth_punctuation_when_it_is_the_filename() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("warp-md-test.md，");
    std::fs::write(&file, "# Hello\n").unwrap();

    let token = "warp-md-test.md，".to_string();
    let end_col = token.chars().count();
    let candidate = WithinModel::AltScreen(PossiblePath {
        path: CleanPathResult {
            path: token,
            line_and_column_num: None,
        },
        range: Point { row: 0, col: 0 }..=Point {
            row: 0,
            col: end_col,
        },
    });

    let working_directory = dir.path().to_str().unwrap().to_string();
    let shell = cfg!(windows).then(|| crate::terminal::ShellLaunchData::WSL {
        distro: "NeoWarp-Link-Test".into(),
    });
    #[cfg(windows)]
    let working_directory = warp_util::path::convert_windows_path_to_wsl(&working_directory);
    let link =
        TerminalView::compute_valid_paths(&working_directory, iter::once(candidate), 1000, shell)
            .expect("the file with fullwidth punctuation should be detected as a link");

    let GridHighlightedLink::File(file_link) = link else {
        panic!("expected a file link");
    };
    let file_link = file_link.get_inner();

    assert_eq!(
        file_link.absolute_path.file_name().unwrap(),
        "warp-md-test.md，"
    );
    assert_eq!(
        *file_link.link.range().end(),
        Point {
            row: 0,
            col: end_col,
        }
    );
}

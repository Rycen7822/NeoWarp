use std::path::Path;

use super::relative_path;

#[test]
fn wsl_tool_path_matches_the_canonical_repository_root() {
    let root = Path::new(r"\\?\UNC\WSL$\Ubuntu\tmp\repo");
    let tool = Path::new(r"\\WSL$\Ubuntu\tmp\repo\src\edit.rs");
    assert_eq!(relative_path(root, tool).as_deref(), Some(r"src\edit.rs"));

    let outside = Path::new(r"\\WSL$\Ubuntu\tmp\repo\..\edit.rs");
    assert_eq!(relative_path(root, outside), None);
}

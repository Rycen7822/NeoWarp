use clap::Parser;

use super::*;

#[test]
fn identifies_worker_subcommands() {
    #[cfg(unix)]
    assert!(is_worker_invocation(&terminal_server_subcommand()));
    assert!(!is_worker_invocation("--prompt"));
    assert!(Args::try_parse_from(["neowarp", "agent", "run", "--prompt", "test"]).is_err());
}

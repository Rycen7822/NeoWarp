# Completion parser architecture

The completion parser turns shell input into commands with source spans. It can then classify a command against a
registered command signature. The lexer and recursive descent parser are private implementation details. Call the
public functions in `simple` or `classify_command` instead.

## Parse a command for completion

Use `simple::parse_for_completions` to find the last unclosed command. Its arguments are the source string, the
shell's escape character, and whether to parse quotes as literals. It returns `Option<LiteCommand>`.

```rust
use warp_util::path::EscapeChar;

use crate::parsers::simple::parse_for_completions;

let command = parse_for_completions(
    "warp --disable-telemetry",
    EscapeChar::Backslash,
    false,
)
.expect("input contains a command");

let parts = command.parts.iter().map(|part| part.item.as_str()).collect::<Vec<_>>();
assert_eq!(parts, ["warp", "--disable-telemetry"]);
assert_eq!(command.parts[0].span.start(), 0);
assert_eq!(command.parts[0].span.end(), 4);
assert_eq!(command.parts[1].span.start(), 5);
assert_eq!(command.parts[1].span.end(), 24);
```

The parser returns the last command when the input contains command separators. It descends into an open subshell
when that subshell contains the command being completed. An input without a command returns `None`.

`LiteCommand::parts` contains the command name and arguments as `Spanned<String>` values. `post_whitespace`
records trailing whitespace after the last part. Call `LiteCommand::joined_by_space` when a consumer needs the
parts joined by single spaces rather than the original source.

## Choose an entry point

The `simple` module provides these public parsing operations:

- `parse_for_completions` returns the last unclosed command for completion generation.
- `top_level_command` returns the first command name after removing leading environment variable assignments.
- `command_at_cursor_position` returns the innermost command containing a `ByteOffset`.
- `all_parsed_commands` iterates over all parsed commands and removes their leading environment assignments.
- `command_without_leading_env_vars` returns the first command's source without leading environment assignments.
- `decompose_command` returns nested command strings and whether the input contains input or output redirection.

Each entry point takes an `EscapeChar`. Pass the escape character for the active shell rather than assuming that
every shell uses a backslash.

## Track source spans

`Span` stores start and end byte offsets. The end offset is exclusive. `Spanned<T>` pairs a value with its span.
The parser uses these types to connect completion results to the original input.

`Span::slice` returns the corresponding source slice. It clamps out-of-range offsets and moves offsets back to
UTF-8 character boundaries before slicing.

```rust
use crate::meta::{Span, SpannedItem};

let input = "warp --disable-telemetry";
let command_name = Span::new(0, 4);
let argument = Span::new(5, 24);

assert_eq!(command_name.slice(input), "warp");
assert_eq!(argument.slice(input), "--disable-telemetry");
assert_eq!(command_name.until(argument), Span::new(0, 24));

let spanned = "warp".spanned(command_name);
assert_eq!(spanned.item, "warp");
assert_eq!(spanned.span, command_name);
```

Use `Span::new` only when `end >= start`. The constructor asserts this invariant.

## Classify a parsed command

`classify_command` converts a `LiteCommand` into a `ClassifiedCommand`. It uses the `CommandRegistry` to find a
matching signature and the `TopLevelCommandCaseSensitivity` value to control command-name matching.

Classification performs these operations:

1. Extract leading environment variable assignments and remove their tokens from the supplied token vector.
2. Find a registered command signature.
3. Classify flags, options, positional arguments, variables, and literal arguments.
4. Return a `Command::Classified` value for a registered signature or `Command::Unclassified` for an external
   command.

`ClassifiedCommand` contains the typed command, the leading environment assignments, and an optional `ParseError`.
An empty command returns `None`. The [parser tests](test.rs) show complete registry setup and classification
assertions.

## Source map

- `simple/lexer.rs` tokenizes shell input.
- `simple/parser.rs` parses commands, command separators, quotes, concatenation, and subshells.
- `simple/convert.rs` converts the internal representation to `LiteCommand`.
- `legacy.rs` applies command signatures to lite commands.
- `hir/mod.rs` defines the typed command representation.
- `../meta.rs` defines `Span` and `Spanned<T>`.

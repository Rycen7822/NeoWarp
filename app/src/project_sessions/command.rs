//! Recognize literal CLI launches without evaluating arbitrary shell input.

use std::collections::BTreeMap;

use warp_terminal::shell::ShellType;

use super::config::{Identity, Provider};

pub(crate) struct ParsedCommand {
    pub(crate) provider: Provider,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) identity: Option<Identity>,
    pub(crate) resuming: bool,
}

pub(crate) fn variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

pub(crate) fn parse(command: &str, shell: ShellType) -> Option<ParsedCommand> {
    let (argv, environment) = words(command, shell)?;
    let index = agent_index(&argv)?;
    let provider = provider(&argv[index])?;
    if argv[index + 1..]
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h" | "--version" | "-V"))
    {
        return None;
    }
    if provider == Provider::Pi
        && argv.get(index + 1).is_some_and(|arg| {
            matches!(
                arg.as_str(),
                "install" | "uninstall" | "remove" | "update" | "list"
            )
        })
    {
        return None;
    }
    let identity = explicit_identity(provider, &argv[index..]);
    let resuming = match provider {
        Provider::Codex => argv.get(index + 1).is_some_and(|arg| arg == "resume"),
        _ => argv[index + 1..].iter().any(|arg| {
            matches!(arg.as_str(), "--resume" | "--continue" | "--session")
                || (matches!(provider, Provider::Pi | Provider::Claude | Provider::Hermes)
                    && matches!(arg.as_str(), "-r" | "-c"))
        }),
    };
    Some(ParsedCommand {
        provider,
        environment,
        identity,
        resuming,
    })
}

pub(crate) fn words(
    command: &str,
    shell: ShellType,
) -> Option<(Vec<String>, BTreeMap<String, String>)> {
    let mut environment = BTreeMap::new();
    let argv = if shell == ShellType::PowerShell {
        let statements = powershell_words(command)?;
        let mut argv = Vec::new();
        for (index, statement) in statements.iter().enumerate() {
            if index + 1 == statements.len() {
                argv = statement.clone();
                if argv.first().is_some_and(|word| word == "&") {
                    argv.remove(0);
                }
            } else {
                let (name, value) = match statement.as_slice() {
                    [assignment] => assignment.strip_prefix("$env:")?.split_once('=')?,
                    [name, equals, value] if equals == "=" => {
                        (name.strip_prefix("$env:")?, value.as_str())
                    }
                    _ => return None,
                };
                if !variable_name(name) {
                    return None;
                }
                environment.insert(name.to_owned(), value.to_owned());
            }
        }
        argv
    } else {
        // Expansion and compound shell expressions need their runtime context; do not guess.
        if command.contains(['$', '`', ';', '|', '&', '\n', '>', '<']) {
            return None;
        }
        let mut argv = shlex::split(command)?;
        if argv.first().is_some_and(|word| word == "env") {
            argv.remove(0);
        }
        while let Some((name, value)) = argv.first().and_then(|word| word.split_once('=')) {
            if !variable_name(name) {
                return None;
            }
            environment.insert(name.to_owned(), value.to_owned());
            argv.remove(0);
        }
        argv
    };
    Some((argv, environment))
}

fn executable(command: &str) -> String {
    command
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(command)
        .to_ascii_lowercase()
        .trim_end_matches(".exe")
        .trim_end_matches(".cmd")
        .trim_end_matches(".ps1")
        .to_owned()
}

fn provider(command: &str) -> Option<Provider> {
    match executable(command).as_str() {
        "codex" => Some(Provider::Codex),
        "claude" => Some(Provider::Claude),
        "pi" => Some(Provider::Pi),
        "hermes" => Some(Provider::Hermes),
        _ => None,
    }
}

pub(crate) fn agent_index(argv: &[String]) -> Option<usize> {
    if provider(argv.first()?).is_some() {
        return Some(0);
    }
    let wrapper = executable(&argv[0]);
    if !matches!(wrapper.as_str(), "uv" | "conda") || argv.get(1)?.as_str() != "run" {
        return None;
    }
    let mut index = 2;
    while let Some(arg) = argv.get(index) {
        if arg == "--" {
            index += 1;
            break;
        }
        if !arg.starts_with('-') {
            break;
        }
        let (flag, joined) = arg
            .split_once('=')
            .map_or((arg.as_str(), false), |(flag, _)| (flag, true));
        match (wrapper.as_str(), flag) {
            ("uv", "--project" | "--python" | "--with" | "--with-editable")
            | ("conda", "--name" | "-n" | "--prefix" | "-p") => {
                index += if joined { 1 } else { 2 };
            }
            (
                "uv",
                "--active" | "--no-sync" | "--frozen" | "--locked" | "--offline" | "--no-project",
            )
            | ("conda", "--no-capture-output" | "--live-stream") => index += 1,
            _ => return None,
        }
    }
    provider(argv.get(index)?).map(|_| index)
}

fn explicit_identity(provider: Provider, argv: &[String]) -> Option<Identity> {
    let value = match provider {
        Provider::Codex => argv
            .windows(2)
            .find(|pair| pair[0] == "resume")
            .map(|pair| pair[1].as_str()),
        Provider::Claude | Provider::Hermes => {
            option(argv, "--resume").or_else(|| option(argv, "-r"))
        }
        Provider::Pi => option(argv, "--session"),
        Provider::Custom => None,
    }?;
    if value.is_empty() || value.starts_with('-') {
        return None;
    }
    Some(Identity {
        id: value.to_owned(),
        transcript: (provider == Provider::Pi).then(|| value.to_owned()),
    })
}

fn option<'a>(argv: &'a [String], name: &str) -> Option<&'a str> {
    argv.iter().enumerate().find_map(|(index, arg)| {
        if arg == name {
            argv.get(index + 1).map(String::as_str)
        } else {
            arg.strip_prefix(name)?.strip_prefix('=')
        }
    })
}

/// A literal PowerShell argument tokenizer. Expressions and substitutions are deliberately rejected.
fn powershell_words(command: &str) -> Option<Vec<Vec<String>>> {
    let mut statements = Vec::new();
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote = None;
    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some('\''), '\'') if chars.peek() == Some(&'\'') => {
                chars.next();
                word.push('\'');
            }
            (Some(delimiter), ch) if ch == delimiter => quote = None,
            (Some('\''), ch) => word.push(ch),
            (_, '`') => {
                let escaped = chars.next()?;
                if escaped.is_ascii_alphabetic() || escaped == '\n' {
                    return None;
                }
                word.push(escaped);
            }
            (None, '$')
                if word.is_empty()
                    && words.is_empty()
                    && chars.clone().take(4).collect::<String>() == "env:" =>
            {
                word.push(ch);
            }
            (_, '$' | '(' | ')' | '|' | '>' | '<' | '\n') => return None,
            (None, '@' | '[' | ']' | '{' | '}' | ',') => return None,
            (None, '\'' | '"') => {
                quote = Some(ch);
                started = true;
            }
            (None, ';') => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                if words.is_empty() {
                    return None;
                }
                statements.push(std::mem::take(&mut words));
                started = false;
            }
            (None, ch) if ch.is_whitespace() => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                started = false;
            }
            (_, ch) => {
                word.push(ch);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    if !words.is_empty() {
        statements.push(words);
    }
    Some(statements)
}

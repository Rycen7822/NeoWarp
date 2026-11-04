use anyhow::{Context, Result, bail, ensure};
use warp_terminal::shell::{ShellLaunchData, ShellType, shell_quote_arg};
use warpui::{AppContext, SingletonEntity};

use super::command;
use super::config::{Activation, Host, Project, Provider, SavedShell, Session};
use crate::terminal::available_shells::{AvailableShell, AvailableShells};

#[derive(Clone, Copy)]
pub(crate) enum Intent {
    New,
    Resume,
}

pub(crate) struct Launch {
    pub(crate) title: String,
    pub(crate) shell: AvailableShell,
    pub(crate) directory: std::path::PathBuf,
    pub(crate) command: String,
}

pub(crate) fn prepare(
    project: &Project,
    session: &Session,
    intent: Intent,
    app: &AppContext,
) -> Result<Launch> {
    let directory = project
        .host
        .native_path(&project.directory)
        .context("Invalid working directory")?;
    ensure!(
        directory.is_absolute() && directory.is_dir(),
        "Working directory is unavailable: {}",
        project.directory
    );
    let shell = resolve_shell(&project.host, &session.shell, app)?;
    Ok(Launch {
        title: session.title.clone(),
        shell,
        directory,
        command: script(project, session, intent)?,
    })
}

pub(crate) fn resolve_shell(
    host: &Host,
    saved: &SavedShell,
    app: &AppContext,
) -> Result<AvailableShell> {
    let shell = match host {
        Host::Local => AvailableShells::as_ref(app)
            .find_by_command_name(&saved.program)
            .filter(|shell| !shell.is_wsl())
            .or_else(|| AvailableShell::try_from(saved.program.as_str()).ok()),
        Host::Wsl(distro) => AvailableShells::as_ref(app)
            .get_from_shell_launch_data(&ShellLaunchData::WSL {
                distro: distro.clone(),
            })
            .and_then(|shell| shell.with_wsl_shell(saved.program.clone(), saved.kind)),
    }
    .context("The saved shell is unavailable; modify the session shell")?;
    let data = shell
        .get_valid_shell_path_and_type()
        .context("The saved shell is no longer available")?;
    if let warp_terminal::shell::ShellLaunchData::Executable { shell_type, .. }
    | warp_terminal::shell::ShellLaunchData::MSYS2 { shell_type, .. } = data
    {
        ensure!(
            shell_type == saved.kind,
            "Shell type does not match the saved command syntax"
        );
    }
    Ok(shell)
}

pub(crate) fn script(project: &Project, session: &Session, intent: Intent) -> Result<String> {
    let kind = session.shell.kind;
    let quote = |value: &str| shell_quote_arg(value, kind);
    let path = |value: &str| {
        if cfg!(windows) && project.host == Host::Local && kind != ShellType::PowerShell {
            value.replace('\\', "/")
        } else {
            value.to_owned()
        }
    };
    let mut environment = session.environment.clone();
    if let Some((_, explicit)) = command::words(&session.command, kind) {
        environment.extend(explicit);
    }
    let mut steps = vec![match kind {
        ShellType::PowerShell => format!("Set-Location -LiteralPath {}", quote(&project.directory)),
        _ => format!("cd -- {}", quote(&path(&project.directory))),
    }];
    for (name, value) in environment {
        ensure!(
            command::variable_name(&name),
            "Invalid environment variable name: {name}"
        );
        steps.push(match kind {
            ShellType::PowerShell => format!("$env:{name}={}", quote(&value)),
            ShellType::Fish => format!("set -gx {name} {}", quote(&value)),
            _ => format!("export {name}={}", quote(&value)),
        });
    }
    match &session.activation {
        Activation::None => {}
        Activation::Venv(directory) => {
            let windows = cfg!(windows) && project.host == Host::Local;
            let suffix = match (kind, windows) {
                (ShellType::PowerShell, true) => "Scripts/Activate.ps1",
                (ShellType::PowerShell, false) => "bin/Activate.ps1",
                (ShellType::Fish, _) => "bin/activate.fish",
                (_, true) => "Scripts/activate",
                (_, false) => "bin/activate",
            };
            let file = format!("{}/{suffix}", path(directory).trim_end_matches(['/', '\\']));
            steps.push(format!(
                "{} {}",
                if kind == ShellType::PowerShell {
                    "."
                } else {
                    "source"
                },
                quote(&file)
            ));
        }
        Activation::Conda {
            environment,
            executable,
        } => {
            let executable = quote(executable.as_deref().unwrap_or("conda"));
            let environment = quote(environment);
            match kind {
                ShellType::PowerShell => {
                    steps.push(format!(
                        "$__neowarp_hook=& {executable} 'shell.powershell' 'hook'"
                    ));
                    steps.push(
                        "if (!$? -or $LASTEXITCODE -ne 0) { throw 'Conda initialization failed' }"
                            .into(),
                    );
                    steps.push(
                        "Invoke-Expression ($__neowarp_hook -join [Environment]::NewLine)".into(),
                    );
                    steps.push(format!("conda activate {environment}"));
                    steps.push(
                        "if (!$? -or $LASTEXITCODE -ne 0) { throw 'Conda activation failed' }"
                            .into(),
                    );
                }
                ShellType::Fish => {
                    steps.push(format!("{executable} shell.fish hook | source"));
                    steps.push("test $pipestatus[1] -eq 0".into());
                    steps.push(format!("conda activate {environment}"));
                }
                ShellType::Bash | ShellType::Zsh => {
                    let shell = if kind == ShellType::Bash {
                        "bash"
                    } else {
                        "zsh"
                    };
                    steps.push(format!("__neowarp_hook=$({executable} shell.{shell} hook)"));
                    steps.push("eval \"$__neowarp_hook\"".into());
                    steps.push(format!("conda activate {environment}"));
                    steps.push("unset __neowarp_hook".into());
                }
            }
        }
    }
    let command = match intent {
        Intent::New => session.command.clone(),
        Intent::Resume => resume_command(session)?,
    };
    ensure!(!command.trim().is_empty(), "Startup command is empty");
    Ok(match kind {
        ShellType::PowerShell => format!(
            "& {{ $ErrorActionPreference='Stop'; try {{ {}; {} }} catch {{ Write-Error -ErrorRecord $_ -ErrorAction Continue }} }}",
            steps.join("; "),
            command
        ),
        ShellType::Fish => format!("{}; and begin\n{command}\nend", steps.join("; and ")),
        _ => format!("{} && {{\n{command}\n}}", steps.join(" && ")),
    })
}

fn resume_command(session: &Session) -> Result<String> {
    if let Some(command) = session
        .resume_command
        .as_ref()
        .filter(|command| !command.trim().is_empty())
    {
        return Ok(command.clone());
    }
    let identity = session
        .identity
        .as_ref()
        .context("Session identity is unconfirmed; use Modify to set its ID or resume command")?;
    let (argv, _) = command::words(&session.command, session.shell.kind)
        .context("Set an explicit resume command for this shell expression")?;
    ensure!(
        command::parse(&session.command, session.shell.kind)
            .is_some_and(|parsed| parsed.provider == session.provider),
        "The startup CLI changed; set an explicit resume command before continuing this conversation"
    );
    let index = command::agent_index(&argv).context("Set a resume command for this CLI")?;
    let mut resumed = argv[..=index].to_vec();
    match session.provider {
        Provider::Codex => resumed.extend(["resume".into(), identity.id.clone()]),
        Provider::Claude | Provider::Hermes => {
            resumed.extend(["--resume".into(), identity.id.clone()])
        }
        Provider::Pi => resumed.extend([
            "--session".into(),
            identity
                .transcript
                .clone()
                .context("Pi requires its session file")?,
        ]),
        Provider::Custom => bail!("Set an explicit resume command for this CLI"),
    }
    let mut args = argv[index + 1..].iter().peekable();
    let mut skip_resume_id = false;
    while let Some(arg) = args.next() {
        if arg == "resume" && session.provider == Provider::Codex {
            skip_resume_id = true;
            continue;
        }
        if skip_resume_id && !arg.starts_with('-') {
            skip_resume_id = false;
            continue;
        }
        if (session.provider == Provider::Pi && matches!(arg.as_str(), "-r" | "--resume" | "-c"))
            || (session.provider == Provider::Claude && arg == "-c")
        {
            continue;
        }
        if matches!(arg.as_str(), "--resume" | "--session")
            || (session.provider == Provider::Claude && arg == "-r")
        {
            if args.peek().is_some_and(|value| !value.starts_with('-')) {
                args.next();
            } else {
                ensure!(arg != "--session", "Missing original session selector");
            }
            continue;
        }
        if arg.starts_with("--resume=")
            || arg.starts_with("--session=")
            || matches!(arg.as_str(), "--last" | "--continue")
        {
            continue;
        }
        if !arg.starts_with('-') {
            continue;
        } // An initial prompt is not replayed.
        let flag = arg.split('=').next().unwrap_or(arg);
        let takes_value = match session.provider {
            Provider::Codex => matches!(
                flag,
                "-m" | "--model"
                    | "-p"
                    | "--profile"
                    | "-c"
                    | "--config"
                    | "-s"
                    | "--sandbox"
                    | "-a"
                    | "--ask-for-approval"
                    | "-C"
                    | "--cd"
                    | "--add-dir"
                    | "--enable"
                    | "--disable"
            ),
            Provider::Claude => matches!(
                flag,
                "--model"
                    | "--permission-mode"
                    | "--allowedTools"
                    | "--disallowedTools"
                    | "--add-dir"
                    | "--effort"
            ),
            Provider::Pi => matches!(flag, "--model" | "--provider" | "--thinking" | "--tools"),
            Provider::Hermes => matches!(flag, "--model" | "--provider"),
            Provider::Custom => false,
        };
        let switch = match session.provider {
            Provider::Codex => matches!(
                flag,
                "--oss"
                    | "--search"
                    | "--no-alt-screen"
                    | "--no-daemon"
                    | "--approve-for-me"
                    | "--dangerously-bypass-approvals-and-sandbox"
            ),
            Provider::Claude => matches!(flag, "--dangerously-skip-permissions" | "--verbose"),
            _ => false,
        };
        ensure!(
            takes_value || switch,
            "Set a resume command to preserve unsupported option {flag}"
        );
        resumed.push(arg.clone());
        if takes_value && !arg.contains('=') {
            resumed.push(args.next().context("Missing CLI option value")?.clone());
        }
    }
    let quote = |value: &str| shell_quote_arg(value, session.shell.kind);
    let command = resumed
        .iter()
        .map(|arg| quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(if session.shell.kind == ShellType::PowerShell {
        format!("& {command}")
    } else {
        command
    })
}

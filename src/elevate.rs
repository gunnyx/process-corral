//! Pure parts of "run `process-corral <subcommand>` elevated": building the parameter string for
//! ShellExecute's `runas` and turning the outcome into a message. The actual ShellExecute call
//! lives in the GUI binary; keeping this here makes it testable.

/// Quote one argument for a Windows command line, following the `CommandLineToArgvW` rules the
/// target program's startup code uses: arguments with spaces/tabs/quotes (or empty) are wrapped in
/// quotes, embedded quotes are backslash-escaped, and backslashes directly before a quote (or the
/// closing quote) are doubled.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '"')) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2)); // before the closing quote
    out.push('"');
    out
}

pub fn build_params<S: AsRef<str>>(args: &[S]) -> String {
    args.iter().map(|a| quote_arg(a.as_ref())).collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    ApplyConfig,
    Start,
    Stop,
    Install,
    Uninstall,
}

impl Command {
    pub fn subcommand(self) -> &'static str {
        match self {
            Command::ApplyConfig => "apply-config",
            Command::Start => "start",
            Command::Stop => "stop",
            Command::Install => "install",
            Command::Uninstall => "uninstall",
        }
    }
    pub fn verb(self) -> &'static str {
        match self {
            Command::ApplyConfig => "Saving settings",
            Command::Start => "Starting the service",
            Command::Stop => "Stopping the service",
            Command::Install => "Installing the service",
            Command::Uninstall => "Uninstalling the service",
        }
    }
}

/// What happened when we tried to run the elevated helper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecOutcome {
    Exited(i32),
    /// The user declined the UAC prompt (ERROR_CANCELLED).
    Cancelled,
    /// ShellExecute itself failed; carries a readable reason.
    LaunchFailed(String),
    /// The helper was still running after the timeout (it is not killed).
    TimedOut(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub ok: bool,
    pub text: String,
}

fn msg(ok: bool, text: impl Into<String>) -> Message {
    Message { ok, text: text.into() }
}

pub fn interpret(cmd: Command, outcome: &ExecOutcome) -> Message {
    match outcome {
        ExecOutcome::Cancelled => msg(
            false,
            format!("{} was cancelled: the Windows permission (UAC) prompt was declined. Nothing was changed.", cmd.verb()),
        ),
        ExecOutcome::LaunchFailed(why) => msg(false, format!("{} failed: could not start the elevated helper ({why}).", cmd.verb())),
        ExecOutcome::TimedOut(secs) => msg(
            false,
            format!("{}: the helper had not finished after {secs}s. It may still be running; check the service state.", cmd.verb()),
        ),
        ExecOutcome::Exited(code) => match (cmd, *code) {
            (Command::ApplyConfig, 0) => msg(
                true,
                "Settings saved. The running engine picks them up within about 2 seconds; no restart is needed.",
            ),
            (Command::ApplyConfig, 2) => msg(false, "Save failed: the helper was called incorrectly (usage error, exit 2). This is a bug in the GUI."),
            (Command::ApplyConfig, 3) => msg(false, "Save failed: the helper did not get administrator rights (exit 3). Nothing was changed."),
            (Command::ApplyConfig, 4) => msg(false, "Save failed: the helper could not read the temporary settings file (exit 4). Nothing was changed."),
            (Command::ApplyConfig, 5) => msg(false, "Save failed: the engine rejected the settings as invalid (exit 5). Nothing was changed."),
            (Command::ApplyConfig, 6) => msg(false, "Save failed: the helper could not write the config file (exit 6). The current config is unchanged."),
            (Command::ApplyConfig, n) => msg(false, format!("Save failed: the helper exited with unexpected code {n}.")),
            (c, 0) => msg(true, format!("{}: done.", c.verb())),
            (c, 1) => msg(
                false,
                format!("{} failed (exit 1). The helper's own message is not visible from here; run `process-corral {}` in an elevated terminal to see it, and check the Log tab.", c.verb(), c.subcommand()),
            ),
            (c, n) => msg(false, format!("{} failed: the helper exited with code {n}.", c.verb())),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_arguments_are_left_alone() {
        assert_eq!(quote_arg("apply-config"), "apply-config");
        assert_eq!(quote_arg(r"C:\Temp\x.ini"), r"C:\Temp\x.ini");
    }

    #[test]
    fn paths_with_spaces_are_quoted() {
        assert_eq!(quote_arg(r"C:\Program Files\ProcessCorral\x.ini"), r#""C:\Program Files\ProcessCorral\x.ini""#);
        assert_eq!(quote_arg(r"C:\Users\Jane Doe\AppData\Local\Temp\a b.ini"), r#""C:\Users\Jane Doe\AppData\Local\Temp\a b.ini""#);
    }

    #[test]
    fn empty_quotes_and_trailing_backslashes_follow_argv_rules() {
        assert_eq!(quote_arg(""), r#""""#);
        assert_eq!(quote_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote_arg(r"C:\dir with space\"), r#""C:\dir with space\\""#, "trailing backslash doubled before closing quote");
        assert_eq!(quote_arg(r#"a\"b c"#), r#""a\\\"b c""#, "backslashes before an embedded quote");
        assert_eq!(quote_arg(r"a\b c"), r#""a\b c""#, "interior backslashes untouched");
    }

    #[test]
    fn params_join_and_survive_a_parse_round_trip() {
        let args = ["apply-config", r"C:\Users\Jane Doe\Temp\pc 1.ini", "--config", r"C:\Program Files\X\c.ini"];
        let p = build_params(&args);
        assert_eq!(p, r#"apply-config "C:\Users\Jane Doe\Temp\pc 1.ini" --config "C:\Program Files\X\c.ini""#);
        assert_eq!(parse_like_argv(&p), args, "what the helper's argv will contain");
        for tricky in [r"C:\a b\", r#"q"uote d"#, "", r"\\server\share name\f.ini", r#"end\\"#] {
            assert_eq!(parse_like_argv(&build_params(&[tricky])), [tricky], "{tricky:?}");
        }
    }

    /// Minimal CommandLineToArgvW for the argument part (no program name).
    fn parse_like_argv(s: &str) -> Vec<String> {
        let (mut out, mut cur, mut in_q, mut started) = (vec![], String::new(), false, false);
        let c: Vec<char> = s.chars().collect();
        let mut i = 0;
        while i < c.len() {
            match c[i] {
                '\\' => {
                    let mut n = 0;
                    while i < c.len() && c[i] == '\\' {
                        n += 1;
                        i += 1;
                    }
                    started = true;
                    if i < c.len() && c[i] == '"' {
                        cur.extend(std::iter::repeat_n('\\', n / 2));
                        if n % 2 == 1 {
                            cur.push('"');
                            i += 1;
                        }
                    } else {
                        cur.extend(std::iter::repeat_n('\\', n));
                    }
                    continue;
                }
                '"' => {
                    in_q = !in_q;
                    started = true;
                }
                ' ' | '\t' if !in_q => {
                    if started {
                        out.push(std::mem::take(&mut cur));
                        started = false;
                    }
                }
                ch => {
                    cur.push(ch);
                    started = true;
                }
            }
            i += 1;
        }
        if started {
            out.push(cur);
        }
        out
    }

    #[test]
    fn apply_config_exit_codes_map_to_distinct_messages() {
        let m = |c| interpret(Command::ApplyConfig, &ExecOutcome::Exited(c));
        assert!(m(0).ok && m(0).text.contains("within about 2 seconds"));
        for (code, needle) in [(2, "usage error"), (3, "administrator"), (4, "could not read"), (5, "invalid"), (6, "could not write")] {
            let r = m(code);
            assert!(!r.ok, "{code}");
            assert!(r.text.contains(needle), "{code}: {}", r.text);
            assert!(r.text.contains(&format!("exit {code}")), "{code}: {}", r.text);
        }
        let all: std::collections::HashSet<_> = [0, 2, 3, 4, 5, 6, 77].iter().map(|c| m(*c).text).collect();
        assert_eq!(all.len(), 7, "every code has its own message");
        assert!(m(77).text.contains("77"));
    }

    #[test]
    fn service_commands_have_success_and_failure_messages() {
        for c in [Command::Start, Command::Stop, Command::Install, Command::Uninstall] {
            assert!(interpret(c, &ExecOutcome::Exited(0)).ok);
            let f = interpret(c, &ExecOutcome::Exited(1));
            assert!(!f.ok && f.text.contains(c.subcommand()), "{}", f.text);
            assert!(interpret(c, &ExecOutcome::Exited(9)).text.contains('9'));
        }
    }

    #[test]
    fn cancel_launch_failure_and_timeout() {
        let c = interpret(Command::ApplyConfig, &ExecOutcome::Cancelled);
        assert!(!c.ok && c.text.contains("cancelled") && c.text.contains("Nothing was changed"));
        let l = interpret(Command::Start, &ExecOutcome::LaunchFailed("file not found".into()));
        assert!(!l.ok && l.text.contains("file not found"));
        let t = interpret(Command::Stop, &ExecOutcome::TimedOut(120));
        assert!(!t.ok && t.text.contains("120"));
    }

    #[test]
    fn subcommand_names_match_the_engine() {
        assert_eq!(Command::ApplyConfig.subcommand(), "apply-config");
        assert_eq!(
            [Command::Start, Command::Stop, Command::Install, Command::Uninstall].map(Command::subcommand),
            ["start", "stop", "install", "uninstall"]
        );
    }
}

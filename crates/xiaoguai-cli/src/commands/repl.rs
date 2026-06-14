//! Pure helpers for the `xiaoguai repl` slash-command surface.
//!
//! Terminal I/O (reading stdin, streaming the reply) stays in
//! `main.rs::handle_repl`; this module is just the testable command grammar
//! plus the help text, so the slash-command behaviour can be unit-tested
//! without a TTY or a running server.

/// The outcome of interpreting one REPL input line.
#[derive(Debug, PartialEq, Eq)]
pub enum ReplAction {
    /// Quit the REPL (`/exit`, `/quit`).
    Quit,
    /// A handled command — print this notice to stderr and read the next line
    /// (nothing is sent to the model).
    Notice(String),
    /// Switch the active model to this name for subsequent messages; the
    /// caller updates its state and confirms.
    SetModel(String),
    /// Clear the terminal screen (`/clear`).
    Clear,
    /// Show the persistent CLI config (`/config`).
    ConfigShow,
    /// Set a persistent CLI config key (`/config set <key> <value>`); the
    /// caller validates, persists, and applies it.
    ConfigSet { key: String, value: String },
    /// Not a command — send this text to the model as a prompt.
    Send(String),
}

/// The lines shown by `/help`.
#[must_use]
pub fn help_text() -> String {
    "commands:\n\
     \x20 /help                     show this list\n\
     \x20 /model [name]             show or switch the model (e.g. /model MiniMax-M2.5)\n\
     \x20 /models                   how to see which models are configured\n\
     \x20 /config                   show persistent settings (prompt, language)\n\
     \x20 /config set <key> <val>   change a setting (e.g. /config set language zh)\n\
     \x20 /clear                    clear the screen\n\
     \x20 /exit, /quit              leave (Ctrl-D also works)"
        .to_string()
}

/// Interpret one input line. `current_model` is used to render `/model` with no
/// argument (empty → the server's default model).
///
/// A line that doesn't start with `/` is [`ReplAction::Send`] verbatim (after
/// trimming) — including the empty string, which the caller skips.
#[must_use]
pub fn parse_command(input: &str, current_model: &str) -> ReplAction {
    let line = input.trim();
    if !line.starts_with('/') {
        return ReplAction::Send(line.to_string());
    }
    let mut it = line.splitn(2, char::is_whitespace);
    let cmd = it.next().unwrap_or("");
    let arg = it.next().unwrap_or("").trim();
    match cmd {
        "/exit" | "/quit" => ReplAction::Quit,
        "/help" | "/?" => ReplAction::Notice(help_text()),
        "/clear" | "/cls" => ReplAction::Clear,
        "/config" => {
            // Accept `/config`, `/config set <key> <value>`, and the shorthand
            // `/config <key> <value>`. Bare `/config` or `/config set` → show.
            let rest = if arg == "set" {
                ""
            } else if let Some(r) = arg.strip_prefix("set ") {
                r.trim()
            } else {
                arg
            };
            if rest.is_empty() {
                return ReplAction::ConfigShow;
            }
            let mut kv = rest.splitn(2, char::is_whitespace);
            let key = kv.next().unwrap_or("").trim().to_string();
            let value = kv.next().unwrap_or("").trim().to_string();
            if key.is_empty() {
                ReplAction::ConfigShow
            } else {
                ReplAction::ConfigSet { key, value }
            }
        }
        "/models" => ReplAction::Notice(
            "configured models depend on your provider — list them with:  \
             xiaoguai provider list\n  then switch with:  /model <name>"
                .to_string(),
        ),
        "/model" => {
            if arg.is_empty() {
                let shown = if current_model.is_empty() {
                    "(server default)"
                } else {
                    current_model
                };
                ReplAction::Notice(format!(
                    "current model: {shown}\n  switch with: /model <name>  (e.g. /model MiniMax-M2.5)"
                ))
            } else {
                ReplAction::SetModel(arg.to_string())
            }
        }
        other => ReplAction::Notice(format!("unknown command: {other} — type /help")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_sent_verbatim() {
        assert_eq!(
            parse_command("hello there", ""),
            ReplAction::Send("hello there".into())
        );
        // leading/trailing whitespace trimmed; empty stays empty (caller skips)
        assert_eq!(parse_command("   ", ""), ReplAction::Send(String::new()));
    }

    #[test]
    fn exit_and_quit_quit() {
        assert_eq!(parse_command("/exit", ""), ReplAction::Quit);
        assert_eq!(parse_command("/quit", "MiniMax-M2"), ReplAction::Quit);
    }

    #[test]
    fn model_without_arg_shows_current_or_default() {
        match parse_command("/model", "MiniMax-M2.5") {
            ReplAction::Notice(s) => assert!(s.contains("MiniMax-M2.5"), "got {s}"),
            other => panic!("{other:?}"),
        }
        match parse_command("/model", "") {
            ReplAction::Notice(s) => assert!(s.contains("server default"), "got {s}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn model_with_arg_switches_and_trims() {
        assert_eq!(
            parse_command("/model MiniMax-M2.7", ""),
            ReplAction::SetModel("MiniMax-M2.7".into())
        );
        assert_eq!(
            parse_command("  /model   MiniMax-M3  ", ""),
            ReplAction::SetModel("MiniMax-M3".into())
        );
    }

    #[test]
    fn help_lists_the_core_commands() {
        let h = help_text();
        assert!(h.contains("/model"));
        assert!(h.contains("/exit"));
        assert!(h.contains("/help"));
    }

    #[test]
    fn config_show_and_set_parse() {
        assert_eq!(parse_command("/config", ""), ReplAction::ConfigShow);
        // both `set key value` and `key value` forms; value keeps its spaces
        assert_eq!(
            parse_command("/config set prompt My agent>", ""),
            ReplAction::ConfigSet {
                key: "prompt".into(),
                value: "My agent>".into()
            }
        );
        assert_eq!(
            parse_command("/config language zh", ""),
            ReplAction::ConfigSet {
                key: "language".into(),
                value: "zh".into()
            }
        );
        // `/config set` with nothing → show
        assert_eq!(parse_command("/config set", ""), ReplAction::ConfigShow);
    }

    #[test]
    fn clear_parses() {
        assert_eq!(parse_command("/clear", ""), ReplAction::Clear);
        assert_eq!(parse_command("/cls", ""), ReplAction::Clear);
    }

    #[test]
    fn unknown_slash_command_is_a_notice_not_sent() {
        match parse_command("/bogus", "") {
            ReplAction::Notice(s) => assert!(s.contains("unknown")),
            other => panic!("{other:?}"),
        }
    }
}

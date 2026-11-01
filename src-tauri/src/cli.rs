//! Command-line control of a running Bifaria: `bifaria --toggle`, `--translate`, `--cancel`.
//!
//! Wayland doesn't let apps register global hotkeys, so there the desktop's own shortcut settings
//! run these commands instead. A second `bifaria` process hands its arguments to the running one
//! (tauri-plugin-single-instance) and exits.

pub const USAGE: &str = "\
Usage: bifaria [COMMAND]

Commands for a running Bifaria (for example from a desktop keyboard shortcut):
  --toggle      Start or stop a recording, like the main hotkey
  --translate   Start or stop a recording that is translated, like the translate hotkey
  --cancel      Cancel the recording or transcription in progress
  --help        Show this help

Without a command, Bifaria starts, or opens Settings if it's already running.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliCommand {
    Toggle,
    Translate,
    Cancel,
    Help,
}

/// The first command in `args` (the program name excluded). Unknown arguments are ignored, so
/// ones added by the OS or a launcher don't break starting the app.
pub fn parse<I, S>(args: I) -> Option<CliCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter().find_map(|arg| match arg.as_ref() {
        "--toggle" | "--toggle-recording" => Some(CliCommand::Toggle),
        "--translate" => Some(CliCommand::Translate),
        "--cancel" => Some(CliCommand::Cancel),
        "--help" | "-h" => Some(CliCommand::Help),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_command_among_other_arguments() {
        assert_eq!(parse(["--toggle"]), Some(CliCommand::Toggle));
        assert_eq!(parse(["-psn_0_12345", "--translate"]), Some(CliCommand::Translate));
        assert_eq!(parse(["--cancel", "--toggle"]), Some(CliCommand::Cancel));
        assert_eq!(parse(["-h"]), Some(CliCommand::Help));
        assert_eq!(parse(Vec::<String>::new()), None);
        assert_eq!(parse(["--unknown"]), None);
    }
}

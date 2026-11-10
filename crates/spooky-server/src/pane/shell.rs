use portable_pty::CommandBuilder;
use std::env;

/// Locale supplied to shells when the server's environment has none.
const DEFAULT_MACOS_LOCALE: &str = "en_US.UTF-8";

fn env_is_unset(name: &str) -> bool {
    env::var_os(name).is_none_or(|value| value.is_empty())
}

/// Builds the shell command for a new pane.
///
/// macOS runs a login shell so `~/.zprofile` (PATH, Homebrew) applies even
/// when the GUI was launched from Finder, whose environment has neither it
/// nor a locale; a UTF-8 locale is supplied only when `LC_ALL` and `LANG` are
/// both unset. Other platforms keep the plain `$SHELL` invocation.
pub(crate) fn shell_command() -> CommandBuilder {
    if cfg!(target_os = "macos") {
        let mut cmd = CommandBuilder::new_default_prog();
        if env_is_unset("LC_ALL") && env_is_unset("LANG") {
            cmd.env("LANG", DEFAULT_MACOS_LOCALE);
        }
        cmd
    } else {
        let shell = env::var_os("SHELL")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        CommandBuilder::new(shell)
    }
}

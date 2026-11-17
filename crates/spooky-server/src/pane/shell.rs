use portable_pty::CommandBuilder;
use std::{env, ffi::OsString};

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
/// both unset. Other platforms run `$SHELL`, or the user's login shell when it is
/// unset (a server started from a session without it, such as a service or a
/// launcher, would otherwise give every pane a bare `/bin/sh`).
pub(crate) fn shell_command() -> CommandBuilder {
    if cfg!(target_os = "macos") {
        let mut cmd = CommandBuilder::new_default_prog();
        if env_is_unset("LC_ALL") && env_is_unset("LANG") {
            cmd.env("LANG", DEFAULT_MACOS_LOCALE);
        }
        cmd
    } else {
        command_for(env::var_os("SHELL"))
    }
}

/// The command for the shell `$SHELL` names, or the login shell when there is none.
fn command_for(shell: Option<OsString>) -> CommandBuilder {
    match shell.filter(|shell| !shell.is_empty()) {
        Some(shell) => CommandBuilder::new(shell),
        None => CommandBuilder::new_default_prog(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_shell_variable_means_the_login_shell_not_a_bare_sh() {
        assert!(command_for(None).is_default_prog());
        assert!(command_for(Some(OsString::new())).is_default_prog());
        assert!(!command_for(Some("/bin/zsh".into())).is_default_prog());
    }
}

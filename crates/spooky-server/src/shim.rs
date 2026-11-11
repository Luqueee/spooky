//! The `omp` shim: lets typing a bare `omp` in a Spooky pane open the agent view.
//!
//! The server writes, into its own private state directory, an `omp` script and a zsh
//! startup file, and gives the shells it starts a `SPOOKY_PANE` variable and this
//! directory at the front of `PATH`. Nothing outside the state directory is touched and
//! nothing is uploaded: a remote machine's own server writes its own shim.
//!
//! Profiles often put their own directories ahead of the inherited `PATH`, so for zsh, bash
//! and fish the directory is put first again just before the first prompt.

use anyhow::{Context, Result};
use portable_pty::CommandBuilder;
use std::{
    env,
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

/// The shim for the shells of one server.
pub(crate) struct Shim {
    dir: PathBuf,
}

impl Shim {
    /// Writes the shim files under `state_dir` and returns the shim.
    pub(crate) fn install(state_dir: &Path) -> Result<Self> {
        let dir = state_dir.join("shim");
        fs::create_dir_all(dir.join("zsh")).context("creating the shim directory")?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
            .context("securing the shim directory")?;
        let server = env::current_exe().context("locating spooky-server")?;
        write_file(&dir.join("omp"), &omp_script(&server), 0o755)?;
        write_file(&dir.join("zsh/.zshenv"), ZSHENV, 0o644)?;
        fs::create_dir_all(dir.join("fish/fish/vendor_conf.d"))
            .context("creating the fish directory")?;
        write_file(
            &dir.join("fish/fish/vendor_conf.d/spooky.fish"),
            FISH,
            0o644,
        )?;
        Ok(Self { dir })
    }

    /// Gives a shell about to start the pane identity and the shim, ahead of the
    /// programs it would otherwise find.
    pub(crate) fn configure(&self, command: &mut CommandBuilder, pane: u64) {
        command.env("SPOOKY_PANE", pane.to_string());
        command.env("SPOOKY_SHIM_DIR", &self.dir);
        let mut path = OsString::from(&self.dir);
        if let Some(inherited) = env::var_os("PATH") {
            path.push(":");
            path.push(inherited);
        }
        command.env("PATH", path);
        match shell_name().as_deref() {
            Some("zsh") => {
                if let Some(original) = env::var_os("ZDOTDIR") {
                    command.env("SPOOKY_ORIG_ZDOTDIR", original);
                }
                command.env("ZDOTDIR", self.dir.join("zsh"));
            }
            Some("fish") => {
                // fish reads `fish/vendor_conf.d` of every XDG data directory.
                let mut data = OsString::from(self.dir.join("fish"));
                data.push(":");
                data.push(
                    env::var_os("XDG_DATA_DIRS")
                        .filter(|d| !d.is_empty())
                        .unwrap_or_else(|| OsString::from("/usr/local/share:/usr/share")),
                );
                command.env("XDG_DATA_DIRS", data);
            }
            Some("bash") => {
                let mut prompt = OsString::from(BASH_PROMPT);
                if let Some(existing) = env::var_os("PROMPT_COMMAND") {
                    prompt.push(";");
                    prompt.push(existing);
                }
                command.env("PROMPT_COMMAND", prompt);
            }
            _ => {}
        }
    }
}

/// The name of the user's shell, such as `zsh`.
fn shell_name() -> Option<String> {
    let shell = env::var_os("SHELL")?;
    Path::new(&shell)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// Puts the shim first on `PATH` again before a bash prompt, if a profile moved it.
const BASH_PROMPT: &str =
    "[ \"${PATH%%:*}\" = \"$SPOOKY_SHIM_DIR\" ] || PATH=\"$SPOOKY_SHIM_DIR:$PATH\"";

/// zsh reads this instead of the user's `.zshenv`: it restores `ZDOTDIR`, runs the
/// user's file, and moves the shim to the front of `PATH` before the first prompt,
/// after every startup file has run.
const ZSHENV: &str = r#"# Written by spooky-server.
if [[ -n "${SPOOKY_ORIG_ZDOTDIR+x}" ]]; then
  ZDOTDIR="$SPOOKY_ORIG_ZDOTDIR"
else
  unset ZDOTDIR
fi
unset SPOOKY_ORIG_ZDOTDIR
[[ -r "${ZDOTDIR:-$HOME}/.zshenv" ]] && source "${ZDOTDIR:-$HOME}/.zshenv"
autoload -Uz add-zsh-hook
_spooky_first_prompt() {
  path=("$SPOOKY_SHIM_DIR" ${path:#"$SPOOKY_SHIM_DIR"})
  add-zsh-hook -d precmd _spooky_first_prompt
  unfunction _spooky_first_prompt
}
add-zsh-hook precmd _spooky_first_prompt
"#;

/// fish runs this before the user's `config.fish`, so it waits for the first prompt to
/// move the shim to the front of `PATH`.
const FISH: &str = r#"# Written by spooky-server.
function _spooky_first_prompt --on-event fish_prompt
    set -l rest
    for entry in $PATH
        test "$entry" != "$SPOOKY_SHIM_DIR"; and set -a rest $entry
    end
    set -gx PATH $SPOOKY_SHIM_DIR $rest
    functions -e _spooky_first_prompt
end
"#;

/// The script that is `omp` inside a Spooky pane. At a terminal it asks the server to open
/// an agent tab with the arguments given; the server accepts a closed set of them. Anything
/// it refuses, a non-terminal, or any failure runs the real `omp` unchanged.
fn omp_script(server: &Path) -> String {
    let server = server.to_string_lossy().replace('\'', r"'\''");
    format!(
        r#"#!/bin/sh
# Written by spooky-server. Opens oh-my-pi in Spooky's agent view when `omp` is typed in a
# Spooky pane with arguments the agent view supports; otherwise runs the real omp.
if [ -n "${{SPOOKY_PANE:-}}" ] && [ -t 0 ] && [ -t 1 ]; then
  if '{server}' agent-open omp -- "$@" 2>/dev/null; then
    exit 0
  fi
fi
IFS=:
for dir in $PATH; do
  [ "$dir" = "${{SPOOKY_SHIM_DIR:-}}" ] && continue
  if [ -x "$dir/omp" ] && [ ! -d "$dir/omp" ]; then
    exec "$dir/omp" "$@"
  fi
done
echo "omp: command not found" >&2
exit 127
"#
    )
}

/// Replaces `path` with `contents` without following a link left at `path`.
fn write_file(path: &Path, contents: &str, mode: u32) -> Result<()> {
    let temporary = path.with_extension("new");
    fs::write(&temporary, contents).with_context(|| format!("writing {}", temporary.display()))?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(mode))
        .with_context(|| format!("securing {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("installing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_quotes_the_server_path_and_skips_its_own_directory() {
        let script = omp_script(Path::new(
            "/Users/o'brien/Application Support/spooky-server",
        ));
        assert!(script.contains(
            r#"'/Users/o'\''brien/Application Support/spooky-server' agent-open omp -- "$@""#
        ));
        assert!(script.contains(r#"[ "$dir" = "${SPOOKY_SHIM_DIR:-}" ] && continue"#));
    }

    #[test]
    fn install_writes_private_executable_files_and_replaces_old_ones() {
        let base = std::env::temp_dir().join(format!("spooky-shim-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).expect("base");
        Shim::install(&base).expect("first install");
        Shim::install(&base).expect("second install replaces the files");
        let omp = base.join("shim/omp");
        assert_eq!(
            fs::metadata(&omp).expect("omp").permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::metadata(base.join("shim"))
                .expect("dir")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert!(
            fs::read_to_string(base.join("shim/zsh/.zshenv"))
                .expect("zshenv")
                .contains("SPOOKY_ORIG_ZDOTDIR")
        );
        let _ = fs::remove_dir_all(&base);
    }
}

//! Finding an agent's program when the server was started with a minimal `PATH`.
//!
//! A server started over SSH (`ssh host spooky-server bridge`) gets the non-interactive
//! `PATH`, which usually lacks the directories user-installed tools go to (`~/.local/bin`,
//! `~/.bun/bin`...), although the shells in its panes find them. The program is looked
//! for in the `PATH` first and then in those well-known directories, and the agent gets
//! them at the end of its own `PATH` so the tools it starts are found too.

use std::{
    env,
    ffi::{OsStr, OsString},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

/// Directories under the home directory where user-installed programs usually are.
const HOME_DIRS: &[&str] = &[
    ".local/bin",
    ".bun/bin",
    ".cargo/bin",
    ".local/share/mise/shims",
];
/// System directories that are often missing from a non-interactive `PATH`.
const SYSTEM_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];

/// The directories searched after the `PATH`.
fn extra_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = env::home_dir()
        .map(|home| HOME_DIRS.iter().map(|dir| home.join(dir)).collect())
        .unwrap_or_default();
    dirs.extend(SYSTEM_DIRS.iter().map(PathBuf::from));
    dirs
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// The executable `program` names: as given when it has a directory part, else the first
/// match in `path` and then in `extra`.
fn find(program: &OsStr, path: &[PathBuf], extra: &[PathBuf]) -> Option<PathBuf> {
    if Path::new(program).components().count() > 1 {
        return Some(PathBuf::from(program));
    }
    path.iter()
        .chain(extra)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

/// The program to run for `program`: where it was found, or the name itself so that a
/// missing program fails to start with the usual error.
pub(super) fn locate(program: &OsStr) -> OsString {
    let path: Vec<PathBuf> = env::var_os("PATH")
        .map(|path| env::split_paths(&path).collect())
        .unwrap_or_default();
    find(program, &path, &extra_dirs()).map_or_else(|| program.to_owned(), PathBuf::into_os_string)
}

/// Appends the well-known directories that exist, and are not there yet, to the
/// `PATH` of `command`.
pub(super) fn extend(command: &mut Command) {
    let mut dirs: Vec<PathBuf> = env::var_os("PATH")
        .map(|path| env::split_paths(&path).collect())
        .unwrap_or_default();
    for dir in extra_dirs() {
        if dir.is_dir() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    if let Ok(joined) = env::join_paths(dirs) {
        command.env("PATH", joined);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process};

    /// A directory that is removed when it goes out of scope.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = env::temp_dir().join(format!("spooky-path-{}-{name}", process::id()));
            fs::create_dir_all(&dir).expect("creating a scratch directory");
            Self(dir)
        }

        fn program(&self, name: &str, mode: u32) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, "#!/bin/sh\n").expect("writing the fixture");
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("setting its mode");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_program_missing_from_the_path_is_found_in_the_extra_directories() {
        let minimal = Scratch::new("minimal");
        let user = Scratch::new("user");
        let omp = user.program("omp", 0o755);
        let found = find(
            OsStr::new("omp"),
            std::slice::from_ref(&minimal.0),
            std::slice::from_ref(&user.0),
        );
        assert_eq!(found, Some(omp));
    }

    #[test]
    fn the_path_wins_over_the_extra_directories() {
        let first = Scratch::new("first");
        let second = Scratch::new("second");
        let wanted = first.program("omp", 0o755);
        second.program("omp", 0o755);
        let found = find(
            OsStr::new("omp"),
            std::slice::from_ref(&first.0),
            std::slice::from_ref(&second.0),
        );
        assert_eq!(found, Some(wanted));
    }

    #[test]
    fn a_file_that_cannot_run_is_not_a_program() {
        let dir = Scratch::new("plain");
        dir.program("omp", 0o644);
        assert_eq!(
            find(OsStr::new("omp"), &[], std::slice::from_ref(&dir.0)),
            None
        );
    }

    #[test]
    fn a_name_with_a_directory_is_used_as_it_is() {
        assert_eq!(
            find(OsStr::new("/opt/x/omp"), &[], &[]),
            Some(PathBuf::from("/opt/x/omp"))
        );
    }
}

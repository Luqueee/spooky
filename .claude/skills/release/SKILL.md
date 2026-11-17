---
name: release
description: Cut a Spooky release, production (`v0.2.0`) or development (`v0.2.0-dev.1`). Use when the user asks to release, publish, tag, ship a version, make a dev/nightly build, or check what a release would contain. Never tags or publishes without the user's explicit go.
---

# Releasing Spooky

A release is a git tag `v<version>`. Pushing it runs `.github/workflows/release.yml`, which builds every asset and creates the GitHub release. The app's **Settings → Updates** and **Install on a remote machine** download from those releases, so a bad release reaches users: be exact, and stop at every confirmation below.

## Two kinds

| | production | development |
| --- | --- | --- |
| tag / `Cargo.toml` version | `v0.2.0` / `0.2.0` | `v0.2.0-dev.1` / `0.2.0-dev.1` |
| GitHub | the release marked **latest** | a **pre-release** |
| commit | must be on `main` (the workflow refuses otherwise) | any branch |
| followed by | the Stable channel and the Development channel | the Development channel only |

Semver orders `0.2.0-dev.9 < 0.2.0-dev.10 < 0.2.0`. A development release of the next version is `X.Y.Z-dev.N` with `N` one past the highest existing `vX.Y.Z-dev.*` tag. A production version must be higher than every published one; features raise the minor, fixes the patch (the major stays 0 until the owner says otherwise).

The tag, the workspace `version` in `Cargo.toml` and the commit must agree: the workflow fails when the tag differs from the version.

## Rules

- Never create or push a tag, create a release, or edit/delete one, without the user's explicit go **for that exact tag and commit**. Releasing is public and not takeable back: a tag must never be moved or reused; fix forward with a new version.
- Everything else (checks, packaging locally, listing releases) is read-only or local: do it.
- Per `AGENTS.md`: no private paths, credentials or reference-project material in anything published; inspect what goes out.
- State what was and was not verified. A macOS run says nothing about Linux; the release workflow is the first place the Linux app archive is built from a tag.

## Procedure

1. **Decide kind and version.** Ask the user only if unclear. Look at what exists:
   ```sh
   git fetch --tags origin
   git tag --list 'v*' --sort=-v:refname | head
   gh release list --limit 10
   grep -n '^version' Cargo.toml
   ```
2. **Protocol.** Compare `PROTOCOL_VERSION` (`crates/spooky-core/src/lib.rs`) with the last release (`git show <last-tag>:crates/spooky-core/src/lib.rs | grep PROTOCOL_VERSION`). If it changed, the release notes must say that updating restarts the local server and ends its shells, remote servers need the new version installed, and `README.md`'s protocol sentence must name the new number.
3. **Preconditions.** Clean worktree on the branch to release, pushed, and CI green on that exact commit:
   ```sh
   git status -sb
   gh run list --commit "$(git rev-parse HEAD)" --json name,status,conclusion
   ```
   A production release comes from an up-to-date `main` (`git merge-base --is-ancestor HEAD origin/main`).
4. **Version commit.** Set `[workspace.package] version` in `Cargo.toml`, refresh `Cargo.lock` (`cargo check --workspace`), and add a `CHANGELOG`/README note only if the repo keeps one. Commit it with the repo's commit flow; on `main` that goes through the PR flow the owner uses. Do not push yet.
5. **Gates** (the commands in `README.md` → Packages and checks): `cargo fmt --all -- --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace --all-targets`, `cargo test --locked --workspace --doc`, `RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --no-deps`, `cargo +1.98.0 check --locked --workspace --all-targets`.
6. **Package smoke test** on this machine: `./scripts/package.sh`, then
   ```sh
   plutil -p dist/Spooky.app/Contents/Info.plist | grep -i version      # short version = numbers, build = full version
   dist/Spooky.app/Contents/MacOS/spooky-server version                  # spooky-server <version> protocol <n>
   codesign --verify --strict dist/Spooky.app
   ```
7. **Rehearse the update** (optional but cheap, and the only way to see the updater run before users do): serve the packaged zip from a local mirror and point a copy of the previous build at it, see *Local mirror* below.
8. **Ask for the go.** Show the user: tag, commit sha and subject, kind, whether the protocol changed, the gate results, and which platforms were not tested. Wait for an explicit yes.
9. **Tag and push** (only after the yes):
   ```sh
   git tag -a vX.Y.Z -m "Spooky X.Y.Z"
   git push origin vX.Y.Z
   ```
10. **Watch the workflow** with `gh run watch` and read failures from `gh run view --log-failed`. A failed run publishes nothing; the fix is a new commit and a new version, not a moved tag (deleting the unpublished tag first needs the user's yes).
11. **Verify the release** once it exists:
    ```sh
    gh release view vX.Y.Z --json isPrerelease,assets --jq '{pre: .isPrerelease, assets: [.assets[].name]}'
    ```
    Expect: `spooky-server-<v>-{x86_64,aarch64}-unknown-linux-musl`, `…-{aarch64,x86_64}-apple-darwin` tarballs; `spooky-app-<v>-aarch64-apple-darwin.zip`; `spooky-app-<v>-x86_64-unknown-linux-gnu.tar.gz`; `spooky_<v>_amd64.deb`; and a `.sha256` for each. `isPrerelease` is true exactly for a development tag. Download one asset to a temp directory, check `shasum -a 256 -c`, and run its `spooky-server version`.
12. **Report** the release URL, what was verified, and what was not (for instance, a Linux install by tarball or a real remote install).

## Local mirror (rehearsal without publishing)

`SPOOKY_RELEASE_URL` (where files live) and `SPOOKY_RELEASE_INDEX` (the list, GitHub's release-list JSON) take `file://` locations, so the app can update from a directory:

```
mirror/index.json                                       [{"tag_name":"v0.2.0-dev.1","draft":false,"prerelease":true}]
mirror/download/v0.2.0-dev.1/spooky-app-0.2.0-dev.1-aarch64-apple-darwin.zip
mirror/download/v0.2.0-dev.1/spooky-app-0.2.0-dev.1-aarch64-apple-darwin.zip.sha256   "<sha256>  <file name>"
```

Build the zip with `ditto -c -k --keepParent dist/Spooky.app <zip>` from a build whose `Cargo.toml` version is the one in the file names. Install the previous build somewhere you own (not `/Applications` when testing), isolate it with its own `SPOOKY_SOCKET` and `XDG_CONFIG_HOME` (set `update_channel = "dev"` and a different `shortcut` in its `config.toml` so it does not take the real shortcut), and give those variables, plus the two mirror variables, to the app: `open` does not pass your shell's environment, so put them in the test bundle's `Info.plist` as `LSEnvironment` and re-sign it, or the relaunch after an update starts without them. Never point a rehearsal at the owner's real socket or config.

## Withdrawing a bad release

Stable users only see releases whose GitHub `prerelease` flag is false, so `gh release edit vX.Y.Z --prerelease` takes a bad production release out of the Stable channel without deleting anything. Do it, or delete a release and its tag (`gh release delete vX.Y.Z --cleanup-tag`), only when the user says so, then publish the fix as the next version.

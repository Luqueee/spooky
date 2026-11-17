//! Who published a downloaded release: minisign signatures checked against pinned keys.
//!
//! The SHA-256 published beside an archive detects a damaged download; it is served from
//! the same place as the archive, so whoever can replace the one can replace the other.
//! A signature cannot be forged without the private key, which never leaves the release
//! workflow's secrets. Every release file has a `.minisig` beside it; the file is only
//! used when one of the keys in `release-keys.pub` (compiled in) signed exactly these
//! bytes, under this name.

use anyhow::{Context, Result, bail};
use minisign_verify::{Error as VerifyError, PublicKey, Signature};
use std::{fs::File, io::Read, path::Path};

/// The keys that are trusted when nothing replaces them.
const COMPILED_IN: &str = include_str!("../../release-keys.pub");
/// Replaces the compiled-in keys (one base64 key per line): for a mirror that signs with
/// its own key, or a development release directory.
const KEY_ENV: &str = "SPOOKY_RELEASE_KEY";
/// A signature file is a few hundred bytes; this bounds the download.
pub(crate) const MAX_SIGNATURE_BYTES: u64 = 4096;

/// The minisign public keys a release may be signed with.
#[derive(Clone, Debug)]
pub(crate) struct TrustedKeys(Vec<String>);

impl TrustedKeys {
    /// The compiled-in keys, or those in `SPOOKY_RELEASE_KEY` when it is set.
    pub(crate) fn release() -> Self {
        let replacement = std::env::var(KEY_ENV).ok().filter(|keys| !keys.is_empty());
        Self::parse(replacement.as_deref().unwrap_or(COMPILED_IN))
    }

    /// Keys written one per line; empty lines and lines starting with `#` are skipped.
    fn parse(text: &str) -> Self {
        Self(
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_owned)
                .collect(),
        )
    }
}

/// Checks that `archive` is exactly what a trusted key signed under the name `name`.
///
/// `signature` is the text of the `.minisig` file. Fails when no trusted key made the
/// signature, when the bytes differ, and when the signature names another file, so a
/// genuine signature on one release file cannot vouch for a different one.
pub(crate) fn verify(
    keys: &TrustedKeys,
    archive: &Path,
    signature: &str,
    name: &str,
) -> Result<()> {
    let signature = Signature::decode(signature)
        .with_context(|| format!("the signature of {name} is not a minisign signature"))?;
    let signed_name = signed_name(signature.trusted_comment());
    if signed_name != Some(name) {
        bail!(
            "the signature published for {name} is for {}; it was not used",
            signed_name.unwrap_or("another file")
        );
    }
    if keys.0.is_empty() {
        bail!("Spooky trusts no release key, so {name} cannot be checked");
    }
    for line in &keys.0 {
        let key = PublicKey::from_base64(line)
            .with_context(|| format!("a trusted release key is not valid: {line}"))?;
        let mut verifier = match key.verify_stream(&signature) {
            Ok(verifier) => verifier,
            // Another key made it; a later one may match.
            Err(VerifyError::UnexpectedKeyId) => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("checking the signature of {name}"));
            }
        };
        let mut file = File::open(archive).context("opening the download")?;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).context("reading the download")?;
            if read == 0 {
                break;
            }
            verifier.update(&buffer[..read]);
        }
        return verifier.finalize().map_err(|_| {
            anyhow::anyhow!(
                "the download of {name} is not what the release key signed; it was not used"
            )
        });
    }
    bail!("{name} is not signed by a release key Spooky trusts; it was not used")
}

/// The file name a signature's trusted comment (`timestamp:<n>\tfile:<name>`) names.
fn signed_name(trusted_comment: &str) -> Option<&str> {
    trusted_comment
        .split('\t')
        .find_map(|field| field.strip_prefix("file:"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::client::fetch::Workspace;
    use std::io::Cursor;

    /// A key pair for tests: the keys to trust, and a way to sign.
    pub(crate) struct TestKey {
        pair: minisign::KeyPair,
    }

    impl TestKey {
        pub(crate) fn new() -> Self {
            Self {
                pair: minisign::KeyPair::generate_unencrypted_keypair().expect("a key pair"),
            }
        }

        pub(crate) fn trusted(&self) -> TrustedKeys {
            TrustedKeys(vec![self.pair.pk.to_base64()])
        }

        /// The text of the `.minisig` file for `bytes` published as `name`, the way the
        /// release workflow's `minisign -S` writes it.
        pub(crate) fn sign(&self, bytes: &[u8], name: &str) -> String {
            let comment = format!("timestamp:1\tfile:{name}");
            minisign::sign(
                Some(&self.pair.pk),
                &self.pair.sk,
                Cursor::new(bytes),
                Some(&comment),
                None,
            )
            .expect("signed")
            .into_string()
        }
    }

    fn archive(bytes: &[u8]) -> (Workspace, std::path::PathBuf) {
        let dir = Workspace::create().expect("scratch directory");
        let path = dir.path().join("release.tar.gz");
        std::fs::write(&path, bytes).expect("archive");
        (dir, path)
    }

    #[test]
    fn the_compiled_in_keys_are_valid() {
        let keys = TrustedKeys::parse(COMPILED_IN);
        assert!(
            !keys.0.is_empty(),
            "a release can only be checked with a key"
        );
        for line in &keys.0 {
            PublicKey::from_base64(line).expect("a valid minisign key");
        }
    }

    #[test]
    fn keys_are_read_one_per_line_without_comments() {
        let keys = TrustedKeys::parse("# a note\n\n  RWQabc  \nRWQdef\n");
        assert_eq!(keys.0, ["RWQabc", "RWQdef"]);
    }

    #[test]
    fn a_release_signed_by_a_trusted_key_passes() {
        let key = TestKey::new();
        let (_dir, path) = archive(b"the release");
        let signature = key.sign(b"the release", "release.tar.gz");
        verify(&key.trusted(), &path, &signature, "release.tar.gz").expect("verified");
    }

    #[test]
    fn changed_bytes_do_not_pass() {
        let key = TestKey::new();
        let (_dir, path) = archive(b"the release, tampered with");
        let signature = key.sign(b"the release", "release.tar.gz");
        let error = verify(&key.trusted(), &path, &signature, "release.tar.gz")
            .expect_err("tampered archive");
        assert!(
            error
                .to_string()
                .contains("not what the release key signed")
        );
    }

    #[test]
    fn a_signature_from_an_untrusted_key_does_not_pass() {
        let (_dir, path) = archive(b"the release");
        let forger = TestKey::new();
        let signature = forger.sign(b"the release", "release.tar.gz");
        let error = verify(
            &TestKey::new().trusted(),
            &path,
            &signature,
            "release.tar.gz",
        )
        .expect_err("forged signature");
        assert!(error.to_string().contains("not signed by a release key"));
    }

    #[test]
    fn any_one_of_several_trusted_keys_may_have_signed() {
        let (old, new) = (TestKey::new(), TestKey::new());
        let (_dir, path) = archive(b"the release");
        let signature = new.sign(b"the release", "release.tar.gz");
        let keys = TrustedKeys(vec![old.pair.pk.to_base64(), new.pair.pk.to_base64()]);
        verify(&keys, &path, &signature, "release.tar.gz").expect("the second key matches");
    }

    #[test]
    fn a_signature_the_minisign_program_wrote_with_the_release_key_passes() {
        // Made by `minisign -S` (what the release workflow runs) with the real release
        // key over these bytes: the compiled-in key is the one that signs, and the
        // program's own trusted comment (which adds a `hashed` field) is understood.
        const NAME: &str = "spooky-server-9.9.9-x86_64-unknown-linux-musl.tar.gz";
        const SIGNATURE: &str = "untrusted comment: signature from minisign secret key\n\
            RUQouq84PJMh9qnD6h+xdYWlyIkAXEAMwElzkYwmB4lpsjfz7DLPDbkDuVP68PyQVvCQJH9h07YVqF7l5v2bdwp2cEU0Ntqm5g4=\n\
            trusted comment: timestamp:1794928364\tfile:spooky-server-9.9.9-x86_64-unknown-linux-musl.tar.gz\thashed\n\
            cPWBYj6vjVwuXKm6C36eBTG10Ab00vZdsgl7gqbZsZtl5nifPv0QINQsarugM6PMcNKXCUUJR699qZ3YFzJ1DA==\n";
        let keys = TrustedKeys::parse(COMPILED_IN);
        let (_dir, path) = archive(b"archive bytes for the cli test\n");
        verify(&keys, &path, SIGNATURE, NAME).expect("verified");
        let (_other, tampered) = archive(b"archive bytes for the cli test, changed\n");
        assert!(verify(&keys, &tampered, SIGNATURE, NAME).is_err());
    }

    #[test]
    fn a_genuine_signature_of_another_file_does_not_vouch_for_this_one() {
        let key = TestKey::new();
        let (_dir, path) = archive(b"the release");
        let signature = key.sign(b"the release", "spooky-server-linux.tar.gz");
        let error = verify(&key.trusted(), &path, &signature, "Spooky-macos.zip")
            .expect_err("signed under another name");
        assert!(
            error
                .to_string()
                .contains("is for spooky-server-linux.tar.gz")
        );
    }

    #[test]
    fn text_that_is_not_a_signature_does_not_pass() {
        let key = TestKey::new();
        let (_dir, path) = archive(b"the release");
        let error = verify(
            &key.trusted(),
            &path,
            "<html>Not Found</html>",
            "release.tar.gz",
        )
        .expect_err("not a signature");
        assert!(error.to_string().contains("not a minisign signature"));
    }

    #[test]
    fn no_trusted_key_means_nothing_passes() {
        let key = TestKey::new();
        let (_dir, path) = archive(b"the release");
        let signature = key.sign(b"the release", "release.tar.gz");
        let error = verify(
            &TrustedKeys(Vec::new()),
            &path,
            &signature,
            "release.tar.gz",
        )
        .expect_err("no keys");
        assert!(error.to_string().contains("trusts no release key"));
    }
}

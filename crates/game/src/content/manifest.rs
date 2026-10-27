//! Pack identity ([`ContentManifest`]) and the optional publisher attestation.
//!
//! The bytes a publisher signs come from [`GameContent::signing_message`] (in `hash.rs`),
//! next to the canonical catalogue feed they digest.
//!
//! [`GameContent::signing_message`]: super::GameContent::signing_message

/// Maximum length of a manifest field.
pub const MAX_MANIFEST_FIELD: usize = 32;

/// Human-readable identity of a content pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentManifest {
    /// Pack name.
    pub name: String,
    /// Pack version string.
    pub version: String,
    /// Author/maintainer.
    pub author: String,
}

/// Revision of the built-in content, independent of the engine version.
///
/// The manifest version is part of the content hash, which replays record. Tying it to the
/// crate version made every engine release invalidate replays of unchanged built-in
/// content. Bump this only when the built-in catalogue itself changes.
pub const BUILTIN_CONTENT_VERSION: &str = "0.3.0";

impl Default for ContentManifest {
    fn default() -> Self {
        Self {
            name: String::from("Core"),
            version: String::from(BUILTIN_CONTENT_VERSION),
            author: String::from("MOBA Engine"),
        }
    }
}

/// Optional Ed25519 attestation of a pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackAttestation {
    /// Publisher Ed25519 public key.
    pub publisher: [u8; 32],
    /// Signature over [`GameContent::signing_message`](super::GameContent::signing_message).
    pub signature: [u8; 64],
}

//! Asset provenance and the licence gate (A0, ADR-0022).
//!
//! Every file in a content root's asset directories (any directory below the root) is
//! covered by exactly one entry of the [`PROVENANCE_FILE`] manifest in its own directory. The
//! root itself may hold only pack files (`*.toml`, gameplay data) and the generated
//! [`CREDITS_FILE`] and [`AI_DISCLOSURE_FILE`]; dot-files, dot-directories, symlinks and
//! special files are errors anywhere (and never read), and directory names are limited to
//! ASCII letters, digits, `.`, `_` and `-`, so nothing can hide from the gate or break the
//! notices. A `*` pattern may cover only `*.toml` data files; binaries are listed by name:
//!
//! ```toml
//! [[asset]]
//! files = ["sentinel.glb", "sentinel.lod"]   # names or `*` patterns in this directory
//! title = "Sentinel sample hero"
//! source = "original"                        # or the https:// page it came from
//! author = "MOBA Engine contributors"
//! licence = "MIT OR Apache-2.0"              # an SPDX id from the allow-list
//! made_with = "moba-tools sample-mesh"       # optional: how it was produced
//! # attribution = "..."                      # required for CC-BY
//! # notes = "..."
//! # sha256 = { "sentinel.glb" = "<64 hex>" } # checked when present; required per file
//! #                                          # unless source = "original"
//!
//! # [asset.ai]                               # only for AI-generated assets
//! # model = "TRELLIS.2-4B"
//! # model_version = "2025-12"
//! # weights_licence = "MIT"
//! # seed = 1234
//! # human_modified = false                 # true (with notes) for a CC-BY licence
//! # prompt = "..."
//! # tools = ["ComfyUI 0.3.40"]
//! # [[asset.ai.inputs]]
//! # name = "concept.png"
//! # source = "original"
//! # licence = "CC0-1.0"
//! ```
//!
//! [`check_provenance`] walks the root and applies the policy: licences on
//! [`ALLOWED_LICENCES`] or [`LICENCE_EXCEPTIONS`] (non-commercial, no-derivatives,
//! share-alike, unknown and all-rights-reserved are rejected), attribution text for CC-BY,
//! and for AI-generated assets the model, version, weights licence, seed, prompt or inputs,
//! and tools, with the roadmap's rules (the asset is CC0, or CC-BY once a person modified
//! it, and third-party inputs are CC0). Exceptions apply only to the use and directories
//! they were approved for. A file taken from elsewhere (`source` not `original`) needs its
//! `sha256`, which pins the reviewed bytes (ADR-0022 amendment 1). Manifest text may not
//! contain control characters, line or paragraph separators, bidirectional overrides or HTML
//! comment markers, so it cannot break or disguise the generated notices. [`third_party_section`], [`credits_text`] and [`ai_disclosure`] render the
//! notices `content-check --provenance` writes; they are deterministic, so `--check` can
//! compare them with the committed files.
//!
//! Provenance is cosmetic metadata: it is never hashed into a pack, signed or sent, and the
//! client never reads it. What it declares is the author's statement: the gate cannot tell
//! that an asset claimed as `original` was really made here, or that one without an `[ai]`
//! table was made without AI. It covers the content root only (`content/`), not engine-built
//! assets elsewhere in the repository.

use std::collections::BTreeMap;

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::Path;

use serde::Deserialize;
use sha2::{Digest as _, Sha256};

/// The manifest's file name in each asset directory. It is never an asset itself, and
/// `check_vfx_dir` skips `vfx/provenance.toml`, so do not name an effect, map visuals or
/// other by-name asset `provenance` (no name validator rejects it).
pub const PROVENANCE_FILE: &str = "provenance.toml";

/// The generated credits file at the content root.
pub const CREDITS_FILE: &str = "CREDITS.txt";

/// The generated Steam AI-disclosure list at the content root.
pub const AI_DISCLOSURE_FILE: &str = "AI-DISCLOSURE.md";

/// Largest manifest accepted, in bytes.
pub const MAX_PROVENANCE_BYTES: u64 = 64 * 1024;

/// Licences an asset may carry without an exception: public-domain dedication, attribution
/// licences, the permissive code licences (the repository's own, `MIT OR Apache-2.0`, among
/// them) and the SIL Open Font License for fonts.
pub const ALLOWED_LICENCES: &[&str] = &[
    "CC0-1.0",
    "CC-BY-3.0",
    "CC-BY-4.0",
    "MIT",
    "Apache-2.0",
    "MIT OR Apache-2.0",
    "OFL-1.1",
];

/// Where a licence id appears in a manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LicenceUse {
    /// The asset's own `licence`.
    Asset,
    /// An AI asset's `ai.weights_licence`.
    Weights,
    /// An AI input's `licence`.
    Input,
}

impl LicenceUse {
    fn field(self) -> &'static str {
        match self {
            Self::Asset => "licence",
            Self::Weights => "ai.weights_licence",
            Self::Input => "an ai input licence",
        }
    }
}

/// A licence the maintainer approved for specific sources although it is not on
/// [`ALLOWED_LICENCES`] (roadmap §9.2, §9.4: approving exceptions is a human step, so adding
/// one is a reviewed code change). It is accepted only for [`Self::uses`] and, when
/// [`Self::dirs`] is not empty, only in those asset directories (and below them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LicenceException {
    /// The id manifests use (`LicenseRef-…` for licences SPDX does not list).
    pub id: &'static str,
    /// Human-readable name.
    pub name: &'static str,
    /// Where the terms are published.
    pub url: &'static str,
    /// What it was approved for, and where.
    pub approved: &'static str,
    /// The manifest fields it may appear in.
    pub uses: &'static [LicenceUse],
    /// The content-root-relative directories it may appear in; empty for any.
    pub dirs: &'static [&'static str],
}

/// The approved exceptions (next-gen roadmap §9.2).
pub const LICENCE_EXCEPTIONS: &[LicenceException] = &[
    LicenceException {
        id: "LicenseRef-CMU-Mocap",
        name: "CMU Graphics Lab Motion Capture Database terms",
        url: "http://mocap.cs.cmu.edu/faqs.php",
        approved: "animation source, next-gen roadmap §9.2 (2026-10-30)",
        uses: &[LicenceUse::Asset],
        dirs: &["animations"],
    },
    LicenceException {
        id: "LicenseRef-NVIDIA-Open-Model-License",
        name: "NVIDIA Open Model License",
        url: "https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/",
        approved: "Kimodo text-to-motion weights, next-gen roadmap §9.2-9.3 (2026-10-30)",
        uses: &[LicenceUse::Weights],
        dirs: &[],
    },
];

/// A proprietary generation service the maintainer approved for concept art only (art bible,
/// concept generation, 2026-10-31). Its output is a reference that people model from by hand:
/// it is never a shipped asset, never an input to a generator for one, and no shipped asset is
/// generated with it. Its licence id is therefore rejected in every licence field, and its
/// model names in `ai.model`, every `ai.tools` entry and every input's `name` and `source`,
/// with a message saying why, so a concept image cannot reach `content/` under another
/// label. (An entry that hides the service entirely, with no `[ai]` table, is beyond any
/// manifest check.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConceptOnlyService {
    /// The licence id concept images from the service carry.
    pub id: &'static str,
    /// Human-readable name.
    pub name: &'static str,
    /// Lower-case substrings that name the service's models (in `ai.model`, `ai.tools` and
    /// the inputs' `name` and `source`).
    pub model_markers: &'static [&'static str],
}

/// The concept-only services.
pub const CONCEPT_ONLY_SERVICES: &[ConceptOnlyService] = &[ConceptOnlyService {
    id: "LicenseRef-Gemini-API-Output",
    name: "Google Gemini API image output (Nano Banana)",
    model_markers: &["gemini", "nano banana", "nano-banana", "nanobanana"],
}];

fn concept_only_licence(licence: &str) -> Option<&'static ConceptOnlyService> {
    CONCEPT_ONLY_SERVICES
        .iter()
        .find(|service| service.id == licence)
}

fn concept_only_model(model: &str) -> Option<&'static ConceptOnlyService> {
    let model = model.to_ascii_lowercase();
    CONCEPT_ONLY_SERVICES.iter().find(|service| {
        service
            .model_markers
            .iter()
            .any(|marker| model.contains(marker))
    })
}

fn concept_only(name: &str) -> String {
    format!(
        "{name} is concept-only art: model from it by hand and say so in `notes`, but never \
         ship it or feed it to a generator"
    )
}

/// `Ok` when `licence` is allowed, or an approved exception for `usage` in `dir` (the
/// content-root-relative directory of the manifest), else why it is rejected.
///
/// # Errors
/// A human-readable reason.
pub fn licence_status(licence: &str, usage: LicenceUse, dir: &str) -> Result<(), String> {
    if let Some(service) = CONCEPT_ONLY_SERVICES
        .iter()
        .find(|service| service.id == licence)
    {
        return Err(format!(
            "licence \"{licence}\": {}",
            concept_only(service.name)
        ));
    }
    if ALLOWED_LICENCES.contains(&licence) {
        return Ok(());
    }
    if let Some(exception) = LICENCE_EXCEPTIONS
        .iter()
        .find(|exception| exception.id == licence)
    {
        let in_dir = exception.dirs.is_empty()
            || exception
                .dirs
                .iter()
                .any(|scope| dir == *scope || dir.starts_with(&format!("{scope}/")));
        if exception.uses.contains(&usage) && in_dir {
            return Ok(());
        }
        let uses: Vec<&str> = exception.uses.iter().map(|usage| usage.field()).collect();
        let dirs = if exception.dirs.is_empty() {
            String::from("any directory")
        } else {
            exception
                .dirs
                .iter()
                .map(|scope| format!("{scope}/"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Err(format!(
            "licence \"{licence}\" is an approved exception only as {} in {dirs}, not as {} \
             in {dir}/",
            uses.join(" or "),
            usage.field()
        ));
    }
    let upper = licence.to_ascii_uppercase();
    let has = |part: &str| {
        upper
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word == part)
    };
    let reason = if licence.trim().is_empty() || has("UNKNOWN") || has("NOASSERTION") {
        "an unknown licence is rejected"
    } else if has("NC") || has("ND") {
        "non-commercial and no-derivatives licences are rejected"
    } else if has("SA") {
        "share-alike licences are not on the allow-list"
    } else if upper.contains("RESERVED") || upper.contains("PROPRIETARY") {
        "all-rights-reserved assets are rejected unless listed as an exception"
    } else {
        "not on the allow-list and not an approved exception"
    };
    Err(format!("licence \"{licence}\": {reason}"))
}

/// Whether `licence` requires attribution text (`CC-BY-*`).
#[must_use]
pub fn needs_attribution(licence: &str) -> bool {
    licence.starts_with("CC-BY-")
}

/// One `[[asset]]` entry of a manifest.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AssetProvenance {
    /// File names or `*` patterns in the manifest's directory (no `/`).
    pub files: Vec<String>,
    /// What the files are, for the credits.
    pub title: String,
    /// `original` (made for this project) or the `http(s)` page it came from.
    pub source: String,
    /// Who made it.
    pub author: String,
    /// SPDX-like licence id; see [`licence_status`].
    pub licence: String,
    /// Attribution text as the licence requires it (required for `CC-BY-*`).
    #[serde(default)]
    pub attribution: Option<String>,
    /// How it was produced (tools, scripts).
    #[serde(default)]
    pub made_with: Option<String>,
    /// Anything a reviewer should know.
    #[serde(default)]
    pub notes: Option<String>,
    /// Present when the asset was generated with AI.
    #[serde(default)]
    pub ai: Option<AiProvenance>,
    /// Lower-case hex SHA-256 of covered files, by file name. Checked when present, and
    /// required for every covered file unless [`Self::is_original`]: it pins the bytes that
    /// were reviewed against the licence (ADR-0022 amendment 1).
    #[serde(default)]
    pub sha256: BTreeMap<String, String>,
}

impl AssetProvenance {
    /// Whether the asset was made for this project rather than taken from elsewhere.
    #[must_use]
    pub fn is_original(&self) -> bool {
        self.source == "original"
    }
}

/// How an AI-generated asset was made (roadmap §9.5).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AiProvenance {
    /// Model name.
    pub model: String,
    /// Model version, checkpoint or release date.
    pub model_version: String,
    /// The weights' licence; see [`licence_status`].
    pub weights_licence: String,
    /// Generation seed.
    pub seed: u64,
    /// A person modified the output, which permits a CC-BY licence (roadmap §9.5); the
    /// asset's `notes` must then say what was changed.
    #[serde(default)]
    pub human_modified: bool,
    /// The prompt; may be empty when [`Self::inputs`] drive the generation.
    #[serde(default)]
    pub prompt: String,
    /// Images, meshes or clips fed to the model.
    #[serde(default)]
    pub inputs: Vec<AiInput>,
    /// Every tool in the chain (generator front end, repair, retopology, baking).
    pub tools: Vec<String>,
}

/// One input to an AI generation.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AiInput {
    /// What it is (a file name or a description).
    pub name: String,
    /// `original` or the `http(s)` page it came from.
    pub source: String,
    /// Its licence; a third-party input must be `CC0-1.0`.
    pub licence: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    #[serde(default)]
    asset: Vec<AssetProvenance>,
}

/// One manifest entry with the files it covers.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProvenanceEntry {
    /// The entry's directory relative to the content root, `/`-separated.
    pub dir: String,
    /// The covered files relative to the content root, `/`-separated, sorted.
    pub files: Vec<String>,
    /// The entry itself.
    pub asset: AssetProvenance,
}

impl ProvenanceEntry {
    /// The entry's patterns relative to the content root (`models/sentinel.glb`).
    #[must_use]
    pub fn patterns(&self) -> Vec<String> {
        self.asset
            .files
            .iter()
            .map(|pattern| format!("{}/{pattern}", self.dir))
            .collect()
    }
}

/// What [`check_provenance`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProvenanceReport {
    /// Every manifest entry, by directory then manifest order.
    pub entries: Vec<ProvenanceEntry>,
    /// Asset files found (manifests excluded).
    pub files: usize,
    /// Gate failures, one line each; the gate passes when empty.
    pub errors: Vec<String>,
}

impl ProvenanceReport {
    /// Entries whose provenance records AI generation.
    pub fn ai_entries(&self) -> impl Iterator<Item = &ProvenanceEntry> {
        self.entries.iter().filter(|entry| entry.asset.ai.is_some())
    }
}

/// `*` matches any run of characters (`?` and classes are not supported).
fn glob_match(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

fn is_source(source: &str) -> bool {
    source == "original" || source.starts_with("https://") || source.starts_with("http://")
}

/// Whether `character` reorders or breaks the text it sits in: the Unicode line and
/// paragraph separators (U+2028, U+2029) and the bidirectional embeddings, overrides and
/// isolates (U+202A–U+202E, U+2066–U+2069), which can make a notice read differently from
/// its bytes.
fn is_layout_control(character: char) -> bool {
    matches!(character, '\u{2028}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Why `value` cannot go into the generated notices, if it cannot: control characters
/// (line breaks included), line and paragraph separators and bidirectional controls, and
/// HTML comment markers, which could end the generated section.
fn unsafe_text(value: &str) -> Option<&'static str> {
    if value.chars().any(char::is_control) {
        Some("contains a control character (line breaks included)")
    } else if value.chars().any(is_layout_control) {
        Some("contains a Unicode line separator or bidirectional control")
    } else if value.contains("<!--") || value.contains("-->") {
        Some("contains an HTML comment marker")
    } else {
        None
    }
}

/// Every text field of an entry, as `(field, value)`.
fn text_fields(asset: &AssetProvenance) -> Vec<(&'static str, &str)> {
    let mut fields = vec![
        ("title", asset.title.as_str()),
        ("source", asset.source.as_str()),
        ("author", asset.author.as_str()),
        ("licence", asset.licence.as_str()),
    ];
    fields.extend(asset.files.iter().map(|file| ("files", file.as_str())));
    for (field, value) in [
        ("attribution", &asset.attribution),
        ("made_with", &asset.made_with),
        ("notes", &asset.notes),
    ] {
        if let Some(value) = value {
            fields.push((field, value.as_str()));
        }
    }
    if let Some(ai) = &asset.ai {
        fields.extend([
            ("ai.model", ai.model.as_str()),
            ("ai.model_version", ai.model_version.as_str()),
            ("ai.weights_licence", ai.weights_licence.as_str()),
            ("ai.prompt", ai.prompt.as_str()),
        ]);
        fields.extend(ai.tools.iter().map(|tool| ("ai.tools", tool.as_str())));
        for input in &ai.inputs {
            fields.extend([
                ("ai.inputs.name", input.name.as_str()),
                ("ai.inputs.source", input.source.as_str()),
                ("ai.inputs.licence", input.licence.as_str()),
            ]);
        }
    }
    fields
}

/// The policy checks of one entry in `dir`, as error lines prefixed with `at`.
fn check_entry(at: &str, dir: &str, asset: &AssetProvenance, errors: &mut Vec<String>) {
    let mut fail = |message: String| errors.push(format!("{at}: {message}"));
    for (field, value) in text_fields(asset) {
        if let Some(problem) = unsafe_text(value) {
            fail(format!("{field} {problem}"));
        }
    }
    for (field, value) in [
        ("title", &asset.title),
        ("author", &asset.author),
        ("source", &asset.source),
    ] {
        if value.trim().is_empty() {
            fail(format!("{field} is empty"));
        }
    }
    if !asset.source.trim().is_empty() && !is_source(&asset.source) {
        fail(format!(
            "source \"{}\" must be \"original\" or an http(s) URL",
            asset.source
        ));
    }
    if let Err(reason) = licence_status(&asset.licence, LicenceUse::Asset, dir) {
        fail(reason);
    }
    if needs_attribution(&asset.licence)
        && asset
            .attribution
            .as_deref()
            .is_none_or(|text| text.trim().is_empty())
    {
        fail(format!(
            "{} needs attribution text (`attribution = \"…\"`)",
            asset.licence
        ));
    }
    let Some(ai) = &asset.ai else {
        return;
    };
    if asset.licence != "CC0-1.0" {
        if !needs_attribution(&asset.licence) {
            fail(format!(
                "an AI-generated asset is licensed CC0-1.0 (or CC-BY once a person modified \
                 it), not {}",
                asset.licence
            ));
        } else if !ai.human_modified
            || asset
                .notes
                .as_deref()
                .is_none_or(|notes| notes.trim().is_empty())
        {
            fail(format!(
                "AI output is CC0-1.0; {} needs `ai.human_modified = true` and notes saying \
                 what a person changed",
                asset.licence
            ));
        }
    }
    for (field, value) in [
        ("ai.model", &ai.model),
        ("ai.model_version", &ai.model_version),
    ] {
        if value.trim().is_empty() {
            fail(format!("{field} is empty"));
        }
    }
    if let Some(service) = concept_only_model(&ai.model) {
        fail(format!("ai.model: {}", concept_only(service.name)));
    }
    for tool in &ai.tools {
        if let Some(service) = concept_only_model(tool) {
            fail(format!(
                "ai.tools \"{tool}\": {}",
                concept_only(service.name)
            ));
        }
    }
    if let Err(reason) = licence_status(&ai.weights_licence, LicenceUse::Weights, dir) {
        fail(format!("ai.weights_licence: {reason}"));
    }
    if ai.prompt.trim().is_empty() && ai.inputs.is_empty() {
        fail(String::from("ai needs a prompt or at least one input"));
    }
    if ai.tools.is_empty() || ai.tools.iter().any(|tool| tool.trim().is_empty()) {
        fail(String::from("ai.tools must list every tool used"));
    }
    check_ai_inputs(dir, ai, &mut fail);
}

/// The checks on one AI entry's `inputs`: named, sourced, never a concept-only service's
/// output, and CC0-1.0 when third-party.
fn check_ai_inputs(dir: &str, ai: &AiProvenance, fail: &mut impl FnMut(String)) {
    for input in &ai.inputs {
        let label = format!("ai input \"{}\"", input.name);
        if input.name.trim().is_empty() {
            fail(String::from("an ai input has no name"));
        }
        for (field, value) in [("name", &input.name), ("source", &input.source)] {
            if let Some(service) = concept_only_model(value) {
                fail(format!("{label}: {field}: {}", concept_only(service.name)));
            }
        }
        if !is_source(&input.source) {
            fail(format!(
                "{label}: source must be \"original\" or an http(s) URL"
            ));
        }
        if input.source == "original" || concept_only_licence(&input.licence).is_some() {
            if let Err(reason) = licence_status(&input.licence, LicenceUse::Input, dir) {
                fail(format!("{label}: {reason}"));
            }
        } else if input.licence != "CC0-1.0" {
            fail(format!(
                "{label}: a third-party input to a generator must be CC0-1.0, not {}",
                input.licence
            ));
        }
    }
}

/// Checks every asset file below `root` against its directory's manifest and the licence
/// policy (see the module docs), and that the root holds only packs and the generated
/// notices. Directories and files are visited in sorted order, so the report is
/// deterministic.
#[must_use]
pub fn check_provenance(root: &Path) -> ProvenanceReport {
    let mut report = ProvenanceReport::default();
    let entries = match sorted_entries(root) {
        Ok(entries) => entries,
        Err(error) => {
            report.errors.push(format!("{}: {error}", root.display()));
            return report;
        }
    };
    for (name, kind) in entries {
        if refuse_entry("", &name, kind, &mut report) {
            continue;
        }
        if kind == EntryKind::Dir {
            walk(root, &name, &mut report);
        } else if !(Path::new(&name)
            .extension()
            .is_some_and(|extension| extension == "toml")
            || name == CREDITS_FILE
            || name == AI_DISCLOSURE_FILE)
        {
            report.errors.push(format!(
                "{name}: only packs (*.toml), {CREDITS_FILE} and {AI_DISCLOSURE_FILE} may sit at \
                 the content root; move assets into a directory with a {PROVENANCE_FILE}"
            ));
        }
    }
    report
}

/// What a directory entry is. Symlinks are never followed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum EntryKind {
    File,
    Dir,
    /// A symlink, socket, device or anything else that is not a plain file or directory.
    Other,
}

/// Reports (and returns `true` for) an entry the gate never accepts: a dot-file or
/// dot-directory (hidden from listings, so it could carry anything unseen), or a symlink or
/// special file (its target is outside the gate). `dir` is `""` at the root.
fn refuse_entry(dir: &str, name: &str, kind: EntryKind, report: &mut ProvenanceReport) -> bool {
    let path = if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    };
    let problem = if name.starts_with('.') {
        "dot-files and dot-directories are not allowed in a content root"
    } else if kind == EntryKind::Other {
        "symlinks and special files are not allowed in a content root"
    } else if kind == EntryKind::Dir && !plain_name(name) {
        "a directory name may use only ASCII letters, digits, `.`, `_` and `-`"
    } else {
        return false;
    };
    // A refused directory's name is escaped: it is not plain text.
    report
        .errors
        .push(format!("{}: {problem}", path.escape_debug()));
    true
}

/// ASCII letters, digits, `.`, `_` and `-` only: nothing Markdown, HTML or a shell reads
/// specially, so the name goes into the notices as it is.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Checks `dir` (relative to `root`), then its subdirectories, depth first.
fn walk(root: &Path, dir: &str, report: &mut ProvenanceReport) {
    let entries = match sorted_entries(&root.join(dir)) {
        Ok(entries) => entries,
        Err(error) => {
            report.errors.push(format!("{dir}: {error}"));
            return;
        }
    };
    let entries: Vec<(String, EntryKind)> = entries
        .into_iter()
        .filter(|(name, kind)| !refuse_entry(dir, name, *kind, report))
        .collect();
    let files: Vec<String> = entries
        .iter()
        .filter(|(name, kind)| *kind == EntryKind::File && name != PROVENANCE_FILE)
        .map(|(name, _)| name.clone())
        .collect();
    // Only a plain file is read as the manifest: a symlinked or special one was refused
    // above, and reading it could follow the link (to `/dev/zero`, say) or block on a FIFO.
    let manifest = entries
        .iter()
        .find(|(name, _)| name == PROVENANCE_FILE)
        .map(|(_, kind)| *kind);
    if manifest == Some(EntryKind::Dir) {
        report.errors.push(format!(
            "{dir}/{PROVENANCE_FILE}: is a directory, not a manifest"
        ));
    }
    check_dir(root, dir, &files, manifest == Some(EntryKind::File), report);
    for (name, kind) in &entries {
        if *kind == EntryKind::Dir {
            walk(root, &format!("{dir}/{name}"), report);
        }
    }
}

/// `(name, kind)` of a directory's entries, sorted by name, dot entries included.
fn sorted_entries(dir: &Path) -> std::io::Result<Vec<(String, EntryKind)>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // `DirEntry::file_type` does not follow symlinks.
        let kind = match entry.file_type()? {
            kind if kind.is_dir() => EntryKind::Dir,
            kind if kind.is_file() => EntryKind::File,
            _ => EntryKind::Other,
        };
        entries.push((name, kind));
    }
    entries.sort();
    Ok(entries)
}

/// Checks the `files` of `dir` against its manifest, which is read only when `has_manifest`
/// (a plain file of that name was listed).
fn check_dir(
    root: &Path,
    dir: &str,
    files: &[String],
    has_manifest: bool,
    report: &mut ProvenanceReport,
) {
    report.files += files.len();
    let manifest_name = format!("{dir}/{PROVENANCE_FILE}");
    let path = root.join(dir).join(PROVENANCE_FILE);
    let manifest = match has_manifest.then(|| read_manifest(&path)).transpose() {
        Ok(Some(manifest)) => manifest,
        Ok(None) => {
            if !files.is_empty() {
                report.errors.push(format!(
                    "{dir}/: no {PROVENANCE_FILE}, so {} file(s) have no provenance ({})",
                    files.len(),
                    files.join(", ")
                ));
            }
            return;
        }
        Err(error) => {
            report.errors.push(format!("{manifest_name}: {error}"));
            return;
        }
    };
    let mut owner: Vec<Option<usize>> = vec![None; files.len()];
    for (index, asset) in manifest.asset.into_iter().enumerate() {
        let at = format!(
            "{manifest_name} [[asset]] #{} \"{}\"",
            index + 1,
            asset.title
        );
        if asset.files.is_empty() {
            report.errors.push(format!("{at}: files is empty"));
        }
        for pattern in &asset.files {
            let plain = pattern.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'*')
            });
            if !plain || pattern.starts_with('.') || pattern == PROVENANCE_FILE {
                report.errors.push(format!(
                    "{at}: \"{pattern}\" must be a file name or `*` pattern in {dir}/ \
                     (ASCII letters, digits, `.`, `_`, `-`, `*`)"
                ));
                continue;
            }
            let mut matched_any = false;
            for (file_index, file) in files.iter().enumerate() {
                if !glob_match(pattern, file) {
                    continue;
                }
                matched_any = true;
                if pattern.contains('*')
                    && Path::new(file)
                        .extension()
                        .is_none_or(|extension| extension != "toml")
                {
                    report.errors.push(format!(
                        "{at}: \"{pattern}\" matches {dir}/{file}; a `*` pattern may cover only \
                         *.toml data files, so list binaries by name"
                    ));
                }
                match owner[file_index] {
                    Some(other) if other != index => report.errors.push(format!(
                        "{dir}/{file} is covered by [[asset]] #{} and #{} in {manifest_name}",
                        other + 1,
                        index + 1
                    )),
                    _ => owner[file_index] = Some(index),
                }
            }
            if !matched_any {
                report.errors.push(format!(
                    "{at}: \"{pattern}\" matches no file in {dir}/ (stale entry)"
                ));
            }
        }
        check_entry(&at, dir, &asset, &mut report.errors);
        let mut matched: Vec<String> = files
            .iter()
            .enumerate()
            .filter(|(file_index, _)| owner[*file_index] == Some(index))
            .map(|(_, file)| format!("{dir}/{file}"))
            .collect();
        matched.sort();
        check_hashes(root, dir, &at, &asset, &matched, &mut report.errors);
        report.entries.push(ProvenanceEntry {
            dir: dir.to_string(),
            files: matched,
            asset,
        });
    }
    for (file, owner) in files.iter().zip(&owner) {
        if owner.is_none() {
            report.errors.push(format!(
                "{dir}/{file} has no provenance: no [[asset]] in {manifest_name} covers it"
            ));
        }
    }
}

/// The baked-tangent rule ([`crate::check_baked_tangents`], ADR-0022 amendment 1) on every
/// `.glb` the report covers below `root`: one error per file that has UVs but no `TANGENT`,
/// or does not load. `content-check --provenance` adds these to the gate's errors.
#[cfg(feature = "glb")]
#[must_use]
pub fn baked_tangent_errors(root: &Path, report: &ProvenanceReport) -> Vec<String> {
    report
        .entries
        .iter()
        .flat_map(|entry| &entry.files)
        .filter(|file| {
            Path::new(file)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("glb"))
        })
        .filter_map(|file| {
            crate::check_baked_tangents_file(&root.join(file))
                .err()
                .map(|reason| format!("{file}: {reason}"))
        })
        .collect()
}

/// The parsed manifest at `path`, which the walk listed as a plain file.
fn read_manifest(path: &Path) -> Result<Manifest, String> {
    let text = read_plain_file(path, MAX_PROVENANCE_BYTES)?
        .ok_or_else(|| String::from("disappeared while the gate ran"))?;
    toml::from_str(&text).map_err(|error| error.to_string().trim_end().to_string())
}

/// The UTF-8 text of the plain file at `path`, at most `limit` bytes, or `None` when nothing
/// is there. A symlink (never followed), directory, FIFO, device or socket is an error, and
/// so is a file over `limit`; the read stops at `limit + 1` bytes even if the file grows, so
/// neither a link to `/dev/zero` nor a FIFO can exhaust memory or block the gate.
///
/// # Errors
/// A human-readable reason.
pub fn read_plain_file(path: &Path, limit: u64) -> Result<Option<String>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.file_type().is_file() {
        return Err(String::from(
            "is a symlink, directory or special file; only a plain file is read",
        ));
    }
    if metadata.len() > limit {
        return Err(format!("larger than {limit} bytes"));
    }
    let file = open_plain(path).map_err(|error| error.to_string())?;
    let mut text = String::new();
    file.take(limit + 1)
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    if text.len() as u64 > limit {
        return Err(format!("larger than {limit} bytes"));
    }
    Ok(Some(text))
}

/// Opens `path` for reading only if it is a plain file at the moment of opening. On unix the
/// open refuses a symlink as the last component (`O_NOFOLLOW`) and cannot block on a FIFO
/// (`O_NONBLOCK`), and on every platform the opened handle must be a regular file (`fstat`),
/// so a path swapped for a link, FIFO or device after an earlier check is never read.
fn open_plain(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|error| {
        // `O_NOFOLLOW` on a symlink fails with `ELOOP` (Linux, macOS) or `EMLINK` (FreeBSD).
        #[cfg(unix)]
        if matches!(error.raw_os_error(), Some(libc::ELOOP | libc::EMLINK)) {
            return std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "is a symlink; only a plain file is read",
            );
        }
        error
    })?;
    if !file.metadata()?.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "is a directory or special file; only a plain file is read",
        ));
    }
    Ok(file)
}

/// Lower-case hex SHA-256 of the plain file at `path`, read in blocks.
fn sha256_hex(path: &Path) -> std::io::Result<String> {
    let mut file = open_plain(path)?;
    let mut hasher = Sha256::new();
    let mut block = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut block)?;
        if read == 0 {
            break;
        }
        hasher.update(&block[..read]);
    }
    let mut hex = String::with_capacity(64);
    for byte in hasher.finalize() {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

/// The `sha256` checks of one entry in `dir`, whose covered files are `matched` (relative
/// to the content root): every key names a covered file, every value is 64 lower-case hex
/// digits equal to that file's SHA-256, and a non-original entry hashes every file.
fn check_hashes(
    root: &Path,
    dir: &str,
    at: &str,
    asset: &AssetProvenance,
    matched: &[String],
    errors: &mut Vec<String>,
) {
    let name_of = |path: &str| path.rsplit('/').next().unwrap_or(path).to_string();
    let covered: Vec<String> = matched.iter().map(|path| name_of(path)).collect();
    for (name, expected) in &asset.sha256 {
        if !covered.contains(name) {
            errors.push(format!(
                "{at}: sha256 names \"{}\", which this entry does not cover",
                name.escape_debug()
            ));
            continue;
        }
        if expected.len() != 64
            || !expected
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            errors.push(format!(
                "{at}: sha256 of {name} must be 64 lower-case hex digits"
            ));
            continue;
        }
        match sha256_hex(&root.join(dir).join(name)) {
            Ok(actual) if actual == *expected => {}
            Ok(actual) => errors.push(format!(
                "{dir}/{name}: SHA-256 is {actual}, but {at} records {expected}; the file \
                 changed since its licence was reviewed"
            )),
            Err(error) => errors.push(format!("{dir}/{name}: {error}")),
        }
    }
    if !asset.is_original() {
        for name in &covered {
            if !asset.sha256.contains_key(name) {
                errors.push(format!(
                    "{at}: {dir}/{name} is third-party, so it needs its SHA-256 \
                     (`sha256 = {{ \"{name}\" = \"…\" }}`)"
                ));
            }
        }
    }
}

/// The first line of every generated section of `THIRD_PARTY.md`.
pub const THIRD_PARTY_BEGIN: &str =
    "<!-- BEGIN content provenance: generated by `content-check --provenance`; do not edit -->";
/// The last line of the generated section.
pub const THIRD_PARTY_END: &str = "<!-- END content provenance -->";

/// Escapes text for a Markdown table cell or list item: `|`, backticks, brackets and `<`
/// (the gate already rejects control characters and comment markers).
fn cell(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '|' | '`' | '[' | ']' => {
                out.push('\\');
                out.push(character);
            }
            '<' => out.push_str("&lt;"),
            character if character.is_control() => out.push(' '),
            character => out.push(character),
        }
    }
    out
}

/// The generated `THIRD_PARTY.md` section (markers included, trailing newline): one table
/// row per manifest entry, the attribution texts and a pointer to the AI disclosure.
/// `content` is how the content root is written in the text (`content`).
#[must_use]
pub fn third_party_section(report: &ProvenanceReport, content: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{THIRD_PARTY_BEGIN}");
    let _ = writeln!(out, "## Content assets");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Every file in `{content}/`'s asset directories has a provenance entry in its \
         directory's `{PROVENANCE_FILE}` (A0, ADR-0022); `content-check --provenance` checks \
         them and writes this section, `{content}/{CREDITS_FILE}` and \
         `{content}/{AI_DISCLOSURE_FILE}`. {} entries cover {} files.",
        report.entries.len(),
        report.files
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "| Files | Title | Source | Author | Licence |");
    let _ = writeln!(out, "|---|---|---|---|---|");
    for entry in &report.entries {
        let files: Vec<String> = entry
            .patterns()
            .iter()
            .map(|pattern| format!("`{}/{}`", cell(content), cell(pattern)))
            .collect();
        let mut title = cell(&entry.asset.title);
        if entry.asset.ai.is_some() {
            title.push_str(" (AI-generated)");
        }
        let _ = writeln!(
            out,
            "| {} ({}) | {} | {} | {} | {} |",
            files.join(", "),
            entry.files.len(),
            title,
            cell(&entry.asset.source),
            cell(&entry.asset.author),
            cell(&entry.asset.licence)
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "### Attributions");
    let _ = writeln!(out);
    let attributed: Vec<&ProvenanceEntry> = report
        .entries
        .iter()
        .filter(|entry| entry.asset.attribution.is_some())
        .collect();
    if attributed.is_empty() {
        let _ = writeln!(out, "None: no asset carries an attribution licence.");
    }
    for entry in attributed {
        let _ = writeln!(
            out,
            "- {}: {}",
            cell(&entry.asset.title),
            cell(entry.asset.attribution.as_deref().unwrap_or(""))
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "### AI-generated assets");
    let _ = writeln!(out);
    let ai = report.ai_entries().count();
    if ai == 0 {
        let _ = writeln!(out, "None.");
    } else {
        let _ = writeln!(
            out,
            "{ai} entries; models, weights licences and tools are listed in \
             `{content}/AI-DISCLOSURE.md`."
        );
    }
    let _ = writeln!(out, "{THIRD_PARTY_END}");
    out
}

/// `existing` with its generated section replaced by `section`, or `section` appended
/// after a blank line when `existing` has no markers (`section` alone for an empty file).
///
/// # Errors
/// When the markers are not exactly one [`THIRD_PARTY_BEGIN`] followed by one
/// [`THIRD_PARTY_END`] on their own lines: a duplicate, missing or reversed marker is a
/// hand edit the tool will not guess at.
pub fn splice_third_party(existing: &str, section: &str) -> Result<String, String> {
    let begins: Vec<usize> = existing
        .match_indices(THIRD_PARTY_BEGIN)
        .map(|(at, _)| at)
        .collect();
    let ends: Vec<usize> = existing
        .match_indices(THIRD_PARTY_END)
        .map(|(at, _)| at)
        .collect();
    match (begins.as_slice(), ends.as_slice()) {
        ([], []) => {
            let mut out = existing.trim_end().to_string();
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(section);
            Ok(out)
        }
        (&[begin], &[end]) if begin < end => {
            let line_start = |at: usize| at == 0 || existing.as_bytes()[at - 1] == b'\n';
            let mut after = end + THIRD_PARTY_END.len();
            if !line_start(begin) || !line_start(end) {
                return Err(String::from(
                    "the content provenance markers must start their own lines",
                ));
            }
            if existing[after..].starts_with('\n') {
                after += 1;
            }
            Ok(format!(
                "{}{section}{}",
                &existing[..begin],
                &existing[after..]
            ))
        }
        _ => Err(format!(
            "expected one BEGIN then one END content provenance marker, found {} BEGIN and {} \
             END{}; fix the file by hand",
            begins.len(),
            ends.len(),
            if begins.len() == 1 && ends.len() == 1 {
                " in the wrong order"
            } else {
                ""
            }
        )),
    }
}

/// A credits heading and which entries it lists.
type Section = (&'static str, fn(&AssetProvenance) -> bool);

/// The credits text the game can show: plain UTF-8, one block per section (original,
/// third-party, AI-generated), one line per entry plus its attribution.
#[must_use]
pub fn credits_text(report: &ProvenanceReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "CONTENT CREDITS");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Generated from the provenance.toml manifests by `content-check --provenance`. Do not edit."
    );
    let sections: [Section; 3] = [
        ("Original content", |asset| {
            asset.ai.is_none() && asset.is_original()
        }),
        ("Third-party content", |asset| {
            asset.ai.is_none() && !asset.is_original()
        }),
        ("AI-generated content", |asset| asset.ai.is_some()),
    ];
    for (heading, belongs) in sections {
        let _ = writeln!(out);
        let _ = writeln!(out, "{heading}");
        let mut any = false;
        for entry in report.entries.iter().filter(|entry| belongs(&entry.asset)) {
            any = true;
            let asset = &entry.asset;
            let _ = write!(
                out,
                "  {} - {} - {}",
                asset.title, asset.author, asset.licence
            );
            if !asset.is_original() {
                let _ = write!(out, " - {}", asset.source);
            }
            if let Some(ai) = &asset.ai {
                let _ = write!(out, " - made with {} {}", ai.model, ai.model_version);
            }
            let _ = writeln!(out);
            if let Some(attribution) = &asset.attribution {
                let _ = writeln!(out, "    {attribution}");
            }
        }
        if !any {
            let _ = writeln!(out, "  (none)");
        }
    }
    out
}

/// The Steam AI-disclosure list (Markdown): every asset whose provenance records AI
/// generation, with its model, version, weights licence, asset licence and tools.
#[must_use]
pub fn ai_disclosure(report: &ProvenanceReport, content: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# AI-generated content disclosure");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Generated from the `{PROVENANCE_FILE}` manifests under `{content}/` by \
         `content-check --provenance` (A0, ADR-0022); do not edit. It lists every shipped \
         asset whose provenance records AI generation, for the \"pre-generated content\" \
         part of Steam's AI content disclosure. The engine generates no content with AI \
         while it runs."
    );
    let _ = writeln!(out);
    let ai: Vec<&ProvenanceEntry> = report.ai_entries().collect();
    if ai.is_empty() {
        let _ = writeln!(
            out,
            "**No shipped asset was generated with AI** ({} manifest entries, {} files).",
            report.entries.len(),
            report.files
        );
        return out;
    }
    let _ = writeln!(
        out,
        "{} of {} manifest entries are AI-generated.",
        ai.len(),
        report.entries.len()
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "| Files | Title | Model | Version | Weights licence | Asset licence | Tools |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|");
    for entry in ai {
        let Some(generation) = &entry.asset.ai else {
            continue;
        };
        let files: Vec<String> = entry
            .patterns()
            .iter()
            .map(|pattern| format!("`{}/{}`", cell(content), cell(pattern)))
            .collect();
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} |",
            files.join(", "),
            cell(&entry.asset.title),
            cell(&generation.model),
            cell(&generation.model_version),
            cell(&generation.weights_licence),
            cell(&entry.asset.licence),
            cell(&generation.tools.join(", "))
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_whole_names() {
        assert!(glob_match("sentinel.glb", "sentinel.glb"));
        assert!(!glob_match("sentinel.glb", "sentinel.glbx"));
        assert!(glob_match("*.toml", "nova.toml"));
        assert!(!glob_match("*.toml", "atlas.ktx2"));
        assert!(glob_match("terrain-*.ktx2", "terrain-rock-normal.ktx2"));
        assert!(!glob_match("terrain-*.ktx2", "sentinel-base.ktx2"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("a*b*c", "aXbYc"));
        assert!(!glob_match("a*b*c", "aXcYb"));
        assert!(!glob_match("ab*ba", "aba"), "the ends may not overlap");
    }

    #[test]
    fn the_licence_policy_follows_the_roadmap() {
        for allowed in ["CC0-1.0", "CC-BY-4.0", "MIT OR Apache-2.0", "OFL-1.1"] {
            for usage in [LicenceUse::Asset, LicenceUse::Weights, LicenceUse::Input] {
                assert_eq!(
                    licence_status(allowed, usage, "models"),
                    Ok(()),
                    "{allowed}"
                );
            }
        }
        for (rejected, why) in [
            ("CC-BY-NC-4.0", "non-commercial"),
            ("CC-BY-ND-4.0", "no-derivatives"),
            ("CC-BY-NC-ND-4.0", "non-commercial"),
            ("CC-BY-SA-4.0", "share-alike"),
            ("", "unknown"),
            ("unknown", "unknown"),
            ("NOASSERTION", "unknown"),
            ("All rights reserved", "all-rights-reserved"),
            ("LicenseRef-Proprietary", "all-rights-reserved"),
            ("GPL-3.0-only", "allow-list"),
        ] {
            let error = licence_status(rejected, LicenceUse::Asset, "models").expect_err(rejected);
            assert!(error.contains(why), "{rejected}: {error}");
        }
        assert!(needs_attribution("CC-BY-4.0"));
        assert!(!needs_attribution("CC0-1.0"));
        // Concept-only service output is rejected in every field and directory.
        for usage in [LicenceUse::Asset, LicenceUse::Weights, LicenceUse::Input] {
            let error = licence_status("LicenseRef-Gemini-API-Output", usage, "concept")
                .expect_err("concept-only");
            assert!(error.contains("concept-only art"), "{error}");
        }
    }

    #[test]
    fn exceptions_apply_only_to_their_approved_use_and_directories() {
        const CMU: &str = "LicenseRef-CMU-Mocap";
        const OML: &str = "LicenseRef-NVIDIA-Open-Model-License";
        assert_eq!(licence_status(CMU, LicenceUse::Asset, "animations"), Ok(()));
        assert_eq!(
            licence_status(CMU, LicenceUse::Asset, "animations/hero"),
            Ok(())
        );
        for (usage, dir) in [
            (LicenceUse::Asset, "models"),
            (LicenceUse::Asset, "animations-extra"),
            (LicenceUse::Weights, "animations"),
            (LicenceUse::Input, "animations"),
        ] {
            let error = licence_status(CMU, usage, dir).expect_err(dir);
            assert!(error.contains("only as licence in animations/"), "{error}");
        }
        for dir in ["animations", "textures"] {
            assert_eq!(licence_status(OML, LicenceUse::Weights, dir), Ok(()));
        }
        for usage in [LicenceUse::Asset, LicenceUse::Input] {
            let error = licence_status(OML, usage, "animations").expect_err("not weights");
            assert!(error.contains("only as ai.weights_licence"), "{error}");
        }
        // Through a manifest entry, too: CMU motion is not a texture licence.
        let cmu = |dir| {
            entry_errors_in(
                dir,
                "source = \"https://mocap.cs.cmu.edu/\"\nlicence = \"LicenseRef-CMU-Mocap\"\n\
                 attribution = \"CMU Graphics Lab Motion Capture Database\"",
            )
        };
        assert_eq!(cmu("animations"), Vec::<String>::new());
        assert!(cmu("textures").iter().any(|e| e.contains("exception only")));
    }

    /// A scratch content root under the system temporary directory, removed on drop.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("moba-provenance-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }

        fn write(&self, path: &str, text: &str) {
            let path = self.0.join(path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, text).expect("write");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const ORIGINAL: &str = r#"
[[asset]]
files = ["a.glb"]
title = "Sample meshes"
source = "original"
author = "MOBA Engine contributors"
licence = "MIT OR Apache-2.0"
"#;

    fn errors(scratch: &Scratch) -> Vec<String> {
        check_provenance(&scratch.0).errors
    }

    #[test]
    fn every_asset_file_needs_exactly_one_entry() {
        let scratch = Scratch::new("coverage");
        scratch.write("pack.toml", "# a pack: not an asset");
        scratch.write("models/a.glb", "");
        scratch.write("models/b.glb", "");
        // No manifest: both files are reported.
        let found = errors(&scratch);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("no provenance.toml") && found[0].contains("a.glb, b.glb"));
        // Covered: clean, and the report lists the files.
        let both = ORIGINAL.replace("[\"a.glb\"]", "[\"a.glb\", \"b.glb\"]");
        scratch.write("models/provenance.toml", &both);
        let report = check_provenance(&scratch.0);
        assert_eq!(report.errors, Vec::<String>::new());
        assert_eq!(report.files, 2);
        assert_eq!(report.entries[0].files, ["models/a.glb", "models/b.glb"]);
        // An uncovered file, a stale pattern and a doubly covered file.
        scratch.write("models/c.lod", "");
        scratch.write(
            "models/provenance.toml",
            &format!(
                "{both}\n[[asset]]\nfiles = [\"a.glb\", \"gone.glb\"]\ntitle = \"A\"\n\
                 source = \"original\"\nauthor = \"x\"\nlicence = \"CC0-1.0\"\n"
            ),
        );
        let found = errors(&scratch);
        assert!(
            found
                .iter()
                .any(|e| e.contains("models/c.lod has no provenance")),
            "{found:?}"
        );
        assert!(
            found
                .iter()
                .any(|e| e.contains("\"gone.glb\" matches no file")),
            "{found:?}"
        );
        assert!(
            found
                .iter()
                .any(|e| e.contains("covered by [[asset]] #1 and #2")),
            "{found:?}"
        );
        // Nested directories need their own manifest.
        scratch.write("models/provenance.toml", &format!("{both}\n[[asset]]\nfiles = [\"c.lod\"]\ntitle = \"C\"\nsource = \"original\"\nauthor = \"x\"\nlicence = \"CC0-1.0\"\n"));
        scratch.write("maps/ao/x.ktx2", "");
        let found = errors(&scratch);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].starts_with("maps/ao/: no provenance.toml"),
            "{found:?}"
        );
        // Unknown keys and bad patterns are rejected.
        scratch.write("maps/ao/provenance.toml", "[[asset]]\nfiles = [\"../x.ktx2\"]\ntitle = \"t\"\nsource = \"original\"\nauthor = \"a\"\nlicence = \"CC0-1.0\"\nlicense = \"CC0-1.0\"\n");
        let found = errors(&scratch);
        assert!(
            found.iter().any(|e| e.contains("unknown field `license`")),
            "{found:?}"
        );
    }

    #[test]
    fn the_root_holds_only_packs_and_notices_and_nothing_hides() {
        let scratch = Scratch::new("root");
        scratch.write("pack.toml", "");
        scratch.write(CREDITS_FILE, "");
        scratch.write(AI_DISCLOSURE_FILE, "");
        scratch.write("models/a.glb", "");
        scratch.write("models/provenance.toml", ORIGINAL);
        assert_eq!(errors(&scratch), Vec::<String>::new());
        // A binary or stray document at the root is not a pack.
        scratch.write("loose.png", "");
        scratch.write("README.md", "");
        let found = errors(&scratch);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].starts_with("README.md: only packs"), "{found:?}");
        assert!(found[1].starts_with("loose.png: only packs"), "{found:?}");
        std::fs::remove_file(scratch.0.join("loose.png")).expect("rm");
        std::fs::remove_file(scratch.0.join("README.md")).expect("rm");
        // Dot-files and dot-directories are errors at the root and below, even when a
        // pattern would match them.
        scratch.write(".secret.png", "");
        scratch.write(".cache/x.glb", "");
        scratch.write("models/.hidden.glb", "");
        scratch.write("models/.git/blob", "");
        let found = errors(&scratch);
        assert_eq!(
            found,
            [
                ".cache: dot-files and dot-directories are not allowed in a content root",
                ".secret.png: dot-files and dot-directories are not allowed in a content root",
                "models/.git: dot-files and dot-directories are not allowed in a content root",
                "models/.hidden.glb: dot-files and dot-directories are not allowed in a content \
                 root",
            ]
        );
    }

    /// A symlinked directory is reported, not followed, so it cannot pull unchecked files
    /// (or a loop) into the gate; a symlinked file likewise.
    #[cfg(unix)]
    #[test]
    fn symlinks_are_reported_not_followed() {
        let scratch = Scratch::new("symlink");
        let outside = Scratch::new("symlink-target");
        outside.write("unvetted.glb", "");
        scratch.write("models/a.glb", "");
        scratch.write("models/provenance.toml", ORIGINAL);
        std::os::unix::fs::symlink(&outside.0, scratch.0.join("models/linked")).expect("link");
        std::os::unix::fs::symlink(&outside.0, scratch.0.join("textures")).expect("link");
        std::os::unix::fs::symlink(
            outside.0.join("unvetted.glb"),
            scratch.0.join("models/b.glb"),
        )
        .expect("link");
        let report = check_provenance(&scratch.0);
        let symlink = "symlinks and special files are not allowed in a content root";
        assert_eq!(
            report.errors,
            [
                format!("models/b.glb: {symlink}"),
                format!("models/linked: {symlink}"),
                format!("textures: {symlink}"),
            ]
        );
        assert_eq!(report.files, 1, "only models/a.glb is a checked asset");
    }

    fn entry_errors(extra: &str) -> Vec<String> {
        entry_errors_in("models", extra)
    }

    fn entry_errors_in(dir: &str, extra: &str) -> Vec<String> {
        let asset: AssetProvenance = toml::from_str(&format!(
            "files = [\"x\"]\ntitle = \"T\"\nauthor = \"A\"\n{extra}"
        ))
        .expect("entry parses");
        let mut errors = Vec::new();
        check_entry("at", dir, &asset, &mut errors);
        errors
    }

    #[test]
    fn licences_sources_and_attribution_are_checked() {
        assert_eq!(
            entry_errors("source = \"original\"\nlicence = \"CC0-1.0\""),
            Vec::<String>::new()
        );
        let nc = entry_errors("source = \"https://example.org/a\"\nlicence = \"CC-BY-NC-4.0\"");
        assert!(nc.iter().any(|e| e.contains("non-commercial")), "{nc:?}");
        let by = entry_errors("source = \"https://example.org/a\"\nlicence = \"CC-BY-4.0\"");
        assert!(by.iter().any(|e| e.contains("needs attribution")), "{by:?}");
        assert_eq!(
            entry_errors(
                "source = \"https://example.org/a\"\nlicence = \"CC-BY-4.0\"\n\
                 attribution = \"\\\"A\\\" by B, CC BY 4.0\""
            ),
            Vec::<String>::new()
        );
        let source = entry_errors("source = \"my hard drive\"\nlicence = \"CC0-1.0\"");
        assert!(
            source.iter().any(|e| e.contains("must be \"original\"")),
            "{source:?}"
        );
    }

    #[test]
    fn manifest_text_cannot_break_the_generated_notices() {
        let ok = "source = \"original\"\nlicence = \"CC0-1.0\"\n";
        assert_eq!(entry_errors(ok), Vec::<String>::new());
        for (field, value, problem) in [
            ("notes", "x <!-- THIRD_PARTY_END", "comment marker"),
            ("made_with", "end -->", "comment marker"),
            ("notes", "two\\nlines", "control character"),
            ("attribution", "tab\\there", "control character"),
            ("notes", "bell\\u0007", "control character"),
            (
                "notes",
                "line\\u2028break",
                "line separator or bidirectional",
            ),
            (
                "made_with",
                "evil\\u202Etxt.exe",
                "line separator or bidirectional",
            ),
            (
                "attribution",
                "isolate\\u2066x\\u2069",
                "line separator or bidirectional",
            ),
        ] {
            let found = entry_errors(&format!("{ok}{field} = \"{value}\"\n"));
            assert!(
                found
                    .iter()
                    .any(|e| e.starts_with(&format!("at: {field} ")) && e.contains(problem)),
                "{field} = {value:?}: {found:?}"
            );
        }
        // Inside the AI block too.
        let prompt = entry_errors(&AI.replace("a bronze brazier", "a <!-- b"));
        assert!(
            prompt.iter().any(|e| e.contains("ai.prompt contains")),
            "{prompt:?}"
        );
        // Patterns are plain ASCII file names.
        let scratch = Scratch::new("patterns");
        scratch.write("models/a.glb", "");
        for bad in ["a`.glb", "[a].glb", "a b.glb", "<a>.glb", "../a.glb"] {
            scratch.write(
                "models/provenance.toml",
                &ORIGINAL.replace("\"a.glb\"", &format!("\"a.glb\", '{bad}'")),
            );
            let found = errors(&scratch);
            assert!(
                found.iter().any(|e| e.contains("must be a file name")),
                "{bad}: {found:?}"
            );
        }
        // Markdown and HTML that the gate allows are escaped in the tables.
        assert_eq!(
            cell("a|b `c` [d](e) <img>"),
            "a\\|b \\`c\\` \\[d\\](e) &lt;img>"
        );
    }

    /// A manifest (or any entry) that is a symlink or special file is reported and never
    /// read: following a link to `/dev/zero` would exhaust memory, and opening a FIFO would
    /// block the gate.
    #[cfg(unix)]
    #[test]
    fn special_files_are_never_read() {
        let scratch = Scratch::new("special");
        scratch.write("models/a.glb", "");
        std::os::unix::fs::symlink("/dev/zero", scratch.0.join("models/provenance.toml"))
            .expect("link");
        let fifo = scratch.0.join("textures/provenance.toml");
        std::fs::create_dir_all(fifo.parent().expect("parent")).expect("dir");
        let made_fifo = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .is_ok_and(|status| status.success());
        let start = std::time::Instant::now();
        let found = errors(&scratch);
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
        let special = "symlinks and special files are not allowed in a content root";
        assert!(
            found.contains(&format!("models/provenance.toml: {special}")),
            "{found:?}"
        );
        assert!(
            found
                .iter()
                .any(|e| e.starts_with("models/: no provenance.toml"))
        );
        if made_fifo {
            assert!(
                found.contains(&format!("textures/provenance.toml: {special}")),
                "{found:?}"
            );
            assert!(read_plain_file(&fifo, 1024).is_err(), "a FIFO is not read");
            // The open itself neither blocks on a FIFO (no writer) nor reads it.
            let start = std::time::Instant::now();
            assert!(open_plain(&fifo).is_err());
            assert!(sha256_hex(&fifo).is_err());
            assert!(start.elapsed() < std::time::Duration::from_secs(5));
        }
        // A link to a plain file, as a path swapped after the walk's check would be: the open
        // refuses it (`O_NOFOLLOW`), so neither the reader nor the hash follows it.
        scratch.write("real.toml", "x = 1");
        let swapped = scratch.0.join("swapped.toml");
        std::os::unix::fs::symlink(scratch.0.join("real.toml"), &swapped).expect("link");
        assert!(
            open_plain(&swapped).is_err(),
            "the open does not follow a link"
        );
        assert!(
            sha256_hex(&swapped).is_err(),
            "the hash does not follow a link"
        );
        assert!(sha256_hex(&scratch.0.join("real.toml")).is_ok());
        let link = scratch.0.join("models/provenance.toml");
        assert!(
            read_plain_file(&link, 1024).is_err(),
            "a link is not followed"
        );
        assert_eq!(read_plain_file(&scratch.0.join("absent"), 1024), Ok(None));
        scratch.write("big.toml", &"x".repeat(2048));
        assert!(read_plain_file(&scratch.0.join("big.toml"), 1024).is_err());
        assert_eq!(
            read_plain_file(&scratch.0.join("big.toml"), 4096).map(|text| text.map(|t| t.len())),
            Ok(Some(2048))
        );
    }

    /// Directory names go into the notices, so they are plain ASCII names like the
    /// patterns; anything else is reported (escaped) and its contents are not walked.
    #[test]
    fn directory_names_are_plain() {
        let scratch = Scratch::new("dirnames");
        scratch.write("models/a.glb", "");
        scratch.write("models/provenance.toml", ORIGINAL);
        scratch.write("bad name/x.glb", "");
        scratch.write("models/sub[1]/x.glb", "");
        let found = errors(&scratch);
        let rule = "a directory name may use only ASCII letters, digits, `.`, `_` and `-`";
        assert_eq!(
            found,
            [
                format!("bad name: {rule}"),
                format!("models/sub[1]: {rule}")
            ]
        );
    }

    /// A directory name cannot forge the generated section's END marker (A0 review).
    #[cfg(unix)]
    #[test]
    fn a_directory_name_cannot_forge_a_marker() {
        let scratch = Scratch::new("forge");
        let forged = format!("a`|b\n{THIRD_PARTY_END}\nx");
        scratch.write(&format!("{forged}/x.glb"), "");
        scratch.write(
            &format!("{forged}/provenance.toml"),
            &ORIGINAL.replace("a.glb", "x.glb"),
        );
        let report = check_provenance(&scratch.0);
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert!(report.errors[0].contains("a directory name may use only"));
        assert!(!report.errors[0].contains('\n'), "the name is escaped");
        assert!(
            report.entries.is_empty(),
            "nothing under it reaches the notices"
        );
        // The renderer escapes the directory as a cell too.
        let entry = ProvenanceEntry {
            dir: String::from("a`|b"),
            files: vec![String::from("a`|b/x.glb")],
            asset: toml::from_str(&ORIGINAL.replace("[[asset]]", "")).expect("entry"),
        };
        let report = ProvenanceReport {
            entries: vec![entry],
            files: 1,
            errors: Vec::new(),
        };
        let section = third_party_section(&report, "content");
        assert!(section.contains("`content/a\\`\\|b/a.glb`"), "{section}");
    }

    /// `*` patterns are for data files (`*.toml`); binaries are listed by name, so a new
    /// binary never slips under an old entry unreviewed.
    #[test]
    fn star_patterns_cover_only_toml() {
        let scratch = Scratch::new("stars");
        scratch.write("vfx/a.toml", "");
        scratch.write("vfx/b.toml", "");
        scratch.write("vfx/atlas.ktx2", "");
        let manifest = |files: &str| {
            format!(
                "[[asset]]\nfiles = [{files}]\ntitle = \"T\"\nsource = \"original\"\n\
                 author = \"A\"\nlicence = \"CC0-1.0\"\n"
            )
        };
        scratch.write(
            "vfx/provenance.toml",
            &manifest("\"*.toml\", \"atlas.ktx2\""),
        );
        assert_eq!(errors(&scratch), Vec::<String>::new());
        scratch.write("vfx/provenance.toml", &manifest("\"*\""));
        let found = errors(&scratch);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("matches vfx/atlas.ktx2; a `*` pattern may cover only *.toml"),
            "{found:?}"
        );
    }

    /// The lower-case hex SHA-256 of `bytes`.
    fn sha256_of(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .fold(String::new(), |mut hex, byte| {
                let _ = write!(hex, "{byte:02x}");
                hex
            })
    }

    /// A third-party file is pinned by its SHA-256; an original one may be. Every recorded
    /// hash is checked, and must name a file its entry covers.
    #[test]
    fn third_party_files_are_pinned_by_sha256() {
        let scratch = Scratch::new("sha256");
        scratch.write("textures/rock.ktx2", "rock bytes");
        scratch.write("textures/moss.ktx2", "moss bytes");
        let rock = sha256_of(b"rock bytes");
        let moss = sha256_of(b"moss bytes");
        let manifest = |source: &str, hashes: &str| {
            format!(
                "[[asset]]\nfiles = [\"rock.ktx2\", \"moss.ktx2\"]\ntitle = \"T\"\n\
                 source = \"{source}\"\nauthor = \"A\"\nlicence = \"CC0-1.0\"\n{hashes}"
            )
        };
        let check = |source: &str, hashes: &str| {
            scratch.write("textures/provenance.toml", &manifest(source, hashes));
            errors(&scratch)
        };
        let downloaded = "https://example.org/rock";
        // Third-party: every file needs a hash.
        let found = check(downloaded, "");
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found
                .iter()
                .all(|e| e.contains("is third-party, so it needs its SHA-256"))
        );
        let both =
            format!("sha256 = {{ \"rock.ktx2\" = \"{rock}\", \"moss.ktx2\" = \"{moss}\" }}\n");
        assert_eq!(check(downloaded, &both), Vec::<String>::new());
        // A changed file, a malformed hash, a name the entry does not cover.
        let found = check(downloaded, &both.replace(&rock, &moss));
        assert!(
            found
                .iter()
                .any(|e| e.starts_with("textures/rock.ktx2: SHA-256 is")
                    && e.contains("changed since its licence was reviewed")),
            "{found:?}"
        );
        let found = check(downloaded, &both.replace(&rock, &rock.to_uppercase()));
        assert!(
            found.iter().any(|e| e.contains("64 lower-case hex digits")),
            "{found:?}"
        );
        let found = check(
            downloaded,
            &both.replace(" }", &format!(", \"gone.ktx2\" = \"{rock}\" }}")),
        );
        assert!(
            found
                .iter()
                .any(|e| e.contains("sha256 names \"gone.ktx2\", which this entry does not cover")),
            "{found:?}"
        );
        // Original work: optional, but checked when present.
        assert_eq!(check("original", ""), Vec::<String>::new());
        let one = format!("sha256 = {{ \"rock.ktx2\" = \"{rock}\" }}\n");
        assert_eq!(check("original", &one), Vec::<String>::new());
        let found = check("original", &one.replace(&rock, &moss));
        assert!(found.iter().any(|e| e.contains("SHA-256 is")), "{found:?}");
    }

    const AI: &str = "source = \"original\"\nlicence = \"CC0-1.0\"\n[ai]\nmodel = \"TRELLIS.2-4B\"\n\
        model_version = \"2025-12\"\nweights_licence = \"MIT\"\nseed = 7\n\
        prompt = \"a bronze brazier\"\ntools = [\"ComfyUI\"]\n";

    #[test]
    fn ai_assets_record_the_generation_and_follow_the_roadmap_rules() {
        assert_eq!(entry_errors(AI), Vec::<String>::new());
        let all_rights = entry_errors(&AI.replace("licence = \"CC0-1.0\"", "licence = \"MIT\""));
        assert!(
            all_rights.iter().any(|e| e.contains("licensed CC0-1.0")),
            "{all_rights:?}"
        );
        // CC-BY needs a person's modification, recorded with what they changed.
        let by = AI.replace(
            "licence = \"CC0-1.0\"",
            "licence = \"CC-BY-4.0\"\nattribution = \"Brazier by A, CC BY 4.0\"",
        );
        let unmodified = entry_errors(&by);
        assert!(
            unmodified
                .iter()
                .any(|e| e.contains("needs `ai.human_modified = true`")),
            "{unmodified:?}"
        );
        let modified = by.replace("seed = 7", "seed = 7\nhuman_modified = true");
        let no_notes = entry_errors(&modified);
        assert!(
            no_notes
                .iter()
                .any(|e| e.contains("notes saying what a person changed")),
            "{no_notes:?}"
        );
        let blank_notes = entry_errors(&modified.replace("[ai]", "notes = \" \"\n[ai]"));
        assert!(
            blank_notes
                .iter()
                .any(|e| e.contains("notes saying what a person changed")),
            "{blank_notes:?}"
        );
        let noted = modified.replace(
            "[ai]",
            "notes = \"Retopologised and repainted by A.\"\n[ai]",
        );
        assert_eq!(entry_errors(&noted), Vec::<String>::new());
        let nc_weights = entry_errors(&AI.replace("\"MIT\"", "\"LicenseRef-FLUX-dev-NC\""));
        assert!(
            nc_weights.iter().any(|e| e.contains("ai.weights_licence")),
            "{nc_weights:?}"
        );
        let kimodo = AI.replace("\"MIT\"", "\"LicenseRef-NVIDIA-Open-Model-License\"");
        assert_eq!(entry_errors(&kimodo), Vec::<String>::new());
        let no_prompt = entry_errors(&AI.replace("a bronze brazier", ""));
        assert!(
            no_prompt
                .iter()
                .any(|e| e.contains("prompt or at least one input"))
        );
        let no_tools = entry_errors(&AI.replace("[\"ComfyUI\"]", "[]"));
        assert!(no_tools.iter().any(|e| e.contains("ai.tools")));
        let input = |source: &str, licence: &str| {
            format!(
                "{AI}[[ai.inputs]]\nname = \"concept.png\"\nsource = \"{source}\"\n\
                 licence = \"{licence}\"\n"
            )
        };
        assert_eq!(
            entry_errors(&input("original", "CC0-1.0")),
            Vec::<String>::new()
        );
        assert_eq!(
            entry_errors(&input("https://polyhaven.com/a/x", "CC0-1.0")),
            Vec::<String>::new()
        );
        let by_input = entry_errors(&input("https://example.org/x", "CC-BY-4.0"));
        assert!(
            by_input.iter().any(|e| e.contains("must be CC0-1.0")),
            "{by_input:?}"
        );
        let missing: Result<AssetProvenance, _> = toml::from_str(
            "files = [\"x\"]\ntitle = \"T\"\nauthor = \"A\"\nsource = \"original\"\n\
             licence = \"CC0-1.0\"\n[ai]\nmodel = \"m\"\nmodel_version = \"1\"\n\
             weights_licence = \"MIT\"\ntools = [\"t\"]\n",
        );
        assert!(missing.is_err(), "the seed is required");
    }

    #[test]
    fn concept_only_services_are_screened_everywhere() {
        // Concept-only services: no shipped asset is generated with them or from their output,
        // whatever licence the entry claims.
        let gemini = entry_errors(&AI.replace("TRELLIS.2-4B", "Gemini 3 Pro Image"));
        assert!(
            gemini
                .iter()
                .any(|e| e.contains("ai.model") && e.contains("concept-only")),
            "{gemini:?}"
        );
        for source in ["original", "https://example.org/sheet"] {
            let fed = entry_errors(&format!(
                "{AI}[[ai.inputs]]\nname = \"sheet.png\"\nsource = \"{source}\"\n\
                 licence = \"LicenseRef-Gemini-API-Output\"\n"
            ));
            assert!(
                fed.iter()
                    .any(|e| e.contains("sheet.png") && e.contains("concept-only")),
                "{source}: {fed:?}"
            );
        }
        // The service named in the tool chain or an input, under any licence label.
        let tool = entry_errors(&AI.replace("[\"ComfyUI\"]", "[\"ComfyUI\", \"Nano Banana Pro\"]"));
        assert!(
            tool.iter()
                .any(|e| e.contains("ai.tools \"Nano Banana Pro\"") && e.contains("concept-only")),
            "{tool:?}"
        );
        for (name, source) in [
            ("gemini-sheet.png", "original"),
            ("sheet.png", "https://gemini.google.com/app/1"),
        ] {
            let fed = entry_errors(&format!(
                "{AI}[[ai.inputs]]\nname = \"{name}\"\nsource = \"{source}\"\n\
                 licence = \"CC0-1.0\"\n"
            ));
            assert!(
                fed.iter().any(|e| e.contains("concept-only")),
                "{name} {source}: {fed:?}"
            );
        }
    }

    #[test]
    fn notices_are_generated_and_spliced() {
        let scratch = Scratch::new("notices");
        scratch.write("models/a.glb", "");
        scratch.write("models/provenance.toml", ORIGINAL);
        scratch.write("textures/rock.ktx2", "");
        scratch.write(
            "textures/provenance.toml",
            &format!(
                "[[asset]]\nfiles = [\"rock.ktx2\"]\ntitle = \"Rock | 042\"\nauthor = \"B\"\n{AI}\
                 [[asset.ai.inputs]]\nname = \"photo\"\nsource = \"https://example.org/p\"\n\
                 licence = \"CC0-1.0\"\n"
            )
            .replace("[ai]", "[asset.ai]"),
        );
        let report = check_provenance(&scratch.0);
        assert_eq!(report.errors, Vec::<String>::new());
        let section = third_party_section(&report, "content");
        assert!(
            section.starts_with(THIRD_PARTY_BEGIN)
                && section.ends_with(&format!("{THIRD_PARTY_END}\n"))
        );
        assert!(section.contains("| `content/models/a.glb` (1) | Sample meshes | original |"));
        assert!(section.contains("Rock \\| 042 (AI-generated)"), "{section}");
        // Splicing replaces the old section and keeps the hand-written text around it.
        let spliced = splice_third_party("# Notices\n\nHand-written.\n", &section).expect("append");
        assert_eq!(spliced, format!("# Notices\n\nHand-written.\n\n{section}"));
        let again =
            splice_third_party(&format!("{spliced}\n## After\n"), &section).expect("replace");
        assert_eq!(again, format!("{spliced}\n## After\n"));
        assert_eq!(splice_third_party("", &section), Ok(section.clone()));
        let credits = credits_text(&report);
        assert!(credits.contains(
            "Original content\n  Sample meshes - MOBA Engine contributors - MIT OR Apache-2.0\n"
        ));
        assert!(credits.contains("Third-party content\n  (none)\n"));
        assert!(credits.contains(
            "AI-generated content\n  Rock | 042 - B - CC0-1.0 - made with TRELLIS.2-4B 2025-12\n"
        ));
        let disclosure = ai_disclosure(&report, "content");
        assert!(disclosure.contains("1 of 2 manifest entries are AI-generated."));
        assert!(disclosure.contains("| `content/textures/rock.ktx2` | Rock \\| 042 | TRELLIS.2-4B | 2025-12 | MIT | CC0-1.0 | ComfyUI |"));
    }

    #[test]
    fn the_generated_section_needs_exactly_one_marker_pair_in_order() {
        let section = format!("{THIRD_PARTY_BEGIN}\nnew\n{THIRD_PARTY_END}\n");
        let begin = THIRD_PARTY_BEGIN;
        let end = THIRD_PARTY_END;
        for broken in [
            format!("a\n{begin}\nold\n"),
            format!("a\n{end}\nold\n"),
            format!("a\n{end}\nold\n{begin}\n"),
            format!("{begin}\nx\n{end}\n{begin}\ny\n{end}\n"),
            format!("{begin}\n{begin}\nx\n{end}\n"),
            format!("{begin}\nx\n{end}\n{end}\n"),
        ] {
            let error = splice_third_party(&broken, &section).expect_err(&broken);
            assert!(error.contains("expected one BEGIN then one END"), "{error}");
        }
        let reversed =
            splice_third_party(&format!("{end}\n{begin}\n"), &section).expect_err("reversed");
        assert!(reversed.contains("wrong order"), "{reversed}");
        let inline =
            splice_third_party(&format!("text {begin}\nx\n{end}\n"), &section).expect_err("inline");
        assert!(inline.contains("own lines"), "{inline}");
    }

    /// The repository's own content passes the gate and its committed notices are current
    /// (the same check as `content-check --provenance --check`, on every CI platform).
    #[test]
    fn shipped_content_passes_the_gate_and_its_notices_are_current() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let report = check_provenance(&repo.join("content"));
        assert_eq!(report.errors, Vec::<String>::new());
        #[cfg(feature = "glb")]
        assert_eq!(
            baked_tangent_errors(&repo.join("content"), &report),
            Vec::<String>::new(),
            "every shipped .glb with UVs carries baked tangents"
        );
        assert!(report.files > 0 && !report.entries.is_empty());
        let read = |path: &str| {
            std::fs::read_to_string(repo.join(path))
                .expect("committed notice")
                .replace("\r\n", "\n")
        };
        let stale = "stale; run `cargo run -p moba-tools --bin content-check -- --provenance`";
        let third_party = read("THIRD_PARTY.md");
        assert_eq!(
            splice_third_party(&third_party, &third_party_section(&report, "content")),
            Ok(third_party.clone()),
            "THIRD_PARTY.md is {stale}"
        );
        assert_eq!(
            read("content/CREDITS.txt"),
            credits_text(&report),
            "CREDITS.txt is {stale}"
        );
        assert_eq!(
            read("content/AI-DISCLOSURE.md"),
            ai_disclosure(&report, "content"),
            "AI-DISCLOSURE.md is {stale}"
        );
    }
}

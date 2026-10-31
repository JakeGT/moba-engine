//! Asset provenance and the licence gate (A0, ADR-0022).
//!
//! Every file in a content root's asset directories (any directory below the root; the pack
//! files at the root itself are data, not assets) is covered by exactly one entry of the
//! [`PROVENANCE_FILE`] manifest in its own directory:
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
//!
//! # [asset.ai]                               # only for AI-generated assets
//! # model = "TRELLIS.2-4B"
//! # model_version = "2025-12"
//! # weights_licence = "MIT"
//! # seed = 1234
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
//! and tools, with the roadmap's rules (the asset is CC0 or CC-BY, and third-party inputs
//! are CC0). [`third_party_section`], [`credits_text`] and [`ai_disclosure`] render the
//! notices `content-check --provenance` writes; they are deterministic, so `--check` can
//! compare them with the committed files.
//!
//! Provenance is cosmetic metadata: it is never hashed, signed or sent, and the client
//! never reads it.

use std::fmt::Write as _;
use std::path::Path;

use serde::Deserialize;

/// The manifest's file name in each asset directory. `provenance` is therefore a reserved
/// asset name (no effect, map visuals or environment may be called that).
pub const PROVENANCE_FILE: &str = "provenance.toml";

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

/// A licence the maintainer approved for specific sources although it is not on
/// [`ALLOWED_LICENCES`] (roadmap §9.2, §9.4: approving exceptions is a human step, so adding
/// one is a reviewed code change).
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
}

/// The approved exceptions (next-gen roadmap §9.2).
pub const LICENCE_EXCEPTIONS: &[LicenceException] = &[
    LicenceException {
        id: "LicenseRef-CMU-Mocap",
        name: "CMU Graphics Lab Motion Capture Database terms",
        url: "http://mocap.cs.cmu.edu/faqs.php",
        approved: "animation source, next-gen roadmap §9.2 (2026-10-30)",
    },
    LicenceException {
        id: "LicenseRef-NVIDIA-Open-Model-License",
        name: "NVIDIA Open Model License",
        url: "https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/",
        approved: "Kimodo text-to-motion weights, next-gen roadmap §9.2-9.3 (2026-10-30)",
    },
];

/// `Ok` when `licence` is allowed or an approved exception, else why it is rejected.
///
/// # Errors
/// A human-readable reason.
pub fn licence_status(licence: &str) -> Result<(), String> {
    if ALLOWED_LICENCES.contains(&licence)
        || LICENCE_EXCEPTIONS
            .iter()
            .any(|exception| exception.id == licence)
    {
        return Ok(());
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

/// The policy checks of one entry, as error lines prefixed with `at`.
fn check_entry(at: &str, asset: &AssetProvenance, errors: &mut Vec<String>) {
    let mut fail = |message: String| errors.push(format!("{at}: {message}"));
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
    if let Err(reason) = licence_status(&asset.licence) {
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
    if asset.licence != "CC0-1.0" && !needs_attribution(&asset.licence) {
        fail(format!(
            "an AI-generated asset is licensed CC0-1.0 (or CC-BY when a person modified it), \
             not {}",
            asset.licence
        ));
    }
    for (field, value) in [
        ("ai.model", &ai.model),
        ("ai.model_version", &ai.model_version),
    ] {
        if value.trim().is_empty() {
            fail(format!("{field} is empty"));
        }
    }
    if let Err(reason) = licence_status(&ai.weights_licence) {
        fail(format!("ai.weights_licence: {reason}"));
    }
    if ai.prompt.trim().is_empty() && ai.inputs.is_empty() {
        fail(String::from("ai needs a prompt or at least one input"));
    }
    if ai.tools.is_empty() || ai.tools.iter().any(|tool| tool.trim().is_empty()) {
        fail(String::from("ai.tools must list every tool used"));
    }
    for input in &ai.inputs {
        let label = format!("ai input \"{}\"", input.name);
        if input.name.trim().is_empty() {
            fail(String::from("an ai input has no name"));
        }
        if !is_source(&input.source) {
            fail(format!(
                "{label}: source must be \"original\" or an http(s) URL"
            ));
        }
        if input.source == "original" {
            if let Err(reason) = licence_status(&input.licence) {
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
/// policy (see the module docs). Files directly in `root` are not assets and are skipped,
/// as are dot-files. Directories and files are visited in sorted order, so the report is
/// deterministic.
#[must_use]
pub fn check_provenance(root: &Path) -> ProvenanceReport {
    let mut report = ProvenanceReport::default();
    match sorted_entries(root) {
        Ok(entries) => {
            for (name, is_dir) in entries {
                if is_dir {
                    walk(root, &name, &mut report);
                }
            }
        }
        Err(error) => report.errors.push(format!("{}: {error}", root.display())),
    }
    report
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
    let files: Vec<String> = entries
        .iter()
        .filter(|(name, is_dir)| !is_dir && name != PROVENANCE_FILE)
        .map(|(name, _)| name.clone())
        .collect();
    check_dir(root, dir, &files, report);
    for (name, is_dir) in &entries {
        if *is_dir {
            walk(root, &format!("{dir}/{name}"), report);
        }
    }
}

/// `(name, is_dir)` of a directory's entries, sorted by name, dot-files skipped.
fn sorted_entries(dir: &Path) -> std::io::Result<Vec<(String, bool)>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        entries.push((name, entry.file_type()?.is_dir()));
    }
    entries.sort();
    Ok(entries)
}

fn check_dir(root: &Path, dir: &str, files: &[String], report: &mut ProvenanceReport) {
    report.files += files.len();
    let manifest_name = format!("{dir}/{PROVENANCE_FILE}");
    let path = root.join(dir).join(PROVENANCE_FILE);
    let manifest = match read_manifest(&path) {
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
            if pattern.is_empty()
                || pattern.contains(['/', '\\'])
                || pattern.starts_with('.')
                || pattern == PROVENANCE_FILE
            {
                report.errors.push(format!(
                    "{at}: \"{pattern}\" must be a file name or `*` pattern in {dir}/"
                ));
                continue;
            }
            let mut matched_any = false;
            for (file_index, file) in files.iter().enumerate() {
                if !glob_match(pattern, file) {
                    continue;
                }
                matched_any = true;
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
        check_entry(&at, &asset, &mut report.errors);
        let mut matched: Vec<String> = files
            .iter()
            .enumerate()
            .filter(|(file_index, _)| owner[*file_index] == Some(index))
            .map(|(_, file)| format!("{dir}/{file}"))
            .collect();
        matched.sort();
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

/// The parsed manifest, `None` when the file does not exist.
fn read_manifest(path: &Path) -> Result<Option<Manifest>, String> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.len() > MAX_PROVENANCE_BYTES {
        return Err(format!("larger than {MAX_PROVENANCE_BYTES} bytes"));
    }
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    toml::from_str(&text)
        .map(Some)
        .map_err(|error| error.to_string().trim_end().to_string())
}

/// The first line of every generated section of `THIRD_PARTY.md`.
pub const THIRD_PARTY_BEGIN: &str =
    "<!-- BEGIN content provenance: generated by `content-check --provenance`; do not edit -->";
/// The last line of the generated section.
pub const THIRD_PARTY_END: &str = "<!-- END content provenance -->";

/// Escapes text for a Markdown table cell.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
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
         them and writes this section, `{content}/CREDITS.txt` and \
         `{content}/AI-DISCLOSURE.md`. {} entries cover {} files.",
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
            .map(|pattern| format!("`{content}/{pattern}`"))
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

/// `existing` with its generated section replaced by `section` (or `section` appended after
/// a blank line when there is none yet; `section` alone for an empty file).
#[must_use]
pub fn splice_third_party(existing: &str, section: &str) -> String {
    if let (Some(begin), Some(end)) = (
        existing.find(THIRD_PARTY_BEGIN),
        existing.find(THIRD_PARTY_END),
    ) && begin < end
    {
        let mut after = end + THIRD_PARTY_END.len();
        if existing[after..].starts_with('\n') {
            after += 1;
        }
        return format!("{}{section}{}", &existing[..begin], &existing[after..]);
    }
    let mut out = existing.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(section);
    out
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
            .map(|pattern| format!("`{content}/{pattern}`"))
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
            assert_eq!(licence_status(allowed), Ok(()), "{allowed}");
        }
        for exception in [
            "LicenseRef-CMU-Mocap",
            "LicenseRef-NVIDIA-Open-Model-License",
        ] {
            assert_eq!(licence_status(exception), Ok(()), "{exception}");
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
            let error = licence_status(rejected).expect_err(rejected);
            assert!(error.contains(why), "{rejected}: {error}");
        }
        assert!(needs_attribution("CC-BY-4.0"));
        assert!(!needs_attribution("CC0-1.0"));
    }

    /// A scratch content root under the target directory, removed on drop.
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
files = ["*.glb"]
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
        scratch.write("models/.keep", "");
        // No manifest: both files are reported.
        let found = errors(&scratch);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("no provenance.toml") && found[0].contains("a.glb, b.glb"));
        // Covered: clean, and the report lists the files.
        scratch.write("models/provenance.toml", ORIGINAL);
        let report = check_provenance(&scratch.0);
        assert_eq!(report.errors, Vec::<String>::new());
        assert_eq!(report.files, 2);
        assert_eq!(report.entries[0].files, ["models/a.glb", "models/b.glb"]);
        // An uncovered file, a stale pattern and a doubly covered file.
        scratch.write("models/c.lod", "");
        scratch.write(
            "models/provenance.toml",
            &format!(
                "{ORIGINAL}\n[[asset]]\nfiles = [\"a.glb\", \"gone.glb\"]\ntitle = \"A\"\n\
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
        scratch.write("models/provenance.toml", &format!("{ORIGINAL}\n[[asset]]\nfiles = [\"c.lod\"]\ntitle = \"C\"\nsource = \"original\"\nauthor = \"x\"\nlicence = \"CC0-1.0\"\n"));
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

    fn entry_errors(extra: &str) -> Vec<String> {
        let asset: AssetProvenance = toml::from_str(&format!(
            "files = [\"x\"]\ntitle = \"T\"\nauthor = \"A\"\n{extra}"
        ))
        .expect("entry parses");
        let mut errors = Vec::new();
        check_entry("at", &asset, &mut errors);
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
        assert!(section.contains("| `content/models/*.glb` (1) | Sample meshes | original |"));
        assert!(section.contains("Rock \\| 042 (AI-generated)"), "{section}");
        // Splicing replaces the old section and keeps the hand-written text around it.
        let spliced = splice_third_party("# Notices\n\nHand-written.\n", &section);
        assert_eq!(spliced, format!("# Notices\n\nHand-written.\n\n{section}"));
        let again = splice_third_party(&format!("{spliced}\n## After\n"), &section);
        assert_eq!(again, format!("{spliced}\n## After\n"));
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

    /// The repository's own content passes the gate and its committed notices are current
    /// (the same check as `content-check --provenance --check`, on every CI platform).
    #[test]
    fn shipped_content_passes_the_gate_and_its_notices_are_current() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let report = check_provenance(&repo.join("content"));
        assert_eq!(report.errors, Vec::<String>::new());
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
            third_party,
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

//! Colour-grading 3D LUTs in the Adobe/Resolve `.cube` text format (R6 post-processing).
//!
//! A map visuals descriptor's `[post]` table names a LUT; on each client it resolves to
//! `<asset root>/grading/<name>.cube` ([`grading_path`]). The LUT maps **display-encoded** RGB
//! (the tonemapper's `AgX` output, roughly a 2.2 gamma) to display-encoded RGB, so it is applied
//! after the tonemap and before the swapchain's encode.
//!
//! ```text
//! # comments and blank lines are ignored
//! TITLE "ember"            # optional, ignored
//! LUT_3D_SIZE 17           # required, exactly once, 2..=64, before any data
//! DOMAIN_MIN 0 0 0         # optional; only the default 0 0 0 is accepted
//! DOMAIN_MAX 1 1 1         # optional; only the default 1 1 1 is accepted
//! 0.000000 0.000000 0.000000
//! 0.062500 0.000000 0.000000
//! ...                      # exactly size³ lines of three floats, red fastest
//! ```
//!
//! **Supported subset.** Only 3D LUTs over the default `0..1` input domain: `LUT_1D_SIZE`, a
//! non-default `DOMAIN_MIN`/`DOMAIN_MAX` and any other keyword are errors, as is a keyword
//! after the first data line. Data values must be finite and within `-0.5..=1.5` (a little
//! slack for tools that overshoot); they are stored clamped to `0..=1`. Texels are ordered
//! red fastest, then green, then blue: texel `(r, g, b)` is line `r + g·n + b·n²` of the
//! data, as in the format's specification.
//!
//! LUTs are untrusted, client-local and cosmetic: they never reach the simulation, the wire
//! or the content hash. [`GradingLut::load_file`] reads at most [`MAX_GRADING_LUT_BYTES`]; the
//! parser never panics, and reserves texel storage only after `LUT_3D_SIZE` passed its bounds
//! (at most 64³ texels, 3 MiB), so a hostile file can only fail to load.

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::map_visuals::valid_mesh_name;

/// Directory under the asset root that holds grading LUTs.
pub const GRADING_DIR: &str = "grading";
/// Largest `.cube` file accepted, in bytes: a 64³ LUT written with six decimals
/// ([`GradingLut::to_cube_string`], 27 bytes a line) is about 7 MB.
pub const MAX_GRADING_LUT_BYTES: u64 = 8 * 1024 * 1024;
/// Smallest accepted `LUT_3D_SIZE`.
pub const MIN_GRADING_LUT_SIZE: u32 = 2;
/// Largest accepted `LUT_3D_SIZE`.
pub const MAX_GRADING_LUT_SIZE: u32 = 64;
/// Accepted data values before clamping to `0..=1`.
const VALUE_RANGE: (f32, f32) = (-0.5, 1.5);

/// A validated 3D colour-grading LUT.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct GradingLut {
    size: u32,
    texels: Vec<[f32; 3]>,
}

/// Why a `.cube` LUT could not be used. `line` is 1-based in the parsed text, or 0 for
/// [`GradingLut::from_texels`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GradingLutError {
    /// The file does not exist or cannot be read (or is not UTF-8); the text is the I/O error.
    Io(String),
    /// The path is not a regular file.
    NotAFile,
    /// The text is larger than [`MAX_GRADING_LUT_BYTES`].
    TooLarge,
    /// No `LUT_3D_SIZE` precedes the data (or there is none at all).
    MissingSize,
    /// A second `LUT_3D_SIZE`.
    DuplicateSize {
        /// Line of the second keyword.
        line: usize,
    },
    /// `LUT_3D_SIZE` is not an integer in
    /// [`MIN_GRADING_LUT_SIZE`]`..=`[`MAX_GRADING_LUT_SIZE`].
    BadSize {
        /// Line of the keyword.
        line: usize,
    },
    /// A malformed line (not three numbers, or a keyword with the wrong arguments).
    BadLine {
        /// The line.
        line: usize,
    },
    /// A data value that is not finite or outside `-0.5..=1.5`.
    OutOfRange {
        /// The line (for [`GradingLut::from_texels`], the 1-based texel index).
        line: usize,
    },
    /// The number of data lines is not `size³`.
    WrongCount {
        /// `size³`.
        expected: usize,
        /// Data lines found.
        found: usize,
    },
    /// A `DOMAIN_MIN`/`DOMAIN_MAX` other than the default `0 0 0`/`1 1 1`.
    UnsupportedDomain {
        /// The line.
        line: usize,
    },
    /// A `LUT_1D_SIZE`: 1D LUTs are not supported.
    Unsupported1d {
        /// The line.
        line: usize,
    },
    /// A keyword this parser does not know.
    UnknownKeyword {
        /// The line.
        line: usize,
    },
    /// A keyword after the first data line.
    KeywordAfterData {
        /// The line.
        line: usize,
    },
}

impl core::fmt::Display for GradingLutError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "missing or unreadable ({error})"),
            Self::NotAFile => f.write_str("not a regular file"),
            Self::TooLarge => write!(f, "larger than {MAX_GRADING_LUT_BYTES} bytes"),
            Self::MissingSize => f.write_str("no LUT_3D_SIZE before the data"),
            Self::DuplicateSize { line } => write!(f, "line {line}: second LUT_3D_SIZE"),
            Self::BadSize { line } => write!(
                f,
                "line {line}: LUT_3D_SIZE must be an integer in \
                 {MIN_GRADING_LUT_SIZE}..={MAX_GRADING_LUT_SIZE}"
            ),
            Self::BadLine { line } => write!(f, "line {line}: malformed line"),
            Self::OutOfRange { line } => {
                write!(f, "line {line}: values must be finite and in -0.5..=1.5")
            }
            Self::WrongCount { expected, found } => {
                write!(f, "expected {expected} data lines, found {found}")
            }
            Self::UnsupportedDomain { line } => write!(
                f,
                "line {line}: only the default domain (0 0 0 to 1 1 1) is supported"
            ),
            Self::Unsupported1d { line } => {
                write!(f, "line {line}: 1D LUTs (LUT_1D_SIZE) are not supported")
            }
            Self::UnknownKeyword { line } => write!(f, "line {line}: unknown keyword"),
            Self::KeywordAfterData { line } => {
                write!(f, "line {line}: keywords must precede the data")
            }
        }
    }
}

impl std::error::Error for GradingLutError {}

/// `<root>/grading/<name>.cube`, or `None` for a name that is not 1-32 of `[A-Za-z0-9_-]`
/// (so it can never leave the directory).
#[must_use]
pub fn grading_path(root: &Path, name: &str) -> Option<PathBuf> {
    valid_mesh_name(name).then(|| root.join(GRADING_DIR).join(format!("{name}.cube")))
}

/// Parses exactly three numbers (no more tokens), or `None`.
fn three_numbers<'a>(mut tokens: impl Iterator<Item = &'a str>) -> Option<[f32; 3]> {
    let mut out = [0.0f32; 3];
    for value in &mut out {
        *value = tokens.next()?.parse().ok()?;
    }
    tokens.next().is_none().then_some(out)
}

fn check_value(value: f32) -> Option<f32> {
    (value.is_finite() && (VALUE_RANGE.0..=VALUE_RANGE.1).contains(&value))
        .then(|| value.clamp(0.0, 1.0))
}

/// `size³` if `size` is within bounds.
fn texel_count(size: u32) -> Option<usize> {
    (MIN_GRADING_LUT_SIZE..=MAX_GRADING_LUT_SIZE)
        .contains(&size)
        .then(|| (size as usize).pow(3))
}

impl GradingLut {
    /// The identity LUT with `size` points per axis (clamped to
    /// [`MIN_GRADING_LUT_SIZE`]`..=`[`MAX_GRADING_LUT_SIZE`]).
    #[must_use]
    pub fn identity(size: u32) -> Self {
        let size = size.clamp(MIN_GRADING_LUT_SIZE, MAX_GRADING_LUT_SIZE);
        let n = size as usize;
        let scale = 1.0 / (size - 1) as f32;
        let mut texels = Vec::with_capacity(n * n * n);
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    texels.push([r as f32 * scale, g as f32 * scale, b as f32 * scale]);
                }
            }
        }
        Self { size, texels }
    }

    /// A LUT from `size³` texels in `.cube` order (red fastest). Values follow the parser's
    /// rule: finite and within `-0.5..=1.5`, stored clamped to `0..=1`.
    ///
    /// # Errors
    /// [`GradingLutError::BadSize`] (line 0), [`GradingLutError::WrongCount`] or
    /// [`GradingLutError::OutOfRange`] (line = 1-based texel index).
    pub fn from_texels(size: u32, mut texels: Vec<[f32; 3]>) -> Result<Self, GradingLutError> {
        let expected = texel_count(size).ok_or(GradingLutError::BadSize { line: 0 })?;
        if texels.len() != expected {
            return Err(GradingLutError::WrongCount {
                expected,
                found: texels.len(),
            });
        }
        for (index, texel) in texels.iter_mut().enumerate() {
            for value in &mut *texel {
                *value =
                    check_value(*value).ok_or(GradingLutError::OutOfRange { line: index + 1 })?;
            }
        }
        Ok(Self { size, texels })
    }

    /// Parses and validates `.cube` text (see the module docs for the accepted subset).
    ///
    /// # Errors
    /// [`GradingLutError::TooLarge`] or the first problem found, with its line.
    pub fn parse(text: &str) -> Result<Self, GradingLutError> {
        if text.len() as u64 > MAX_GRADING_LUT_BYTES {
            return Err(GradingLutError::TooLarge);
        }
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut size = None;
        let mut expected = 0usize;
        let mut found = 0usize;
        let mut texels: Vec<[f32; 3]> = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line_no = index + 1;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let mut tokens = trimmed.split_ascii_whitespace();
            let first = tokens.next().unwrap_or_default();
            let keyword = first.starts_with(|c: char| c.is_ascii_alphabetic())
                && first.parse::<f32>().is_err();
            if keyword && found > 0 {
                return Err(GradingLutError::KeywordAfterData { line: line_no });
            }
            match first {
                "TITLE" => {}
                "LUT_3D_SIZE" => {
                    if size.is_some() {
                        return Err(GradingLutError::DuplicateSize { line: line_no });
                    }
                    let bad = GradingLutError::BadSize { line: line_no };
                    let (Some(Ok(value)), None) =
                        (tokens.next().map(str::parse::<u32>), tokens.next())
                    else {
                        return Err(bad);
                    };
                    expected = texel_count(value).ok_or(bad)?;
                    // Bounded above: at most 64³ texels.
                    texels.reserve_exact(expected);
                    size = Some(value);
                }
                "DOMAIN_MIN" | "DOMAIN_MAX" => {
                    let domain =
                        three_numbers(tokens).ok_or(GradingLutError::BadLine { line: line_no })?;
                    // The text must say exactly the default (0 or 1, any spelling).
                    let default = if first == "DOMAIN_MIN" { 0.0 } else { 1.0 };
                    if domain
                        .iter()
                        .any(|&value| !value.is_finite() || (value - default).abs() > 0.0)
                    {
                        return Err(GradingLutError::UnsupportedDomain { line: line_no });
                    }
                }
                "LUT_1D_SIZE" => return Err(GradingLutError::Unsupported1d { line: line_no }),
                _ if keyword => return Err(GradingLutError::UnknownKeyword { line: line_no }),
                _ => {
                    if size.is_none() {
                        return Err(GradingLutError::MissingSize);
                    }
                    let values = three_numbers(trimmed.split_ascii_whitespace())
                        .ok_or(GradingLutError::BadLine { line: line_no })?;
                    let mut texel = [0.0f32; 3];
                    for (out, value) in texel.iter_mut().zip(values) {
                        *out = check_value(value)
                            .ok_or(GradingLutError::OutOfRange { line: line_no })?;
                    }
                    found += 1;
                    if found <= expected {
                        texels.push(texel);
                    }
                }
            }
        }
        let size = size.ok_or(GradingLutError::MissingSize)?;
        if found != expected {
            return Err(GradingLutError::WrongCount { expected, found });
        }
        Ok(Self { size, texels })
    }

    /// Reads and parses a `.cube` file, reading at most [`MAX_GRADING_LUT_BYTES`] + 1 bytes
    /// whatever the file claims or grows to.
    ///
    /// # Errors
    /// An I/O, size or parse error.
    pub fn load_file(path: &Path) -> Result<Self, GradingLutError> {
        let file =
            std::fs::File::open(path).map_err(|error| GradingLutError::Io(error.to_string()))?;
        let metadata = file
            .metadata()
            .map_err(|error| GradingLutError::Io(error.to_string()))?;
        if !metadata.is_file() {
            return Err(GradingLutError::NotAFile);
        }
        if metadata.len() > MAX_GRADING_LUT_BYTES {
            return Err(GradingLutError::TooLarge);
        }
        let mut text = String::new();
        file.take(MAX_GRADING_LUT_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|error| GradingLutError::Io(error.to_string()))?;
        Self::parse(&text)
    }

    /// Points per axis.
    #[must_use]
    pub fn size(&self) -> u32 {
        self.size
    }

    /// The `size³` texels, red fastest: texel `(r, g, b)` is at `r + g·size + b·size²`.
    #[must_use]
    pub fn texels(&self) -> &[[f32; 3]] {
        &self.texels
    }

    /// Trilinear lookup of display-encoded `rgb` (each channel clamped to `0..=1`, NaN to 0);
    /// the result is in `0..=1`.
    #[must_use]
    pub fn sample(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n = self.size as usize;
        let max = (n - 1) as f32;
        let axis = |value: f32| {
            let value = if value.is_nan() {
                0.0
            } else {
                value.clamp(0.0, 1.0)
            };
            let position = value * max;
            let low = (position.floor() as usize).min(n - 2);
            (low, position - low as f32)
        };
        let (r0, fr) = axis(rgb[0]);
        let (g0, fg) = axis(rgb[1]);
        let (b0, fb) = axis(rgb[2]);
        let at = |r: usize, g: usize, b: usize| self.texels[r + g * n + b * n * n];
        let lerp = |a: [f32; 3], b: [f32; 3], t: f32| {
            [
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ]
        };
        let plane = |b: usize| {
            lerp(
                lerp(at(r0, g0, b), at(r0 + 1, g0, b), fr),
                lerp(at(r0, g0 + 1, b), at(r0 + 1, g0 + 1, b), fr),
                fg,
            )
        };
        // Rounding in the lerps can step a hair outside 0..=1.
        lerp(plane(b0), plane(b0 + 1), fb).map(|value| value.clamp(0.0, 1.0))
    }

    /// Writes the LUT as `.cube` text: `TITLE` (quotes and control characters in `title`
    /// replaced by spaces), `LUT_3D_SIZE`, the default domain and six-decimal data. The text
    /// parses back to the same LUT within 5e-7 per value.
    #[must_use]
    pub fn to_cube_string(&self, title: &str) -> String {
        use core::fmt::Write as _;
        let title: String = title
            .chars()
            .map(|c| if c == '"' || c.is_control() { ' ' } else { c })
            .collect();
        let mut text = String::with_capacity(self.texels.len() * 27 + 128);
        let _ = writeln!(text, "TITLE \"{title}\"");
        let _ = writeln!(text, "LUT_3D_SIZE {}", self.size);
        text.push_str("DOMAIN_MIN 0.0 0.0 0.0\nDOMAIN_MAX 1.0 1.0 1.0\n");
        for [r, g, b] in &self.texels {
            let _ = writeln!(text, "{r:.6} {g:.6} {b:.6}");
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3], tolerance: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tolerance)
    }

    const HEADER: &str = "LUT_3D_SIZE 2\n";
    const IDENTITY_2: &str = "0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";

    #[test]
    fn identity_round_trips_and_samples_exactly() {
        for size in [2, 3, 17, 33] {
            let lut = GradingLut::identity(size);
            assert_eq!(lut.size(), size);
            assert_eq!(lut.texels().len(), (size as usize).pow(3));
            let parsed = GradingLut::parse(&lut.to_cube_string("id \"x\"\n")).expect("parses");
            assert_eq!(parsed.size(), size);
            for (a, b) in parsed.texels().iter().zip(lut.texels()) {
                assert!(close(*a, *b, 5e-7), "{a:?} {b:?}");
            }
            for rgb in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.3, 0.71, 0.05]] {
                assert!(close(lut.sample(rgb), rgb, 1e-5), "{size} {rgb:?}");
            }
        }
        assert_eq!(GradingLut::identity(0).size(), MIN_GRADING_LUT_SIZE);
        assert_eq!(GradingLut::identity(1000).size(), MAX_GRADING_LUT_SIZE);
        let text = GradingLut::identity(64).to_cube_string("big");
        assert!(text.len() as u64 <= MAX_GRADING_LUT_BYTES);
        assert_eq!(GradingLut::parse(&text).map(|lut| lut.size()), Ok(64));
    }

    #[test]
    fn red_varies_fastest_and_sampling_interpolates() {
        // An inverting LUT: texel (r, g, b) = 1 - lattice point; plus a marker in green.
        let mut texels = GradingLut::identity(3).texels().to_vec();
        for texel in &mut texels {
            *texel = texel.map(|value| 1.0 - value);
        }
        // Index r + g·3 + b·9 = (1, 0, 0) -> 1.
        texels[1] = [0.25, 0.5, 0.75];
        let lut = GradingLut::from_texels(3, texels).expect("valid");
        assert_eq!(lut.sample([0.5, 0.0, 0.0]), [0.25, 0.5, 0.75]);
        assert_eq!(lut.sample([0.0, 0.0, 0.0]), [1.0, 1.0, 1.0]);
        assert_eq!(lut.sample([1.0, 1.0, 1.0]), [0.0, 0.0, 0.0]);
        // Halfway between lattice points (0,1,2) and (0,1,2)+1 in blue: the mean.
        let mid = lut.sample([0.0, 0.5, 0.25]);
        assert!(close(mid, [1.0, 0.5, 0.75], 1e-6), "{mid:?}");
        // Clamped and NaN-safe input.
        assert_eq!(lut.sample([-3.0, f32::NAN, -1.0]), [1.0, 1.0, 1.0]);
        assert_eq!(lut.sample([9.0, 9.0, f32::INFINITY]), [0.0, 0.0, 0.0]);
        // The parser reads the same order.
        let parsed = GradingLut::parse(&format!(
            "{HEADER}0 0 0\n0.5 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n"
        ))
        .expect("parses");
        assert_eq!(parsed.sample([1.0, 0.0, 0.0]), [0.5, 0.0, 0.0]);
        assert_eq!(parsed.sample([0.5, 0.0, 0.0]), [0.25, 0.0, 0.0]);
    }

    #[test]
    fn accepts_the_format_subset() {
        let text = format!(
            "\u{feff}# made by hand\r\nTITLE \"two\"\r\n\r\n  LUT_3D_SIZE 2  \nDOMAIN_MIN 0 0 0\n\
             DOMAIN_MAX 1.0 1 1\n{}",
            IDENTITY_2
                .replace("1 1 1", "1.2 1e0 +1")
                .replace("0 0 0", "-0.4 0 0")
        );
        let lut = GradingLut::parse(&text).expect("parses");
        assert_eq!(lut.texels()[0], [0.0, 0.0, 0.0]);
        assert_eq!(lut.texels()[7], [1.0, 1.0, 1.0]);
        assert_eq!(lut, GradingLut::identity(2));
        // Comments between data lines are fine.
        assert!(GradingLut::parse(&format!("{HEADER}0 0 0\n# mid\n{}", &IDENTITY_2[6..])).is_ok());
    }

    #[test]
    fn rejects_every_bad_input() {
        use GradingLutError as E;
        let cases: Vec<(String, GradingLutError)> = vec![
            (String::new(), E::MissingSize),
            (String::from("# only a comment\n"), E::MissingSize),
            (IDENTITY_2.to_owned(), E::MissingSize),
            (
                format!("LUT_3D_SIZE 1\n{IDENTITY_2}"),
                E::BadSize { line: 1 },
            ),
            (String::from("LUT_3D_SIZE 65\n"), E::BadSize { line: 1 }),
            (String::from("LUT_3D_SIZE -2\n"), E::BadSize { line: 1 }),
            (String::from("LUT_3D_SIZE 2.5\n"), E::BadSize { line: 1 }),
            (String::from("LUT_3D_SIZE\n"), E::BadSize { line: 1 }),
            (String::from("LUT_3D_SIZE 2 2\n"), E::BadSize { line: 1 }),
            (
                format!("{HEADER}LUT_3D_SIZE 2\n{IDENTITY_2}"),
                E::DuplicateSize { line: 2 },
            ),
            (
                format!("{HEADER}{IDENTITY_2}LUT_3D_SIZE 2\n"),
                E::KeywordAfterData { line: 10 },
            ),
            (
                format!("{HEADER}0 0 0\nTITLE \"late\"\n"),
                E::KeywordAfterData { line: 3 },
            ),
            (
                format!("LUT_1D_SIZE 16\n{IDENTITY_2}"),
                E::Unsupported1d { line: 1 },
            ),
            (
                format!("{HEADER}DOMAIN_MIN -1 0 0\n{IDENTITY_2}"),
                E::UnsupportedDomain { line: 2 },
            ),
            (
                format!("{HEADER}DOMAIN_MAX 1 1 2\n{IDENTITY_2}"),
                E::UnsupportedDomain { line: 2 },
            ),
            (
                format!("{HEADER}DOMAIN_MAX 1 1\n{IDENTITY_2}"),
                E::BadLine { line: 2 },
            ),
            (
                format!("{HEADER}LUT_3D_INPUT_RANGE 0 1\n{IDENTITY_2}"),
                E::UnknownKeyword { line: 2 },
            ),
            (format!("{HEADER}0 0\n"), E::BadLine { line: 2 }),
            (format!("{HEADER}0 0 0 0\n"), E::BadLine { line: 2 }),
            (format!("{HEADER}0 0 x\n"), E::BadLine { line: 2 }),
            (format!("{HEADER}0,0,0\n"), E::BadLine { line: 2 }),
            (format!("{HEADER}0 0 1.6\n"), E::OutOfRange { line: 2 }),
            (format!("{HEADER}-0.6 0 0\n"), E::OutOfRange { line: 2 }),
            (format!("{HEADER}nan 0 0\n"), E::OutOfRange { line: 2 }),
            (format!("{HEADER}0 inf 0\n"), E::OutOfRange { line: 2 }),
            (
                format!("{HEADER}0 0 0\n"),
                E::WrongCount {
                    expected: 8,
                    found: 1,
                },
            ),
            (
                format!("{HEADER}{IDENTITY_2}1 1 1\n"),
                E::WrongCount {
                    expected: 8,
                    found: 9,
                },
            ),
            (
                format!("#{}", "x".repeat(MAX_GRADING_LUT_BYTES as usize)),
                E::TooLarge,
            ),
        ];
        for (text, error) in cases {
            let shown: String = text.chars().take(60).collect();
            assert_eq!(GradingLut::parse(&text), Err(error), "{shown:?}");
        }
    }

    #[test]
    fn hostile_sizes_never_allocate_or_panic() {
        // Rejected before any reservation: a naive parser would try to reserve 4e9³ texels.
        for size in [
            "4000000000",
            "18446744073709551616",
            "99999999999999999999999",
        ] {
            assert_eq!(
                GradingLut::parse(&format!("LUT_3D_SIZE {size}\n0 0 0\n")),
                Err(GradingLutError::BadSize { line: 1 })
            );
        }
        // The largest legal header with no data fails on the count.
        assert_eq!(
            GradingLut::parse("LUT_3D_SIZE 64\n"),
            Err(GradingLutError::WrongCount {
                expected: 64 * 64 * 64,
                found: 0,
            })
        );
        // Millions of extra data lines are counted, not stored.
        let many = format!("{HEADER}{}", "0 0 0\n".repeat(100_000));
        assert!(matches!(
            GradingLut::parse(&many),
            Err(GradingLutError::WrongCount { found: 100_000, .. })
        ));
        for junk in [
            "\0\0\0",
            "LUT_3D_SIZE \u{1F600}",
            "TITLE",
            "\u{feff}",
            "#\n\n\r\n",
        ] {
            assert!(GradingLut::parse(junk).is_err(), "{junk:?}");
        }
    }

    #[test]
    fn from_texels_validates() {
        assert_eq!(
            GradingLut::from_texels(1, vec![[0.0; 3]]),
            Err(GradingLutError::BadSize { line: 0 })
        );
        assert_eq!(
            GradingLut::from_texels(2, vec![[0.0; 3]; 7]),
            Err(GradingLutError::WrongCount {
                expected: 8,
                found: 7,
            })
        );
        let mut texels = vec![[0.5; 3]; 8];
        texels[3] = [0.0, f32::NAN, 0.0];
        assert_eq!(
            GradingLut::from_texels(2, texels),
            Err(GradingLutError::OutOfRange { line: 4 })
        );
        let lut = GradingLut::from_texels(2, vec![[1.2, -0.2, 0.5]; 8]).expect("clamped");
        assert_eq!(lut.texels()[0], [1.0, 0.0, 0.5]);
    }

    #[test]
    fn paths_and_files_are_bounded() {
        let root = Path::new("assets");
        assert_eq!(
            grading_path(root, "ember"),
            Some(root.join("grading").join("ember.cube"))
        );
        for bad in ["", "../x", "a/b", "a.cube", "c:x"] {
            assert_eq!(grading_path(root, bad), None, "{bad}");
        }
        let dir = std::env::temp_dir().join(format!(
            "moba-grading-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("dir.cube")).expect("temp dir");
        std::fs::write(
            dir.join("ok.cube"),
            GradingLut::identity(2).to_cube_string("ok"),
        )
        .expect("write");
        std::fs::write(dir.join("binary.cube"), [0xff, 0xfe, 0x00]).expect("write");
        let big = std::fs::File::create(dir.join("big.cube")).expect("create");
        big.set_len(MAX_GRADING_LUT_BYTES + 1).expect("sparse");
        assert_eq!(
            GradingLut::load_file(&dir.join("ok.cube")),
            Ok(GradingLut::identity(2))
        );
        assert_eq!(
            GradingLut::load_file(&dir.join("big.cube")),
            Err(GradingLutError::TooLarge)
        );
        // NotAFile on Unix; opening a directory already fails on Windows.
        assert!(GradingLut::load_file(&dir.join("dir.cube")).is_err());
        assert!(matches!(
            GradingLut::load_file(&dir.join("binary.cube")),
            Err(GradingLutError::Io(_))
        ));
        assert!(matches!(
            GradingLut::load_file(&dir.join("missing.cube")),
            Err(GradingLutError::Io(_))
        ));
        let _ = std::fs::remove_dir_all(dir);
    }
}

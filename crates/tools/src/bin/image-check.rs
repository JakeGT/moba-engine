//! Compare a rendered PPM against a checked-in downscaled fingerprint, or (`--against`)
//! against another full-resolution capture of the same frame: at least `--min-changed`
//! pixels must differ and every `--same-box` must be byte-identical (ADR-0019: the heat haze
//! is drawn, and the units' pixels are never touched by it). With `--parity` the two
//! captures must instead agree: at least 99.5 % of the pixels within 2/255 on every channel
//! and none past 6/255 (ADR-0023 gate 2, the deferred–forward parity).
#![allow(clippy::print_stdout)]

use std::path::Path;
use std::process::ExitCode;

const FINGERPRINT_WIDTH: usize = 64;
const FINGERPRINT_HEIGHT: usize = 36;
const USAGE: &str = "usage: image-check <actual.ppm> --expected <fingerprint.rgb> [--write] [--diff <out.ppm>]\n       image-check <actual.ppm> --against <other.ppm> [--min-changed <pixels>] [--same-box <x0,y0,x1,y1>]... [--diff <out.ppm>]\n       image-check <actual.ppm> --against <other.ppm> --parity [--diff <out.ppm>]";

/// ADR-0023 gate 2: the per-channel difference most pixels stay within, the share of pixels
/// that must (per mille), and the difference no pixel may exceed.
const PARITY_CLOSE: u8 = 2;
const PARITY_CLOSE_PER_MILLE: usize = 995;
const PARITY_MAX: u8 = 6;

/// Per-pixel largest channel difference, as a histogram over 0..=255.
fn difference_histogram(actual: &[u8], other: &[u8]) -> [usize; 256] {
    let mut histogram = [0usize; 256];
    let (actual, _) = actual.as_chunks::<3>();
    let (other, _) = other.as_chunks::<3>();
    for (a, b) in actual.iter().zip(other) {
        let delta = (0..3).map(|channel| a[channel].abs_diff(b[channel])).max();
        histogram[usize::from(delta.unwrap_or(0))] += 1;
    }
    histogram
}

/// Whether `histogram` passes gate 2: the close pixels and the largest difference.
fn parity(histogram: &[usize; 256]) -> (usize, u8, bool) {
    let total: usize = histogram.iter().sum();
    let close: usize = histogram[..=usize::from(PARITY_CLOSE)].iter().sum();
    let max = histogram
        .iter()
        .rposition(|&count| count > 0)
        .map_or(0, |index| u8::try_from(index).unwrap_or(u8::MAX));
    let ok = close * 1000 >= total * PARITY_CLOSE_PER_MILLE && max <= PARITY_MAX;
    (close, max, ok)
}

/// A pixel box `[x0, x1) x [y0, y1)`.
type PixelBox = [usize; 4];

fn parse_box(text: &str) -> Option<PixelBox> {
    let mut parts = text
        .split(',')
        .map(|part| part.trim().parse::<usize>().ok());
    let value = [
        parts.next()??,
        parts.next()??,
        parts.next()??,
        parts.next()??,
    ];
    (parts.next().is_none() && value[0] < value[2] && value[1] < value[3]).then_some(value)
}

/// Pixels that differ between two equal-sized RGB images, and per box whether any of its
/// pixels differs (a box past the image counts as differing).
fn compare_full(
    actual: &[u8],
    other: &[u8],
    width: usize,
    height: usize,
    boxes: &[PixelBox],
) -> (usize, Vec<usize>) {
    let differs =
        |index: usize| actual[index * 3..index * 3 + 3] != other[index * 3..index * 3 + 3];
    let changed = (0..width * height).filter(|&index| differs(index)).count();
    let in_box = boxes
        .iter()
        .map(|&[x0, y0, x1, y1]| {
            if x1 > width || y1 > height {
                return usize::MAX;
            }
            (y0..y1)
                .flat_map(|y| (x0..x1).map(move |x| y * width + x))
                .filter(|&index| differs(index))
                .count()
        })
        .collect();
    (changed, in_box)
}

/// `--against`: see the module docs.
fn against(
    actual_path: &str,
    other_path: &Path,
    min_changed: usize,
    boxes: &[PixelBox],
    diff_path: Option<&Path>,
    parity_mode: bool,
) -> ExitCode {
    let (Ok((width, height, actual)), Ok((other_width, other_height, other))) =
        (read_ppm(Path::new(actual_path)), read_ppm(other_path))
    else {
        eprintln!("cannot read {actual_path} or {}", other_path.display());
        return ExitCode::from(1);
    };
    if (width, height) != (other_width, other_height) {
        eprintln!("size mismatch: {width}x{height} vs {other_width}x{other_height}");
        return ExitCode::from(1);
    }
    if parity_mode {
        let histogram = difference_histogram(&actual, &other);
        let (close, max, ok) = parity(&histogram);
        let total = width * height;
        let tail: Vec<String> = histogram
            .iter()
            .enumerate()
            .skip(usize::from(PARITY_CLOSE) + 1)
            .filter(|&(_, &count)| count > 0)
            .map(|(delta, count)| format!("{delta}:{count}"))
            .collect();
        #[allow(clippy::cast_precision_loss)] // A percentage for the log.
        let percent = close as f64 * 100.0 / total.max(1) as f64;
        println!(
            "render parity: within_{PARITY_CLOSE}={percent:.3}% ({close} of {total}) max_diff={max} \
             over_{PARITY_CLOSE}=[{}]",
            tail.join(" ")
        );
        if let Some(path) = diff_path {
            let diff: Vec<u8> = actual
                .iter()
                .zip(&other)
                .map(|(a, b)| a.abs_diff(*b).saturating_mul(32))
                .collect();
            let _ = write_ppm(path, width, height, &diff);
        }
        if ok {
            return ExitCode::SUCCESS;
        }
        eprintln!(
            "render parity failed: fewer than 99.5% of pixels within {PARITY_CLOSE}/255, or one \
             past {PARITY_MAX}/255"
        );
        return ExitCode::from(1);
    }
    let (changed, in_box) = compare_full(&actual, &other, width, height, boxes);
    if let Some(path) = diff_path {
        let diff: Vec<u8> = actual
            .iter()
            .zip(&other)
            .map(|(a, b)| a.abs_diff(*b).saturating_mul(8))
            .collect();
        let _ = write_ppm(path, width, height, &diff);
    }
    println!(
        "render diff: changed_pixels={changed} of {} (min {min_changed})",
        width * height
    );
    let mut ok = changed >= min_changed;
    for (&[x0, y0, x1, y1], &count) in boxes.iter().zip(&in_box) {
        println!("same box {x0},{y0},{x1},{y1}: changed_pixels={count}");
        ok &= count == 0;
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        eprintln!("render diff check failed: too few pixels changed, or a same box changed");
        ExitCode::from(1)
    }
}

#[allow(clippy::too_many_lines)] // The argument parser and the fingerprint mode, in order.
fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(actual_path) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let mut expected_path = None;
    let mut write = false;
    let mut diff_path = None;
    let mut against_path = None;
    let mut min_changed = 1usize;
    let mut boxes = Vec::new();
    let mut parity_mode = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--expected" => expected_path = args.next().map(std::path::PathBuf::from),
            "--write" => write = true,
            "--parity" => parity_mode = true,
            "--diff" => diff_path = args.next().map(std::path::PathBuf::from),
            "--against" => against_path = args.next().map(std::path::PathBuf::from),
            "--min-changed" => {
                let Some(value) = args.next().and_then(|value| value.parse().ok()) else {
                    eprintln!("--min-changed needs a pixel count");
                    return ExitCode::from(2);
                };
                min_changed = value;
            }
            "--same-box" => {
                let Some(value) = args.next().as_deref().and_then(parse_box) else {
                    eprintln!("--same-box needs x0,y0,x1,y1 with x0 < x1 and y0 < y1");
                    return ExitCode::from(2);
                };
                boxes.push(value);
            }
            "--help" => {
                eprintln!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument: {other}");
                return ExitCode::from(2);
            }
        }
    }
    if let Some(other) = against_path {
        return against(
            &actual_path,
            &other,
            min_changed,
            &boxes,
            diff_path.as_deref(),
            parity_mode,
        );
    }
    if parity_mode {
        eprintln!("--parity needs --against <other.ppm>");
        return ExitCode::from(2);
    }
    let Ok((width, height, pixels)) = read_ppm(Path::new(&actual_path)) else {
        eprintln!("cannot read PPM {actual_path}");
        return ExitCode::from(1);
    };
    let fingerprint = downscale(&pixels, width, height);
    if write {
        let Some(path) = expected_path else {
            eprintln!("--write requires --expected <path>");
            return ExitCode::from(2);
        };
        if let Err(error) = std::fs::write(&path, &fingerprint) {
            eprintln!("cannot write {}: {error}", path.display());
            return ExitCode::from(1);
        }
        println!(
            "wrote {FINGERPRINT_WIDTH}x{FINGERPRINT_HEIGHT} fingerprint to {}",
            path.display()
        );
        return ExitCode::SUCCESS;
    }
    let Some(expected_path) = expected_path else {
        eprintln!("--expected <fingerprint.rgb> is required for comparison");
        return ExitCode::from(2);
    };
    let Ok(expected) = std::fs::read(&expected_path) else {
        eprintln!("cannot read fingerprint {}", expected_path.display());
        return ExitCode::from(1);
    };
    if expected.len() != fingerprint.len() {
        eprintln!(
            "fingerprint size mismatch: expected {} bytes, got {}",
            expected.len(),
            fingerprint.len()
        );
        return ExitCode::from(1);
    }
    let mut sum = 0u64;
    let mut max = 0u8;
    let mut diff = Vec::with_capacity(fingerprint.len());
    for (actual, expected) in fingerprint.iter().zip(&expected) {
        let delta = actual.abs_diff(*expected);
        sum += u64::from(delta);
        max = max.max(delta);
        diff.push(delta.saturating_mul(4));
    }
    let samples = u64::try_from(fingerprint.len()).unwrap_or(1);
    let mean = sum as f64 / samples as f64;
    if let Some(path) = diff_path {
        let _ = write_ppm(&path, FINGERPRINT_WIDTH, FINGERPRINT_HEIGHT, &diff);
    }
    println!("render fingerprint: mean_abs_diff={mean:.3} max_diff={max} samples={samples}");
    // Downscaled averages tolerate rasterisation rounding while catching layout/colour changes.
    if mean <= 2.0 && max <= 24 {
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "render fingerprint mismatch (mean {mean:.3} > 2.0 or max {max} > 24); upload actual/diff artifacts"
        );
        ExitCode::from(1)
    }
}

fn read_ppm(path: &Path) -> Result<(usize, usize, Vec<u8>), String> {
    let data = std::fs::read(path).map_err(|error| error.to_string())?;
    let mut position = 0;
    let mut token = || -> Result<String, String> {
        loop {
            while data.get(position).is_some_and(u8::is_ascii_whitespace) {
                position += 1;
            }
            if data.get(position) == Some(&b'#') {
                while data.get(position).is_some_and(|byte| *byte != b'\n') {
                    position += 1;
                }
                continue;
            }
            let start = position;
            while data
                .get(position)
                .is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b'#')
            {
                position += 1;
            }
            if position == start {
                return Err(String::from("truncated PPM header"));
            }
            return Ok(String::from_utf8_lossy(&data[start..position]).into_owned());
        }
    };
    if token()? != "P6" {
        return Err(String::from("expected binary PPM P6"));
    }
    let width: usize = token()?.parse().map_err(|_| String::from("bad width"))?;
    let height: usize = token()?.parse().map_err(|_| String::from("bad height"))?;
    let max: u16 = token()?.parse().map_err(|_| String::from("bad maxval"))?;
    if max != 255 || width == 0 || height == 0 {
        return Err(String::from("unsupported PPM format"));
    }
    position += 1; // single whitespace byte before the raster
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| String::from("PPM dimensions overflow"))?;
    let pixels = data
        .get(position..position + expected)
        .ok_or_else(|| String::from("truncated PPM raster"))?;
    Ok((width, height, pixels.to_vec()))
}

fn write_ppm(path: &Path, width: usize, height: usize, pixels: &[u8]) -> std::io::Result<()> {
    let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
    out.extend_from_slice(pixels);
    std::fs::write(path, out)
}

fn downscale(pixels: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0u8; FINGERPRINT_WIDTH * FINGERPRINT_HEIGHT * 3];
    for y in 0..FINGERPRINT_HEIGHT {
        let y0 = y * height / FINGERPRINT_HEIGHT;
        let y1 = ((y + 1) * height / FINGERPRINT_HEIGHT).max(y0 + 1);
        for x in 0..FINGERPRINT_WIDTH {
            let x0 = x * width / FINGERPRINT_WIDTH;
            let x1 = ((x + 1) * width / FINGERPRINT_WIDTH).max(x0 + 1);
            let mut sum = [0u64; 3];
            let mut count = 0u64;
            for py in y0..y1.min(height) {
                for px in x0..x1.min(width) {
                    let index = (py * width + px) * 3;
                    for channel in 0..3 {
                        sum[channel] += u64::from(pixels[index + channel]);
                    }
                    count += 1;
                }
            }
            let index = (y * FINGERPRINT_WIDTH + x) * 3;
            for channel in 0..3 {
                out[index + channel] = u8::try_from(sum[channel] / count.max(1)).unwrap_or(0);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_fingerprint_round_trip() {
        let pixels = vec![7u8; 32 * 18 * 3];
        let fingerprint = downscale(&pixels, 32, 18);
        assert_eq!(
            fingerprint.len(),
            FINGERPRINT_WIDTH * FINGERPRINT_HEIGHT * 3
        );
        assert!(fingerprint.iter().all(|value| *value == 7));
    }

    #[test]
    fn full_comparison_counts_changes_and_checks_boxes() {
        let (width, height) = (8, 4);
        let base = vec![10u8; width * height * 3];
        let mut other = base.clone();
        // Two pixels differ: (1, 1) and (6, 2).
        other[(width + 1) * 3] = 11;
        other[(2 * width + 6) * 3 + 2] = 0;
        let boxes = [[0, 0, 4, 4], [4, 0, 6, 4], [5, 0, 9, 4]];
        let (changed, in_box) = compare_full(&base, &other, width, height, &boxes);
        assert_eq!(changed, 2);
        assert_eq!(
            in_box,
            [1, 0, usize::MAX],
            "past the image counts as changed"
        );
        assert_eq!(compare_full(&base, &base, width, height, &[]).0, 0);
        assert_eq!(parse_box("1,2,3,4"), Some([1, 2, 3, 4]));
        assert_eq!(parse_box("3,2,1,4"), None);
        assert_eq!(parse_box("1,2,3"), None);
        assert_eq!(parse_box("1,2,3,4,5"), None);
    }

    #[test]
    fn parity_counts_close_pixels_and_the_largest_difference() {
        let base = vec![100u8; 1000 * 3];
        let mut other = base.clone();
        // Five pixels 3/255 off (99.5 % close), one channel each.
        for pixel in 0..5 {
            other[pixel * 3 + pixel % 3] = 103;
        }
        let (close, max, ok) = parity(&difference_histogram(&base, &other));
        assert_eq!((close, max, ok), (995, 3, true));
        // A sixth fails the share; one pixel 7/255 off fails the maximum.
        other[5 * 3] = 97;
        assert!(!parity(&difference_histogram(&base, &other)).2);
        let mut spike = base.clone();
        spike[42] = 107;
        assert_eq!(
            parity(&difference_histogram(&base, &spike)),
            (999, 7, false)
        );
        assert_eq!(parity(&difference_histogram(&base, &base)), (1000, 0, true));
    }

    #[test]
    fn ppm_parser_reads_comments_and_raster() {
        let dir = std::env::temp_dir().join(format!("moba-ppm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("tiny.ppm");
        let mut data = b"P6\n# generated\n1 1\n255\n".to_vec();
        data.extend_from_slice(&[1, 2, 3]);
        std::fs::write(&path, data).expect("write");
        let (width, height, pixels) = read_ppm(&path).expect("parse");
        assert_eq!((width, height, pixels), (1, 1, vec![1, 2, 3]));
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(dir);
    }
}

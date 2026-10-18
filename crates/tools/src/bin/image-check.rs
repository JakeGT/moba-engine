//! Compare a rendered PPM against a checked-in downscaled fingerprint.
#![allow(clippy::print_stdout)]

use std::path::Path;
use std::process::ExitCode;

const FINGERPRINT_WIDTH: usize = 64;
const FINGERPRINT_HEIGHT: usize = 36;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(actual_path) = args.next() else {
        eprintln!(
            "usage: image-check <actual.ppm> --expected <fingerprint.rgb> [--write] [--diff <out.ppm>]"
        );
        return ExitCode::from(2);
    };
    let mut expected_path = None;
    let mut write = false;
    let mut diff_path = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--expected" => expected_path = args.next().map(std::path::PathBuf::from),
            "--write" => write = true,
            "--diff" => diff_path = args.next().map(std::path::PathBuf::from),
            "--help" => {
                eprintln!(
                    "usage: image-check <actual.ppm> --expected <fingerprint.rgb> [--write] [--diff <out.ppm>]"
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument: {other}");
                return ExitCode::from(2);
            }
        }
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

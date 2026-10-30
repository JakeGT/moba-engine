#![no_main]

use libfuzzer_sys::fuzz_target;

// Colour-grading LUTs come from a client-local, player-controlled asset directory (R6): the
// `.cube` parser must return an error, never panic, and never allocate beyond its 64³ cap. A
// LUT it accepts must hold size³ texels in 0..=1, sample to finite values in 0..=1 for any
// input, and round trip through the writer.
fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let Ok(lut) = moba_assets::GradingLut::parse(&text) else {
        return;
    };
    let size = lut.size();
    assert!(
        (moba_assets::MIN_GRADING_LUT_SIZE..=moba_assets::MAX_GRADING_LUT_SIZE).contains(&size)
    );
    assert_eq!(lut.texels().len(), (size as usize).pow(3));
    let in_range = |rgb: [f32; 3]| rgb.iter().all(|c| c.is_finite() && (0.0..=1.0).contains(c));
    assert!(lut.texels().iter().all(|&texel| in_range(texel)));
    for rgb in [
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        [0.5, 0.25, 0.75],
        [-1.0, f32::NAN, f32::INFINITY],
    ] {
        assert!(in_range(lut.sample(rgb)));
    }
    let round =
        moba_assets::GradingLut::parse(&lut.to_cube_string("fuzz")).expect("writer output parses");
    assert_eq!(round.size(), size);
    for (a, b) in round.texels().iter().zip(lut.texels()) {
        assert!((0..3).all(|c| (a[c] - b[c]).abs() <= 1e-6));
    }
});

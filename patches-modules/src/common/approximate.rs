 use std::f32::consts::TAU;

#[inline(always)]
pub fn fast_tanh(x: f32) -> f32 {
    let x2 = x * x;
    let x4 = x2 * x2;
    let x6 = x4 * x2;
    x * (10395.0 + 1260.0 * x2 + 21.0 * x4) / 
        (10395.0 + 4725.0 * x2 + 210.0 * x4 + 4.0 * x6)
}

use std::sync::LazyLock;
static SINE_TABLE: LazyLock<Vec<f32>> = LazyLock::new(|| {
    (0..1024).map(|i| (i as f32 / 1024.0 * TAU).sin()).collect()
});

// Phase is always in [0, 1), so the caller can use it directly to index into a sine table without needing to wrap it.
#[inline(always)]
pub fn lookup_sine(phase: f32) -> f32 {
    let index = phase * 1024.0;
    let index_whole = index as usize;
    let index_frac = index - (index_whole as f32);
    let a = SINE_TABLE[index_whole % 1024];
    let b = SINE_TABLE[(index_whole + 1) % 1024];
    a + (b - a) * index_frac
}
//! Audio kernels of CoreAV: the inner loop of the resampler of
//! `coreav-audio` (`docs/design/audio-decode.md`, "Resampling and
//! downmixing"), and the FFT of `coreav-opus`'s inverse MDCT ([`Fft`]).
//!
//! [`Fir::run`] computes output samples of a polyphase filter: each is the
//! dot product of a window of the input with a row of a coefficient table.
//! The kernels hold no state, read no bitstream and allocate nothing, and
//! [`Fir::run`] checks every window against its slices before any kernel
//! runs, so the vector kernel reads inside them by construction.
//!
//! The portable kernel ([`Fir::portable`]) is safe Rust and the reference.
//! The x86-64 kernel uses AVX and gives the same bits: both multiply and add
//! in eight lanes (no fused multiply-add, which rounds once instead of
//! twice) and sum the lanes in the same order, so the output is the same on
//! every CPU. [`Fir::detect`] picks the AVX kernel where the CPU has it.
//!
//! ```
//! use coreav_kernels_audio::Fir;
//!
//! let input = [1.0f32; 16];
//! let table = [0.125f32; 8];
//! let mut out = [0.0f32; 2];
//! Fir::detect().run(&input, &table, 8, &[(0, 0), (8, 0)], &mut out).expect("in range");
//! assert_eq!(out, [1.0, 1.0]);
//! ```

#![cfg_attr(not(target_arch = "x86_64"), forbid(unsafe_code))]

#[cfg(target_arch = "x86_64")]
mod avx;
mod checked;
mod fft;
mod portable;

use checked::Checked;
pub use fft::{Fft, Twiddles};

/// Taps are a multiple of this: the kernels' lanes.
pub const LANES: usize = 8;

/// Why [`Fir::run`] refused its arguments; nothing was computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutOfRange;

impl core::fmt::Display for OutOfRange {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("a filter window outside its input or table")
    }
}

impl std::error::Error for OutOfRange {}

/// The filter kernels the CPU runs, chosen once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fir {
    #[cfg(target_arch = "x86_64")]
    avx: Option<avx::Avx>,
}

impl Fir {
    /// The fastest kernel the CPU has.
    pub fn detect() -> Self {
        Self {
            #[cfg(target_arch = "x86_64")]
            avx: avx::Avx::detect(),
        }
    }

    /// The portable kernel, the reference.
    pub fn portable() -> Self {
        Self {
            #[cfg(target_arch = "x86_64")]
            avx: None,
        }
    }

    /// Whether a vector kernel runs.
    pub fn is_vector(&self) -> bool {
        #[cfg(target_arch = "x86_64")]
        {
            self.avx.is_some()
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            false
        }
    }

    /// For each `(start, row)` of `windows`, the dot product of
    /// `input[start..start + taps]` with `table[row..row + taps]` into the
    /// same position of `out`. `taps` is a positive multiple of [`LANES`];
    /// `out` is as long as `windows`, and every window lies inside `input`
    /// and `table`. Otherwise nothing is written.
    ///
    /// The products are summed in [`LANES`] lanes, lane `i` taking taps `i`,
    /// `i + 8`, ..., in order, and the lanes as `((l0 + l1) + (l2 + l3)) +
    /// ((l4 + l5) + (l6 + l7))`; every kernel gives these bits.
    pub fn run(
        &self,
        input: &[f32],
        table: &[f32],
        taps: usize,
        windows: &[(usize, usize)],
        out: &mut [f32],
    ) -> Result<(), OutOfRange> {
        let checked = Checked::new(input, table, taps, windows, out.len()).ok_or(OutOfRange)?;
        #[cfg(target_arch = "x86_64")]
        if let Some(avx) = self.avx {
            avx.run(checked, out);
            return Ok(());
        }
        portable::run(checked, out);
        Ok(())
    }
}

/// The dot product of two slices of equal length, a multiple of [`LANES`],
/// summed as [`Fir::run`] sums: the portable kernel for one window.
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    portable::dot(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic sequence of values in [-1, 1).
    fn values(seed: u32, n: usize) -> Vec<f32> {
        let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 8) as f32 / (1u32 << 23) as f32 - 1.0
            })
            .collect()
    }

    #[test]
    fn vector_equals_portable_bit_for_bit() {
        let input = values(1, 5000);
        let table = values(2, 4000);
        for taps in [8, 16, 32, 40, 96, 776] {
            let windows: Vec<(usize, usize)> = (0..403)
                .map(|i| {
                    (
                        (i * 7) % (input.len() - taps),
                        (i * 13) % (table.len() - taps),
                    )
                })
                .collect();
            let mut a = vec![0.0; windows.len()];
            let mut b = vec![0.0; windows.len()];
            Fir::portable()
                .run(&input, &table, taps, &windows, &mut a)
                .unwrap();
            Fir::detect()
                .run(&input, &table, taps, &windows, &mut b)
                .unwrap();
            assert!(
                a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()),
                "taps {taps}"
            );
            for (o, &(s, r)) in a.iter().zip(&windows) {
                assert_eq!(
                    o.to_bits(),
                    dot(&input[s..s + taps], &table[r..r + taps]).to_bits()
                );
            }
        }
    }

    #[test]
    fn refuses_windows_outside() {
        let mut out = [0.0f32; 1];
        let fir = Fir::detect();
        assert_eq!(
            fir.run(&[0.0; 8], &[0.0; 8], 8, &[(1, 0)], &mut out),
            Err(OutOfRange)
        );
        assert_eq!(
            fir.run(&[0.0; 8], &[0.0; 8], 8, &[(0, 1)], &mut out),
            Err(OutOfRange)
        );
        assert_eq!(
            fir.run(&[0.0; 8], &[0.0; 8], 4, &[(0, 0)], &mut out),
            Err(OutOfRange)
        );
        assert_eq!(
            fir.run(&[0.0; 8], &[0.0; 8], 0, &[(0, 0)], &mut out),
            Err(OutOfRange)
        );
        assert_eq!(
            fir.run(&[0.0; 8], &[0.0; 8], 8, &[], &mut out),
            Err(OutOfRange)
        );
        assert_eq!(
            fir.run(&[0.0; 8], &[0.0; 8], 8, &[(usize::MAX, 0)], &mut out),
            Err(OutOfRange)
        );
        assert_eq!(
            fir.run(&[1.0; 8], &[1.0; 8], 8, &[(0, 0)], &mut out),
            Ok(())
        );
        assert_eq!(out, [8.0]);
    }
}

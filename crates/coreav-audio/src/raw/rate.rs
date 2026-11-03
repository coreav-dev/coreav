use crate::Error;

pub(super) struct Rate {
    pub length: usize,
    pub phases: u64,
    pub step: u64,
    coefficients: Vec<f32>,
}

impl Rate {
    pub fn new(input: u32, output: u32) -> Result<Self, Error> {
        let mut a = input;
        let mut b = output;
        while b != 0 {
            (a, b) = (b, a % b);
        }
        let phases = output / a;
        let step = input / a;
        let cutoff = (0.97 * f64::from(output) / f64::from(input)).min(1.0);
        let width = 32.0 / cutoff;
        let whole = width as usize;
        let mut length = whole + usize::from((whole as f64) < width);
        length += length % 2;
        if phases > 1024 || length > 4096 || u64::from(phases) * length as u64 > 65_536 {
            return Err(Error::Unsupported(
                "raw PCM resampling ratio exceeds the coefficient limit",
            ));
        }
        let center = length / 2 - 1;
        let half = (length / 2) as f64;
        let divisor = bessel(9.0);
        let coefficient = |index: usize, phase: f64| {
            let position = index as f64 - center as f64 - phase;
            let x = cutoff * position;
            let sinc = if x == 0.0 {
                1.0
            } else if x == x as i64 as f64 {
                0.0
            } else {
                crate::resample::sin_pi(x) / (std::f64::consts::PI * x)
            };
            let window = position / half;
            let window = bessel(9.0 * (1.0 - window * window).max(0.0).sqrt()) / divisor;
            cutoff * sinc * window
        };
        let total: f64 = (0..length).map(|i| coefficient(i, 0.0)).sum();
        let mut coefficients = Vec::new();
        coefficients
            .try_reserve_exact(length * phases as usize)
            .map_err(|_| Error::Unsupported("raw PCM coefficient allocation failed"))?;
        for phase in 0..phases {
            for index in 0..length {
                coefficients.push(
                    (coefficient(index, f64::from(phase) / f64::from(phases)) / total) as f32,
                );
            }
        }
        Ok(Self {
            length,
            phases: u64::from(phases),
            step: u64::from(step),
            coefficients,
        })
    }

    pub fn output_frames(&self, frames: u64) -> Result<u64, Error> {
        let frames = u128::from(frames);
        let length = self.length as u128;
        let half = length / 2;
        let phases = u128::from(self.phases);
        let step = u128::from(self.step);
        let padding = if frames <= length {
            let padding = frames.div_ceil(2);
            if frames + padding <= length {
                return Ok(0);
            }
            padding
        } else {
            let before = ((frames - half) * phases).div_ceil(step);
            let position = before * step / phases;
            (frames - position + half - 1).div_ceil(2)
        };
        u64::try_from(((frames + padding - half) * phases).div_ceil(step))
            .map_err(|_| Error::Unsupported("raw PCM output sample count overflows"))
    }

    pub fn row(&self, phase: u64) -> Result<&[f32], Error> {
        let start = usize::try_from(phase)
            .ok()
            .and_then(|p| p.checked_mul(self.length))
            .ok_or(Error::Invalid("raw PCM phase"))?;
        self.coefficients
            .get(start..start + self.length)
            .ok_or(Error::Invalid("raw PCM phase"))
    }
}

fn bessel(value: f64) -> f64 {
    let square = value * value / 4.0;
    let mut total = 1.0;
    let mut term = 1.0;
    for k in 1..80 {
        term *= square / f64::from(k * k);
        let next = total + term;
        if next == total {
            return total;
        }
        total = next;
    }
    total
}

use super::md5::Md5;
use crate::crc::{crc8, crc16};
use crate::{Error, LimitKind, Limits, OutputFormat, SampleFormat, Sink};
use std::io::Write;

const BLOCK: usize = 4096;

/// A bounded native FLAC writer for signed 16-bit or packed 24-bit PCM.
/// Call `finish` to flush the tail and obtain the completed file header.
pub struct FlacSink<W: Write> {
    out: W,
    state: State,
    limits: Limits,
}

enum State {
    New,
    Encoding(Stream),
    Failed,
}

struct Stream {
    format: OutputFormat,
    pending: Vec<u8>,
    md5: Md5,
    samples: u64,
    frames: u32,
    min_frame: u32,
    max_frame: u32,
}

impl<W: Write> FlacSink<W> {
    /// Creates a writer with the supplied channel, sample and frame limits.
    pub fn new(out: W, limits: Limits) -> Self {
        Self {
            out,
            state: State::New,
            limits,
        }
    }

    /// Writes the final frame and returns the 42-byte header to patch at zero.
    pub fn finish(mut self) -> Result<Vec<u8>, Error> {
        let State::Encoding(mut stream) = std::mem::replace(&mut self.state, State::Failed) else {
            return Err(Error::Invalid("FLAC encoder is not encoding"));
        };
        if !stream.pending.is_empty() {
            let tail = std::mem::take(&mut stream.pending);
            stream.frame(&tail, &mut self.out, &self.limits)?;
        }
        let digest = stream.md5.finish();
        Ok(header(
            &stream.format,
            stream.samples,
            stream.min_frame,
            stream.max_frame,
            digest,
        ))
    }
}

impl<W: Write> Sink for FlacSink<W> {
    fn start(&mut self, format: &OutputFormat) -> Result<(), Error> {
        let previous = std::mem::replace(&mut self.state, State::Failed);
        if !matches!(previous, State::New) {
            return Err(Error::Invalid("FLAC encoder was already started"));
        }
        if !matches!(format.format, SampleFormat::S16 | SampleFormat::S24)
            || !(1..=8).contains(&format.channels)
            || !(1..=0xF_FFFF).contains(&format.sample_rate)
        {
            return Err(Error::Unsupported("FLAC encoder sample format"));
        }
        crate::limits::check_channels(usize::from(format.channels), self.limits.max_channels)?;
        let mask = [0x4, 0x3, 0x7, 0x33, 0x37, 0x3F, 0x70F, 0x63F]
            .get(usize::from(format.channels - 1))
            .copied()
            .unwrap_or(0);
        if format.channel_mask != mask {
            return Err(Error::Unsupported("FLAC encoder speaker layout"));
        }
        self.out
            .write_all(&header(format, 0, 0, 0, [0; 16]))
            .map_err(Error::Output)?;
        self.state = State::Encoding(Stream {
            format: *format,
            pending: Vec::with_capacity(
                BLOCK * usize::from(format.channels) * format.format.bytes(),
            ),
            md5: Md5::new(),
            samples: 0,
            frames: 0,
            min_frame: 0,
            max_frame: 0,
        });
        Ok(())
    }

    fn write(&mut self, mut input: &[u8]) -> Result<(), Error> {
        let State::Encoding(mut stream) = std::mem::replace(&mut self.state, State::Failed) else {
            return Err(Error::Invalid("FLAC encoder is not encoding"));
        };
        let width = usize::from(stream.format.channels) * stream.format.format.bytes();
        if input.len() % width != 0 {
            return Err(Error::Invalid("incomplete FLAC input sample frame"));
        }
        let samples = stream
            .samples
            .checked_add((input.len() / width) as u64)
            .filter(|count| *count < (1u64 << 36))
            .filter(|count| {
                count.saturating_mul(u64::from(stream.format.channels))
                    <= self.limits.max_output_samples
            })
            .ok_or(Error::LimitExceeded(LimitKind::OutputSamples))?;
        stream.md5.update(input);
        let bytes = BLOCK * width;
        while !input.is_empty() {
            if stream.pending.is_empty() && input.len() >= bytes {
                let (block, rest) = input.split_at(bytes);
                stream.frame(block, &mut self.out, &self.limits)?;
                input = rest;
            } else {
                let take = (bytes - stream.pending.len()).min(input.len());
                let (part, rest) = input.split_at(take);
                stream.pending.extend_from_slice(part);
                input = rest;
                if stream.pending.len() == bytes {
                    let block = std::mem::take(&mut stream.pending);
                    stream.frame(&block, &mut self.out, &self.limits)?;
                    stream.pending = block;
                    stream.pending.clear();
                }
            }
        }
        stream.samples = samples;
        self.state = State::Encoding(stream);
        Ok(())
    }
}

impl Stream {
    fn frame(&mut self, input: &[u8], out: &mut impl Write, limits: &Limits) -> Result<(), Error> {
        let channels = usize::from(self.format.channels);
        let width = self.format.format.bytes();
        let frames = input.len() / (channels * width);
        let bits = self.format.format.bits() as u8;
        let mut planes = vec![Vec::with_capacity(frames); channels];
        for frame in input.chunks_exact(channels * width) {
            for (plane, value) in planes.iter_mut().zip(frame.chunks_exact(width)) {
                let value = match value {
                    [a, b] => i64::from(i16::from_le_bytes([*a, *b])),
                    [a, b, c] => i64::from(i32::from_le_bytes([
                        *a,
                        *b,
                        *c,
                        if c & 0x80 == 0 { 0 } else { 255 },
                    ])),
                    _ => return Err(Error::Invalid("FLAC encoder sample width")),
                };
                plane.push(value);
            }
        }
        let mut subframes: Vec<Subframe> = planes.iter().map(|p| Subframe::plan(p, bits)).collect();
        let mut assignment = channels as u8 - 1;
        if let [left, right] = planes.as_slice() {
            let side: Vec<i64> = left.iter().zip(right).map(|(l, r)| l - r).collect();
            let mid: Vec<i64> = left.iter().zip(right).map(|(l, r)| (l + r) >> 1).collect();
            let side = Subframe::plan(&side, bits + 1);
            let mid = Subframe::plan(&mid, bits);
            if let [left, right] = subframes.as_slice() {
                let choices = [
                    (left.cost + right.cost, 1),
                    (left.cost + side.cost, 8),
                    (side.cost + right.cost, 9),
                    (mid.cost + side.cost, 10),
                ];
                assignment = choices
                    .into_iter()
                    .min_by_key(|(cost, _)| *cost)
                    .map(|(_, a)| a)
                    .unwrap_or(1);
            }
            match assignment {
                8 => {
                    subframes.truncate(1);
                    subframes.push(side);
                }
                9 => {
                    subframes.remove(0);
                    subframes.insert(0, side);
                }
                10 => {
                    subframes.clear();
                    subframes.extend([mid, side]);
                }
                _ => {}
            }
        }
        let rate_code = match self.format.sample_rate {
            88_200 => 1,
            176_400 => 2,
            192_000 => 3,
            8_000 => 4,
            16_000 => 5,
            22_050 => 6,
            24_000 => 7,
            32_000 => 8,
            44_100 => 9,
            48_000 => 10,
            96_000 => 11,
            _ => 0,
        };
        let mut output = Bits::new();
        output.bytes.extend([
            0xFF,
            0xF8,
            0x70 | rate_code,
            (assignment << 4) | if bits == 16 { 8 } else { 12 },
        ]);
        coded_number(self.frames, &mut output.bytes);
        output
            .bytes
            .extend_from_slice(&((frames - 1) as u16).to_be_bytes());
        output.bytes.push(crc8(&output.bytes));
        for subframe in subframes {
            subframe.write(&mut output);
        }
        output.align();
        output
            .bytes
            .extend_from_slice(&crc16(&output.bytes).to_be_bytes());
        let bytes = output.bytes.len() as u32;
        if u64::from(bytes) > limits.max_packet_bytes {
            return Err(Error::LimitExceeded(LimitKind::PacketBytes));
        }
        out.write_all(&output.bytes).map_err(Error::Output)?;
        self.frames += 1;
        self.min_frame = if self.min_frame == 0 {
            bytes
        } else {
            self.min_frame.min(bytes)
        };
        self.max_frame = self.max_frame.max(bytes);
        Ok(())
    }
}

fn header(format: &OutputFormat, samples: u64, min: u32, max: u32, md5: [u8; 16]) -> Vec<u8> {
    let mut bytes = b"fLaC\x80\0\0\x22".to_vec();
    bytes.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    bytes.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    for size in [min, max] {
        bytes.extend(size.to_be_bytes().into_iter().skip(1));
    }
    let description = (u64::from(format.sample_rate) << 44)
        | (u64::from(format.channels - 1) << 41)
        | (u64::from(format.format.bits() - 1) << 36)
        | samples;
    bytes.extend_from_slice(&description.to_be_bytes());
    bytes.extend_from_slice(&md5);
    bytes
}

fn coded_number(number: u32, bytes: &mut Vec<u8>) {
    if number < 128 {
        bytes.push(number as u8);
        return;
    }
    let count = (2..=6)
        .find(|n| number < (1u32 << (5 * n + 1)))
        .unwrap_or(6);
    bytes.push((0xFFu8 << (8 - count)) | (number >> (6 * (count - 1))) as u8);
    for shift in (0..count - 1).rev() {
        bytes.push(0x80 | ((number >> (6 * shift)) & 0x3F) as u8);
    }
}

struct Bits {
    bytes: Vec<u8>,
    partial: u8,
    used: u8,
}

impl Bits {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            partial: 0,
            used: 0,
        }
    }

    fn put(&mut self, mut count: u8, value: u64) {
        while count > 0 {
            let take = count.min(8 - self.used);
            count -= take;
            self.partial |=
                (((value >> count) & ((1 << take) - 1)) as u8) << (8 - self.used - take);
            self.used += take;
            if self.used == 8 {
                self.bytes.push(self.partial);
                self.partial = 0;
                self.used = 0;
            }
        }
    }

    fn unary(&mut self, mut zeros: u64) {
        if self.used != 0 && zeros > 0 {
            let take = zeros.min(u64::from(8 - self.used)) as u8;
            self.put(take, 0);
            zeros -= u64::from(take);
        }
        self.bytes
            .resize(self.bytes.len() + (zeros / 8) as usize, 0);
        self.put((zeros % 8) as u8, 0);
        self.put(1, 1);
    }

    fn align(&mut self) {
        if self.used != 0 {
            self.put(8 - self.used, 0);
        }
    }
}

enum Predictor {
    Constant,
    Verbatim,
    Fixed(u8),
    Linear { coefficients: Vec<i16>, shift: u8 },
}

struct Subframe {
    samples: Vec<i64>,
    bits: u8,
    wasted: u8,
    predictor: Predictor,
    residual: Vec<u32>,
    rice: Rice,
    cost: u64,
}

struct Rice {
    order: u8,
    parameters: Vec<u8>,
    cost: u64,
}

impl Subframe {
    fn plan(input: &[i64], bits: u8) -> Self {
        let wasted = input
            .iter()
            .map(|v| v.trailing_zeros() as u8)
            .min()
            .unwrap_or(0)
            .min(bits - 1);
        let bits = bits - wasted;
        let samples: Vec<i64> = input.iter().map(|v| v >> wasted).collect();
        let overhead = 8 + u64::from(wasted);
        let cost = overhead + samples.len() as u64 * u64::from(bits);
        let mut best = Self {
            samples,
            bits,
            wasted,
            predictor: Predictor::Verbatim,
            residual: Vec::new(),
            rice: Rice {
                order: 0,
                parameters: Vec::new(),
                cost: 0,
            },
            cost,
        };
        if best.samples.iter().all(|s| Some(s) == best.samples.first()) {
            best.predictor = Predictor::Constant;
            best.cost = overhead + u64::from(bits);
            return best;
        }
        for (order, coefficients) in [
            &[][..],
            &[1][..],
            &[2, -1][..],
            &[3, -3, 1][..],
            &[4, -6, 4, -1][..],
        ]
        .into_iter()
        .enumerate()
        {
            best.consider(coefficients, 0, Predictor::Fixed(order as u8), overhead);
        }
        let max_order = 8.min(best.samples.len().saturating_sub(1));
        let correlation: Vec<f64> = (0..=max_order)
            .map(|lag| {
                best.samples
                    .iter()
                    .skip(lag)
                    .zip(&best.samples)
                    .map(|(a, b)| (*a as f64) * (*b as f64))
                    .sum()
            })
            .collect();
        let mut error = correlation.first().copied().unwrap_or(0.0);
        let mut coefficients: Vec<f64> = Vec::new();
        for order in 1..=max_order {
            if !error.is_finite() || error <= 0.0 {
                break;
            }
            let sum: f64 = coefficients
                .iter()
                .zip(correlation.iter().take(order).skip(1).rev())
                .map(|(a, r)| a * r)
                .sum();
            let reflection = (correlation.get(order).copied().unwrap_or(0.0) - sum) / error;
            if !reflection.is_finite() || reflection.abs() >= 1.0 {
                break;
            }
            let mut next: Vec<f64> = coefficients
                .iter()
                .zip(coefficients.iter().rev())
                .map(|(a, b)| a - reflection * b)
                .collect();
            next.push(reflection);
            coefficients = next;
            error *= 1.0 - reflection * reflection;
            let largest = coefficients.iter().map(|v| v.abs()).fold(0.0, f64::max);
            let shift = ((16382.0 / largest).log2().floor() as i32).clamp(0, 15) as u8;
            let mut carry = 0.0;
            let quantized: Vec<i16> = coefficients
                .iter()
                .map(|a| {
                    let scaled = a * f64::from(1u32 << shift) + carry;
                    let quantized = scaled.round().clamp(-16384.0, 16383.0);
                    carry = scaled - quantized;
                    quantized as i16
                })
                .collect();
            best.consider(
                &quantized,
                shift,
                Predictor::Linear {
                    coefficients: quantized.clone(),
                    shift,
                },
                overhead,
            );
        }
        best
    }

    fn consider(&mut self, coefficients: &[i16], shift: u8, predictor: Predictor, overhead: u64) {
        let order = coefficients.len();
        if order >= self.samples.len() {
            return;
        }
        let residual: Option<Vec<u32>> = self
            .samples
            .windows(order + 1)
            .map(|window| {
                let (sample, previous) = window.split_last()?;
                let predicted = previous
                    .iter()
                    .rev()
                    .zip(coefficients)
                    .map(|(v, c)| v * i64::from(*c))
                    .sum::<i64>()
                    >> shift;
                let residual = sample - predicted;
                (residual.abs() < (1i64 << 31)).then(|| {
                    if residual < 0 {
                        (-residual * 2 - 1) as u32
                    } else {
                        (residual * 2) as u32
                    }
                })
            })
            .collect();
        let Some(residual) = residual else {
            return;
        };
        let rice = Rice::plan(&residual, self.samples.len(), order);
        let extra = if matches!(predictor, Predictor::Linear { .. }) {
            9 + 15 * order as u64
        } else {
            0
        };
        let cost = overhead + order as u64 * u64::from(self.bits) + extra + rice.cost;
        if cost < self.cost {
            self.predictor = predictor;
            self.residual = residual;
            self.rice = rice;
            self.cost = cost;
        }
    }

    fn write(self, output: &mut Bits) {
        let (kind, order) = match &self.predictor {
            Predictor::Constant => (0, 0),
            Predictor::Verbatim => (1, 0),
            Predictor::Fixed(order) => (8 + *order, usize::from(*order)),
            Predictor::Linear { coefficients, .. } => {
                (31 + coefficients.len() as u8, coefficients.len())
            }
        };
        output.put(8, u64::from((kind << 1) | u8::from(self.wasted != 0)));
        if self.wasted != 0 {
            output.unary(u64::from(self.wasted - 1));
        }
        let count = match self.predictor {
            Predictor::Constant => 1,
            Predictor::Verbatim => self.samples.len(),
            _ => order,
        };
        for value in self.samples.iter().take(count) {
            output.put(self.bits, *value as u64);
        }
        if matches!(self.predictor, Predictor::Constant | Predictor::Verbatim) {
            return;
        }
        if let Predictor::Linear {
            coefficients,
            shift,
        } = self.predictor
        {
            output.put(4, 14);
            output.put(5, u64::from(shift));
            for coefficient in coefficients {
                output.put(15, coefficient as u64);
            }
        }
        output.put(2, 0);
        output.put(4, u64::from(self.rice.order));
        let partition = self.samples.len() >> self.rice.order;
        let mut residual = self.residual.as_slice();
        for (index, k) in self.rice.parameters.into_iter().enumerate() {
            output.put(4, u64::from(k));
            let count = partition - if index == 0 { order } else { 0 };
            let (values, rest) = residual.split_at(count);
            residual = rest;
            for value in values {
                output.unary(u64::from(*value >> k));
                output.put(k, u64::from(*value));
            }
        }
    }
}

impl Rice {
    fn plan(residual: &[u32], samples: usize, predictor: usize) -> Self {
        let mut best = Self {
            order: 0,
            parameters: Vec::new(),
            cost: u64::MAX,
        };
        for order in 0..=4 {
            let partition = samples >> order;
            if samples % (1 << order) != 0 || partition <= predictor {
                break;
            }
            let mut cost = 6;
            let mut parameters = Vec::new();
            let mut remaining = residual;
            for index in 0..1 << order {
                let count = partition - if index == 0 { predictor } else { 0 };
                let (values, rest) = remaining.split_at(count);
                remaining = rest;
                let sum: u64 = values.iter().map(|v| u64::from(*v)).sum();
                let estimate = (sum / count as u64).checked_ilog2().unwrap_or(0).min(14) as u8;
                let (parameter, bits) = (estimate.saturating_sub(1)
                    ..=estimate.saturating_add(1).min(14))
                    .map(|k| {
                        (
                            k,
                            count as u64 * (u64::from(k) + 1)
                                + values.iter().map(|v| u64::from(*v >> k)).sum::<u64>(),
                        )
                    })
                    .min_by_key(|(_, bits)| *bits)
                    .unwrap_or((0, 0));
                parameters.push(parameter);
                cost += 4 + bits;
            }
            if cost < best.cost {
                best = Self {
                    order,
                    parameters,
                    cost,
                };
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_numbers_use_canonical_utf8_and_its_flac_extension() {
        for number in [0, 1, 127, 128, 2047, 2048, 65535, 65536, 1048575] {
            let mut bytes = Vec::new();
            coded_number(number, &mut bytes);
            let mut buffer = [0; 4];
            let expected =
                char::from_u32(number).map(|value| value.encode_utf8(&mut buffer).as_bytes());
            assert_eq!(Some(bytes.as_slice()), expected);
        }
        for (number, expected) in [
            (0x20_0000, vec![0xf8, 0x88, 0x80, 0x80, 0x80]),
            (0x400_0000, vec![0xfc, 0x84, 0x80, 0x80, 0x80, 0x80]),
            (0x7fff_ffff, vec![0xfd, 0xbf, 0xbf, 0xbf, 0xbf, 0xbf]),
        ] {
            let mut bytes = Vec::new();
            coded_number(number, &mut bytes);
            assert_eq!(bytes, expected);
        }
    }

    #[test]
    fn coding_choices_include_linear_prediction_and_partitioned_rice() {
        let constant = Subframe::plan(&[0; 4096], 16);
        assert!(matches!(constant.predictor, Predictor::Constant));
        assert_eq!(constant.wasted, 15);
        let ramp: Vec<i64> = (0..4096).map(|i| i * 3 - 6000).collect();
        assert!(matches!(
            Subframe::plan(&ramp, 16).predictor,
            Predictor::Fixed(_)
        ));
        let tone: Vec<i64> = (0..4096)
            .map(|i| ((i as f64 * 0.13).sin() * 20000.0) as i64)
            .collect();
        assert!(matches!(
            Subframe::plan(&tone, 16).predictor,
            Predictor::Linear { .. }
        ));
        let mut random = 7u32;
        let noise: Vec<i64> = (0..4096)
            .map(|_| {
                random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                i64::from((random >> 16) as i16)
            })
            .collect();
        assert!(matches!(
            Subframe::plan(&noise, 16).predictor,
            Predictor::Verbatim
        ));
        let residual: Vec<u32> = (0..4096)
            .map(|i| if i < 2048 { 0 } else { 65535 })
            .collect();
        let rice = Rice::plan(&residual, 4096, 0);
        assert!(rice.order > 0);
        assert!(rice.cost < 6 + 4 + 4096 * 17);
    }
}

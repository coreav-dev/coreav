use super::md5::Md5;
use crate::crc::{crc8, crc16};
use crate::{Error, LimitKind, Limits, OutputFormat, SampleFormat, Sink};
use std::io::Write;

const BLOCK: usize = 4096;

/// The RFC 9639 speaker mask for one to eight channels, or `None` otherwise.
pub fn channel_mask(channels: u16) -> Option<u32> {
    match channels {
        1..=8 => Some(crate::convert::default_mask(channels)),
        _ => None,
    }
}

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
    residual_scratch: Vec<u32>,
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

    /// Writes the final frame and returns the file header to patch at zero.
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
            || !(1..=0xF_FFFF).contains(&format.sample_rate)
        {
            return Err(Error::Unsupported("FLAC encoder sample format"));
        }
        if channel_mask(format.channels) != Some(format.channel_mask) {
            return Err(Error::Unsupported("FLAC encoder speaker layout"));
        }
        crate::limits::check_channels(usize::from(format.channels), self.limits.max_channels)?;
        self.out
            .write_all(&header(format, 0, 0, 0, [0; 16]))
            .map_err(Error::Output)?;
        self.state = State::Encoding(Stream {
            format: *format,
            pending: Vec::with_capacity(
                BLOCK * usize::from(format.channels) * format.format.bytes(),
            ),
            residual_scratch: Vec::new(),
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
        let mut subframes: Vec<Subframe> = planes
            .iter()
            .map(|p| Subframe::plan(p, bits, &mut self.residual_scratch))
            .collect();
        let mut assignment = channels as u8 - 1;
        if let [left, right] = planes.as_slice() {
            let side: Vec<i64> = left.iter().zip(right).map(|(l, r)| l - r).collect();
            let mid: Vec<i64> = left.iter().zip(right).map(|(l, r)| (l + r) >> 1).collect();
            let side = Subframe::plan(&side, bits + 1, &mut self.residual_scratch);
            let mid = Subframe::plan(&mid, bits, &mut self.residual_scratch);
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
    let needs_mask = matches!(format.channels, 5 | 6);
    let mut bytes = b"fLaC".to_vec();
    bytes.extend_from_slice(&[if needs_mask { 0 } else { 0x80 }, 0, 0, 34]);
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
    if needs_mask {
        let vendor = b"CoreAV";
        let field = format!(
            "WAVEFORMATEXTENSIBLE_CHANNEL_MASK=0x{:04X}",
            format.channel_mask
        );
        let size = 12 + vendor.len() + field.len();
        bytes.push(0x84);
        bytes.extend((size as u32).to_be_bytes().into_iter().skip(1));
        bytes.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        bytes.extend_from_slice(vendor);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(field.len() as u32).to_le_bytes());
        bytes.extend_from_slice(field.as_bytes());
    }
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

fn fold_residual(value: i64) -> Option<u32> {
    if value <= -(1i64 << 31) || value >= (1i64 << 31) {
        return None;
    }
    Some(if value < 0 {
        (-value * 2 - 1) as u32
    } else {
        (value * 2) as u32
    })
}

fn fixed_residual<const ORDER: usize>(samples: &[i64], residual: &mut Vec<u32>) -> bool {
    for window in samples.windows(ORDER + 1) {
        let (sample, predicted) = match window {
            [sample] => (*sample, 0),
            [a, sample] => (*sample, *a),
            [a, b, sample] => (*sample, 2 * b - a),
            [a, b, c, sample] => (*sample, 3 * c - 3 * b + a),
            [a, b, c, d, sample] => (*sample, 4 * d - 6 * c + 4 * b - a),
            _ => return false,
        };
        let Some(value) = fold_residual(sample - predicted) else {
            return false;
        };
        residual.push(value);
    }
    true
}

impl Predictor {
    fn residual(&self, samples: &[i64], residual: &mut Vec<u32>) -> Option<usize> {
        let order = match self {
            Self::Fixed(order) => usize::from(*order),
            Self::Linear { coefficients, .. } => coefficients.len(),
            Self::Constant | Self::Verbatim => return None,
        };
        if order >= samples.len() {
            return None;
        }
        residual.clear();
        residual.reserve_exact(samples.len() - order);
        let valid = match self {
            Self::Fixed(0) => fixed_residual::<0>(samples, residual),
            Self::Fixed(1) => fixed_residual::<1>(samples, residual),
            Self::Fixed(2) => fixed_residual::<2>(samples, residual),
            Self::Fixed(3) => fixed_residual::<3>(samples, residual),
            Self::Fixed(4) => fixed_residual::<4>(samples, residual),
            Self::Linear {
                coefficients,
                shift,
            } => {
                for window in samples.windows(order + 1) {
                    let (sample, previous) = window.split_last()?;
                    let mut predicted = 0i64;
                    for (value, coefficient) in previous.iter().rev().zip(coefficients) {
                        predicted += value * i64::from(*coefficient);
                    }
                    let value = fold_residual(sample - (predicted >> shift))?;
                    residual.push(value);
                }
                true
            }
            Self::Fixed(_) | Self::Constant | Self::Verbatim => false,
        };
        valid.then_some(order)
    }
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
    parameter_bits: u8,
    order: u8,
    parameters: Vec<Partition>,
    cost: u64,
}

enum Partition {
    Rice(u8),
    Escape(u8),
}

fn lpc_shift(largest: f64) -> u8 {
    (0..=15)
        .rev()
        .find(|&shift| largest * f64::from(1u32 << shift) <= 16382.0)
        .unwrap_or(0)
}

fn autocorrelation(windowed: &[f64], max_order: usize) -> Vec<f64> {
    let mut correlation = vec![std::iter::empty::<f64>().sum(); max_order + 1];
    for (group, sums) in correlation.chunks_mut(4).enumerate() {
        let lag = group * 4;
        if let [s0, s1, s2, s3] = sums {
            for (terms, b) in windowed.windows(4).skip(lag).zip(windowed) {
                if let [a0, a1, a2, a3] = terms {
                    *s0 += a0 * b;
                    *s1 += a1 * b;
                    *s2 += a2 * b;
                    *s3 += a3 * b;
                }
            }
            let common = windowed.len().saturating_sub(lag + 3);
            for (offset, sum) in sums.iter_mut().enumerate() {
                for (a, b) in windowed
                    .iter()
                    .skip(lag + offset + common)
                    .zip(windowed.iter().skip(common))
                {
                    *sum += a * b;
                }
            }
        } else {
            for (offset, sum) in sums.iter_mut().enumerate() {
                *sum = windowed
                    .iter()
                    .skip(lag + offset)
                    .zip(windowed)
                    .map(|(a, b)| a * b)
                    .sum();
            }
        }
    }
    correlation
}

impl Subframe {
    fn plan(input: &[i64], bits: u8, residual: &mut Vec<u32>) -> Self {
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
                parameter_bits: 4,
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
        for order in 0..=4 {
            best.consider(Predictor::Fixed(order), overhead, residual);
        }
        let max_order = 8.min(best.samples.len().saturating_sub(1));
        let last = best.samples.len().saturating_sub(1) as f64;
        let windowed: Vec<f64> = best
            .samples
            .iter()
            .enumerate()
            .map(|(index, sample)| {
                let position = 2.0 * index as f64 / last - 1.0;
                *sample as f64 * (1.0 - position * position)
            })
            .collect();
        let correlation = autocorrelation(&windowed, max_order);
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
            let shift = lpc_shift(largest);
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
                Predictor::Linear {
                    coefficients: quantized,
                    shift,
                },
                overhead,
                residual,
            );
        }
        best
    }

    fn consider(&mut self, predictor: Predictor, overhead: u64, residual: &mut Vec<u32>) {
        let Some(order) = predictor.residual(&self.samples, residual) else {
            return;
        };
        let rice = Rice::plan(residual, self.samples.len(), order);
        let extra = if matches!(predictor, Predictor::Linear { .. }) {
            9 + 15 * order as u64
        } else {
            0
        };
        let cost = overhead + order as u64 * u64::from(self.bits) + extra + rice.cost;
        if cost < self.cost {
            self.predictor = predictor;
            std::mem::swap(&mut self.residual, residual);
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
        output.put(2, u64::from(self.rice.parameter_bits - 4));
        output.put(4, u64::from(self.rice.order));
        let partition = self.samples.len() >> self.rice.order;
        let mut residual = self.residual.as_slice();
        for (index, parameter) in self.rice.parameters.into_iter().enumerate() {
            let count = partition - if index == 0 { order } else { 0 };
            let (values, rest) = residual.split_at(count);
            residual = rest;
            match parameter {
                Partition::Rice(k) => {
                    output.put(self.rice.parameter_bits, u64::from(k));
                    for value in values {
                        output.unary(u64::from(*value >> k));
                        output.put(k, u64::from(*value));
                    }
                }
                Partition::Escape(width) => {
                    output.put(
                        self.rice.parameter_bits,
                        (1 << self.rice.parameter_bits) - 1,
                    );
                    output.put(5, u64::from(width));
                    for value in values {
                        let value = *value;
                        let signed = if value & 1 == 0 {
                            i64::from(value >> 1)
                        } else {
                            -i64::from(value >> 1) - 1
                        };
                        output.put(width, signed as u64);
                    }
                }
            }
        }
    }
}

struct RiceCosts<'a> {
    residual: &'a [u32],
    order: u8,
    partition: usize,
    predictor: usize,
    quotients: [[u64; 16]; 31],
    computed: u32,
    widths: [u8; 16],
}

impl<'a> RiceCosts<'a> {
    fn new(residual: &'a [u32], samples: usize, predictor: usize, order: u8) -> Self {
        let partition = samples >> order;
        let mut sums = [0; 16];
        let mut widths = [0; 16];
        let mut remaining = residual;
        for (index, (sum, width)) in sums
            .iter_mut()
            .zip(&mut widths)
            .take(1 << order)
            .enumerate()
        {
            let count = partition - if index == 0 { predictor } else { 0 };
            let (values, rest) = remaining.split_at(count);
            remaining = rest;
            let (total, maximum) = values.iter().fold((0, 0), |(sum, maximum), value| {
                (sum + u64::from(*value), maximum.max(*value))
            });
            *sum = total;
            *width = (32 - maximum.leading_zeros()) as u8;
        }
        Self {
            residual,
            order,
            partition,
            predictor,
            quotients: std::array::from_fn(|k| if k == 0 { sums } else { [0; 16] }),
            computed: 1,
            widths,
        }
    }

    fn quotient_sum(&mut self, order: u8, partition: usize, k: u8) -> u64 {
        let Some(sums) = self.quotients.get_mut(usize::from(k)) else {
            return 0;
        };
        if self.computed & (1 << k) == 0 {
            let mut remaining = self.residual;
            for (index, sum) in sums.iter_mut().take(1 << self.order).enumerate() {
                let count = self.partition - if index == 0 { self.predictor } else { 0 };
                let (values, rest) = remaining.split_at(count);
                remaining = rest;
                *sum = values.iter().map(|v| u64::from(*v >> k)).sum();
            }
            self.computed |= 1 << k;
        }
        let leaves = 1 << (self.order - order);
        sums.iter().skip(partition * leaves).take(leaves).sum()
    }

    fn escaped_width(&self, order: u8, partition: usize) -> u8 {
        let leaves = 1 << (self.order - order);
        self.widths
            .iter()
            .skip(partition * leaves)
            .take(leaves)
            .copied()
            .max()
            .unwrap_or(0)
    }
}

impl Rice {
    fn plan(residual: &[u32], samples: usize, predictor: usize) -> Self {
        let mut best = Self {
            parameter_bits: 4,
            order: 0,
            parameters: Vec::new(),
            cost: u64::MAX,
        };
        let Some(max_order) = (0..=4)
            .take_while(|order| samples % (1 << order) == 0 && samples >> order > predictor)
            .last()
        else {
            return best;
        };
        let mut costs = RiceCosts::new(residual, samples, predictor, max_order);
        for order in 0..=max_order {
            let partition = samples >> order;
            let mut candidates = [4, 5].map(|parameter_bits| Self {
                parameter_bits,
                order,
                parameters: Vec::new(),
                cost: 6,
            });
            for index in 0..1 << order {
                let count = partition - if index == 0 { predictor } else { 0 };
                let sum = costs.quotient_sum(order, index, 0);
                let mut code_cost =
                    |k: u8| count as u64 * (u64::from(k) + 1) + costs.quotient_sum(order, index, k);
                let mut parameter = (sum / count as u64).checked_ilog2().unwrap_or(0).min(30) as u8;
                let mut bits = code_cost(parameter);
                for k in (0..parameter).rev() {
                    let candidate = code_cost(k);
                    if candidate > bits {
                        break;
                    }
                    parameter = k;
                    bits = candidate;
                }
                for k in parameter + 1..=30 {
                    let candidate = code_cost(k);
                    if candidate >= bits {
                        break;
                    }
                    parameter = k;
                    bits = candidate;
                }
                let four_parameter = parameter.min(14);
                let four_bits = if four_parameter == parameter {
                    bits
                } else {
                    code_cost(four_parameter)
                };
                let width = costs.escaped_width(order, index);
                let escaped = 5 + count as u64 * u64::from(width);
                for (candidate, (parameter, mut bits)) in candidates
                    .iter_mut()
                    .zip([(four_parameter, four_bits), (parameter, bits)])
                {
                    let selected = if width <= 31 && escaped < bits {
                        bits = escaped;
                        Partition::Escape(width)
                    } else {
                        Partition::Rice(parameter)
                    };
                    candidate.parameters.push(selected);
                    candidate.cost += u64::from(candidate.parameter_bits) + bits;
                }
            }
            for candidate in candidates {
                if candidate.cost < best.cost {
                    best = candidate;
                }
            }
        }
        best
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn cached_quotients_and_widths_match_every_scalar_partition() {
        assert!(std::mem::size_of::<RiceCosts<'_>>() <= 4096 + 128);
        let mut state = 19u32;
        for samples in [
            1, 2, 3, 4, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 255, 256, 257,
            4095, 4096,
        ] {
            for predictor in 0..=8.min(samples - 1) {
                let max_order = (0..=4)
                    .filter(|order| samples % (1 << order) == 0 && samples >> order > predictor)
                    .max()
                    .unwrap();
                for mode in 0..4 {
                    let residual: Vec<u32> = (predictor..samples)
                        .map(|_| {
                            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                            match mode {
                                0 => 0,
                                1 => state >> 31,
                                2 => state & 0x7fff_ffff,
                                _ => state.min(u32::MAX - 1),
                            }
                        })
                        .collect();
                    let mut costs = RiceCosts::new(&residual, samples, predictor, max_order);
                    for order in (0..=max_order).rev() {
                        let partition = samples >> order;
                        let mut start = 0;
                        for index in 0..1 << order {
                            let count = partition - if index == 0 { predictor } else { 0 };
                            let values = &residual[start..start + count];
                            let maximum = values.iter().copied().max().unwrap_or(0);
                            assert_eq!(
                                costs.escaped_width(order, index),
                                (32 - maximum.leading_zeros()) as u8
                            );
                            for k in (0..=30).rev().chain(0..=30) {
                                let expected: u64 = values.iter().map(|v| u64::from(v >> k)).sum();
                                assert_eq!(costs.quotient_sum(order, index, k), expected);
                            }
                            start += count;
                        }
                        assert_eq!(start, residual.len());
                    }
                    assert_eq!(costs.computed, 0x7fff_ffff);
                }
            }
        }
    }

    #[test]
    fn cached_plans_match_exhaustive_choices_at_partition_boundaries() {
        let mut state = 29u32;
        for samples in [
            9, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 255, 256, 257, 4095, 4096,
        ] {
            for predictor in 0..=8 {
                for shift in [0, 16, 31] {
                    let residual: Vec<u32> = (predictor..samples)
                        .map(|index| {
                            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                            let value = (state >> shift).min(u32::MAX - 1);
                            if index % 17 < 8 { value } else { 0 }
                        })
                        .collect();
                    let actual = Rice::plan(&residual, samples, predictor);
                    let expected = exhaustive(&residual, samples, predictor);
                    assert_eq!(
                        (actual.cost, actual.order, actual.parameter_bits),
                        (expected.cost, expected.order, expected.parameter_bits)
                    );
                    let choices = |rice: &Rice| {
                        rice.parameters
                            .iter()
                            .map(|p| match p {
                                Partition::Rice(k) => (false, *k),
                                Partition::Escape(width) => (true, *width),
                            })
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(choices(&actual), choices(&expected));
                }
            }
        }
    }

    #[test]
    fn interleaved_autocorrelation_preserves_every_scalar_reduction_bit() {
        let mut state = 37u32;
        for length in (0usize..=33).chain([63, 64, 65, 127, 128, 129, 4095, 4096]) {
            for mode in 0..4 {
                let windowed: Vec<f64> = (0..length)
                    .map(|index| {
                        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                        match mode {
                            0 => 0.0,
                            1 => -0.0,
                            2 => {
                                let position = 2.0 * index as f64
                                    / length.saturating_sub(1).max(1) as f64
                                    - 1.0;
                                f64::from(state as i32 >> 7) * (1.0 - position * position)
                            }
                            _ => f64::from(state as i32) / f64::from(1u32 << (index % 31)),
                        }
                    })
                    .collect();
                for max_order in 0..=8 {
                    let expected: Vec<_> = (0..=max_order)
                        .map(|lag| {
                            windowed
                                .iter()
                                .skip(lag)
                                .zip(&windowed)
                                .map(|(a, b)| a * b)
                                .sum::<f64>()
                                .to_bits()
                        })
                        .collect();
                    let actual: Vec<_> = autocorrelation(&windowed, max_order)
                        .iter()
                        .map(|v| v.to_bits())
                        .collect();
                    assert_eq!(
                        actual, expected,
                        "length={length}, mode={mode}, order={max_order}"
                    );
                }
            }
        }
    }

    #[test]
    fn residual_planning_includes_escapes_and_five_bit_parameters() {
        assert_eq!(Rice::plan(&[0; 63], 63, 0).cost, 15);
        let one_bit: Vec<u32> = (0..63).map(|i| i % 2).collect();
        assert_eq!(Rice::plan(&one_bit, 63, 0).cost, 78);
        assert_eq!(Rice::plan(&[0x7fff_ffff; 63], 63, 0).cost, 1968);
        let wide: Vec<u32> = std::iter::once(96 << 15)
            .chain(std::iter::repeat_n(0, 62))
            .collect();
        assert_eq!(Rice::plan(&wide, 63, 0).cost, 1115);
        assert_eq!(Rice::plan(&[3 << 29], 1, 0).cost, 43);
    }

    fn exhaustive(residual: &[u32], samples: usize, predictor: usize) -> Rice {
        let mut best = Rice {
            parameter_bits: 4,
            order: 0,
            parameters: Vec::new(),
            cost: u64::MAX,
        };
        let mut padded = vec![0; predictor];
        padded.extend_from_slice(residual);
        for order in 0..=4 {
            if samples % (1 << order) != 0 || samples >> order <= predictor {
                continue;
            }
            for width in 4..=5u8 {
                let mut cost = 6;
                let mut parameters = Vec::new();
                for (index, part) in padded.chunks_exact(samples >> order).enumerate() {
                    let part = if index == 0 { &part[predictor..] } else { part };
                    let (k, mut bits) = (0..(1 << width) - 1)
                        .map(|k| {
                            let bits = part
                                .iter()
                                .map(|value| u64::from(value >> k) + 1 + u64::from(k))
                                .sum::<u64>();
                            (k, bits)
                        })
                        .min_by_key(|(_, cost)| *cost)
                        .unwrap();
                    let mut parameter = Partition::Rice(k);
                    let signed: Vec<i64> = part
                        .iter()
                        .map(|v| {
                            if v & 1 == 0 {
                                i64::from(v >> 1)
                            } else {
                                -i64::from(v >> 1) - 1
                            }
                        })
                        .collect();
                    let escaped_width = (0..=31u8).find(|bits| {
                        signed.iter().all(|value| {
                            if *bits == 0 {
                                *value == 0
                            } else {
                                -(1i64 << (bits - 1)) <= *value && *value < (1i64 << (bits - 1))
                            }
                        })
                    });
                    if let Some(escaped_width) = escaped_width {
                        let escaped = 5 + part.len() as u64 * u64::from(escaped_width);
                        if escaped < bits {
                            parameter = Partition::Escape(escaped_width);
                            bits = escaped;
                        }
                    }
                    parameters.push(parameter);
                    cost += u64::from(width) + bits;
                }
                if cost < best.cost {
                    best = Rice {
                        parameter_bits: width,
                        order,
                        parameters,
                        cost,
                    };
                }
            }
        }
        best
    }

    fn check_residual(residual: &[u32], predictor: usize) {
        let samples = residual.len() + predictor;
        let actual = Rice::plan(residual, samples, predictor);
        let expected = exhaustive(residual, samples, predictor);
        assert_eq!(
            (actual.cost, actual.order, actual.parameter_bits),
            (expected.cost, expected.order, expected.parameter_bits)
        );
        let choices = |rice: &Rice| {
            rice.parameters
                .iter()
                .map(|p| match p {
                    Partition::Rice(k) => (false, *k),
                    Partition::Escape(bits) => (true, *bits),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(choices(&actual), choices(&expected));
        let charged = actual.cost;
        let subframe = Subframe {
            samples: vec![0; samples],
            bits: 24,
            wasted: 0,
            predictor: Predictor::Fixed(predictor as u8),
            residual: residual.to_vec(),
            rice: actual,
            cost: 0,
        };
        let mut output = Bits::new();
        subframe.write(&mut output);
        assert_eq!(
            output.bytes.len() as u64 * 8 + u64::from(output.used),
            8 + predictor as u64 * 24 + charged
        );
        output.align();
        let mut bits = crate::bits::Bits::new(&output.bytes);
        assert_eq!(bits.read(8).unwrap(), (8 + predictor as u32) << 1);
        for _ in 0..predictor {
            assert_eq!(bits.read(24).unwrap(), 0);
        }
        let mut decoded = vec![0; samples];
        super::super::residual(&mut bits, &mut decoded, predictor).unwrap();
        let expected: Vec<i32> = residual
            .iter()
            .map(|v| {
                if v & 1 == 0 {
                    (v >> 1) as i32
                } else {
                    -(i64::from(v >> 1) + 1) as i32
                }
            })
            .collect();
        assert_eq!(&decoded[predictor..], expected);
    }

    #[test]
    fn residual_cost_and_wire_match_exhaustive_legal_choices() {
        for length in 1..=5 {
            for packed in 0..1usize << (length * 2) {
                let values: Vec<u32> = (0..length)
                    .map(|i| ((packed >> (2 * i)) & 3) as u32)
                    .collect();
                check_residual(&values, 0);
            }
        }
        let mut state = 7u32;
        for samples in [5, 15, 16, 17, 31, 32, 63, 64, 127] {
            for predictor in 0..=4 {
                for shift in [0, 8, 16, 24, 31] {
                    let values: Vec<u32> = (predictor..samples)
                        .map(|_| {
                            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                            (state >> shift).min(u32::MAX - 1)
                        })
                        .collect();
                    check_residual(&values, predictor);
                }
            }
        }
    }

    #[test]
    fn residual_parameter_and_escape_boundaries_roundtrip() {
        assert!(matches!(
            Rice::plan(&[0; 5], 5, 0).parameters.as_slice(),
            [Partition::Rice(0)]
        ));
        let tied_methods = Rice::plan(&[3 << 14], 1, 0);
        assert_eq!(tied_methods.parameter_bits, 4);
        assert!(matches!(
            tied_methods.parameters.as_slice(),
            [Partition::Rice(14)]
        ));
        for k in 0..=30 {
            check_residual(&[if k == 0 { 0 } else { 3 << (k - 1) }], 0);
        }
        for value in [0, 1, 0x7fff_ffff, 0x8000_0000, u32::MAX - 2, u32::MAX - 1] {
            check_residual(&vec![value; 63], 0);
        }
        for k in [0, 14, 15] {
            let values: Vec<u32> = std::iter::once(96 << k)
                .chain(std::iter::repeat_n(0, 62))
                .collect();
            let rice = Rice::plan(&values, 63, 0);
            assert!(
                matches!(rice.parameters.as_slice(), [Partition::Rice(parameter)] if *parameter == k)
            );
            assert_eq!(rice.parameter_bits, if k == 15 { 5 } else { 4 });
            check_residual(&values, 0);
        }
        let mixed: Vec<u32> = (0..128)
            .map(|i| match i / 32 {
                0 => 0,
                1 => i % 2,
                2 => 96 << 15,
                _ => u32::MAX - 1,
            })
            .collect();
        check_residual(&mixed, 4);
        let mixed_plan = Rice::plan(&mixed, 132, 4);
        assert!(
            mixed_plan
                .parameters
                .iter()
                .any(|p| matches!(p, Partition::Escape(_)))
        );
        assert!(
            mixed_plan
                .parameters
                .iter()
                .any(|p| matches!(p, Partition::Rice(_)))
        );
        assert!(matches!(
            Rice::plan(&[3 << 29], 1, 0).parameters.as_slice(),
            [Partition::Rice(30)]
        ));
    }

    #[test]
    fn complete_subframes_select_escaped_and_wide_residuals() {
        let ramp: Vec<i64> = (0..4096).map(|i| i * 3 - 6000).collect();
        let escaped = Subframe::plan(&ramp, 16, &mut Vec::new());
        assert!(
            escaped
                .rice
                .parameters
                .iter()
                .any(|p| matches!(p, Partition::Escape(0)))
        );
        let polynomial: Vec<i64> = (0..4096)
            .map(|i| {
                let p = i % 4093;
                (p * p * 13 + p * 71) % ((1 << 24) - 1) - ((1 << 23) - 1)
            })
            .collect();
        let wide = Subframe::plan(&polynomial, 24, &mut Vec::new());
        assert_eq!(wide.rice.parameter_bits, 5);
        assert!(
            wide.rice
                .parameters
                .iter()
                .any(|p| matches!(p, Partition::Rice(15..=30)))
        );
    }

    fn scalar_residual(samples: &[i64], coefficients: &[i16], shift: u8) -> Option<Vec<u32>> {
        let order = coefficients.len();
        if order >= samples.len() {
            return None;
        }
        let mut result = Vec::new();
        for index in order..samples.len() {
            let mut predicted = 0i128;
            for lag in 0..order {
                predicted += i128::from(samples[index - lag - 1]) * i128::from(coefficients[lag]);
            }
            let value = i128::from(samples[index]) - (predicted >> shift);
            if !(-(1i128 << 31) + 1..(1i128 << 31)).contains(&value) {
                return None;
            }
            result.push(((value << 1) ^ (value >> 127)) as u32);
        }
        Some(result)
    }

    #[test]
    fn residual_loops_match_scalar_arithmetic_at_every_order_and_shift() {
        let mut scratch = vec![u32::MAX; 4096];
        let fixed = [&[][..], &[1], &[2, -1], &[3, -3, 1], &[4, -6, 4, -1]];
        let mut state = 7u32;
        for bits in [16, 24, 25, 31] {
            for length in [1, 2, 4, 9, 16, 17, 129, 4096, 4097] {
                let bound = 1i64 << (bits - 1);
                let samples: Vec<i64> = (0..length)
                    .map(|i| {
                        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                        match i % 5 {
                            0 => -bound,
                            1 => bound - 1,
                            2 => 0,
                            _ => i64::from(state) % (2 * bound) - bound,
                        }
                    })
                    .collect();
                for (order, coefficients) in fixed.iter().enumerate() {
                    let actual = Predictor::Fixed(order as u8)
                        .residual(&samples, &mut scratch)
                        .map(|_| scratch.clone());
                    assert_eq!(actual, scalar_residual(&samples, coefficients, 0));
                }
                for order in 1..=8 {
                    let coefficients: Vec<i16> = (0..order)
                        .map(|i| if i % 2 == 0 { 16383 } else { -16384 })
                        .collect();
                    for shift in 0..=15 {
                        let predictor = Predictor::Linear {
                            coefficients: coefficients.clone(),
                            shift,
                        };
                        let actual = predictor
                            .residual(&samples, &mut scratch)
                            .map(|_| scratch.clone());
                        assert_eq!(actual, scalar_residual(&samples, &coefficients, shift));
                    }
                }
            }
        }
        for (value, expected) in [
            (-(1i64 << 31), None),
            (-(1i64 << 31) + 1, Some(u32::MAX - 2)),
            (-1, Some(1)),
            (0, Some(0)),
            (1, Some(2)),
            ((1i64 << 31) - 1, Some(u32::MAX - 1)),
            (1i64 << 31, None),
        ] {
            assert_eq!(fold_residual(value), expected);
        }
    }

    #[test]
    fn rejected_candidates_cannot_corrupt_selected_or_reused_residuals() {
        let mut scratch = vec![u32::MAX; 17];
        let invalid = Predictor::Linear {
            coefficients: vec![16383],
            shift: 0,
        };
        assert_eq!(invalid.residual(&[1, 1, 1 << 24, 0], &mut scratch), None);
        assert_eq!(scratch.len(), 2);
        assert_eq!(
            Predictor::Fixed(1).residual(&[2, 3, 5, 8], &mut scratch),
            Some(1)
        );
        assert_eq!(scratch, [2, 4, 6]);

        let ramp: Vec<i64> = (0..4096).map(|i| i * 2047 + i % 2).collect();
        let mut chosen = Subframe::plan(&ramp, 24, &mut scratch);
        let expected = chosen.residual.clone();
        let cost = chosen.cost;
        chosen.consider(Predictor::Fixed(0), u64::MAX / 2, &mut scratch);
        assert_eq!(chosen.residual, expected);
        assert_eq!(chosen.cost, cost);
        let invalid = Predictor::Linear {
            coefficients: vec![16383; 8],
            shift: 0,
        };
        assert_eq!(invalid.residual(&chosen.samples, &mut scratch), None);
        chosen.consider(invalid, u64::MAX / 2, &mut scratch);
        assert_eq!(chosen.residual, expected);
        assert_eq!(chosen.cost, cost);
        let other: Vec<i64> = ramp.iter().map(|value| value / 3 - 4000).collect();
        let _ = Subframe::plan(&other, 24, &mut scratch);
        assert_eq!(chosen.residual, expected);
    }

    #[test]
    fn linear_coefficient_shifts_respect_exact_power_of_two_boundaries() {
        for shift in 0..=15u8 {
            let boundary = 16382.0 / f64::from(1u32 << shift);
            assert_eq!(lpc_shift(boundary), shift);
            assert_eq!(lpc_shift(f64::from_bits(boundary.to_bits() - 1)), shift);
            assert_eq!(
                lpc_shift(f64::from_bits(boundary.to_bits() + 1)),
                shift.saturating_sub(1)
            );
        }
        assert_eq!(lpc_shift(0.0), 15);
        assert_eq!(lpc_shift(f64::MIN_POSITIVE), 15);
        assert_eq!(lpc_shift(f64::INFINITY), 0);
        assert_eq!(lpc_shift(f64::NAN), 0);
    }

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
        let constant = Subframe::plan(&[0; 4096], 16, &mut Vec::new());
        assert!(matches!(constant.predictor, Predictor::Constant));
        assert_eq!(constant.wasted, 15);
        let ramp: Vec<i64> = (0..4096).map(|i| i * 3 - 6000).collect();
        assert!(matches!(
            Subframe::plan(&ramp, 16, &mut Vec::new()).predictor,
            Predictor::Fixed(_)
        ));
        let tone: Vec<i64> = (0..4096)
            .map(|i| ((i as f64 * 0.13).sin() * 20000.0) as i64)
            .collect();
        assert!(matches!(
            Subframe::plan(&tone, 16, &mut Vec::new()).predictor,
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
            Subframe::plan(&noise, 16, &mut Vec::new()).predictor,
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

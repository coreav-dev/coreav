//! The frame decoder: `raw_data_block()` (ISO/IEC 14496-3 Table 4.3), its
//! elements matched to the configuration's channels, coupling channels
//! (4.6.8.3), and the filterbank of every channel.

use crate::Error;
use crate::bits::Bits;
use crate::config::{Config, skip_program};
use crate::filterbank::{Filterbank, Overlap};
use crate::ics::{Bands, Channel, Info, MAX_BANDS, ZERO_HCB, pow2_eighth, sf_difference};
use crate::tools::{self, MsMask, Noise, bands_of};

const ID_SCE: u32 = 0;
const ID_CPE: u32 = 1;
const ID_CCE: u32 = 2;
const ID_LFE: u32 = 3;
const ID_DSE: u32 = 4;
const ID_PCE: u32 = 5;
const ID_FIL: u32 = 6;

const EXT_SBR_DATA: u32 = 0xd;
const EXT_SBR_DATA_CRC: u32 = 0xe;

/// Coupling channel elements in one frame, at most (a program
/// configuration names up to 15).
const MAX_CCE: usize = 16;
/// Gain element lists of a coupling channel, at most: two for each of its
/// eight targets.
const MAX_GAIN_LISTS: usize = 16;

/// What decoding one frame gave besides its samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Samples per channel: the configuration's frame length.
    pub samples: usize,
    /// Scalefactor bands coded as noise (perceptual noise substitution),
    /// all channels and window groups: a value of the bitstream. A frame
    /// with noise bands has random samples by definition, in its own
    /// output and in its overlap into the next frame.
    pub noise_bands: u32,
    /// Intensity-coded bands of channel pairs whose `ms_mask_present` is 2
    /// (`ms_used` all ones), a value of the bitstream. ISO/IEC 13818-7
    /// 12.2.3 keeps their direction (`invert_intensity` is +1 unless
    /// `ms_mask_present` is 1), which this decoder follows; a decoder that
    /// reverses them writes the same band level with the opposite sign in
    /// the right channel.
    pub intensity_all_ms_bands: u32,
}

/// One coupling target: an element (pair or not, by tag), the channel of
/// a pair (0 left, 1 right), and the gain list applied.
#[derive(Clone, Copy, Debug)]
struct Coupling {
    pair: bool,
    tag: u32,
    side: usize,
    list: usize,
}

/// A coupling channel element of the frame.
#[derive(Clone, Default)]
struct Cce {
    channel: Box<Channel>,
    independent: bool,
    /// 0: coupled before the targets' TNS, 1: after.
    after_tns: bool,
    couplings: Vec<Coupling>,
    /// The gain of each list, group and band; list 0's are all 1.
    gains: Vec<[[f32; MAX_BANDS]; 8]>,
}

/// An AAC LC decoder for one stream.
#[derive(Clone)]
pub struct Decoder {
    config: Config,
    bands: Bands,
    /// The spectrum's normalization, long and short windows: the IMDCT's
    /// 2/N and the output's full scale of 32,768.
    scale: [f32; 2],
    channels: Vec<Box<Channel>>,
    overlaps: Vec<Overlap>,
    cces: Vec<Cce>,
    cce_overlaps: Vec<Overlap>,
    filterbank: Filterbank,
    /// The elements of the frame that a coupling channel can target:
    /// (pair, tag, output channels).
    targets: Vec<(bool, u32, [usize; 2])>,
    time: Vec<f32>,
    /// The stream position of the next packet: part of its noise seed.
    position: u64,
}

impl Decoder {
    /// A decoder for `config`, its state reset.
    pub fn new(config: &Config) -> Self {
        let frame = config.frame_length();
        let bands = Bands::new(config.rate_index, frame);
        let channels = config.channels();
        let full_scale = 32768.0f32;
        Self {
            config: config.clone(),
            bands,
            scale: [
                1.0 / (full_scale * frame as f32),
                1.0 / (full_scale * (frame / 8) as f32),
            ],
            channels: (0..channels).map(|_| Box::default()).collect(),
            overlaps: (0..channels).map(|_| Overlap::new(frame)).collect(),
            cces: Vec::new(),
            cce_overlaps: Vec::new(),
            filterbank: Filterbank::new(frame),
            targets: Vec::new(),
            time: vec![0.0; frame],
            position: 0,
        }
    }

    /// The configuration decoded.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Forgets the frames before: the overlap is silence and the window
    /// shape the sine, as at the start of a stream.
    pub fn reset(&mut self) {
        for overlap in self.overlaps.iter_mut().chain(self.cce_overlaps.iter_mut()) {
            overlap.reset();
        }
    }

    /// Sets the stream position of the next packet decoded (0 for the
    /// first packet of the stream; each decoded packet adds one). The
    /// noise of a frame with noise bands depends on its packet and this
    /// position, so a caller that decodes a stream in parts sets it to
    /// keep the output the same as in one pass.
    pub fn set_position(&mut self, position: u64) {
        self.position = position;
    }

    /// Decodes one frame (`raw_data_block()`, as MP4 and Matroska store
    /// it) into `out`: planar, channel c's samples at `c * frame_length`
    /// on, as `f32` with full scale ±1.0, in the order of
    /// [`Config::channel_mask`]. `out` must hold `channels * frame_length`
    /// samples. A frame that fails to decode resets the decoder, so the
    /// state after any frame depends on that frame alone; an SBR payload
    /// (HE-AAC) is [`Error::Unsupported`].
    pub fn decode(&mut self, packet: &[u8], out: &mut [f32]) -> Result<Frame, Error> {
        let result = self.decode_frame(packet, out);
        self.position = self.position.wrapping_add(1);
        if result.is_err() {
            self.reset();
        }
        result
    }

    fn decode_frame(&mut self, packet: &[u8], out: &mut [f32]) -> Result<Frame, Error> {
        let frame = self.bands.frame;
        let channels = self.config.channels();
        if out.len() < channels.saturating_mul(frame) {
            return Err(Error::Invalid("output buffer smaller than a frame"));
        }
        let mut bits = Bits::new(packet);
        let mut rng = Noise::for_packet(packet, self.position);
        let bands = self.bands;
        let scale = self.scale;
        let layout = &self.config.layout;
        // Configuration slots filled by this frame, by element type.
        let (mut sce, mut cpe, mut lfe) = (0u64, 0u64, 0u64);
        let mut cce = 0usize;
        let mut noise_bands = 0u32;
        let mut intensity_all_ms_bands = 0u32;
        self.targets.clear();
        loop {
            let id = bits.read(3);
            match id {
                ID_SCE | ID_LFE => {
                    let tag = bits.read(4);
                    let slot = if id == ID_SCE {
                        claim(&layout.sce_tags, &mut sce, tag).and_then(|i| layout.sce.get(i))
                    } else {
                        claim(&layout.lfe_tags, &mut lfe, tag).and_then(|i| layout.lfe.get(i))
                    };
                    let Some(&c) = slot else {
                        return Err(Error::Invalid("more elements than the configuration has"));
                    };
                    let Some(channel) = self.channels.get_mut(c) else {
                        return Err(Error::Invalid("channel"));
                    };
                    channel.read(&mut bits, &bands, None, scale)?;
                    tools::noise(channel, &bands, &mut rng, scale);
                    noise_bands += channel.noise_bands;
                    if id == ID_SCE {
                        self.targets.push((false, tag, [c, c]));
                    }
                }
                ID_CPE => {
                    let tag = bits.read(4);
                    let slot =
                        claim(&layout.cpe_tags, &mut cpe, tag).and_then(|i| layout.cpe.get(i));
                    let Some(&[l, r]) = slot else {
                        return Err(Error::Invalid("more elements than the configuration has"));
                    };
                    let common = bits.bit();
                    let mut ms = MsMask::default();
                    let info = if common {
                        let info = Info::read(&mut bits, &bands)?;
                        ms.present = bits.read(2) as u8;
                        if ms.present == 3 {
                            return Err(Error::Invalid("reserved ms_mask_present"));
                        }
                        if ms.present == 1 {
                            for g in 0..info.groups {
                                if let Some(row) = ms.used.get_mut(g) {
                                    for used in row.iter_mut().take(info.max_sfb) {
                                        *used = bits.bit();
                                    }
                                }
                            }
                        }
                        Some(info)
                    } else {
                        None
                    };
                    let (left, right) = pair_mut(&mut self.channels, l, r)?;
                    left.read(&mut bits, &bands, info.as_ref(), scale)?;
                    right.read(&mut bits, &bands, info.as_ref(), scale)?;
                    if common {
                        tools::noise_pair(left, right, &ms, &bands, &mut rng, scale);
                        intensity_all_ms_bands += tools::stereo(left, right, &ms, &bands);
                    } else {
                        tools::noise(left, &bands, &mut rng, scale);
                        tools::noise(right, &bands, &mut rng, scale);
                    }
                    noise_bands += left.noise_bands + right.noise_bands;
                    self.targets.push((true, tag, [l, r]));
                }
                ID_CCE => {
                    if cce >= MAX_CCE {
                        return Err(Error::Invalid("too many coupling channel elements"));
                    }
                    if self.cces.len() <= cce {
                        self.cces.push(Cce::default());
                        self.cce_overlaps.push(Overlap::new(frame));
                    }
                    let Some(element) = self.cces.get_mut(cce) else {
                        return Err(Error::Invalid("coupling channel"));
                    };
                    element.read(&mut bits, &bands, scale)?;
                    tools::noise(&mut element.channel, &bands, &mut rng, scale);
                    tools::tns(&mut element.channel, &bands);
                    noise_bands += element.channel.noise_bands;
                    cce += 1;
                }
                ID_DSE => {
                    bits.skip(4); // element_instance_tag
                    let align = bits.bit();
                    let mut count = bits.read(8) as usize;
                    if count == 255 {
                        count += bits.read(8) as usize;
                    }
                    if align {
                        bits.align(0);
                    }
                    bits.skip(8 * count);
                }
                ID_PCE => skip_program(&mut bits),
                ID_FIL => {
                    let mut count = bits.read(4) as usize;
                    if count == 15 {
                        // cnt += esc_count - 1: an escape of 0 means 14.
                        count = 14 + bits.read(8) as usize;
                    }
                    if count > 0 {
                        let extension = bits.peek32() >> 28;
                        if extension == EXT_SBR_DATA || extension == EXT_SBR_DATA_CRC {
                            return Err(Error::Unsupported("HE-AAC (SBR)"));
                        }
                        bits.skip(8 * count);
                    }
                }
                _ => break,
            }
            if bits.overrun() {
                return Err(Error::Invalid("truncated frame"));
            }
        }
        let filled = |used: u64, slots: usize| used.count_ones() as usize == slots;
        if !filled(sce, layout.sce.len())
            || !filled(cpe, layout.cpe.len())
            || !filled(lfe, layout.lfe.len())
        {
            return Err(Error::Invalid(
                "a frame lacks an element of the configuration",
            ));
        }

        let cces = self.cces.get(..cce).unwrap_or_default();
        for element in cces.iter().filter(|e| !e.independent && !e.after_tns) {
            element.couple_spectra(&mut self.channels, &self.targets, &bands);
        }
        for channel in &mut self.channels {
            tools::tns(channel, &bands);
        }
        for element in cces.iter().filter(|e| !e.independent && e.after_tns) {
            element.couple_spectra(&mut self.channels, &self.targets, &bands);
        }
        for (c, (channel, overlap)) in self
            .channels
            .iter()
            .zip(self.overlaps.iter_mut())
            .enumerate()
        {
            let Some(dst) = out.get_mut(c * frame..(c + 1) * frame) else {
                break;
            };
            self.filterbank
                .synthesize(&channel.info, &channel.spec, overlap, dst);
        }
        for (element, overlap) in cces.iter().zip(self.cce_overlaps.iter_mut()) {
            if !element.independent {
                continue;
            }
            self.filterbank.synthesize(
                &element.channel.info,
                &element.channel.spec,
                overlap,
                &mut self.time,
            );
            for coupling in &element.couplings {
                let Some(c) = target(&self.targets, coupling) else {
                    continue;
                };
                let gain = element
                    .gains
                    .get(coupling.list)
                    .map_or(1.0, |list| list[0][0]);
                if let Some(dst) = out.get_mut(c * frame..(c + 1) * frame) {
                    for (o, &t) in dst.iter_mut().zip(&self.time) {
                        *o += gain * t;
                    }
                }
            }
        }
        Ok(Frame {
            samples: frame,
            noise_bands,
            intensity_all_ms_bands,
        })
    }
}

/// The configuration slot of an element with tag `tag` among slots with
/// `tags`, none of those in `used` (a bit per slot), marked used: the slot
/// the configuration names that tag for, else the first free one, so that
/// elements without named tags go in their order of appearance.
fn claim(tags: &[Option<u8>], used: &mut u64, tag: u32) -> Option<usize> {
    let free = |i: usize| i < 64 && (*used >> i) & 1 == 0;
    let slot = (0..tags.len())
        .find(|&i| free(i) && tags.get(i).copied().flatten() == Some(tag as u8))
        .or_else(|| (0..tags.len()).find(|&i| free(i)))?;
    *used |= 1 << slot;
    Some(slot)
}

/// Two distinct elements of `v`, mutably.
fn pair_mut(
    v: &mut [Box<Channel>],
    a: usize,
    b: usize,
) -> Result<(&mut Channel, &mut Channel), Error> {
    if a < b {
        let (low, high) = v.split_at_mut(b);
        if let (Some(x), Some(y)) = (low.get_mut(a), high.first_mut()) {
            return Ok((x, y));
        }
    } else if b < a {
        let (low, high) = v.split_at_mut(a);
        if let (Some(y), Some(x)) = (low.get_mut(b), high.first_mut()) {
            return Ok((x, y));
        }
    }
    Err(Error::Invalid("channel pair"))
}

/// The output channel a coupling targets, if the frame has its element.
fn target(targets: &[(bool, u32, [usize; 2])], coupling: &Coupling) -> Option<usize> {
    targets
        .iter()
        .find(|(pair, tag, _)| *pair == coupling.pair && *tag == coupling.tag)
        .and_then(|(_, _, channels)| channels.get(coupling.side).copied())
}

impl Cce {
    /// Reads `coupling_channel_element()` (Table 4.8) and its gains
    /// (4.6.8.3.3, `couple_channel()`).
    fn read(&mut self, bits: &mut Bits<'_>, bands: &Bands, scale: [f32; 2]) -> Result<(), Error> {
        bits.skip(4); // element_instance_tag
        self.independent = bits.bit();
        let coupled = bits.read(3) as usize + 1;
        self.couplings.clear();
        let mut list = 0;
        for _ in 0..coupled {
            let pair = bits.bit();
            let tag = bits.read(4);
            if !pair {
                self.couplings.push(Coupling {
                    pair,
                    tag,
                    side: 0,
                    list,
                });
                list += 1;
                continue;
            }
            let l = bits.bit();
            let r = bits.bit();
            if !l && !r {
                self.couplings.push(Coupling {
                    pair,
                    tag,
                    side: 0,
                    list,
                });
                self.couplings.push(Coupling {
                    pair,
                    tag,
                    side: 1,
                    list,
                });
                list += 1;
            }
            if l {
                self.couplings.push(Coupling {
                    pair,
                    tag,
                    side: 0,
                    list,
                });
                list += 1;
            }
            if r {
                self.couplings.push(Coupling {
                    pair,
                    tag,
                    side: 1,
                    list,
                });
                list += 1;
            }
        }
        let lists = list.min(MAX_GAIN_LISTS);
        self.after_tns = bits.bit();
        let signed = bits.bit();
        let gain_scale = bits.read(2);
        self.channel.read(bits, bands, None, scale)?;
        self.gains.resize(lists, [[1.0; MAX_BANDS]; 8]);
        if let Some(first) = self.gains.first_mut() {
            *first = [[1.0; MAX_BANDS]; 8];
        }
        let info = self.channel.info;
        // The gain of a gain element: its sign (its low bit, with
        // gain_element_sign) and cc_scale^-gain, cc_scale = 2^(2^scale / 8)
        // (4.6.8.3.3, Table 4.154). The standard's pseudo-code writes
        // cc_scale^gain; the conformance references (al07) attenuate as the
        // element grows, so the exponent is negated here.
        let to_gain = |element: i32| -> f32 {
            let (sign, gain) = if signed {
                (if element & 1 == 1 { -1.0 } else { 1.0 }, element >> 1)
            } else {
                (1.0, element)
            };
            sign * pow2_eighth(gain.saturating_mul(1 << gain_scale).saturating_neg())
        };
        for gains in self.gains.iter_mut().skip(1) {
            let common = self.independent || bits.bit();
            if common {
                let value = to_gain(sf_difference(bits));
                *gains = [[value; MAX_BANDS]; 8];
                continue;
            }
            let mut a = 0i32;
            for g in 0..info.groups {
                let (Some(row), Some(cbs)) = (gains.get_mut(g), self.channel.cb.get(g)) else {
                    break;
                };
                for (slot, &cb) in row.iter_mut().zip(cbs).take(info.max_sfb) {
                    if cb != ZERO_HCB {
                        a += sf_difference(bits);
                        *slot = to_gain(a);
                    }
                }
            }
        }
        if bits.overrun() {
            return Err(Error::Invalid("truncated coupling channel element"));
        }
        Ok(())
    }

    /// Adds the coupling channel's spectrum, by each target's gains, to
    /// the spectra of the targets the frame has. Bands the coupling
    /// channel codes as zero are skipped (4.6.8.3.3, the second
    /// `couple_channel()`), even where its own TNS filter has spread
    /// energy into them; the al07 references at 48 kHz and up need this.
    fn couple_spectra(
        &self,
        channels: &mut [Box<Channel>],
        targets: &[(bool, u32, [usize; 2])],
        bands: &Bands,
    ) {
        for coupling in &self.couplings {
            let Some(c) = target(targets, coupling) else {
                continue;
            };
            let (Some(dest), Some(gains)) = (channels.get_mut(c), self.gains.get(coupling.list))
            else {
                continue;
            };
            for (g, sfb, start, end) in bands_of(self.channel.info, bands) {
                let cb = self.channel.cb.get(g).and_then(|row| row.get(sfb)).copied();
                if cb == Some(ZERO_HCB) {
                    continue;
                }
                let gain = gains
                    .get(g)
                    .and_then(|row| row.get(sfb))
                    .copied()
                    .unwrap_or(0.0);
                if let (Some(d), Some(s)) = (
                    dest.spec.get_mut(start..end),
                    self.channel.spec.get(start..end),
                ) {
                    for (o, &v) in d.iter_mut().zip(s) {
                        *o += gain * v;
                    }
                }
            }
        }
    }
}

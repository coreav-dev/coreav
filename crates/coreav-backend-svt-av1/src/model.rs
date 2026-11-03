//! Checked settings and the retained-input envelope of a segmented job.

use crate::Error;

/// The maximum retained-input budget, not total worker memory.
pub const RETAINED_BYTES: u64 = 1 << 30;

/// An encoder packet's maximum copied size.
pub const MAX_PACKET_BYTES: usize = 64 << 20;

/// Fixed, explicitly signaled SDR colour metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Color {
    primaries: u8,
    transfer: u8,
    matrix: u8,
    full_range: bool,
    chroma_position: u8,
}

impl Color {
    /// Checks assigned H.273 values and the supported SDR 4:2:0 subset.
    pub fn new(
        primaries: u8,
        transfer: u8,
        matrix: u8,
        full_range: bool,
        chroma_position: u8,
    ) -> Result<Self, Error> {
        if !matches!(primaries, 1 | 2 | 4..=12 | 22)
            || !matches!(transfer, 1 | 2 | 4..=18)
            || !matches!(matrix, 0..=2 | 4..=14)
            || chroma_position > 2
        {
            return Err(Error::Invalid("invalid AV1 colour description"));
        }
        if matches!(transfer, 16 | 18) || matrix == 0 {
            return Err(Error::Unsupported("AV1 encoding requires SDR YUV 4:2:0"));
        }
        if primaries == 2 && matches!(matrix, 12 | 13) {
            return Err(Error::Invalid(
                "derived matrices require specified primaries",
            ));
        }
        Ok(Self {
            primaries,
            transfer,
            matrix,
            full_range,
            chroma_position,
        })
    }

    /// The H.273 primaries code.
    pub fn primaries(self) -> u8 {
        self.primaries
    }

    /// The H.273 transfer code.
    pub fn transfer(self) -> u8 {
        self.transfer
    }

    /// The H.273 matrix code.
    pub fn matrix(self) -> u8 {
        self.matrix
    }

    /// Whether full-range samples are signaled.
    pub fn full_range(self) -> bool {
        self.full_range
    }

    /// AV1 chroma position, from zero through two.
    pub fn chroma_position(self) -> u8 {
        self.chroma_position
    }
}

impl Default for Color {
    fn default() -> Self {
        Self {
            primaries: 2,
            transfer: 2,
            matrix: 2,
            full_range: false,
            chroma_position: 0,
        }
    }
}

/// Checked fixed geometry, rational clock, and integer SVT rate settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Settings {
    width: u32,
    height: u32,
    rate_num: u32,
    rate_den: u32,
    preset: u8,
    crf: u8,
    color: Color,
    raw_bytes: usize,
    keyint: u32,
}

impl Settings {
    /// Checks the installed API's input range and computes the five-second interval.
    pub fn new(
        width: u32,
        height: u32,
        rate: (u32, u32),
        preset: u8,
        crf: u8,
        color: Color,
    ) -> Result<Self, Error> {
        if !(64..=16_384).contains(&width)
            || !(64..=8_704).contains(&height)
            || width % 2 != 0
            || height % 2 != 0
        {
            return Err(Error::Unsupported(
                "SVT-AV1 requires even dimensions within 64x64 to 16384x8704",
            ));
        }
        if rate.0 == 0 || rate.1 == 0 || preset > 13 || crf > 63 {
            return Err(Error::Invalid("invalid AV1 frame rate, preset, or CRF"));
        }
        let keyint = u32::try_from((u64::from(rate.0) * 5 / u64::from(rate.1)).max(1))
            .ok()
            .filter(|value| *value <= i32::MAX as u32)
            .ok_or(Error::Unsupported(
                "AV1 keyframe interval exceeds the native field",
            ))?;
        let raw_bytes = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|value| value.checked_mul(3))
            .and_then(|value| usize::try_from(value / 2).ok())
            .ok_or(Error::Limit("AV1 picture size overflows"))?;
        Ok(Self {
            width,
            height,
            rate_num: rate.0,
            rate_den: rate.1,
            preset,
            crf,
            color,
            raw_bytes,
            keyint,
        })
    }

    /// Width in samples.
    pub fn width(self) -> u32 {
        self.width
    }

    /// Height in samples.
    pub fn height(self) -> u32 {
        self.height
    }

    /// The unchanged rational source rate.
    pub fn rate(self) -> (u32, u32) {
        (self.rate_num, self.rate_den)
    }

    /// Numeric SVT preset.
    pub fn preset(self) -> u8 {
        self.preset
    }

    /// Integer SVT CRF, not an x264 CRF conversion.
    pub fn crf(self) -> u8 {
        self.crf
    }

    /// The signaled colour metadata.
    pub fn color(self) -> Color {
        self.color
    }

    /// Bytes in tightly packed Y, U, V planes.
    pub fn raw_bytes(self) -> usize {
        self.raw_bytes
    }

    /// The configured five-second closed-GOP interval, at least one picture.
    pub fn keyint(self) -> u32 {
        self.keyint
    }
}

/// The complete retained-input envelope, immutable throughout a job.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Retention {
    budget: u64,
    fixed: u64,
    picture: u64,
    pictures: u32,
    slot: u64,
}

impl Retention {
    /// Accounts for the EOF allocation, owner, slots, and each native input allocation.
    pub fn new(
        settings: Settings,
        budget: u64,
        native_input_bytes: usize,
        slot_bytes: usize,
        owner_bytes: usize,
    ) -> Result<Self, Error> {
        if native_input_bytes == 0 || slot_bytes == 0 {
            return Err(Error::Invalid("invalid native input footprint"));
        }
        let budget = budget.min(RETAINED_BYTES);
        let fixed = native_input_bytes
            .checked_add(owner_bytes)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(Error::Limit("AV1 retained owner size overflows"))?;
        let picture = settings
            .raw_bytes()
            .checked_add(native_input_bytes)
            .and_then(|value| value.checked_add(slot_bytes))
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(Error::Limit("AV1 retained picture size overflows"))?;
        let slot = u64::try_from(slot_bytes)
            .map_err(|_| Error::Limit("AV1 retained slot size overflows"))?;
        let capacity = budget.saturating_sub(fixed) / picture;
        let pictures = u32::try_from(capacity.min(u64::from(settings.keyint())))
            .map_err(|_| Error::Limit("AV1 retained count overflows"))?;
        if pictures == 0 {
            return Err(Error::Limit(
                "one retained AV1 picture does not fit the budget",
            ));
        }
        Ok(Self {
            budget,
            fixed,
            picture,
            pictures,
            slot,
        })
    }

    /// Maximum retained input bytes, excluding native internal allocations.
    pub fn budget(self) -> u64 {
        self.budget
    }

    /// Retained fixed bytes including the EOF allocation.
    pub fn fixed_bytes(self) -> u64 {
        self.fixed
    }

    /// Charged retained bytes per submitted picture.
    pub fn picture_bytes(self) -> u64 {
        self.picture
    }

    /// The immutable maximum pictures per segment.
    pub fn pictures(self) -> u32 {
        self.pictures
    }

    /// Exact retained bytes after submission, including every preallocated owner slot.
    pub fn peak_bytes(self, submitted: u32) -> Result<u64, Error> {
        if submitted > self.pictures {
            return Err(Error::Invalid("AV1 retained count exceeds its session cap"));
        }
        let slots = u64::from(self.pictures).checked_mul(self.slot);
        let pictures = self
            .picture
            .checked_sub(self.slot)
            .and_then(|bytes| bytes.checked_mul(u64::from(submitted)));
        slots
            .and_then(|slots| self.fixed.checked_add(slots))
            .and_then(|bytes| bytes.checked_add(pictures?))
            .filter(|bytes| *bytes <= self.budget)
            .ok_or(Error::Limit("AV1 retained peak exceeds its envelope"))
    }

    /// The bound F + S * P, already checked by construction.
    pub fn bytes(self) -> u64 {
        self.fixed + u64::from(self.pictures) * self.picture
    }
}

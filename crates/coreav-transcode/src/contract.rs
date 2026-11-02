//! Codec-neutral video submission and packet accounting.
//!
//! Codec readers validate their bitstreams and configuration separately.
//! Packet accounting binds their complete samples, in decode order, to
//! submitted frames and derives timing from the source presentation times.

use std::num::NonZeroU64;

use coreav_decode::{Error, ErrorKind};
use coreav_frame::{Picture, PixelFormat};

use crate::timing::{SampleTime, sample_times};

/// A frame's position in submission and presentation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameId(u64);

impl FrameId {
    /// The zero-based position, for worker protocols and codec readers.
    pub fn index(self) -> u64 {
        self.0
    }
}

/// Fixed raw geometry and format for one encoder session.
#[derive(Clone, Copy, Debug)]
pub struct VideoLayout {
    width: u32,
    height: u32,
    format: PixelFormat,
    bytes: usize,
}

impl VideoLayout {
    /// Checks geometry, pixel format, arithmetic and the raw-frame budget.
    /// Backend-specific format and geometry restrictions remain separate.
    pub fn new(
        width: u32,
        height: u32,
        format: PixelFormat,
        max_frame_bytes: u64,
    ) -> Result<Self, Error> {
        if width == 0 || height == 0 || format.validate().is_err() {
            return Err(invalid("invalid encoder frame layout"));
        }
        let bytes = (0..format.plane_count())
            .try_fold(0usize, |sum, plane| {
                let (_, rows) = format.plane_size(plane, width, height)?;
                let row = format.row_bytes(plane, width, height)?;
                sum.checked_add(usize::try_from(rows).ok()?.checked_mul(row)?)
            })
            .ok_or_else(|| limit("encoder frame size overflows"))?;
        if u64::try_from(bytes).map_or(true, |bytes| bytes > max_frame_bytes) {
            return Err(limit("encoder frame exceeds the raw-frame budget"));
        }
        Ok(Self {
            width,
            height,
            format,
            bytes,
        })
    }
}

/// A checked, owned raw frame. No borrowed or device memory crosses this API.
#[derive(Debug)]
pub struct SubmittedFrame {
    id: FrameId,
    pts: i64,
    picture: Picture,
}

impl SubmittedFrame {
    /// Its position in presentation order.
    pub fn id(&self) -> FrameId {
        self.id
    }

    /// Source presentation time, in the session's source time base.
    pub fn pts(&self) -> i64 {
        self.pts
    }

    /// The owned picture, checked against the session's layout.
    pub fn picture(&self) -> &Picture {
        &self.picture
    }

    /// Transfers the picture to the encoder's bounded queue.
    pub fn into_picture(self) -> Picture {
        self.picture
    }
}

/// Bounded presentation times. Pixel storage belongs to each submitted frame.
#[derive(Debug)]
pub struct Submissions {
    layout: VideoLayout,
    max_frames: u64,
    times: Vec<i64>,
}

impl Submissions {
    /// Starts accounting with one fixed raw layout and a frame-count limit.
    pub fn new(layout: VideoLayout, max_frames: u64) -> Self {
        Self {
            layout,
            max_frames,
            times: Vec::new(),
        }
    }

    /// Checks a frame before assigning its identity. Times increase strictly.
    /// A failed submission leaves the recorded times unchanged.
    pub fn submit(&mut self, picture: Picture, pts: i64) -> Result<SubmittedFrame, Error> {
        if (
            picture.width(),
            picture.height(),
            picture.format(),
            picture.packed_len(),
        ) != (
            self.layout.width,
            self.layout.height,
            self.layout.format,
            self.layout.bytes,
        ) {
            return Err(invalid("submitted frame differs from the encoder layout"));
        }
        if self.times.last().is_some_and(|last| pts <= *last) {
            return Err(invalid("submitted frame times must increase strictly"));
        }
        let index = u64::try_from(self.times.len())
            .map_err(|_| limit("submitted frame count overflows"))?;
        if index >= self.max_frames {
            return Err(limit("submitted frame count exceeds its limit"));
        }
        self.times
            .try_reserve(1)
            .map_err(|_| limit("cannot store submitted frame time"))?;
        self.times.push(pts);
        Ok(SubmittedFrame {
            id: FrameId(index),
            pts,
            picture,
        })
    }
}

/// A complete sample's identity, extent and random-access status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PacketMeta {
    frame: FrameId,
    bytes: u32,
    sync: bool,
}

impl PacketMeta {
    /// The submitted frame this sample reconstructs.
    pub fn frame(self) -> FrameId {
        self.frame
    }

    /// Sample bytes, excluding container overhead and separate configuration.
    pub fn bytes(self) -> u32 {
        self.bytes
    }

    /// Whether the codec reader validated this as a random-access sample.
    pub fn sync(self) -> bool {
        self.sync
    }
}

/// A bounded Rust-owned sample, independent of codec and framework buffers.
#[derive(Debug)]
pub struct EncodedPacket {
    meta: PacketMeta,
    data: Vec<u8>,
}

impl EncodedPacket {
    /// Metadata checked against the submitted work and output limits.
    pub fn meta(&self) -> PacketMeta {
        self.meta
    }

    /// Sample bytes. Codec syntax validation belongs to the codec reader.
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

/// Output extent and count limits for a video session.
#[derive(Clone, Copy, Debug)]
pub struct PacketLimits {
    /// Maximum number of complete samples retained before finalization.
    pub max_packets: u64,
    /// Maximum bytes in one complete sample.
    pub max_packet_bytes: u32,
    /// Maximum bytes across all samples.
    pub max_output_bytes: u64,
}

/// Bounded decode-order metadata; packet payloads may be spooled immediately.
#[derive(Debug)]
pub struct Packets {
    limits: PacketLimits,
    bytes: u64,
    meta: Vec<PacketMeta>,
}

impl Packets {
    /// Starts a packet ledger with caller-supplied limits.
    pub fn new(limits: PacketLimits) -> Self {
        Self {
            limits,
            bytes: 0,
            meta: Vec::new(),
        }
    }

    /// Accepts an owned sample after checking its extent and submission ID.
    /// The caller's codec reader must first validate syntax and sync status.
    pub fn accept(
        &mut self,
        data: Vec<u8>,
        presentation: u64,
        sync: bool,
        submitted: &Submissions,
    ) -> Result<EncodedPacket, Error> {
        if usize::try_from(presentation)
            .ok()
            .is_none_or(|id| id >= submitted.times.len())
        {
            return Err(invalid("encoded packet names an unsubmitted frame"));
        }
        if data.is_empty() {
            return Err(invalid("encoded packet is empty"));
        }
        let size = u32::try_from(data.len())
            .ok()
            .filter(|size| *size <= self.limits.max_packet_bytes)
            .ok_or_else(|| limit("encoded packet exceeds its byte limit"))?;
        let bytes = self
            .bytes
            .checked_add(u64::from(size))
            .filter(|bytes| *bytes <= self.limits.max_output_bytes)
            .ok_or_else(|| limit("encoded video exceeds its byte limit"))?;
        if u64::try_from(self.meta.len()).map_or(true, |count| count >= self.limits.max_packets) {
            return Err(limit("encoded packet count exceeds its limit"));
        }
        if self.meta.len() >= submitted.times.len() {
            return Err(invalid(
                "encoder returned more packets than submitted frames",
            ));
        }
        self.meta
            .try_reserve(1)
            .map_err(|_| limit("cannot store encoded packet metadata"))?;
        let meta = PacketMeta {
            frame: FrameId(presentation),
            bytes: size,
            sync,
        };
        self.meta.push(meta);
        self.bytes = bytes;
        Ok(EncodedPacket { meta, data })
    }

    /// Requires exactly one sample per submitted frame and derives times.
    /// Samples remain in decode order; their identities form a permutation
    /// of the presentation-order submissions. Final duration must be positive.
    pub fn finish(
        self,
        submitted: &Submissions,
        last_duration: NonZeroU64,
    ) -> Result<Vec<(PacketMeta, SampleTime)>, Error> {
        if submitted.times.is_empty() {
            return Err(invalid("encoder returned no submitted video"));
        }
        let order: Vec<u64> = self.meta.iter().map(|meta| meta.frame.index()).collect();
        let times = sample_times(&submitted.times, &order, last_duration.get())
            .map_err(|error| invalid(&error.to_string()))?;
        Ok(self.meta.into_iter().zip(times).collect())
    }
}

fn invalid(message: &str) -> Error {
    Error::new(ErrorKind::InvalidData, message)
}

fn limit(message: &str) -> Error {
    Error::new(ErrorKind::Limit, message)
}

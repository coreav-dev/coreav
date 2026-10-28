//! The element kinds of the Matroska schema, for the structure audit
//! (`docs/design/policy-matroska.md`, decision 8): master elements, whose
//! children are read in turn, and the kinds that bound an element's length
//! (RFC 8794 7.1 to 7.8: an integer of at most 8 octets, a float of 0, 4 or
//! 8, a date of 0 or 8, the binaries with a `length`), and strings and other
//! binaries, which the audit bounds by size. Generated from the Matroska EBML
//! Schema of RFC 9559 (`ebml_matroska.xml`, the source of its section 5), and
//! CRC-32 from RFC 8794 11.3.2. Void and unknown IDs have no kind: readers
//! skip them by their size, whatever it is.

/// What the schema says about an element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A master element: its data holds child elements.
    Master,
    /// An unsigned or signed integer: at most 8 octets.
    Integer,
    /// A float: 0, 4 or 8 octets.
    Float,
    /// A date: 0 or 8 octets.
    Date,
    /// A binary of exactly this many octets.
    Fixed(u64),
    /// A string (ASCII or UTF-8): any length in RFC 8794.
    Text,
    /// A binary of any length in RFC 8794 (blocks among them).
    Binary,
}

impl Kind {
    /// Whether an element of this kind may have `len` octets of data, by
    /// its type alone.
    pub(crate) fn allows(self, len: u64) -> bool {
        match self {
            Self::Master | Self::Text | Self::Binary => true,
            Self::Integer => len <= 8,
            Self::Float => matches!(len, 0 | 4 | 8),
            Self::Date => matches!(len, 0 | 8),
            Self::Fixed(size) => len == size,
        }
    }
}

/// The kind of the element with this ID; `None` for Void and unknown IDs.
pub(crate) fn kind(id: u32) -> Option<Kind> {
    match id {
        // ChapterDisplay
        0x80 |
        // Slices
        0x8E |
        // ChapterTrack
        0x8F |
        // BlockGroup
        0xA0 |
        // BlockMore
        0xA6 |
        // TrackEntry
        0xAE |
        // ChapterAtom
        0xB6 |
        // CueTrackPositions
        0xB7 |
        // CuePoint
        0xBB |
        // ReferenceFrame
        0xC8 |
        // CueReference
        0xDB |
        // Video
        0xE0 |
        // Audio
        0xE1 |
        // TrackOperation
        0xE2 |
        // TrackCombinePlanes
        0xE3 |
        // TrackPlane
        0xE4 |
        // TimeSlice
        0xE8 |
        // TrackJoinBlocks
        0xE9 |
        // BlockAdditionMapping
        0x41E4 |
        // EditionDisplay
        0x4520 |
        // EditionEntry
        0x45B9 |
        // ContentEncAESSettings
        0x47E7 |
        // Seek
        0x4DBB |
        // ContentCompression
        0x5034 |
        // ContentEncryption
        0x5035 |
        // Colour
        0x55B0 |
        // MasteringMetadata
        0x55D0 |
        // SilentTracks
        0x5854 |
        // AttachedFile
        0x61A7 |
        // ContentEncoding
        0x6240 |
        // Targets
        0x63C0 |
        // TrackTranslate
        0x6624 |
        // SimpleTag
        0x67C8 |
        // ChapProcessCommand
        0x6911 |
        // ChapterTranslate
        0x6924 |
        // ChapProcess
        0x6944 |
        // ContentEncodings
        0x6D80 |
        // Tag
        0x7373 |
        // BlockAdditions
        0x75A1 |
        // Projection
        0x7670 |
        // Chapters
        0x1043A770 |
        // SeekHead
        0x114D9B74 |
        // Tags
        0x1254C367 |
        // Info
        0x1549A966 |
        // Tracks
        0x1654AE6B |
        // Segment
        0x18538067 |
        // Attachments
        0x1941A469 |
        // Cues
        0x1C53BB6B |
        // Cluster
        0x1F43B675
        => Some(Kind::Master),
        // TrackType
        0x83 |
        // FlagDefault
        0x88 |
        // ChapterTrackUID
        0x89 |
        // ChapterTimeStart
        0x91 |
        // ChapterTimeEnd
        0x92 |
        // CueRefTime
        0x96 |
        // CueRefCluster
        0x97 |
        // ChapterFlagHidden
        0x98 |
        // FlagInterlaced
        0x9A |
        // BlockDuration
        0x9B |
        // FlagLacing
        0x9C |
        // FieldOrder
        0x9D |
        // Channels
        0x9F |
        // Position
        0xA7 |
        // CodecDecodeAll
        0xAA |
        // PrevSize
        0xAB |
        // PixelWidth
        0xB0 |
        // CueDuration
        0xB2 |
        // CueTime
        0xB3 |
        // FlagEnabled
        0xB9 |
        // PixelHeight
        0xBA |
        // TrickTrackUID
        0xC0 |
        // TrickTrackFlag
        0xC6 |
        // TrickMasterTrackUID
        0xC7 |
        // ReferenceOffset
        0xC9 |
        // ReferenceTimestamp
        0xCA |
        // BlockAdditionID
        0xCB |
        // LaceNumber
        0xCC |
        // FrameNumber
        0xCD |
        // Delay
        0xCE |
        // SliceDuration
        0xCF |
        // TrackNumber
        0xD7 |
        // TrackPlaneUID
        0xE5 |
        // TrackPlaneType
        0xE6 |
        // Timestamp
        0xE7 |
        // CueCodecState
        0xEA |
        // CueRefCodecState
        0xEB |
        // TrackJoinUID
        0xED |
        // BlockAddID
        0xEE |
        // CueRelativePosition
        0xF0 |
        // CueClusterPosition
        0xF1 |
        // CueTrack
        0xF7 |
        // ReferencePriority
        0xFA |
        // ReferenceBlock
        0xFB |
        // ReferenceVirtual
        0xFD |
        // BlockAddIDType
        0x41E7 |
        // BlockAddIDValue
        0x41F0 |
        // ContentCompAlgo
        0x4254 |
        // EBMLMaxIDLength
        0x42F2 |
        // EBMLMaxSizeLength
        0x42F3 |
        // TagDefault
        0x4484 |
        // TagDefaultBogus
        0x44B4 |
        // ChapterSkipType
        0x4588 |
        // ChapterFlagEnabled
        0x4598 |
        // EditionUID
        0x45BC |
        // EditionFlagHidden
        0x45BD |
        // EditionFlagDefault
        0x45DB |
        // EditionFlagOrdered
        0x45DD |
        // FileUsedStartTime
        0x4661 |
        // FileUsedEndTime
        0x4662 |
        // FileUID
        0x46AE |
        // ContentEncAlgo
        0x47E1 |
        // ContentSigAlgo
        0x47E5 |
        // ContentSigHashAlgo
        0x47E6 |
        // AESSettingsCipherMode
        0x47E8 |
        // ContentEncodingOrder
        0x5031 |
        // ContentEncodingScope
        0x5032 |
        // ContentEncodingType
        0x5033 |
        // Emphasis
        0x52F1 |
        // CueRefNumber
        0x535F |
        // CueBlockNumber
        0x5378 |
        // TrackOffset
        0x537F |
        // SeekPosition
        0x53AC |
        // StereoMode
        0x53B8 |
        // OldStereoMode
        0x53B9 |
        // AlphaMode
        0x53C0 |
        // PixelCropBottom
        0x54AA |
        // DisplayWidth
        0x54B0 |
        // DisplayUnit
        0x54B2 |
        // AspectRatioType
        0x54B3 |
        // DisplayHeight
        0x54BA |
        // PixelCropTop
        0x54BB |
        // PixelCropLeft
        0x54CC |
        // PixelCropRight
        0x54DD |
        // FlagForced
        0x55AA |
        // FlagHearingImpaired
        0x55AB |
        // FlagVisualImpaired
        0x55AC |
        // FlagTextDescriptions
        0x55AD |
        // FlagOriginal
        0x55AE |
        // FlagCommentary
        0x55AF |
        // MatrixCoefficients
        0x55B1 |
        // BitsPerChannel
        0x55B2 |
        // ChromaSubsamplingHorz
        0x55B3 |
        // ChromaSubsamplingVert
        0x55B4 |
        // CbSubsamplingHorz
        0x55B5 |
        // CbSubsamplingVert
        0x55B6 |
        // ChromaSitingHorz
        0x55B7 |
        // ChromaSitingVert
        0x55B8 |
        // Range
        0x55B9 |
        // TransferCharacteristics
        0x55BA |
        // Primaries
        0x55BB |
        // MaxCLL
        0x55BC |
        // MaxFALL
        0x55BD |
        // MaxBlockAdditionID
        0x55EE |
        // CodecDelay
        0x56AA |
        // SeekPreRoll
        0x56BB |
        // SilentTrackNumber
        0x58D7 |
        // BitDepth
        0x6264 |
        // ChapterPhysicalEquiv
        0x63C3 |
        // TagChapterUID
        0x63C4 |
        // TagTrackUID
        0x63C5 |
        // TagAttachmentUID
        0x63C6 |
        // TagBlockAddIDValue
        0x63C7 |
        // TagEditionUID
        0x63C9 |
        // TrackTranslateCodec
        0x66BF |
        // TrackTranslateEditionUID
        0x66FC |
        // TargetTypeValue
        0x68CA |
        // ChapProcessTime
        0x6922 |
        // ChapProcessCodecID
        0x6955 |
        // ChapterTranslateCodec
        0x69BF |
        // ChapterTranslateEditionUID
        0x69FC |
        // MinCache
        0x6DE7 |
        // MaxCache
        0x6DF8 |
        // ChapterSegmentEditionUID
        0x6EBC |
        // TrackOverlay
        0x6FAB |
        // ChapterUID
        0x73C4 |
        // TrackUID
        0x73C5 |
        // AttachmentLink
        0x7446 |
        // DiscardPadding
        0x75A2 |
        // ProjectionType
        0x7671 |
        // DefaultDecodedFieldDuration
        0x234E7A |
        // DefaultDuration
        0x23E383 |
        // TimestampScale
        0x2AD7B1
        => Some(Kind::Integer),
        // SamplingFrequency
        0xB5 |
        // Duration
        0x4489 |
        // PrimaryRChromaticityX
        0x55D1 |
        // PrimaryRChromaticityY
        0x55D2 |
        // PrimaryGChromaticityX
        0x55D3 |
        // PrimaryGChromaticityY
        0x55D4 |
        // PrimaryBChromaticityX
        0x55D5 |
        // PrimaryBChromaticityY
        0x55D6 |
        // WhitePointChromaticityX
        0x55D7 |
        // WhitePointChromaticityY
        0x55D8 |
        // LuminanceMax
        0x55D9 |
        // LuminanceMin
        0x55DA |
        // ProjectionPoseYaw
        0x7673 |
        // ProjectionPosePitch
        0x7674 |
        // ProjectionPoseRoll
        0x7675 |
        // OutputSamplingFrequency
        0x78B5 |
        // TrackTimestampScale
        0x23314F |
        // FrameRate
        0x2383E3 |
        // GammaValue
        0x2FB523
        => Some(Kind::Float),
        // DateUTC
        0x4461
        => Some(Kind::Date),
        // CRC-32
        0xBF |
        // SeekID
        0x53AB |
        // UncompressedFourCC
        0x2EB524
        => Some(Kind::Fixed(4)),
        // TrickTrackSegmentUID
        0xC1 |
        // TrickMasterTrackSegmentUID
        0xC4 |
        // SegmentFamily
        0x4444 |
        // ChapterSegmentUUID
        0x6E67 |
        // SegmentUUID
        0x73A4 |
        // PrevUUID
        0x3CB923 |
        // NextUUID
        0x3EB923
        => Some(Kind::Fixed(16)),
        // ChapString
        0x85 |
        // CodecID
        0x86 |
        // BlockAddIDName
        0x41A4 |
        // ChapLanguage
        0x437C |
        // ChapLanguageBCP47
        0x437D |
        // ChapCountry
        0x437E |
        // TagLanguage
        0x447A |
        // TagLanguageBCP47
        0x447B |
        // TagString
        0x4487 |
        // EditionString
        0x4521 |
        // TagName
        0x45A3 |
        // EditionLanguageIETF
        0x45E4 |
        // FileMediaType
        0x4660 |
        // FileName
        0x466E |
        // FileDescription
        0x467E |
        // MuxingApp
        0x4D80 |
        // Name
        0x536E |
        // ChapterStringUID
        0x5654 |
        // WritingApp
        0x5741 |
        // TargetType
        0x63CA |
        // SegmentFilename
        0x7384 |
        // Title
        0x7BA9 |
        // Language
        0x22B59C |
        // LanguageBCP47
        0x22B59D |
        // CodecName
        0x258688 |
        // CodecDownloadURL
        0x26B240 |
        // CodecSettings
        0x3A9697 |
        // CodecInfoURL
        0x3B4040 |
        // PrevFilename
        0x3C83AB |
        // NextFilename
        0x3E83BB
        => Some(Kind::Text),
        // Block
        0xA1 |
        // BlockVirtual
        0xA2 |
        // SimpleBlock
        0xA3 |
        // CodecState
        0xA4 |
        // BlockAdditional
        0xA5 |
        // EncryptedBlock
        0xAF |
        // BlockAddIDExtraData
        0x41ED |
        // ContentCompSettings
        0x4255 |
        // TagBinary
        0x4485 |
        // ChapProcessPrivate
        0x450D |
        // FileData
        0x465C |
        // FileReferral
        0x4675 |
        // ContentEncKeyID
        0x47E2 |
        // ContentSignature
        0x47E3 |
        // ContentSigKeyID
        0x47E4 |
        // CodecPrivate
        0x63A2 |
        // TrackTranslateTrackID
        0x66A5 |
        // ChapProcessData
        0x6933 |
        // ChapterTranslateID
        0x69A5 |
        // ProjectionPrivate
        0x7672 |
        // ChannelPositions
        0x7D7B
        => Some(Kind::Binary),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, kind};

    #[test]
    fn lengths_follow_the_types() {
        assert_eq!(kind(0x1F43_B675), Some(Kind::Master)); // Cluster
        assert_eq!(kind(0xFB), Some(Kind::Integer)); // ReferenceBlock
        assert_eq!(kind(0x4489), Some(Kind::Float)); // Duration
        assert_eq!(kind(0x4461), Some(Kind::Date)); // DateUTC
        assert_eq!(kind(0x53AB), Some(Kind::Fixed(4))); // SeekID
        assert_eq!(kind(0x4487), Some(Kind::Text)); // TagString
        assert_eq!(kind(0x465C), Some(Kind::Binary)); // FileData
        assert_eq!(kind(0xA3), Some(Kind::Binary)); // SimpleBlock
        assert_eq!(kind(0xEC), None); // Void
        assert!(Kind::Integer.allows(8) && !Kind::Integer.allows(9));
        assert!(Kind::Float.allows(4) && !Kind::Float.allows(5));
        assert!(Kind::Date.allows(0) && !Kind::Date.allows(4));
        assert!(Kind::Fixed(16).allows(16) && !Kind::Fixed(16).allows(15));
        assert!(Kind::Text.allows(1 << 30));
    }
}

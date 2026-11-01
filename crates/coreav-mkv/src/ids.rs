//! Element IDs, marker bits included (RFC 8794 section 11, RFC 9559
//! section 5).

// EBML header (RFC 8794 11.2) and global elements (11.3).
pub(crate) const EBML: u32 = 0x1A45_DFA3;
pub(crate) const EBML_VERSION: u32 = 0x4286;
pub(crate) const EBML_READ_VERSION: u32 = 0x42F7;
pub(crate) const EBML_MAX_ID_LENGTH: u32 = 0x42F2;
pub(crate) const EBML_MAX_SIZE_LENGTH: u32 = 0x42F3;
pub(crate) const DOC_TYPE: u32 = 0x4282;
pub(crate) const DOC_TYPE_VERSION: u32 = 0x4287;
pub(crate) const DOC_TYPE_READ_VERSION: u32 = 0x4285;
pub(crate) const CRC32: u32 = 0xBF;
pub(crate) const VOID: u32 = 0xEC;

// Segment and its top-level elements (RFC 9559 5.1).
pub(crate) const SEGMENT: u32 = 0x1853_8067;
pub(crate) const SEEK_HEAD: u32 = 0x114D_9B74;
pub(crate) const INFO: u32 = 0x1549_A966;
pub(crate) const CLUSTER: u32 = 0x1F43_B675;
pub(crate) const TRACKS: u32 = 0x1654_AE6B;
pub(crate) const CUES: u32 = 0x1C53_BB6B;
pub(crate) const ATTACHMENTS: u32 = 0x1941_A469;
pub(crate) const CHAPTERS: u32 = 0x1043_A770;
pub(crate) const TAGS: u32 = 0x1254_C367;

/// The Top-Level Elements: any of them ends an unknown-sized Cluster
/// (RFC 8794 6.2).
pub(crate) const TOP_LEVEL: [u32; 8] = [
    SEEK_HEAD,
    INFO,
    CLUSTER,
    TRACKS,
    CUES,
    ATTACHMENTS,
    CHAPTERS,
    TAGS,
];

// SeekHead (5.1.1).
pub(crate) const SEEK: u32 = 0x4DBB;
pub(crate) const SEEK_ID: u32 = 0x53AB;
pub(crate) const SEEK_POSITION: u32 = 0x53AC;

// Info (5.1.2).
pub(crate) const TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
pub(crate) const DURATION: u32 = 0x4489;
pub(crate) const DATE_UTC: u32 = 0x4461;
pub(crate) const TITLE: u32 = 0x7BA9;
pub(crate) const MUXING_APP: u32 = 0x4D80;
pub(crate) const WRITING_APP: u32 = 0x5741;

// Cluster (5.1.3).
pub(crate) const TIMESTAMP: u32 = 0xE7;
pub(crate) const SIMPLE_BLOCK: u32 = 0xA3;
pub(crate) const BLOCK_GROUP: u32 = 0xA0;
pub(crate) const BLOCK: u32 = 0xA1;
pub(crate) const BLOCK_DURATION: u32 = 0x9B;
pub(crate) const REFERENCE_BLOCK: u32 = 0xFB;
pub(crate) const DISCARD_PADDING: u32 = 0x75A2;
pub(crate) const BLOCK_ADDITIONS: u32 = 0x75A1;
pub(crate) const BLOCK_MORE: u32 = 0xA6;
pub(crate) const BLOCK_ADD_ID: u32 = 0xEE;

// Tracks (5.1.4).
pub(crate) const TRACK_ENTRY: u32 = 0xAE;
pub(crate) const TRACK_NUMBER: u32 = 0xD7;
pub(crate) const TRACK_UID: u32 = 0x73C5;
pub(crate) const TRACK_TYPE: u32 = 0x83;
pub(crate) const FLAG_ENABLED: u32 = 0xB9;
pub(crate) const FLAG_DEFAULT: u32 = 0x88;
pub(crate) const FLAG_FORCED: u32 = 0x55AA;
pub(crate) const FLAG_HEARING_IMPAIRED: u32 = 0x55AB;
pub(crate) const FLAG_VISUAL_IMPAIRED: u32 = 0x55AC;
pub(crate) const FLAG_TEXT_DESCRIPTIONS: u32 = 0x55AD;
pub(crate) const FLAG_ORIGINAL: u32 = 0x55AE;
pub(crate) const FLAG_COMMENTARY: u32 = 0x55AF;
pub(crate) const FLAG_LACING: u32 = 0x9C;
pub(crate) const DEFAULT_DURATION: u32 = 0x23_E383;
pub(crate) const TRACK_TIMESTAMP_SCALE: u32 = 0x23_314F;
pub(crate) const NAME: u32 = 0x536E;
pub(crate) const LANGUAGE: u32 = 0x22_B59C;
pub(crate) const LANGUAGE_BCP47: u32 = 0x22_B59D;
pub(crate) const CODEC_ID: u32 = 0x86;
pub(crate) const CODEC_PRIVATE: u32 = 0x63A2;
pub(crate) const CODEC_DELAY: u32 = 0x56AA;
pub(crate) const SEEK_PRE_ROLL: u32 = 0x56BB;
pub(crate) const VIDEO: u32 = 0xE0;
pub(crate) const AUDIO: u32 = 0xE1;
pub(crate) const CONTENT_ENCODINGS: u32 = 0x6D80;

// Video (5.1.4.1.28).
pub(crate) const FLAG_INTERLACED: u32 = 0x9A;
pub(crate) const FIELD_ORDER: u32 = 0x9D;
pub(crate) const FRAME_RATE: u32 = 0x23_83E3;
pub(crate) const STEREO_MODE: u32 = 0x53B8;
pub(crate) const ALPHA_MODE: u32 = 0x53C0;
pub(crate) const PIXEL_WIDTH: u32 = 0xB0;
pub(crate) const PIXEL_HEIGHT: u32 = 0xBA;
pub(crate) const PIXEL_CROP_BOTTOM: u32 = 0x54AA;
pub(crate) const PIXEL_CROP_TOP: u32 = 0x54BB;
pub(crate) const PIXEL_CROP_LEFT: u32 = 0x54CC;
pub(crate) const PIXEL_CROP_RIGHT: u32 = 0x54DD;
pub(crate) const DISPLAY_WIDTH: u32 = 0x54B0;
pub(crate) const DISPLAY_HEIGHT: u32 = 0x54BA;
pub(crate) const DISPLAY_UNIT: u32 = 0x54B2;
pub(crate) const COLOUR: u32 = 0x55B0;
pub(crate) const MATRIX_COEFFICIENTS: u32 = 0x55B1;
pub(crate) const BITS_PER_CHANNEL: u32 = 0x55B2;
pub(crate) const RANGE: u32 = 0x55B9;
pub(crate) const TRANSFER_CHARACTERISTICS: u32 = 0x55BA;
pub(crate) const PRIMARIES: u32 = 0x55BB;
pub(crate) const CHROMA_SITING_HORZ: u32 = 0x55B7;
pub(crate) const MAX_CLL: u32 = 0x55BC;
pub(crate) const MAX_FALL: u32 = 0x55BD;
pub(crate) const MASTERING_METADATA: u32 = 0x55D0;
pub(crate) const CHROMA_SITING_VERT: u32 = 0x55B8;
pub(crate) const PROJECTION: u32 = 0x7670;
pub(crate) const PROJECTION_TYPE: u32 = 0x7671;
pub(crate) const PROJECTION_POSE_YAW: u32 = 0x7673;
pub(crate) const PROJECTION_POSE_PITCH: u32 = 0x7674;
pub(crate) const PROJECTION_POSE_ROLL: u32 = 0x7675;

// Audio (5.1.4.1.29).
pub(crate) const SAMPLING_FREQUENCY: u32 = 0xB5;
pub(crate) const OUTPUT_SAMPLING_FREQUENCY: u32 = 0x78B5;
pub(crate) const CHANNELS: u32 = 0x9F;
pub(crate) const BIT_DEPTH: u32 = 0x6264;

// ContentEncodings (5.1.4.1.31).
pub(crate) const CONTENT_ENCODING: u32 = 0x6240;
pub(crate) const CONTENT_ENCODING_SCOPE: u32 = 0x5032;
pub(crate) const CONTENT_ENCODING_TYPE: u32 = 0x5033;
pub(crate) const CONTENT_COMPRESSION: u32 = 0x5034;
pub(crate) const CONTENT_COMP_ALGO: u32 = 0x4254;
pub(crate) const CONTENT_COMP_SETTINGS: u32 = 0x4255;

// Cues (5.1.5).
pub(crate) const CUE_POINT: u32 = 0xBB;
pub(crate) const CUE_TIME: u32 = 0xB3;
pub(crate) const CUE_TRACK_POSITIONS: u32 = 0xB7;
pub(crate) const CUE_TRACK: u32 = 0xF7;
pub(crate) const CUE_CLUSTER_POSITION: u32 = 0xF1;
pub(crate) const CUE_RELATIVE_POSITION: u32 = 0xF0;

// Attachments (5.1.6).
pub(crate) const ATTACHED_FILE: u32 = 0x61A7;
pub(crate) const FILE_NAME: u32 = 0x466E;
pub(crate) const FILE_MEDIA_TYPE: u32 = 0x4660;
pub(crate) const FILE_DATA: u32 = 0x465C;
pub(crate) const FILE_DESCRIPTION: u32 = 0x467E;
pub(crate) const FILE_UID: u32 = 0x46AE;

// Chapters (5.1.7).
pub(crate) const EDITION_ENTRY: u32 = 0x45B9;
pub(crate) const EDITION_UID: u32 = 0x45BC;
pub(crate) const EDITION_FLAG_HIDDEN: u32 = 0x45BD;
pub(crate) const EDITION_FLAG_DEFAULT: u32 = 0x45DB;
pub(crate) const EDITION_FLAG_ORDERED: u32 = 0x45DD;
pub(crate) const CHAPTER_ATOM: u32 = 0xB6;
pub(crate) const CHAPTER_UID: u32 = 0x73C4;
pub(crate) const CHAPTER_STRING_UID: u32 = 0x5654;
pub(crate) const CHAPTER_TIME_START: u32 = 0x91;
pub(crate) const CHAPTER_TIME_END: u32 = 0x92;
pub(crate) const CHAPTER_FLAG_HIDDEN: u32 = 0x98;
pub(crate) const CHAPTER_FLAG_ENABLED: u32 = 0x4598;
pub(crate) const CHAPTER_DISPLAY: u32 = 0x80;
pub(crate) const CHAP_STRING: u32 = 0x85;
pub(crate) const CHAP_LANGUAGE: u32 = 0x437C;

// Tags (5.1.8).
pub(crate) const TAG: u32 = 0x7373;
pub(crate) const TARGETS: u32 = 0x63C0;
pub(crate) const TARGET_TYPE_VALUE: u32 = 0x68CA;
pub(crate) const TARGET_TYPE: u32 = 0x63CA;
pub(crate) const TAG_TRACK_UID: u32 = 0x63C5;
pub(crate) const TAG_EDITION_UID: u32 = 0x63C9;
pub(crate) const TAG_CHAPTER_UID: u32 = 0x63C4;
pub(crate) const TAG_ATTACHMENT_UID: u32 = 0x63C6;
pub(crate) const SIMPLE_TAG: u32 = 0x67C8;
pub(crate) const TAG_NAME: u32 = 0x45A3;
pub(crate) const TAG_LANGUAGE: u32 = 0x447A;
pub(crate) const TAG_LANGUAGE_BCP47: u32 = 0x447B;
pub(crate) const TAG_DEFAULT: u32 = 0x4484;
pub(crate) const TAG_STRING: u32 = 0x4487;
pub(crate) const TAG_BINARY: u32 = 0x4485;

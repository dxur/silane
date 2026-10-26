//! IVF container format parsing.
//!
//! This module implements a parser for the IVF (Indeo Video Format) container,
//! which is a simple container format commonly used for VP8 and VP9 video streams.
//!
//! The IVF format consists of:
//! - A 32-byte file header containing codec information and frame dimensions
//! - A series of frames, each with a 12-byte frame header followed by frame data
//!
//! # Example
//!
//! ```no_run
//! # use vp_ate::model::ivf::IVFParser;
//! let data = std::fs::read("video.ivf").unwrap();
//! let mut parser = IVFParser::new(&data).unwrap();
//!
//! while let Some(frame) = parser.next_frame().unwrap() {
//!     println!("Frame PTS: {}, size: {}", frame.pts, frame.data.len());
//!     // Process frame.data...
//! }
//! ```

/// Errors that can occur when parsing an IVF container.
#[derive(Debug)]
pub enum IVFError {
    /// Unexpected end of file while reading header or frame data.
    UnexpectedEof,
    /// Invalid file signature (expected "DKIF").
    InvalidSignature,
    /// Unsupported IVF version (only version 0 is supported).
    UnsupportedVersion(u16),
    /// Invalid header size (expected 32 bytes).
    InvalidHeaderSize(u16),
    /// Frame data size exceeds available data.
    FrameTooShort,
}

/// The header of an IVF file.
///
/// Contains metadata about the video stream including codec type,
/// dimensions, and frame rate.
#[derive(Debug, Clone)]
pub struct IVFHeader {
    /// FourCC code identifying the codec (e.g., "VP80" for VP8).
    pub fourcc: [u8; 4],
    /// Width of the video in pixels.
    pub width: u16,
    /// Height of the video in pixels.
    pub height: u16,
    /// Frame rate numerator.
    pub framerate_num: u32,
    /// Frame rate denominator (fps = framerate_num / framerate_den).
    pub framerate_den: u32,
    /// Total number of frames in the file.
    pub num_frames: u64,
}

/// A single frame from an IVF file.
///
/// Contains the presentation timestamp and compressed frame data.
#[derive(Debug, Clone)]
pub struct IVFFrame<'a> {
    /// Presentation timestamp for this frame.
    pub pts: u64,
    /// Raw compressed frame data (codec-specific format).
    pub data: &'a [u8],
}

/// Parser for the IVF container format.
///
/// Provides sequential access to frames in an IVF file.
pub struct IVFParser<'a> {
    data: &'a [u8],
    cursor: usize,
    /// The parsed file header containing video metadata.
    pub header: IVFHeader,
}

impl<'a> IVFParser<'a> {
    /// Creates a new IVF parser from the provided byte slice.
    ///
    /// This parses and validates the 32-byte IVF file header.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The data is too short to contain a header
    /// - The file signature is not "DKIF"
    /// - The version is not 0
    /// - The header size is not 32 bytes
    pub fn new(data: &'a [u8]) -> Result<Self, IVFError> {
        if data.len() < 32 {
            return Err(IVFError::UnexpectedEof);
        }

        if &data[0..4] != b"DKIF" {
            return Err(IVFError::InvalidSignature);
        }

        let version = u16::from_le_bytes(data[4..6].try_into().unwrap());
        if version != 0 {
            return Err(IVFError::UnsupportedVersion(version));
        }

        let header_size = u16::from_le_bytes(data[6..8].try_into().unwrap());
        if header_size != 32 {
            return Err(IVFError::InvalidHeaderSize(header_size));
        }

        let header = IVFHeader {
            fourcc: data[8..12].try_into().unwrap(),
            width: u16::from_le_bytes(data[12..14].try_into().unwrap()),
            height: u16::from_le_bytes(data[14..16].try_into().unwrap()),
            framerate_num: u32::from_le_bytes(data[16..20].try_into().unwrap()),
            framerate_den: u32::from_le_bytes(data[20..24].try_into().unwrap()),
            num_frames: u64::from_le_bytes(data[24..32].try_into().unwrap()),
        };

        Ok(Self {
            data,
            cursor: 32,
            header,
        })
    }

    /// Returns the next frame from the container, or `None` if EOF is reached.
    ///
    /// Each frame has a 12-byte header (4 bytes size + 8 bytes PTS) followed
    /// by the frame data.
    ///
    /// # Errors
    ///
    /// Returns `Err(IVFError::FrameTooShort)` if the frame size field indicates
    /// more data than is available in the file.
    pub fn next_frame(&mut self) -> Result<Option<IVFFrame<'a>>, IVFError> {
        if self.cursor + 12 > self.data.len() {
            return Ok(None);
        }

        let frame_size =
            u32::from_le_bytes(self.data[self.cursor..self.cursor + 4].try_into().unwrap())
                as usize;
        let pts = u64::from_le_bytes(
            self.data[self.cursor + 4..self.cursor + 12]
                .try_into()
                .unwrap(),
        );

        self.cursor += 12;
        if self.cursor + frame_size > self.data.len() {
            return Err(IVFError::FrameTooShort);
        }

        let frame_data = &self.data[self.cursor..self.cursor + frame_size];
        self.cursor += frame_size;

        Ok(Some(IVFFrame {
            pts,
            data: frame_data,
        }))
    }
}

impl std::error::Error for IVFError {}
impl std::fmt::Display for IVFError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use IVFError::*;
        match self {
            UnexpectedEof => write!(f, "Unexpected EOF"),
            InvalidSignature => write!(f, "Invalid IVF signature"),
            UnsupportedVersion(v) => write!(f, "Unsupported IVF version {}", v),
            InvalidHeaderSize(s) => write!(f, "Invalid IVF header size {}", s),
            FrameTooShort => write!(f, "Frame data too short"),
        }
    }
}

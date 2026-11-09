//! The COIN-D6, the 360-degree dToF lidar on Hiwonder's `MentorPi` robots, and the serial protocol it scans over.
//!
//! The lidar spins at about 10 Hz and takes about 4000 samples a second, sending them over a UART at [`BAUD_RATE`],
//! 8N1, which the robot reaches through a CH340 USB adapter. Its scans arrive as packets, little-endian:
//!
//! | offset | bytes | field                                                                                      |
//! | ------ | ----- | ------------------------------------------------------------------------------------------ |
//! | 0      | 2     | header, `AA 55`                                                                            |
//! | 2      | 1     | bit 0 set when the packet starts a revolution; above it, the scan frequency in tenths of Hz |
//! | 3      | 1     | how many samples the packet holds, never 0                                                 |
//! | 4      | 2     | angle of the first sample, in 1/64 degrees, shifted left over a check bit that is set      |
//! | 6      | 2     | angle of the last sample, encoded the same way                                             |
//! | 8      | 2     | checksum                                                                                   |
//! | 10     | 3 * n | the samples                                                                                |
//!
//! Each sample is three bytes: the first holds the intensity's low six bits above a high-reflectivity flag in bit 0,
//! the second the distance's low six bits above the intensity's high two, and the third the distance's high eight.
//! The samples between the first and last are evenly spaced in angle, which wraps past 360 degrees within a packet.
//! The checksum is the XOR of the packet's 16-bit words before it, and, for each sample, its first byte and the word
//! its other two make. A revolution's first packet holds a single sample, at the lidar's zero.
//!
//! The vendor's driver corrects each angle for the distance it was measured at, a fix for the parallax of a
//! triangulating lidar. The COIN-D6 measures time of flight through a coaxial optic, so it has no parallax and
//! the angles here are the ones the lidar reports.

use alloc::vec::Vec;
use core::mem;

/// The rate the lidar's UART runs at, with 8 data bits, no parity and one stop bit.
pub const BAUD_RATE: u32 = 230_400;
/// Asks the lidar to start scanning.
///
/// Units in the field stream from power-on whether or not they are sent it, so sending it costs nothing.
pub const START_SCAN: [u8; 4] = [0xAA, 0x55, 0xF0, 0x0F];
/// Asks the lidar to stop scanning.
pub const STOP_SCAN: [u8; 4] = [0xAA, 0x55, 0xF5, 0x0A];
/// A whole turn, in the 1/64 degrees the lidar measures angles in: every [`Sample::angle`] is below it.
pub const FULL_TURN: u16 = 23_040;
/// The farthest a sample can say the beam reached, in millimetres: the protocol gives a distance 14 bits.
pub const MAX_DISTANCE_MM: u16 = 0x3FFF;
/// The fastest scan frequency a packet can carry, in tenths of a hertz: the protocol gives it 7 bits.
pub const MAX_FREQUENCY_DHZ: u8 = 0x7F;

/// The first byte of a packet's header.
const SYNC_FIRST: u8 = 0xAA;
/// The second byte of a packet's header.
const SYNC_SECOND: u8 = 0x55;
/// A packet's header.
const SYNC: [u8; 2] = [SYNC_FIRST, SYNC_SECOND];
/// How many bytes a packet has before its samples.
const HEAD_LEN: usize = 10;
/// How many bytes a sample takes.
const SAMPLE_LEN: usize = 3;

/// One sample: how far the beam reached at one angle, and how strongly it came back.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Sample {
    /// Clockwise from the lidar's zero, which faces its single mounting ear, in 1/64 degrees.
    pub angle: u16,
    /// Millimetres to what the beam hit, or 0 when nothing came back.
    pub distance_mm: u16,
    /// How strongly the echo came back.
    pub intensity: u8,
    /// The lidar's flag for a surface reflective enough, such as a retroreflector, to make the distance unreliable.
    pub high_reflectivity: bool,
}

impl Sample {
    /// A sample taken at `angle`, `distance_mm` away, its echo `intensity` strong.
    #[must_use]
    pub const fn new(angle: u16, distance_mm: u16, intensity: u8, high_reflectivity: bool) -> Self {
        Self {
            angle,
            distance_mm,
            intensity,
            high_reflectivity,
        }
    }

    /// The sample's angle in degrees, clockwise from the lidar's zero.
    #[must_use]
    #[expect(
        clippy::float_arithmetic,
        reason = "degrees are a float for whoever reads them; the protocol's angles are fixed-point"
    )]
    pub fn degrees(self) -> f64 {
        f64::from(self.angle) / 64.0
    }
}

/// One packet's samples.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Packet {
    /// Whether the packet starts a revolution, when it holds the one sample at the lidar's zero.
    pub starts_revolution: bool,
    /// The scan frequency in tenths of a hertz, which only a packet that starts a revolution carries.
    pub frequency_dhz: u8,
    /// The samples, in the order the lidar took them.
    pub samples: Vec<Sample>,
}

/// Why [`Packet::encode`] cannot render a packet: it holds what the protocol has no room for.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EncodeError {
    /// A packet holds 1 to 255 samples.
    #[error("a packet holds 1 to 255 samples, not {0}")]
    SampleCount(usize),
    /// A first or last sample's angle is not below [`FULL_TURN`].
    #[error("an angle is below {limit}, not {0}", limit = FULL_TURN)]
    Angle(u16),
    /// A sample's distance is beyond [`MAX_DISTANCE_MM`].
    #[error("a distance is at most {limit} mm, not {0}", limit = MAX_DISTANCE_MM)]
    Distance(u16),
    /// The scan frequency is beyond [`MAX_FREQUENCY_DHZ`].
    #[error("a scan frequency is at most {limit} tenths of a hertz, not {0}", limit = MAX_FREQUENCY_DHZ)]
    Frequency(u8),
}

impl Packet {
    /// A packet of `samples`, starting a revolution at `frequency_dhz` when `starts_revolution`.
    #[must_use]
    pub const fn new(starts_revolution: bool, frequency_dhz: u8, samples: Vec<Sample>) -> Self {
        Self {
            starts_revolution,
            frequency_dhz,
            samples,
        }
    }

    /// The bytes the lidar sends for the packet, for a simulated lidar to send.
    ///
    /// The protocol carries only the first and last samples' angles and spaces the rest evenly between them, so a
    /// [`Decoder`] gives the samples in between those even steps, whatever angles they hold here.
    ///
    /// # Errors
    ///
    /// When the packet holds no samples or more than 255, or a value the protocol has too few bits for.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let Some((first, rest)) = self.samples.split_first() else {
            return Err(EncodeError::SampleCount(0));
        };
        let Ok(count) = u8::try_from(self.samples.len()) else {
            return Err(EncodeError::SampleCount(self.samples.len()));
        };
        let last = rest.last().unwrap_or(first);
        for angle in [first.angle, last.angle] {
            if angle >= FULL_TURN {
                return Err(EncodeError::Angle(angle));
            }
        }
        if let Some(far) = self.samples.iter().find(|sample| sample.distance_mm > MAX_DISTANCE_MM) {
            return Err(EncodeError::Distance(far.distance_mm));
        }
        if self.frequency_dhz > MAX_FREQUENCY_DHZ {
            return Err(EncodeError::Frequency(self.frequency_dhz));
        }
        let flags = (self.frequency_dhz << 1_u32) | u8::from(self.starts_revolution);
        let first = (first.angle << 1_u32) | 1;
        let last = (last.angle << 1_u32) | 1;
        let chunks: Vec<[u8; SAMPLE_LEN]> = self
            .samples
            .iter()
            .map(|sample| {
                let [middle, high] = bytes_of((sample.distance_mm << 2_u32) | u16::from(sample.intensity >> 6_u32));
                [
                    (sample.intensity << 2_u32) | u8::from(sample.high_reflectivity),
                    middle,
                    high,
                ]
            })
            .collect();
        let mut bytes = Vec::with_capacity(HEAD_LEN.saturating_add(SAMPLE_LEN.saturating_mul(chunks.len())));
        bytes.extend_from_slice(&SYNC);
        bytes.extend_from_slice(&[flags, count]);
        for value in [first, last, checksum(flags, count, first, last, &chunks)] {
            bytes.extend_from_slice(&bytes_of(value));
        }
        bytes.extend(chunks.iter().flatten());
        Ok(bytes)
    }
}

/// One revolution of samples, from the lidar's zero round to just short of it.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Scan {
    /// How fast the lidar was spinning, in tenths of a hertz: 100 is ten revolutions a second.
    pub frequency_dhz: u8,
    /// The samples, in the order the lidar took them, which is clockwise.
    pub samples: Vec<Sample>,
}

impl Scan {
    /// How fast the lidar was spinning, in revolutions a second.
    #[must_use]
    #[expect(
        clippy::float_arithmetic,
        reason = "hertz are a float for whoever reads them; the protocol's frequency is fixed-point"
    )]
    pub fn frequency_hz(&self) -> f64 {
        f64::from(self.frequency_dhz) / 10.0
    }
}

/// Turns the lidar's byte stream into packets, however the bytes are split as they arrive.
///
/// A packet whose checksum or check bits are wrong is dropped, and the decoder finds the next header after it.
/// [`Decoder::default`] is one that has seen no bytes.
#[derive(Debug, Default)]
pub struct Decoder {
    /// Bytes received that do not yet make a whole packet.
    pending: Vec<u8>,
    /// How many bytes were dropped as belonging to no packet.
    skipped: u64,
    /// How many headers started no valid packet.
    rejected: u64,
}

impl Decoder {
    /// Every packet that the bytes received so far complete, `bytes` the latest of them.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Packet> {
        self.pending.extend_from_slice(bytes);
        let mut packets = Vec::new();
        let mut cursor = 0_usize;
        loop {
            let consumed = match parse(self.pending.get(cursor..).unwrap_or_default()) {
                Parse::Packet(packet, length) => {
                    packets.push(packet);
                    length
                }
                Parse::Noise(length) => {
                    self.skipped = self.skipped.saturating_add(u64::try_from(length).unwrap_or(u64::MAX));
                    length
                }
                Parse::Corrupt => {
                    self.rejected = self.rejected.saturating_add(1);
                    self.skipped = self.skipped.saturating_add(1);
                    1
                }
                Parse::Incomplete => break,
            };
            cursor = cursor.saturating_add(consumed);
        }
        self.pending = self.pending.split_off(cursor);
        packets
    }

    /// How many bytes were dropped as belonging to no packet.
    ///
    /// Bytes arriving with none of them decoding means the port is not at [`BAUD_RATE`], or is not a COIN-D6.
    #[must_use]
    pub const fn skipped_bytes(&self) -> u64 {
        self.skipped
    }

    /// How many headers started no valid packet, through a bad checksum, check bit or sample count.
    #[must_use]
    pub const fn rejected_packets(&self) -> u64 {
        self.rejected
    }
}

/// Turns the lidar's byte stream into whole revolutions.
///
/// The samples that arrive before the first packet to start a revolution belong to one only partly seen, so they are
/// dropped. A revolution is complete when the next one starts. [`Scanner::default`] is one that has seen no bytes.
#[derive(Debug, Default)]
pub struct Scanner {
    /// The packets the bytes make.
    decoder: Decoder,
    /// The revolution the latest packets belong to.
    revolution: Revolution,
}

/// The revolution a scanner is part way through.
#[derive(Debug, Default)]
enum Revolution {
    /// None yet: no packet has started one.
    #[default]
    Unsynced,
    /// This one, with the samples so far.
    Started(Scan),
}

impl Scanner {
    /// Every revolution that the bytes received so far complete, `bytes` the latest of them.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Scan> {
        let mut scans = Vec::new();
        for packet in self.decoder.push(bytes) {
            if packet.starts_revolution {
                let next = Revolution::Started(Scan {
                    frequency_dhz: packet.frequency_dhz,
                    samples: packet.samples,
                });
                if let Revolution::Started(scan) = mem::replace(&mut self.revolution, next) {
                    scans.push(scan);
                }
                continue;
            }
            if let Revolution::Started(scan) = &mut self.revolution {
                scan.samples.extend(packet.samples);
            }
        }
        scans
    }

    /// The decoder beneath, whose counts tell whether the bytes are a COIN-D6's.
    #[must_use]
    pub const fn decoder(&self) -> &Decoder {
        &self.decoder
    }
}

/// What the bytes at the decoder's cursor start with.
enum Parse {
    /// A whole packet, and how many bytes it took.
    Packet(Packet, usize),
    /// That many bytes before the next header, which belong to no packet.
    Noise(usize),
    /// A header that starts no valid packet, whose first byte is dropped to look for the next one.
    Corrupt,
    /// Too few bytes to tell: the rest of a packet, or of its header, has yet to arrive.
    Incomplete,
}

/// What `bytes` start with.
#[expect(
    clippy::single_call_fn,
    reason = "the decoder's parse of one packet, kept apart from its buffering"
)]
fn parse(bytes: &[u8]) -> Parse {
    let Some(start) = bytes.windows(SYNC.len()).position(|pair| pair == SYNC) else {
        // A last byte that could be the first of a header is kept until the next byte says whether it is.
        let noise = bytes.len().saturating_sub(usize::from(bytes.ends_with(&[SYNC_FIRST])));
        return if noise == 0 {
            Parse::Incomplete
        } else {
            Parse::Noise(noise)
        };
    };
    if start > 0 {
        return Parse::Noise(start);
    }
    let Some(
        &[
            _,
            _,
            flags,
            count,
            first_low,
            first_high,
            last_low,
            last_high,
            sum_low,
            sum_high,
        ],
    ) = bytes.first_chunk::<HEAD_LEN>()
    else {
        return Parse::Incomplete;
    };
    let first = word(first_low, first_high);
    let last = word(last_low, last_high);
    if count == 0 || first & 1 == 0 || last & 1 == 0 {
        return Parse::Corrupt;
    }
    let length = HEAD_LEN.saturating_add(SAMPLE_LEN.saturating_mul(usize::from(count)));
    let Some(body) = bytes.get(HEAD_LEN..length) else {
        return Parse::Incomplete;
    };
    let (chunks, _) = body.as_chunks::<SAMPLE_LEN>();
    if checksum(flags, count, first, last, chunks) != word(sum_low, sum_high) {
        return Parse::Corrupt;
    }
    let packet = Packet {
        frequency_dhz: flags >> 1,
        samples: spread(first >> 1, last >> 1, chunks),
        starts_revolution: flags & 1 == 1,
    };
    Parse::Packet(packet, length)
}

/// The samples `chunks` encode, spaced evenly in angle from `first` to `last`.
#[expect(
    clippy::single_call_fn,
    reason = "the sample decoding of a packet, kept apart from its framing"
)]
fn spread(first: u16, last: u16, chunks: &[[u8; SAMPLE_LEN]]) -> Vec<Sample> {
    let turn = u32::from(FULL_TURN);
    let first = u32::from(first);
    let last = u32::from(last);
    let span = if last < first { last.saturating_add(turn) } else { last }.saturating_sub(first);
    let steps = u32::try_from(chunks.len()).unwrap_or(u32::MAX).saturating_sub(1);
    chunks
        .iter()
        .zip(0_u32..)
        .map(|(&[low, middle, high], index)| {
            let offset = span.saturating_mul(index).checked_div(steps).unwrap_or(0);
            let angle = first.saturating_add(offset).checked_rem(turn).unwrap_or(0);
            Sample {
                angle: u16::try_from(angle).unwrap_or(0),
                distance_mm: word(middle, high) >> 2,
                high_reflectivity: low & 1 == 1,
                intensity: ((middle & 0b11) << 6) | (low >> 2),
            }
        })
        .collect()
}

/// The checksum of a packet with this head, `first` and `last` its angle words, and these samples.
fn checksum(flags: u8, count: u8, first: u16, last: u16, chunks: &[[u8; SAMPLE_LEN]]) -> u16 {
    let head = word(SYNC_FIRST, SYNC_SECOND) ^ word(flags, count) ^ first ^ last;
    chunks.iter().fold(head, |sum, &[low, middle, high]| {
        sum ^ u16::from(low) ^ word(middle, high)
    })
}

/// The 16-bit word that `low` and `high` make, in the protocol's little-endian order.
fn word(low: u8, high: u8) -> u16 {
    (u16::from(high) << 8) | u16::from(low)
}

/// The two bytes of `value`, in the protocol's little-endian order.
fn bytes_of(value: u16) -> [u8; 2] {
    [
        u8::try_from(value & 0xFF).unwrap_or(u8::MAX),
        u8::try_from(value >> 8).unwrap_or(u8::MAX),
    ]
}

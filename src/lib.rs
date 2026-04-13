#![deny(missing_docs)]

//! Parser data formatted according to
//! [SCTE-35](http://www.scte.org/SCTEDocs/Standards/SCTE%2035%202016.pdf).
//!
//! Intended to be used in conjunction with the
//! [mpeg2ts-reader](https://crates.io/crates/mpeg2ts-reader) crate's facilities for processing
//! the Transport Stream structures within which SCTE-35 data is usually embedded.
//!
//! ## Example
//!
//! ```
//! # use hex_literal::*;
//! # use scte35_reader::Scte35SectionProcessor;
//! # use mpeg2ts_reader::psi::WholeCompactSyntaxPayloadParser;
//! # use mpeg2ts_reader::{ psi, demultiplex };
//! # mpeg2ts_reader::demux_context!(
//! #        NullDemuxContext,
//! #        demultiplex::NullPacketFilter<NullDemuxContext>
//! #    );
//! # impl NullDemuxContext {
//! #    fn do_construct(
//! #        &mut self,
//! #        _req: demultiplex::FilterRequest<'_, '_>,
//! #    ) -> demultiplex::NullPacketFilter<NullDemuxContext> {
//! #        unimplemented!();
//! #    }
//! # }
//! pub struct DumpSpliceInfoProcessor;
//! impl scte35_reader::SpliceInfoProcessor for DumpSpliceInfoProcessor {
//!     fn process(
//!         &self,
//!         header: scte35_reader::SpliceInfoHeader<'_>,
//!         command: scte35_reader::SpliceCommand,
//!         descriptors: scte35_reader::SpliceDescriptors<'_>,
//!     ) {
//!         println!("{:?} {:#?}", header, command);
//!         for d in &descriptors {
//!             println!(" - {:?}", d);
//!         }
//!     }
//! }
//!
//! let data = hex!(
//!             "fc302500000000000000fff01405000000017feffe2d142b00fe0123d3080001010100007f157a49"
//!         );
//! let mut parser = Scte35SectionProcessor::new(DumpSpliceInfoProcessor);
//! let header = psi::SectionCommonHeader::new(&data[..psi::SectionCommonHeader::SIZE]);
//! let mut ctx = NullDemuxContext::new();
//! parser.section(&mut ctx, &header, &data[..]);
//! ```
//!
//! Output:
//!
//! ```plain
//! SpliceInfoHeader { protocol_version: 0, encrypted_packet: false, encryption_algorithm: None, pts_adjustment: 0, cw_index: 0, tier: 4095 } SpliceInsert {
//!     splice_event_id: 1,
//!     reserved: 127,
//!     splice_detail: Insert {
//!         network_indicator: Out,
//!         splice_mode: Program(
//!             Timed(
//!                 Some(
//!                     756296448
//!                 )
//!             )
//!         ),
//!         duration: Some(
//!             SpliceDuration {
//!                 return_mode: Automatic,
//!                 duration: 19125000
//!             }
//!         ),
//!         unique_program_id: 1,
//!         avail_num: 1,
//!         avails_expected: 1
//!     }
//! }
//! ```

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms, future_incompatible)]

pub mod upid;

use bitreader::BitReaderError;
use mpeg2ts_reader::demultiplex;
use mpeg2ts_reader::psi;
use mpeg2ts_reader::smptera::FormatIdentifier;
use serde::ser::{SerializeSeq, SerializeStruct};
use std::convert::TryInto;
use std::marker;

/// The StreamType which might be used for `SCTE-35` data
pub const SCTE35_STREAM_TYPE: mpeg2ts_reader::StreamType = mpeg2ts_reader::StreamType(0x86);

/// Utility function to search the PTM section for a `CUEI` registration descriptor per
/// _SCTE-35, section 8.1_, which indicates that streams with `stream_type` equal to the private
/// value `0x86` within this PMT section are formatted according to SCTE-35.
///
/// Returns `true` if the descriptor is attached to the given PMT section and `false` otherwise.
pub fn is_scte35(pmt: &mpeg2ts_reader::psi::pmt::PmtSection<'_>) -> bool {
    for d in pmt.descriptors().flatten() {
        if let mpeg2ts_reader::descriptor::CoreDescriptors::Registration(reg) = d {
            if reg.is_format(FormatIdentifier::CUEI) {
                return true;
            }
        }
    }
    false
}
/// Encryption algorithm applied to portions of a _splice_info_section_.
#[derive(Debug, PartialEq, serde_derive::Serialize)]
pub enum EncryptionAlgorithm {
    /// No encryption.
    None,
    /// DES in Electronic Code Book (ECB) mode.
    DesEcb,
    /// DES in Cipher Block Chaining (CBC) mode.
    DesCbc,
    /// Triple DES (EDE3) in ECB mode.
    TripleDesEde3Ecb,
    /// Reserved algorithm identifier.
    Reserved(u8),
    /// User-private algorithm identifier.
    Private(u8),
}
impl EncryptionAlgorithm {
    /// Returns the `EncryptionAlgorithm` variant corresponding to the given identifier byte.
    pub fn from_id(id: u8) -> EncryptionAlgorithm {
        match id {
            0 => EncryptionAlgorithm::None,
            1 => EncryptionAlgorithm::DesEcb,
            2 => EncryptionAlgorithm::DesCbc,
            3 => EncryptionAlgorithm::TripleDesEde3Ecb,
            _ => {
                if id < 32 {
                    EncryptionAlgorithm::Reserved(id)
                } else {
                    EncryptionAlgorithm::Private(id)
                }
            }
        }
    }
}

/// Identifies the type of _splice-command_ present in a _splice_info_section_.
#[derive(Debug, PartialEq, serde_derive::Serialize)]
pub enum SpliceCommandType {
    /// A `splice_null` command.
    SpliceNull,
    /// A reserved or unrecognised command type.
    Reserved(u8),
    /// A `splice_schedule` command.
    SpliceSchedule,
    /// A `splice_insert` command.
    SpliceInsert,
    /// A `time_signal` command.
    TimeSignal,
    /// A `bandwidth_reservation` command.
    BandwidthReservation,
    /// A `private_command`.
    PrivateCommand,
}
impl SpliceCommandType {
    /// Returns the `SpliceCommandType` variant corresponding to the given identifier byte.
    pub fn from_id(id: u8) -> SpliceCommandType {
        match id {
            0x00 => SpliceCommandType::SpliceNull,
            0x04 => SpliceCommandType::SpliceSchedule,
            0x05 => SpliceCommandType::SpliceInsert,
            0x06 => SpliceCommandType::TimeSignal,
            0x07 => SpliceCommandType::BandwidthReservation,
            0xff => SpliceCommandType::PrivateCommand,
            _ => SpliceCommandType::Reserved(id),
        }
    }
}

/// Header element within a SCTE-43 _splice_info_section_ containing metadata generic across all kinds of _splice-command_.
///
/// This is a wrapper around a byte-slice that will extract requested fields on demand, as its
/// methods are called.
pub struct SpliceInfoHeader<'a> {
    buf: &'a [u8],
}
impl<'a> SpliceInfoHeader<'a> {
    const HEADER_LENGTH: usize = 11;

    /// Splits the given buffer into a `SpliceInfoHeader` element, and a remainder which will
    /// include the _splice-command_ itself, plus any _descriptor_loop_.
    pub fn new(buf: &'a [u8]) -> (SpliceInfoHeader<'a>, &'a [u8]) {
        if buf.len() < 11 {
            panic!("buffer too short: {} (expected 11)", buf.len());
        }
        let (head, tail) = buf.split_at(11);
        (SpliceInfoHeader { buf: head }, tail)
        // TODO: change this to return Err if the protocol_version or encrypted_packet values are
        //       unsupported
    }

    /// The version of the SCTE-35 data structures carried in this _splice_info_section_ (only
    /// version `0` is supported by this library).
    pub fn protocol_version(&self) -> u8 {
        self.buf[0]
    }

    /// Indicates that portions of this _splice_info_section_ are encrypted (only un-encrypted
    /// data is supported by this library).
    pub fn encrypted_packet(&self) -> bool {
        self.buf[1] & 0b1000_0000 != 0
    }
    /// The algorithm by which portions of this _splice_info_section_ are encrypted (only
    /// un-encrypted data is supported by this library).
    pub fn encryption_algorithm(&self) -> EncryptionAlgorithm {
        EncryptionAlgorithm::from_id((self.buf[1] & 0b0111_1110) >> 1)
    }
    /// A 33-bit adjustment value to be applied to any PTS value in a _splice-command_ within this
    /// _splice_info_section_.
    pub fn pts_adjustment(&self) -> u64 {
        u64::from(self.buf[1] & 1) << 32
            | u64::from(self.buf[2]) << 24
            | u64::from(self.buf[3]) << 16
            | u64::from(self.buf[4]) << 8
            | u64::from(self.buf[5])
    }
    /// Identifier for the 'control word' (key) used to encrypt the message, if `encrypted_packet`
    /// is true.
    pub fn cw_index(&self) -> u8 {
        self.buf[6]
    }
    /// 12-bit authorization tier.
    pub fn tier(&self) -> u16 {
        u16::from(self.buf[7]) << 4 | u16::from(self.buf[8]) >> 4
    }
    /// Length in bytes of the _splice-command_ data within this message.
    pub fn splice_command_length(&self) -> u16 {
        u16::from(self.buf[8] & 0b0000_1111) << 8 | u16::from(self.buf[9])
    }
    /// Type of _splice-command_ within this message.
    pub fn splice_command_type(&self) -> SpliceCommandType {
        SpliceCommandType::from_id(self.buf[10])
    }
}
impl<'a> serde::Serialize for SpliceInfoHeader<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut s = serializer.serialize_struct("SpliceInfoHeader", 6)?;
        s.serialize_field("protocol_version", &self.protocol_version())?;
        s.serialize_field("encrypted_packet", &self.encrypted_packet())?;
        s.serialize_field("encryption_algorithm", &self.encryption_algorithm())?;
        s.serialize_field("pts_adjustment", &self.pts_adjustment())?;
        s.serialize_field("cw_index", &self.cw_index())?;
        s.serialize_field("tier", &self.tier())?;
        s.end()
    }
}
impl<'a> std::fmt::Debug for SpliceInfoHeader<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("SpliceInfoHeader");
        s.field("protocol_version", &self.protocol_version());
        s.field("encrypted_packet", &self.encrypted_packet());
        s.field("encryption_algorithm", &self.encryption_algorithm());
        s.field("pts_adjustment", &self.pts_adjustment());
        s.field("cw_index", &self.cw_index());
        s.field("tier", &self.tier());
        s.finish()
    }
}

/// Parsed _splice-command_ from a _splice_info_section_.
#[non_exhaustive]
#[derive(Debug, serde_derive::Serialize)]
pub enum SpliceCommand {
    /// A no-op command.
    SpliceNull {},
    /// A request to insert a splice point.
    SpliceInsert {
        /// Unique identifier for this splice event.
        splice_event_id: u32,
        /// Reserved bits.
        reserved: u8,
        /// Details of the splice event, or cancellation.
        splice_detail: SpliceInsert,
    },
    /// A time signal carrying a PTS timestamp.
    TimeSignal {
        /// The splice time conveyed by this command.
        splice_time: SpliceTime,
    },
    /// A bandwidth reservation command (carries no additional data).
    BandwidthReservation {},
    /// A user-defined private command.
    PrivateCommand {
        /// 32-bit format identifier registered by the owner of this private command.
        identifier: u32,
        /// The private command payload bytes.
        private_bytes: Vec<u8>,
    }
}

/// Indicates whether the splice point is an out-of-network or in-to-network transition.
#[derive(Debug, serde_derive::Serialize)]
pub enum NetworkIndicator {
    /// Splice out of the network (e.g. to an ad break).
    Out,
    /// Splice back into the network.
    In,
}
impl NetworkIndicator {
    /// panics if `id` is something other than `0` or `1`
    pub fn from_flag(id: u8) -> NetworkIndicator {
        match id {
            0 => NetworkIndicator::In,
            1 => NetworkIndicator::Out,
            _ => panic!(
                "Invalid out_of_network_indicator value: {} (expected 0 or 1)",
                id
            ),
        }
    }
}

/// Detail of a `splice_insert` command: either a cancellation or an insertion with full parameters.
#[derive(Debug, serde_derive::Serialize)]
pub enum SpliceInsert {
    /// The previously-announced splice event has been cancelled.
    Cancel,
    /// A splice point insertion.
    Insert {
        /// Whether this is an out-of-network or in-to-network splice.
        network_indicator: NetworkIndicator,
        /// Program-level or component-level splice mode.
        splice_mode: SpliceMode,
        /// Optional break duration.
        duration: Option<SpliceDuration>,
        /// Unique identifier for the viewing event.
        unique_program_id: u16,
        /// Identification of a specific avail within this viewing event.
        avail_num: u8,
        /// Expected number of avails within this viewing event.
        avails_expected: u8,
    },
}

/// Indicates the time of a splice point.
#[derive(Debug, serde_derive::Serialize)]
pub enum SpliceTime {
    /// The `splice_immediate_flag` was set in the `splice_insert` command — no `splice_time()`
    /// structure is present in the bitstream.
    Immediate,
    /// A `splice_time()` structure was present.  Contains `Some(pts)` when
    /// `time_specified_flag` was set, or `None` when it was not (no PTS value given).
    Timed(Option<u64>),
}

/// A per-component splice point within a component-mode splice.
#[derive(Debug, serde_derive::Serialize)]
pub struct ComponentSplice {
    /// Identifies the elementary stream component.  This value matches a
    /// `component_tag` value carried in the PMT's `stream_identifier_descriptor`
    /// for the corresponding elementary stream.
    pub component_tag: u8,
    /// The splice time for this component.
    pub splice_time: SpliceTime,
}

/// Whether a splice applies to the whole program or to individual components.
#[derive(Debug, serde_derive::Serialize)]
pub enum SpliceMode {
    /// Program-level splice: all components share a single splice time.
    Program(SpliceTime),
    /// Component-level splice: each listed component has its own splice time.
    Components(Vec<ComponentSplice>),
}

/// Whether return from the break is automatic or manual.
#[derive(Debug, serde_derive::Serialize)]
pub enum ReturnMode {
    /// The splicer automatically returns to the network feed after the break duration elapses.
    Automatic,
    /// A subsequent splice command is required to return to the network feed.
    Manual,
}
impl ReturnMode {
    /// Returns the `ReturnMode` corresponding to the `auto_return` flag value.
    pub fn from_flag(flag: u8) -> ReturnMode {
        match flag {
            0 => ReturnMode::Manual,
            1 => ReturnMode::Automatic,
            _ => panic!("Invalid auto_return value: {} (expected 0 or 1)", flag),
        }
    }
}

/// Identifies the type of _Segmentation UPID_ carried in a `segmentation_descriptor`.
#[derive(Debug, PartialEq, serde_derive::Serialize, Copy, Clone)]
pub enum SegmentationUpidType {
    /// No UPID present (`0x00`).
    NotUsed,
    /// Deprecated user-defined UPID (`0x01`).
    UserDefinedDeprecated,
    /// _Industry Standard Commercial Identifier_ (deprecated, `0x02`).
    ISCIDeprecated,
    /// Defined by the _Advertising Digital Identification_ group (`0x03`).
    AdID,
    /// SMPTE UMID (`0x04`).
    UMID,
    /// Deprecated ISAN (`0x05`).
    ISANDeprecated,
    /// Versioned ISAN per ISO 15706-2 (`0x06`).
    ISAN,
    /// Tribune Media Systems Program identifier (`0x07`).
    TID,
    /// AiringID, formerly Turner ID (`0x08`).
    TI,
    /// CableLabs metadata identifier (`0x09`).
    ADI,
    /// Entertainment ID Registry Association identifier (`0x0A`).
    EIDR,
    /// ATSC content identifier (`0x0B`).
    ATSC,
    /// Managed Private UPID (`0x0C`).
    MPU,
    /// Multiple UPID structure (`0x0D`).
    MID,
    /// Advertising information (`0x0E`).
    ADS,
    /// Universal Resource Identifier (`0x0F`).
    URI,
    /// Reserved for future standardisation.
    Reserved(u8),
}
impl SegmentationUpidType {
    /// Returns the `SegmentationUpidType` variant for the given `segmentation_upid_type` byte.
    pub fn from_type(id: u8) -> SegmentationUpidType {
        match id {
            0 => SegmentationUpidType::NotUsed,
            1 => SegmentationUpidType::UserDefinedDeprecated,
            2 => SegmentationUpidType::ISCIDeprecated,
            3 => SegmentationUpidType::AdID,
            4 => SegmentationUpidType::UMID,
            5 => SegmentationUpidType::ISANDeprecated,
            6 => SegmentationUpidType::ISAN,
            7 => SegmentationUpidType::TID,
            8 => SegmentationUpidType::TI,
            9 => SegmentationUpidType::ADI,
            10 => SegmentationUpidType::EIDR,
            11 => SegmentationUpidType::ATSC,
            12 => SegmentationUpidType::MPU,
            13 => SegmentationUpidType::MID,
            14 => SegmentationUpidType::ADS,
            15 => SegmentationUpidType::URI,
            _ => SegmentationUpidType::Reserved(id),
        }
    }
}

/// Identifies the segmentation type in a `segmentation_descriptor`.
///
/// Named constants are provided for the well-known values defined in SCTE-35 Table 22.
#[derive(Debug, PartialEq, serde_derive::Serialize)]
pub struct SegmentationTypeId(pub u8);

impl SegmentationTypeId {
    /// No segmentation type specified (`0x00`).
    pub const NOT_INDICATED: SegmentationTypeId = SegmentationTypeId(0);
    /// Identifies content without signaling a segmentation point (`0x01`).
    pub const CONTENT_IDENTIFICATION: SegmentationTypeId = SegmentationTypeId(1);
    /// Marks the beginning of a program (`0x10`).
    pub const PROGRAM_START: SegmentationTypeId = SegmentationTypeId(16);
    /// Marks the end of a program (`0x11`).
    pub const PROGRAM_END: SegmentationTypeId = SegmentationTypeId(17);
    /// The program ended earlier than its scheduled end time (`0x12`).
    pub const PROGRAM_EARLY_TERMINATION: SegmentationTypeId = SegmentationTypeId(18);
    /// Temporary departure from the scheduled program to unscheduled content (`0x13`).
    pub const PROGRAM_BREAKAWAY: SegmentationTypeId = SegmentationTypeId(19);
    /// Return to the scheduled program after a breakaway (`0x14`).
    pub const PROGRAM_RESUMPTION: SegmentationTypeId = SegmentationTypeId(20);
    /// The program has run past its scheduled end time, and the overrun was expected (`0x15`).
    pub const PROGRAM_RUNOVER_PLANNED: SegmentationTypeId = SegmentationTypeId(21);
    /// The program has run past its scheduled end time unexpectedly (`0x16`).
    pub const PROGRAM_RUNOVER_UNPLANNED: SegmentationTypeId = SegmentationTypeId(22);
    /// Start of content that overlaps with the previous or next program (`0x17`).
    pub const PROGRAM_OVERLAP_START: SegmentationTypeId = SegmentationTypeId(23);
    /// Overrides a blackout applied to the current program (`0x18`).
    pub const PROGRAM_BLACKOUT_OVERRIDE: SegmentationTypeId = SegmentationTypeId(24);
    /// Joining a program that is already in progress (`0x19`).
    pub const PROGRAM_START_IN_PROGRESS: SegmentationTypeId = SegmentationTypeId(25);
    /// Start of a chapter within a program (`0x20`).
    pub const CHAPTER_START: SegmentationTypeId = SegmentationTypeId(32);
    /// End of a chapter within a program (`0x21`).
    pub const CHAPTER_END: SegmentationTypeId = SegmentationTypeId(33);
    /// Start of a break within a program (e.g. commercial break, `0x22`).
    pub const BREAK_START: SegmentationTypeId = SegmentationTypeId(34);
    /// End of a break within a program (`0x23`).
    pub const BREAK_END: SegmentationTypeId = SegmentationTypeId(35);
    /// Start of an advertisement placed by the content provider (`0x30`).
    pub const PROVIDER_ADVERTISEMENT_START: SegmentationTypeId = SegmentationTypeId(48);
    /// End of an advertisement placed by the content provider (`0x31`).
    pub const PROVIDER_ADVERTISEMENT_END: SegmentationTypeId = SegmentationTypeId(49);
    /// Start of an advertisement placed by the distributor (`0x32`).
    pub const DISTRIBUTOR_ADVERTISEMENT_START: SegmentationTypeId = SegmentationTypeId(50);
    /// End of an advertisement placed by the distributor (`0x33`).
    pub const DISTRIBUTOR_ADVERTISEMENT_END: SegmentationTypeId = SegmentationTypeId(51);
    /// Start of an opportunity for the provider to place content (`0x34`).
    pub const PROVIDER_PLACEMENT_OPPORTUNITY_START: SegmentationTypeId = SegmentationTypeId(52);
    /// End of a provider placement opportunity (`0x35`).
    pub const PROVIDER_PLACEMENT_OPPORTUNITY_END: SegmentationTypeId = SegmentationTypeId(53);
    /// Start of an opportunity for the distributor to place content (`0x36`).
    pub const DISTRIBUTOR_PLACEMENT_OPPORTUNITY_START: SegmentationTypeId = SegmentationTypeId(54);
    /// End of a distributor placement opportunity (`0x37`).
    pub const DISTRIBUTOR_PLACEMENT_OPPORTUNITY_END: SegmentationTypeId = SegmentationTypeId(55);
    /// Start of an event not on the regular schedule, such as breaking news (`0x40`).
    pub const UNSCHEDULED_EVENT_START: SegmentationTypeId = SegmentationTypeId(64);
    /// End of an unscheduled event (`0x41`).
    pub const UNSCHEDULED_EVENT_END: SegmentationTypeId = SegmentationTypeId(65);
    /// Start of a network feed (e.g. return from local programming, `0x50`).
    pub const NETWORK_START: SegmentationTypeId = SegmentationTypeId(80);
    /// End of a network feed (`0x51`).
    pub const NETWORK_END: SegmentationTypeId = SegmentationTypeId(81);
}
impl SegmentationTypeId {
    /// Wraps the raw byte value as a `SegmentationTypeId`.
    pub fn from_id(id: u8) -> SegmentationTypeId {
        SegmentationTypeId(id)
    }

    /// Returns a human-readable description of this segmentation type.
    pub fn description(&self) -> &'static str {
        match *self {
            SegmentationTypeId::NOT_INDICATED => "Not Indicated",
            SegmentationTypeId::CONTENT_IDENTIFICATION => "Content Identification",
            SegmentationTypeId::PROGRAM_START => "Program Start",
            SegmentationTypeId::PROGRAM_END => "Program End",
            SegmentationTypeId::PROGRAM_EARLY_TERMINATION => "Program Early Termination",
            SegmentationTypeId::PROGRAM_BREAKAWAY => "Program Breakaway",
            SegmentationTypeId::PROGRAM_RESUMPTION => "Program Resumption",
            SegmentationTypeId::PROGRAM_RUNOVER_PLANNED => "Program Runover Planned",
            SegmentationTypeId::PROGRAM_RUNOVER_UNPLANNED => "Program Runover Unplanned",
            SegmentationTypeId::PROGRAM_OVERLAP_START => "Program Overlap Start",
            SegmentationTypeId::PROGRAM_BLACKOUT_OVERRIDE => "Program Blackout Override",
            SegmentationTypeId::PROGRAM_START_IN_PROGRESS => "Program Start In Progress",
            SegmentationTypeId::CHAPTER_START => "Chapter Start",
            SegmentationTypeId::CHAPTER_END => "Chapter End",
            SegmentationTypeId::BREAK_START => "Break Start",
            SegmentationTypeId::BREAK_END => "Break End",
            SegmentationTypeId::PROVIDER_ADVERTISEMENT_START => "Provider Advertisement Start",
            SegmentationTypeId::PROVIDER_ADVERTISEMENT_END => "Provider Advertisement End",
            SegmentationTypeId::DISTRIBUTOR_ADVERTISEMENT_START => "Distributor Advertisement Start",
            SegmentationTypeId::DISTRIBUTOR_ADVERTISEMENT_END => "Distributor Advertisement End",
            SegmentationTypeId::PROVIDER_PLACEMENT_OPPORTUNITY_START => "Provider Placement Opportunity Start",
            SegmentationTypeId::PROVIDER_PLACEMENT_OPPORTUNITY_END => "Provider Placement Opportunity End",
            SegmentationTypeId::DISTRIBUTOR_PLACEMENT_OPPORTUNITY_START => "Distributor Placement Opportunity Start",
            SegmentationTypeId::DISTRIBUTOR_PLACEMENT_OPPORTUNITY_END => "Distributor Placement Opportunity End",
            SegmentationTypeId::UNSCHEDULED_EVENT_START => "Unscheduled Event Start",
            SegmentationTypeId::UNSCHEDULED_EVENT_END => "Unscheduled Event End",
            SegmentationTypeId::NETWORK_START => "Network Start",
            SegmentationTypeId::NETWORK_END => "Network End",
            _ => "Reserved",
        }
    }
}

/// Parsed _Segmentation Unique Program Identifier_ from a `segmentation_descriptor`.
///
/// Each variant corresponds to a different UPID scheme; see the [`upid`] module for the
/// inner payload types.
#[derive(Debug, serde_derive::Serialize)]
pub enum SegmentationUpid {
    /// No UPID present.
    None,
    /// Deprecated user-defined UPID.
    UserDefined(upid::UserDefinedDeprecated),
    /// Deprecated ISCI identifier.
    Isci(upid::IsciDeprecated),
    /// Ad-ID identifier.
    AdID(upid::AdID),
    /// Deprecated (non-versioned) ISAN.
    IsanDeprecated(upid::IsanDeprecated),
    /// SMPTE UMID.
    Umid(upid::Umid),
    /// Tribune Media Systems Program identifier.
    TID(upid::TID),
    /// AiringID (formerly Turner ID).
    TI(upid::TI),
    /// CableLabs metadata identifier.
    ADI(upid::ADI),
    /// EIDR identifier in compact binary form.
    EIDR(upid::EIDR),
    /// ATSC content identifier.
    ATSC(upid::ATSC),
    /// Managed Private UPID.
    MPU(upid::MPU),
    /// Multiple UPID structure containing a list of sub-UPIDs.
    MID(Vec<SegmentationUpid>),
    /// Advertising information.
    ADS(upid::ADSInformation),
    /// URI.
    URI(upid::Url),
    /// Reserved or unrecognised UPID type with its raw bytes.
    Reserved(SegmentationUpidType, Vec<u8>),
}
impl SegmentationUpid {
    fn parse(
        r: &mut bitreader::BitReader<'_>,
        segmentation_upid_type: SegmentationUpidType,
        segmentation_upid_length: u8,
    ) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        if segmentation_upid_length > 0 {
            let upid_result: Result<Vec<u8>, bitreader::BitReaderError> = (0
                ..segmentation_upid_length)
                .map(|_| r.read_u8(8))
                .collect();
            let upid = upid_result.named("segmentation_descriptor.segmentation_upid")?;
            SegmentationUpid::parse_payload(segmentation_upid_type, upid)
        } else {
            Ok(SegmentationUpid::None)
        }
    }

    // TODO: rework 'upid' param from Vec<u8> into &[u8]
    fn parse_payload(
        segmentation_upid_type: SegmentationUpidType,
        upid: Vec<u8>,
    ) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        match segmentation_upid_type {
            SegmentationUpidType::NotUsed => Err(
                SpliceDescriptorErr::SegmentationUpidLengthTypeMismatch(segmentation_upid_type),
            ),
            SegmentationUpidType::UserDefinedDeprecated => Self::parse_user_defined(upid),
            SegmentationUpidType::ISCIDeprecated => Self::parse_isci(upid),
            SegmentationUpidType::AdID => Self::parse_adid(upid),
            SegmentationUpidType::UMID => Self::parse_umid(upid),
            SegmentationUpidType::ISANDeprecated => Self::parse_isan_deprecated(upid),
            SegmentationUpidType::ISAN => Self::parse_isan(upid),
            SegmentationUpidType::TID => Self::parse_tid(upid),
            SegmentationUpidType::TI => Self::parse_ti(upid),
            SegmentationUpidType::ADI => Self::parse_adi(upid),
            SegmentationUpidType::EIDR => Self::parse_eidr(upid),
            SegmentationUpidType::ATSC => Self::parse_atsc(upid),
            SegmentationUpidType::MPU => Self::parse_mpu(upid),
            SegmentationUpidType::MID => Self::parse_mid(upid),
            SegmentationUpidType::ADS => Self::parse_ads(upid),
            SegmentationUpidType::URI => Self::parse_url(upid),
            SegmentationUpidType::Reserved(_) => Self::parse_reserved(segmentation_upid_type, upid),
        }
    }

    /// Returns the encoded length of the UPID payload in bytes.
    pub fn segmentation_upid_length(&self) -> usize {
        match self {
            SegmentationUpid::None => 0,
            SegmentationUpid::UserDefined(v) => v.0.len(),
            SegmentationUpid::Isci(_) => 8,
            SegmentationUpid::AdID(_) => 12,
            SegmentationUpid::IsanDeprecated(_) => 8,
            SegmentationUpid::Umid(_) => 32,
            SegmentationUpid::TID(_) => 12,
            SegmentationUpid::TI(_) => 8,
            SegmentationUpid::ADI(adi) => adi.0.len(),
            SegmentationUpid::EIDR(_) => 12,
            SegmentationUpid::ATSC(atsc) => atsc.0.len(),
            SegmentationUpid::MPU(m) => m.0.len(),
            SegmentationUpid::MID(v) => {
                v.len() * 2
                    + v.iter()
                        .map(|upid| upid.segmentation_upid_length())
                        .sum::<usize>()
            }
            SegmentationUpid::ADS(a) => a.0.len(),
            SegmentationUpid::URI(u) => u.0.as_str().len(),
            SegmentationUpid::Reserved(_, r) => r.len(),
        }
    }

    /// Returns the `SegmentationUpidType` that corresponds to this UPID variant.
    pub fn segmentation_upid_type(&self) -> SegmentationUpidType {
        match self {
            SegmentationUpid::None => SegmentationUpidType::NotUsed,
            SegmentationUpid::UserDefined(_) => SegmentationUpidType::UserDefinedDeprecated,
            SegmentationUpid::Isci(_) => SegmentationUpidType::ISCIDeprecated,
            SegmentationUpid::AdID(_) => SegmentationUpidType::AdID,
            SegmentationUpid::IsanDeprecated(_) => SegmentationUpidType::ISAN,
            SegmentationUpid::Umid(_) => SegmentationUpidType::UMID,
            SegmentationUpid::TID(_) => SegmentationUpidType::TID,
            SegmentationUpid::TI(_) => SegmentationUpidType::TI,
            SegmentationUpid::ADI(_) => SegmentationUpidType::ADI,
            SegmentationUpid::EIDR(_) => SegmentationUpidType::EIDR,
            SegmentationUpid::ATSC(_) => SegmentationUpidType::ATSC,
            SegmentationUpid::MPU(_) => SegmentationUpidType::MPU,
            SegmentationUpid::MID(_) => SegmentationUpidType::MID,
            SegmentationUpid::ADS(_) => SegmentationUpidType::ADS,
            SegmentationUpid::URI(_) => SegmentationUpidType::URI,
            SegmentationUpid::Reserved(t, _) => *t,
        }
    }

    fn parse_user_defined(upid: Vec<u8>) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        Ok(SegmentationUpid::UserDefined(upid::UserDefinedDeprecated(
            upid,
        )))
    }
    fn parse_isci(upid: Vec<u8>) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        chk_upid(&upid, 8, SegmentationUpidType::ISCIDeprecated)?;
        upid_from_utf8(upid, SegmentationUpidType::ISCIDeprecated)
            .map(|s| SegmentationUpid::Isci(upid::IsciDeprecated(s)))
    }
    fn parse_adid(upid: Vec<u8>) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        chk_upid(&upid, 12, SegmentationUpidType::AdID)?;
        upid_from_utf8(upid, SegmentationUpidType::AdID)
            .map(|s| SegmentationUpid::AdID(upid::AdID(s)))
    }
    fn parse_umid(upid: Vec<u8>) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        chk_upid(&upid, 32, SegmentationUpidType::UMID)?;
        Ok(SegmentationUpid::Umid(upid::Umid(upid)))
    }
    fn parse_isan_deprecated(upid: Vec<u8>) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        chk_upid(&upid, 8, SegmentationUpidType::ISANDeprecated)?;
        Ok(SegmentationUpid::IsanDeprecated(upid::IsanDeprecated(upid)))
    }
    fn parse_isan(upid: Vec<u8>) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        chk_upid(&upid, 12, SegmentationUpidType::ISAN)?;
        Ok(SegmentationUpid::IsanDeprecated(upid::IsanDeprecated(upid)))
    }
    fn parse_tid(upid: Vec<u8>) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        chk_upid(&upid, 12, SegmentationUpidType::TID)?;
        upid_from_utf8(upid, SegmentationUpidType::TID).map(|s| SegmentationUpid::TID(upid::TID(s)))
    }
    fn parse_ti(upid: Vec<u8>) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        chk_upid(&upid, 8, SegmentationUpidType::TI)?;
        Ok(SegmentationUpid::TI(upid::TI(upid)))
    }
    fn parse_adi(upid: Vec<u8>) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        upid_from_utf8(upid, SegmentationUpidType::ADI).map(|s| SegmentationUpid::ADI(upid::ADI(s)))
    }
    fn parse_eidr(upid: Vec<u8>) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        chk_upid(&upid, 12, SegmentationUpidType::EIDR)?;
        Ok(SegmentationUpid::EIDR(upid::EIDR(
            upid.as_slice().try_into().unwrap(),
        )))
    }
    fn parse_atsc(upid: Vec<u8>) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        Ok(SegmentationUpid::ATSC(upid::ATSC(upid)))
    }
    fn parse_mpu(upid: Vec<u8>) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        // TODO: first 4 bytes a 'format_identifier' per https://crates.io/crates/smptera-format-identifiers-rust
        Ok(SegmentationUpid::MPU(upid::MPU(upid)))
    }
    fn parse_mid(upid: Vec<u8>) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        let mut data = &upid[..];
        let mut result = vec![];
        while !data.is_empty() {
            if data.len() < 2 {
                return Err(SpliceDescriptorErr::not_enough_data("MID.length", 1, 0));
            }
            let segmentation_upid_type = SegmentationUpidType::from_type(data[0]);
            let length = data[1] as usize;
            let payload_end = 2 + length;
            if data.len() < payload_end {
                return Err(SpliceDescriptorErr::not_enough_data(
                    "MID.segmentation_upid",
                    length,
                    data.len() - 2,
                ));
            }
            let payload = &data[2..payload_end];
            result.push(Self::parse_payload(
                segmentation_upid_type,
                payload.to_vec(),
            )?);
            data = &data[payload_end..];
        }
        Ok(SegmentationUpid::MID(result))
    }
    fn parse_ads(upid: Vec<u8>) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        Ok(SegmentationUpid::ADS(upid::ADSInformation(upid)))
    }
    fn parse_url(upid: Vec<u8>) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        upid_from_utf8(upid, SegmentationUpidType::URI)
            .and_then(|s| {
                url::Url::parse(&s).map_err(|_| SpliceDescriptorErr::InvalidUpidContent {
                    upid_type: SegmentationUpidType::URI,
                    bytes: s.into_bytes(),
                })
            })
            .map(|u| SegmentationUpid::URI(upid::Url(u)))
    }
    fn parse_reserved(
        segmentation_upid_type: SegmentationUpidType,
        upid: Vec<u8>,
    ) -> Result<SegmentationUpid, SpliceDescriptorErr> {
        Ok(SegmentationUpid::Reserved(segmentation_upid_type, upid))
    }
}

/// helper wrapping String::from_utf8() and producing a useful error type
fn upid_from_utf8(
    upid: Vec<u8>,
    upid_type: SegmentationUpidType,
) -> Result<String, SpliceDescriptorErr> {
    String::from_utf8(upid).map_err(|e| SpliceDescriptorErr::InvalidUpidContent {
        upid_type,
        bytes: e.into_bytes(),
    })
}

fn chk_upid(
    upid: &[u8],
    expected: usize,
    upid_type: SegmentationUpidType,
) -> Result<(), SpliceDescriptorErr> {
    if upid.len() == expected {
        Ok(())
    } else {
        Err(SpliceDescriptorErr::InvalidUpidLength {
            upid_type,
            expected,
            actual: upid.len(),
        })
    }
}

/// Device group restrictions that may be applied to a segmentation descriptor.
#[derive(Debug, serde_derive::Serialize)]
pub enum DeviceRestrictions {
    /// Restrict to device group 0.
    RestrictGroup0,
    /// Restrict to device group 1.
    RestrictGroup1,
    /// Restrict to device group 2.
    RestrictGroup2,
    /// No device restrictions.
    None,
}
impl DeviceRestrictions {
    /// panics if `id` is something other than `0`, `1`, `2` or `3`
    pub fn from_bits(restriction: u8) -> DeviceRestrictions {
        match restriction {
            0 => DeviceRestrictions::RestrictGroup0,
            1 => DeviceRestrictions::RestrictGroup1,
            2 => DeviceRestrictions::RestrictGroup2,
            3 => DeviceRestrictions::None,
            _ => panic!(
                "Invalid device_restrictions value: {} (expected 0, 1, 2, 3)",
                restriction
            ),
        }
    }
}

/// Delivery restriction flags from a `segmentation_descriptor`.
#[derive(Debug, serde_derive::Serialize)]
pub enum DeliveryRestrictionFlags {
    /// Delivery is not restricted.
    None,
    /// Delivery restrictions are in effect.
    DeliveryRestrictions {
        /// Whether web delivery of this segment is allowed.
        web_delivery_allowed_flag: bool,
        /// When `true`, no regional blackout applies to this segment.
        no_regional_blackout_flag: bool,
        /// Whether archiving of this segment is allowed.
        archive_allowed_flag: bool,
        /// Device group restrictions.
        device_restrictions: DeviceRestrictions,
    },
}

/// Whether segmentation applies to the whole program or to individual components.
#[derive(Debug, serde_derive::Serialize)]
pub enum SegmentationMode {
    /// Program-level segmentation.
    Program,
    /// Component-level segmentation with per-component PTS offsets.
    Component {
        /// The components affected by this segmentation event.
        components: Vec<SegmentationModeComponent>,
    },
}

/// A component entry within a component-mode `segmentation_descriptor`.
#[derive(Debug, serde_derive::Serialize)]
pub struct SegmentationModeComponent {
    /// Identifies the elementary stream component, matching a `component_tag`
    /// in the PMT's `stream_identifier_descriptor`.
    pub component_tag: u8,
    /// PTS offset to be added to the `splice_time` to obtain this component's splice point.
    pub pts_offset: u64,
}

/// Parsed detail of a `segmentation_descriptor`.
#[derive(Debug, serde_derive::Serialize)]
pub enum SegmentationDescriptor {
    /// This segmentation event has been cancelled.
    Cancel,
    /// A segmentation event.
    Insert {
        /// `true` when segmentation applies to the whole program rather than individual components.
        program_segmentation_flag: bool,
        /// `true` when `segmentation_duration` is present.
        segmentation_duration_flag: bool,
        /// `true` when delivery is not restricted.
        delivery_not_restricted_flag: bool,
        /// Delivery restriction flags, if applicable.
        delivery_restrictions: DeliveryRestrictionFlags,
        /// Program-level or component-level segmentation mode.
        segmentation_mode: SegmentationMode,
        /// Duration of the segment in 90 kHz ticks, if signaled.
        segmentation_duration: Option<u64>,
        /// The Unique Program Identifier for this segment.
        segmentation_upid: SegmentationUpid,
        /// The type of segmentation (e.g. program start, ad break, etc.).
        segmentation_type_id: SegmentationTypeId,
        /// Current segment number within the segmentation event.
        segment_num: u8,
        /// Total number of expected segments.
        segments_expected: u8,
        /// Optional sub-segment numbering.
        sub_segments: Option<SubSegments>,
    },
}

/// Optional sub-segment numbering that may appear at the end of a `segmentation_descriptor`.
#[derive(Debug, serde_derive::Serialize)]
pub struct SubSegments {
    /// Current sub-segment number.
    pub sub_segment_num: u8,
    /// Total number of expected sub-segments.
    pub sub_segments_expected: u8,
}

/// Duration of a break, as carried in a `break_duration()` structure.
#[derive(Debug, serde_derive::Serialize)]
pub struct SpliceDuration {
    /// Whether return from the break is automatic or requires a subsequent command.
    pub return_mode: ReturnMode,
    /// Duration of the break in 90 kHz ticks.
    pub duration: u64,
}

/// Callback trait for receiving parsed SCTE-35 splice information.
pub trait SpliceInfoProcessor {
    /// Called with a successfully parsed splice information section.
    // TODO: take &mut self?
    fn process(
        &self,
        header: SpliceInfoHeader<'_>,
        command: SpliceCommand,
        descriptors: SpliceDescriptors<'_>,
    );

    /// Called when a SCTE-35 section could not be parsed.  The default implementation
    /// silently drops the error; override this to surface failures to your application
    /// (for example, by logging them).
    fn error(&self, _err: Scte35Error) {}
}

/// A parsed `splice_descriptor` from the descriptor loop of a _splice_info_section_.
#[derive(Debug, serde_derive::Serialize)]
pub enum SpliceDescriptor {
    /// An `avail_descriptor` (`tag 0x00`), identifying a specific avail.
    AvailDescriptor {
        /// Provider-defined avail identifier.
        provider_avail_id: u32,
    },
    /// A `DTMF_descriptor` (`tag 0x01`), carrying legacy DTMF signaling characters.
    DTMFDescriptor {
        /// Pre-roll time in tenths of a second.
        preroll: u8,
        /// DTMF characters (ASCII digit bytes).
        dtmf_chars: Vec<u8>,
    },
    /// A `segmentation_descriptor` (`tag 0x02`).
    SegmentationDescriptor {
        /// Unique identifier for this segmentation event.
        segmentation_event_id: u32,
        /// Parsed detail of the segmentation descriptor.
        descriptor_detail: SegmentationDescriptor,
    },
    /// A `time_descriptor` (`tag 0x03`), carrying a TAI time reference.
    TimeDescriptor {
        /// Seconds since 1 January 1970 00:00:00 International Atomic Time (TAI).
        tai_seconds: u64,
        /// Nanosecond component of the TAI time.
        tai_nanoseconds: u32,
        /// Offset in seconds between TAI and UTC at the time this descriptor was generated.
        utc_offset: u16,
    },
    /// An unrecognised or private descriptor.
    Reserved {
        /// The `splice_descriptor_tag` byte.
        tag: u8,
        /// The 32-bit identifier field.
        identifier: [u8; 4],
        /// The remaining private payload bytes.
        private_bytes: Vec<u8>,
    },
}
impl SpliceDescriptor {
    fn parse_segmentation_descriptor_details(
        r: &mut bitreader::BitReader<'_>,
        cancelled: bool,
    ) -> Result<SegmentationDescriptor, SpliceDescriptorErr> {
        if cancelled {
            Ok(SegmentationDescriptor::Cancel)
        } else {
            let program_segmentation_flag = r
                .read_bool()
                .named("segmentation_descriptor.program_segmentation_flag")?;
            let segmentation_duration_flag = r
                .read_bool()
                .named("segmentation_descriptor.segmentation_duration_flag")?;
            let delivery_not_restricted_flag = r
                .read_bool()
                .named("segmentation_descriptor.delivery_not_restricted_flag")?;
            let delivery_restrictions;
            if !delivery_not_restricted_flag {
                delivery_restrictions = DeliveryRestrictionFlags::DeliveryRestrictions {
                    web_delivery_allowed_flag: r
                        .read_bool()
                        .named("segmentation_descriptor.web_delivery_allowed_flag")?,
                    no_regional_blackout_flag: r
                        .read_bool()
                        .named("segmentation_descriptor.no_regional_blackout_flag")?,
                    archive_allowed_flag: r
                        .read_bool()
                        .named("segmentation_descriptor.archive_allowed_flag")?,
                    device_restrictions: DeviceRestrictions::from_bits(
                        r.read_u8(2)
                            .named("segmentation_descriptor.device_restrictions")?,
                    ),
                }
            } else {
                delivery_restrictions = DeliveryRestrictionFlags::None;
                r.skip(5).named("segmentation_descriptor.reserved")?;
            }
            let segmentation_mode = if !program_segmentation_flag {
                let component_count = r
                    .read_u8(8)
                    .named("segmentation_descriptor.component_count")?;
                let mut components = Vec::with_capacity(component_count as usize);

                for _ in 0..component_count {
                    let component_tag = r
                        .read_u8(8)
                        .named("segmentation_descriptor.component.component_tag")?;
                    r.skip(7)
                        .named("segmentation_descriptor.component.reserved")?;
                    let pts_offset = r
                        .read_u64(33)
                        .named("segmentation_descriptor.component.pts_offset")?;
                    components.push(SegmentationModeComponent {
                        component_tag,
                        pts_offset,
                    })
                }

                SegmentationMode::Component { components }
            } else {
                SegmentationMode::Program
            };

            let segmentation_duration = if segmentation_duration_flag {
                Some(
                    r.read_u64(40)
                        .named("segmentation_descriptor.segmentation_duration")?,
                )
            } else {
                None
            };

            let segmentation_upid_type = SegmentationUpidType::from_type(
                r.read_u8(8)
                    .named("segmentation_descriptor.segmentation_upid_type")?,
            );
            let segmentation_upid_length = r
                .read_u8(8)
                .named("segmentation_descriptor.segmentation_upid_length")?;
            let segmentation_upid =
                SegmentationUpid::parse(r, segmentation_upid_type, segmentation_upid_length)?;

            let segmentation_type_id =
                SegmentationTypeId::from_id(r.read_u8(8).named("segmentation_type_id")?);
            let segment_num = r.read_u8(8).named("segment_num")?;
            let segments_expected = r.read_u8(8).named("segments_expected")?;

            // The spec notes: "sub_segment_num and sub_segments_expected can form an optional
            // appendix to the segmentation descriptor. The presence or absence of this optional
            // data block is determined by the descriptor loop's descriptor_length."
            let sub_segments = if r.relative_reader().skip(1).is_ok() {
                Some(SubSegments {
                    sub_segment_num: r.read_u8(8).named("sub_segment_num")?,
                    sub_segments_expected: r.read_u8(8).named("sub_segments_expected")?,
                })
            } else {
                None
            };

            Ok(SegmentationDescriptor::Insert {
                program_segmentation_flag,
                segmentation_duration_flag,
                delivery_not_restricted_flag,
                delivery_restrictions,
                segmentation_mode,
                segmentation_duration,
                segmentation_upid,
                segmentation_type_id,
                segment_num,
                segments_expected,
                sub_segments,
            })
        }
    }

    fn parse_segmentation_descriptor(buf: &[u8]) -> Result<SpliceDescriptor, SpliceDescriptorErr> {
        let mut r = bitreader::BitReader::new(buf);
        let id = r.read_u32(32).named("segmentation_descriptor.id")?;
        let cancel = r.read_bool().named("segmentation_descriptor.cancel")?;
        r.skip(7).named("segmentation_descriptor.reserved")?;

        let result = SpliceDescriptor::SegmentationDescriptor {
            segmentation_event_id: id,
            descriptor_detail: Self::parse_segmentation_descriptor_details(&mut r, cancel)?,
        };

        // if we end up without reading to the end of a byte, this must indicate a bug in the
        // parsing routine,
        assert!(r.is_aligned(1));

        if buf.len() > (r.position() / 8) as usize {
            return Err(SpliceDescriptorErr::LeftoverData {
                field_name: "segmentation_descriptor",
                consumed: (r.position() / 8) as usize,
                total: buf.len(),
            });
        }
        Ok(result)
    }

    fn parse_dtmf_descriptor(buf: &[u8]) -> Result<SpliceDescriptor, SpliceDescriptorErr> {
        let mut r = bitreader::BitReader::new(buf);
        let preroll = r.read_u8(8).named("dtmf_descriptor.preroll")?;
        let dtmf_count = r.read_u8(3).named("dtmf_descriptor.dtmf_count")?;
        r.skip(5).named("dtmf_descriptor.reserved")?;
        let dtmf_chars_result: Result<Vec<u8>, BitReaderError> =
            (0..dtmf_count).map(|_| r.read_u8(8)).collect();
        let dtmf_chars = dtmf_chars_result.named("dtmf_descriptor")?;

        // if we end up without reading to the end of a byte, this must indicate a bug in the
        // parsing routine,
        assert!(r.is_aligned(1));

        if buf.len() > (r.position() / 8) as usize {
            return Err(SpliceDescriptorErr::LeftoverData {
                field_name: "dtmf_descriptor",
                consumed: (r.position() / 8) as usize,
                total: buf.len(),
            });
        }

        Ok(SpliceDescriptor::DTMFDescriptor {
            preroll,
            dtmf_chars,
        })
    }
    fn parse(buf: &[u8]) -> Result<SpliceDescriptor, SpliceDescriptorErr> {
        if buf.len() < 6 {
            return Err(SpliceDescriptorErr::NotEnoughData {
                field_name: "splice_descriptor",
                actual: buf.len(),
                expected: 6,
            });
        }
        let splice_descriptor_tag = buf[0];
        let splice_descriptor_len = buf[1] as usize;
        if splice_descriptor_len < 4 {
            // descriptor must at least be big enough to hold the 4-byte id value
            return Err(SpliceDescriptorErr::InvalidDescriptorLength(
                splice_descriptor_len,
            ));
        }
        let splice_descriptor_end = splice_descriptor_len + 2;
        if splice_descriptor_end > buf.len() {
            return Err(SpliceDescriptorErr::NotEnoughData {
                field_name: "splice_descriptor.private_byte",
                actual: buf.len(),
                expected: splice_descriptor_end,
            });
        }
        let id = &buf[2..6];
        let payload = &buf[6..splice_descriptor_end];
        if id == b"CUEI" {
            match splice_descriptor_tag {
                0x00 => Self::parse_avail_descriptor(payload),
                0x01 => Self::parse_dtmf_descriptor(payload),
                0x02 => Self::parse_segmentation_descriptor(payload),
                0x03 => Self::parse_time_descriptor(payload),
                _ => Self::parse_reserved(payload, splice_descriptor_tag, id),
            }
        } else {
            Self::parse_reserved(payload, splice_descriptor_tag, id)
        }
    }

    fn parse_reserved(
        buf: &[u8],
        splice_descriptor_tag: u8,
        id: &[u8],
    ) -> Result<SpliceDescriptor, SpliceDescriptorErr> {
        Ok(SpliceDescriptor::Reserved {
            tag: splice_descriptor_tag,
            identifier: [id[0], id[1], id[2], id[3]],
            private_bytes: buf.to_owned(),
        })
    }

    fn parse_avail_descriptor(buf: &[u8]) -> Result<SpliceDescriptor, SpliceDescriptorErr> {
        if buf.len() < 4 {
            return Err(SpliceDescriptorErr::NotEnoughData {
                field_name: "avail_descriptor",
                expected: 4,
                actual: buf.len(),
            });
        }
        Ok(SpliceDescriptor::AvailDescriptor {
            provider_avail_id: u32::from(buf[0]) << 24
                | u32::from(buf[1]) << 16
                | u32::from(buf[2]) << 8
                | u32::from(buf[3]),
        })
    }

    fn parse_time_descriptor(buf: &[u8]) -> Result<SpliceDescriptor, SpliceDescriptorErr> {
        if buf.len() < 12 {
            return Err(SpliceDescriptorErr::NotEnoughData {
                field_name: "time_descriptor",
                expected: 12,
                actual: buf.len(),
            });
        }
        Ok(SpliceDescriptor::TimeDescriptor {
            tai_seconds: u64::from(buf[0]) << 40
                | u64::from(buf[1]) << 32
                | u64::from(buf[2]) << 24
                | u64::from(buf[3]) << 16
                | u64::from(buf[4]) << 8
                | u64::from(buf[5]),
            tai_nanoseconds: u32::from(buf[6]) << 24
                | u32::from(buf[7]) << 16
                | u32::from(buf[8]) << 8
                | u32::from(buf[9]),
            utc_offset: u16::from(buf[10]) << 8 | u16::from(buf[11]),
        })
    }
}

/// Errors that can occur while parsing a splice descriptor or its sub-fields.
#[derive(Debug, serde_derive::Serialize)]
pub enum SpliceDescriptorErr {
    /// The `descriptor_length` value was invalid for the descriptor type.
    InvalidDescriptorLength(usize),
    /// The data was too short to read the expected field.
    NotEnoughData {
        /// Name of the field being parsed.
        field_name: &'static str,
        /// Number of bytes needed.
        expected: usize,
        /// Number of bytes actually available.
        actual: usize,
    },
    /// The segmentation_upid_length field value was `0`, but the segmentation_upid_type value was
    /// non-`0` (as indicated by the given `SegmentationUpidType` enum variant)
    SegmentationUpidLengthTypeMismatch(SegmentationUpidType),
    /// The UPID field contained byte values that are invalid for the given UPID type.
    InvalidUpidContent {
        /// The UPID type that was being parsed.
        upid_type: SegmentationUpidType,
        /// The raw bytes that could not be interpreted.
        bytes: Vec<u8>,
    },
    /// The UPID field had a length invalid for its type.
    InvalidUpidLength {
        /// The UPID type that was being parsed.
        upid_type: SegmentationUpidType,
        /// Expected length in bytes.
        expected: usize,
        /// Actual length in bytes.
        actual: usize,
    },
    /// The parser consumed fewer bytes than the enclosing field contained, which indicates
    /// either a bug in this crate or malformed input data.
    LeftoverData {
        /// Name of the field being parsed.
        field_name: &'static str,
        /// Number of bytes consumed by the parser.
        consumed: usize,
        /// Total number of bytes in the field.
        total: usize,
    },
}
/// Reports failures encountered while parsing a SCTE-35 section.  These are delivered
/// to the caller via [`SpliceInfoProcessor::error`] since the underlying `psi` trait method
/// driving section processing does not return a `Result`.
#[derive(Debug)]
pub enum Scte35Error {
    /// The section header carried an unexpected `table_id` value (expected `0xfc`).
    BadTableId(u8),
    /// The section's CRC check did not match the expected value of zero.
    CrcFailed(u32),
    /// The section was too short to contain a valid `splice_info_section`.
    SectionTooShort {
        /// Actual section length in bytes.
        actual: usize,
        /// Minimum required length in bytes.
        minimum: usize,
    },
    /// The section's `encrypted_packet` flag was set; this crate does not support
    /// decrypting SCTE-35 payloads.
    EncryptedNotSupported,
    /// The `splice_command_length` field named more bytes than remained in the section.
    SpliceCommandLengthTooLong {
        /// Declared command length in bytes.
        command_len: usize,
        /// Bytes remaining in the section.
        remaining: usize,
    },
    /// The section ended before the two-byte `descriptor_loop_length` field could be read.
    DescriptorLoopLengthShort,
    /// The `descriptor_loop_length` field named more bytes than remained in the section.
    DescriptorLoopLengthTooLong {
        /// Declared descriptor loop length in bytes.
        length: usize,
        /// Bytes remaining in the section.
        remaining: usize,
    },
    /// The section carried a `splice_command_type` value that this crate does not (yet)
    /// know how to parse.
    UnhandledCommand(SpliceCommandType),
    /// The splice command payload could not be parsed.
    ParseError(SpliceDescriptorErr),
}

impl SpliceDescriptorErr {
    fn not_enough_data(
        field_name: &'static str,
        expected: usize,
        actual: usize,
    ) -> SpliceDescriptorErr {
        SpliceDescriptorErr::NotEnoughData {
            field_name,
            expected,
            actual,
        }
    }
}

trait ErrorFieldNamed<T> {
    fn named(self, field_name: &'static str) -> Result<T, SpliceDescriptorErr>;
}
impl<T> ErrorFieldNamed<T> for Result<T, bitreader::BitReaderError> {
    fn named(self, field_name: &'static str) -> Result<T, SpliceDescriptorErr> {
        match self {
            Err(bitreader::BitReaderError::NotEnoughData {
                position,
                length,
                requested,
            }) => {
                // TODO: round numbers up to nearest byte,
                Err(SpliceDescriptorErr::NotEnoughData {
                    field_name,
                    expected: (requested / 8) as usize,
                    actual: ((length - position) / 8) as usize,
                })
            }
            Err(e) => {
                panic!("scte35-reader bug: {:?}", e)
            }
            Ok(v) => Ok(v),
        }
    }
}

/// The descriptor loop from a _splice_info_section_, iterable to yield individual
/// [`SpliceDescriptor`] values.
pub struct SpliceDescriptors<'buf> {
    buf: &'buf [u8],
}
impl<'buf> IntoIterator for &SpliceDescriptors<'buf> {
    type Item = Result<SpliceDescriptor, SpliceDescriptorErr>;
    type IntoIter = SpliceDescriptorIter<'buf>;

    fn into_iter(self) -> <Self as IntoIterator>::IntoIter {
        SpliceDescriptorIter::new(self.buf)
    }
}
impl<'a> serde::Serialize for SpliceDescriptors<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut s = serializer.serialize_seq(None)?;
        for elem in self.into_iter().flatten() {
            s.serialize_element(&elem)?;
        }
        s.end()
    }
}

/// Iterator over the [`SpliceDescriptor`] entries in a descriptor loop.
pub struct SpliceDescriptorIter<'buf> {
    buf: &'buf [u8],
}
impl<'buf> SpliceDescriptorIter<'buf> {
    fn new(buf: &'buf [u8]) -> SpliceDescriptorIter<'buf> {
        SpliceDescriptorIter { buf }
    }
}
impl<'buf> Iterator for SpliceDescriptorIter<'buf> {
    type Item = Result<SpliceDescriptor, SpliceDescriptorErr>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.buf.is_empty() {
            return None;
        }
        if self.buf.len() < 6 {
            self.buf = &self.buf[0..0];
            return Some(Err(SpliceDescriptorErr::NotEnoughData {
                field_name: "splice_descriptor",
                expected: 2,
                actual: self.buf.len(),
            }));
        }
        let descriptor_length = self.buf[1] as usize;
        if self.buf.len() < descriptor_length + 2 {
            self.buf = &self.buf[0..0];
            return Some(Err(SpliceDescriptorErr::NotEnoughData {
                field_name: "splice_descriptor",
                expected: descriptor_length + 2,
                actual: self.buf.len(),
            }));
        }
        if descriptor_length > 254 {
            self.buf = &self.buf[0..0];
            return Some(Err(SpliceDescriptorErr::InvalidDescriptorLength(
                descriptor_length,
            )));
        }
        let (desc, rest) = self.buf.split_at(2 + descriptor_length);
        let result = SpliceDescriptor::parse(desc);
        self.buf = rest;
        Some(result)
    }
}

/// A PSI section processor that parses SCTE-35 _splice_info_section_ data and delivers
/// results to a [`SpliceInfoProcessor`] implementation.
pub struct Scte35SectionProcessor<P, Ctx: demultiplex::DemuxContext>
where
    P: SpliceInfoProcessor,
{
    processor: P,
    phantom: marker::PhantomData<Ctx>,
}
impl<P, Ctx: demultiplex::DemuxContext> psi::WholeCompactSyntaxPayloadParser
    for Scte35SectionProcessor<P, Ctx>
where
    P: SpliceInfoProcessor,
{
    type Context = Ctx;

    fn section(
        &mut self,
        _ctx: &mut Self::Context,
        header: &psi::SectionCommonHeader,
        data: &[u8],
    ) {
        if header.table_id == 0xfc {
            // no CRC while fuzz-testing, to make it more likely to find parser bugs,
            if !cfg!(fuzzing) {
                let crc = mpeg2ts_reader::mpegts_crc::sum32(data);
                if crc != 0 {
                    self.processor.error(Scte35Error::CrcFailed(crc));
                    return;
                }
            }
            let section_data = &data[psi::SectionCommonHeader::SIZE..];
            if section_data.len() < SpliceInfoHeader::HEADER_LENGTH + 4 {
                self.processor.error(Scte35Error::SectionTooShort {
                    actual: section_data.len(),
                    minimum: SpliceInfoHeader::HEADER_LENGTH + 4,
                });
                return;
            }
            // trim off the 32-bit CRC
            let section_data = &section_data[..section_data.len() - 4];
            let (splice_header, rest) = SpliceInfoHeader::new(section_data);
            if splice_header.encrypted_packet() {
                self.processor.error(Scte35Error::EncryptedNotSupported);
                return;
            }
            let command_len = splice_header.splice_command_length() as usize;
            if command_len > rest.len() {
                self.processor.error(Scte35Error::SpliceCommandLengthTooLong {
                    command_len,
                    remaining: rest.len(),
                });
                return;
            }
            let (payload, rest) = rest.split_at(command_len);
            if rest.len() < 2 {
                self.processor.error(Scte35Error::DescriptorLoopLengthShort);
                return;
            }
            let descriptor_loop_length = (u16::from(rest[0]) << 8 | u16::from(rest[1])) as usize;
            if descriptor_loop_length + 2 > rest.len() {
                self.processor.error(Scte35Error::DescriptorLoopLengthTooLong {
                    length: descriptor_loop_length,
                    remaining: rest.len(),
                });
                return;
            }
            let descriptors = &rest[2..2 + descriptor_loop_length];
            let splice_command = match splice_header.splice_command_type() {
                SpliceCommandType::SpliceNull => Some(Self::splice_null(payload)),
                SpliceCommandType::SpliceInsert => Some(Self::splice_insert(payload)),
                SpliceCommandType::TimeSignal => Some(Self::time_signal(payload)),
                SpliceCommandType::BandwidthReservation => {
                    Some(Self::bandwidth_reservation(payload))
                }
                SpliceCommandType::PrivateCommand => Some(Self::private_command(payload)),
                _ => None,
            };
            match splice_command {
                Some(Ok(splice_command)) => {
                    self.processor.process(
                        splice_header,
                        splice_command,
                        SpliceDescriptors { buf: descriptors },
                    );
                }
                Some(Err(e)) => {
                    self.processor.error(Scte35Error::ParseError(e));
                }
                None => {
                    self.processor.error(Scte35Error::UnhandledCommand(
                        splice_header.splice_command_type(),
                    ));
                }
            }
        } else {
            self.processor.error(Scte35Error::BadTableId(header.table_id));
        }
    }
}
impl<P, Ctx: demultiplex::DemuxContext> Scte35SectionProcessor<P, Ctx>
where
    P: SpliceInfoProcessor,
{
    /// Creates a new `Scte35SectionProcessor` that will deliver parsed results to the given
    /// `processor`.
    pub fn new(processor: P) -> Scte35SectionProcessor<P, Ctx> {
        Scte35SectionProcessor {
            processor,
            phantom: marker::PhantomData,
        }
    }
    fn splice_null(payload: &[u8]) -> Result<SpliceCommand, SpliceDescriptorErr> {
        if payload.is_empty() {
            Ok(SpliceCommand::SpliceNull {})
        } else {
            Err(SpliceDescriptorErr::InvalidDescriptorLength(payload.len()))
        }
    }

    fn splice_insert(payload: &[u8]) -> Result<SpliceCommand, SpliceDescriptorErr> {
        let mut r = bitreader::BitReader::new(payload);

        let splice_event_id = r.read_u32(32).named("splice_insert.splice_event_id")?;
        let splice_event_cancel_indicator = r
            .read_bool()
            .named("splice_insert.splice_event_cancel_indicator")?;
        let reserved = r.read_u8(7).named("splice_insert.reserved")?;
        let result = SpliceCommand::SpliceInsert {
            splice_event_id,
            reserved,
            splice_detail: Self::read_splice_detail(&mut r, splice_event_cancel_indicator)?,
        };

        // if we end up without reading to the end of a byte, this must indicate a bug in the
        // parsing routine,
        assert!(r.is_aligned(1));

        if payload.len() > (r.position() / 8) as usize {
            return Err(SpliceDescriptorErr::LeftoverData {
                field_name: "splice_insert",
                consumed: (r.position() / 8) as usize,
                total: payload.len(),
            });
        }
        Ok(result)
    }

    fn time_signal(payload: &[u8]) -> Result<SpliceCommand, SpliceDescriptorErr> {
        let mut r = bitreader::BitReader::new(payload);

        let result = SpliceCommand::TimeSignal {
            splice_time: SpliceTime::Timed(Self::read_splice_time(&mut r)?),
        };

        // if we end up without reading to the end of a byte, this must indicate a bug in the
        // parsing routine,
        assert!(r.is_aligned(1));

        if payload.len() > (r.position() / 8) as usize {
            return Err(SpliceDescriptorErr::LeftoverData {
                field_name: "time_signal",
                consumed: (r.position() / 8) as usize,
                total: payload.len(),
            });
        }
        Ok(result)
    }

    fn bandwidth_reservation(payload: &[u8]) -> Result<SpliceCommand, SpliceDescriptorErr> {
        if payload.is_empty() {
            Ok(SpliceCommand::BandwidthReservation {})
        } else {
            Err(SpliceDescriptorErr::InvalidDescriptorLength(payload.len()))
        }
    }

    fn private_command(payload: &[u8]) -> Result<SpliceCommand, SpliceDescriptorErr> {
        let mut r = bitreader::BitReader::new(payload);
        let identifier = r.read_u32(32).named("private_command.identifier")?;
        Ok(SpliceCommand::PrivateCommand {
            identifier,
            private_bytes: payload[4..].to_vec(),
        })
    }

    fn read_splice_detail(
        r: &mut bitreader::BitReader<'_>,
        splice_event_cancel_indicator: bool,
    ) -> Result<SpliceInsert, SpliceDescriptorErr> {
        if splice_event_cancel_indicator {
            Ok(SpliceInsert::Cancel)
        } else {
            r.relative_reader().skip(1).named("splice_insert.flags")?;
            let network_indicator =
                NetworkIndicator::from_flag(r.read_u8(1).named("splice_insert.network_indicator")?);
            let program_splice_flag = r.read_bool().named("splice_insert.program_splice_flag")?;
            let duration_flag = r.read_bool().named("splice_insert.duration_flag")?;
            let splice_immediate_flag =
                r.read_bool().named("splice_insert.splice_immediate_flag")?;
            r.skip(4).named("splice_insert.reserved")?;

            Ok(SpliceInsert::Insert {
                network_indicator,
                splice_mode: Self::read_splice_mode(r, program_splice_flag, splice_immediate_flag)?,
                duration: if duration_flag {
                    Some(Self::read_duration(r)?)
                } else {
                    None
                },
                unique_program_id: r.read_u16(16).named("unique_program_id")?,
                avail_num: r.read_u8(8).named("avail_num")?,
                avails_expected: r.read_u8(8).named("avails_expected")?,
            })
        }
    }

    fn read_splice_mode(
        r: &mut bitreader::BitReader<'_>,
        program_splice_flag: bool,
        splice_immediate_flag: bool,
    ) -> Result<SpliceMode, SpliceDescriptorErr> {
        if program_splice_flag {
            let time = if splice_immediate_flag {
                SpliceTime::Immediate
            } else {
                SpliceTime::Timed(Self::read_splice_time(r)?)
            };
            Ok(SpliceMode::Program(time))
        } else {
            let component_count = r.read_u8(8).named("component_count")? as usize;
            let mut components = Vec::with_capacity(component_count);
            for _ in 0..component_count {
                let component_tag = r.read_u8(8).named("component_tag")?;
                let splice_time = if splice_immediate_flag {
                    SpliceTime::Immediate
                } else {
                    SpliceTime::Timed(Self::read_splice_time(r)?)
                };
                components.push(ComponentSplice {
                    component_tag,
                    splice_time,
                });
            }
            Ok(SpliceMode::Components(components))
        }
    }

    fn read_splice_time(
        r: &mut bitreader::BitReader<'_>,
    ) -> Result<Option<u64>, SpliceDescriptorErr> {
        Ok(if r.read_bool().named("splice_time.time_specified_flag")? {
            r.skip(6).named("splice_time.reserved")?; // reserved
            Some(r.read_u64(33).named("splice_time.pts_time")?)
        } else {
            r.skip(7).named("splice_time.reserved")?; // reserved
            None
        })
    }

    fn read_duration(
        r: &mut bitreader::BitReader<'_>,
    ) -> Result<SpliceDuration, SpliceDescriptorErr> {
        let return_mode = ReturnMode::from_flag(r.read_u8(1).named("break_duration.auto_return")?);
        r.skip(6).named("break_duration.reserved")?;
        Ok(SpliceDuration {
            return_mode,
            duration: r.read_u64(33).named("break_duration.duration")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_literal::*;
    use matches::*;
    use mpeg2ts_reader::demultiplex;
    use mpeg2ts_reader::psi;
    use mpeg2ts_reader::psi::WholeCompactSyntaxPayloadParser;

    mpeg2ts_reader::demux_context!(
        NullDemuxContext,
        demultiplex::NullPacketFilter<NullDemuxContext>
    );
    impl NullDemuxContext {
        fn do_construct(
            &mut self,
            _req: demultiplex::FilterRequest<'_, '_>,
        ) -> demultiplex::NullPacketFilter<NullDemuxContext> {
            unimplemented!();
        }
    }

    struct MockSpliceInsertProcessor;
    impl SpliceInfoProcessor for MockSpliceInsertProcessor {
        fn process(
            &self,
            header: SpliceInfoHeader<'_>,
            command: SpliceCommand,
            descriptors: SpliceDescriptors<'_>,
        ) {
            assert_eq!(header.encryption_algorithm(), EncryptionAlgorithm::None);
            assert_matches!(command, SpliceCommand::SpliceInsert { .. });
            for d in &descriptors {
                d.unwrap();
            }
        }
    }

    #[test]
    fn it_works() {
        let data = hex!(
            "fc302500000000000000fff01405000000017feffe2d142b00fe0123d3080001010100007f157a49"
        );
        let mut parser = Scte35SectionProcessor::new(MockSpliceInsertProcessor);
        let header = psi::SectionCommonHeader::new(&data[..psi::SectionCommonHeader::SIZE]);
        let mut ctx = NullDemuxContext::new();
        parser.section(&mut ctx, &header, &data[..]);
    }

    struct MockTimeSignalProcessor;
    impl SpliceInfoProcessor for MockTimeSignalProcessor {
        fn process(
            &self,
            header: SpliceInfoHeader<'_>,
            command: SpliceCommand,
            descriptors: SpliceDescriptors<'_>,
        ) {
            assert_eq!(header.encryption_algorithm(), EncryptionAlgorithm::None);
            assert_matches!(command, SpliceCommand::TimeSignal { .. });
            for d in &descriptors {
                d.unwrap();
            }
        }
    }

    #[test]
    fn it_understands_time_signal() {
        let data = hex!(
            "fc302700000000000000fff00506ff592d03c00011020f43554549000000017fbf000010010112ce0e6b"
        );
        let mut parser = Scte35SectionProcessor::new(MockTimeSignalProcessor);
        let header = psi::SectionCommonHeader::new(&data[..psi::SectionCommonHeader::SIZE]);
        let mut ctx = NullDemuxContext::new();
        parser.section(&mut ctx, &header, &data[..]);
    }

    #[test]
    fn splice_descriptor() {
        let data = [];
        assert_matches!(
            SpliceDescriptor::parse(&data[..]),
            Err(SpliceDescriptorErr::NotEnoughData { .. })
        );
        let data = hex!("01084D5949440000"); // descriptor payload too short
        assert_matches!(
            SpliceDescriptor::parse(&data[..]),
            Err(SpliceDescriptorErr::NotEnoughData { .. })
        );
        let data = hex!("01034D59494400000003");
        assert_matches!(
            SpliceDescriptor::parse(&data[..]),
            Err(SpliceDescriptorErr::InvalidDescriptorLength { .. })
        );
        let data = hex!("01084D59494400000003");
        assert_matches!(
            SpliceDescriptor::parse(&data[..]),
            Ok(SpliceDescriptor::Reserved {
                tag: 01,
                identifier: [0x4D, 0x59, 0x49, 0x44],
                private_bytes: _,
            })
        );

        let data = hex!("020f43554549000000017fbf0000100101");
        assert_matches!(
            SpliceDescriptor::parse(&data[..]),
            Ok(SpliceDescriptor::SegmentationDescriptor {
                segmentation_event_id: 1,
                descriptor_detail: SegmentationDescriptor::Insert {
                    program_segmentation_flag: true,
                    segmentation_duration_flag: false,
                    delivery_not_restricted_flag: true,
                    delivery_restrictions: DeliveryRestrictionFlags::None,
                    segmentation_mode: SegmentationMode::Program,
                    segmentation_duration: None,
                    segmentation_upid: SegmentationUpid::None,
                    segmentation_type_id: SegmentationTypeId::PROGRAM_START,
                    segment_num: 1,
                    segments_expected: 1,
                    sub_segments: None,
                }
            })
        );
    }

    #[test]
    fn segmentation_descriptor() {
        let data = hex!("480000ad7f9f0808000000002cb2d79d350200");
        let desc = SpliceDescriptor::parse_segmentation_descriptor(&data[..]).unwrap();
        match desc {
            SpliceDescriptor::SegmentationDescriptor {
                descriptor_detail:
                    SegmentationDescriptor::Insert {
                        segmentation_upid: SegmentationUpid::TI(ti),
                        ..
                    },
                ..
            } => {
                // TODO: assert_eq!(upid.len(), 8);
                assert_eq!(ti, upid::TI(hex!("000000002cb2d79d").to_vec()));
            }
            _ => panic!("unexpected {:?}", desc),
        };
    }

    #[test]
    fn no_sub_segment_num() {
        // This segmentation_descriptor() does not include sub_segment_num or
        // sub_segments_expected fields.  Their absence should not cause parsing problems.
        let data = hex!("480000bf7fcf0000f8fa630d110e054c413330390808000000002e538481340000");
        SpliceDescriptor::parse_segmentation_descriptor(&data[..]).unwrap();
    }

    #[test]
    fn too_large_segment_descriptor() {
        // there are more bytes than expected; this should not panic, and the parser
        // should now report LeftoverData rather than silently accepting the extra bytes.
        let data = hex!("480000ad7f9f0808000000002cb2d79d350200000000");
        assert_matches!(
            SpliceDescriptor::parse_segmentation_descriptor(&data[..]),
            Err(SpliceDescriptorErr::LeftoverData {
                field_name: "segmentation_descriptor",
                ..
            })
        );
    }

    #[derive(Default)]
    struct RecordingProcessor {
        errors: std::cell::RefCell<Vec<String>>,
    }
    impl SpliceInfoProcessor for RecordingProcessor {
        fn process(
            &self,
            _header: SpliceInfoHeader<'_>,
            _command: SpliceCommand,
            _descriptors: SpliceDescriptors<'_>,
        ) {
            panic!("process() should not be called for these error-path tests");
        }
        fn error(&self, err: Scte35Error) {
            self.errors.borrow_mut().push(format!("{:?}", err));
        }
    }

    fn run_section(data: &[u8]) -> Vec<String> {
        let processor = RecordingProcessor::default();
        let mut parser = Scte35SectionProcessor::new(processor);
        let header = psi::SectionCommonHeader::new(&data[..psi::SectionCommonHeader::SIZE]);
        let mut ctx = NullDemuxContext::new();
        parser.section(&mut ctx, &header, data);
        parser.processor.errors.into_inner()
    }

    #[test]
    fn error_callback_bad_table_id() {
        // same payload as it_works but with table_id changed from 0xfc to 0xfd
        let data = hex!(
            "fd302500000000000000fff01405000000017feffe2d142b00fe0123d3080001010100007f157a49"
        );
        let errors = run_section(&data[..]);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("BadTableId"), "got: {}", errors[0]);
    }

    #[test]
    fn error_callback_crc_failed() {
        // same payload as it_works but with the final CRC byte corrupted
        let data = hex!(
            "fc302500000000000000fff01405000000017feffe2d142b00fe0123d3080001010100007f157a4a"
        );
        let errors = run_section(&data[..]);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("CrcFailed"), "got: {}", errors[0]);
    }
}

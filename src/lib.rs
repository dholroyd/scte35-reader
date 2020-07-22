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

use mpeg2ts_reader::demultiplex;
use mpeg2ts_reader::psi;
use serde::ser::{SerializeSeq, SerializeStruct};
use serdebug::*;
use std::marker;
use bitreader::BitReaderError;

/// Utility function to search the PTM section for a `CUEI` registration descriptor per
/// _SCTE-35, section 8.1_, which indicates that streams with `stream_type` equal to the private
/// value `0x86` within this PMT section are formatted according to SCTE-35.
///
/// Returns `true` if the descriptor is attached to the given PMT section and `false` otherwise.
pub fn is_scte35(pmt: &mpeg2ts_reader::psi::pmt::PmtSection<'_>) -> bool {
    for d in pmt.descriptors() {
        if let Ok(mpeg2ts_reader::descriptor::CoreDescriptors::Registration(
            mpeg2ts_reader::descriptor::registration::RegistrationDescriptor { buf: b"CUEI" },
        )) = d
        {
            return true;
        }
    }
    false
}
#[derive(Debug, PartialEq, serde_derive::Serialize)]
pub enum EncryptionAlgorithm {
    None,
    DesEcb,
    DesCbc,
    TripleDesEde3Ecb,
    Reserved(u8),
    Private(u8),
}
impl EncryptionAlgorithm {
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

#[derive(Debug, PartialEq, serde_derive::Serialize)]
pub enum SpliceCommandType {
    SpliceNull,
    Reserved(u8),
    SpliceSchedule,
    SpliceInsert,
    TimeSignal,
    BandwidthReservation,
    PrivateCommand,
}
impl SpliceCommandType {
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
#[derive(SerDebug)]
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

#[derive(Debug, serde_derive::Serialize)]
pub enum SpliceCommand {
    SpliceNull {},
    SpliceInsert {
        splice_event_id: u32,
        reserved: u8,
        splice_detail: SpliceInsert,
    },
    TimeSignal {
        splice_time: SpliceTime,
    },
    BandwidthReservation {},
}

#[derive(Debug, serde_derive::Serialize)]
pub enum NetworkIndicator {
    Out,
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

#[derive(Debug, serde_derive::Serialize)]
pub enum SpliceInsert {
    Cancel,
    Insert {
        network_indicator: NetworkIndicator,
        splice_mode: SpliceMode,
        duration: Option<SpliceDuration>,
        unique_program_id: u16,
        avail_num: u8,
        avails_expected: u8,
    },
}

#[derive(Debug, serde_derive::Serialize)]
pub enum SpliceTime {
    Immediate,
    Timed(Option<u64>),
}

#[derive(Debug, serde_derive::Serialize)]
pub struct ComponentSplice {
    component_tag: u8,
    splice_time: SpliceTime,
}

#[derive(Debug, serde_derive::Serialize)]
pub enum SpliceMode {
    Program(SpliceTime),
    Components(Vec<ComponentSplice>),
}

#[derive(Debug, serde_derive::Serialize)]
pub enum ReturnMode {
    Automatic,
    Manual,
}
impl ReturnMode {
    pub fn from_flag(flag: u8) -> ReturnMode {
        match flag {
            0 => ReturnMode::Manual,
            1 => ReturnMode::Automatic,
            _ => panic!("Invalid auto_return value: {} (expected 0 or 1)", flag),
        }
    }
}

#[derive(Debug, PartialEq, serde_derive::Serialize)]
pub enum SegmentationUpidType {
    NotUsed,
    UserDefinedDeprecated,
    ISCIDeprecated,
    AdID,
    UMID,
    ISANDeprecated,
    ISAN,
    TID,
    TI,
    ADI,
    EIDR,
    ATSC,
    MPU,
    MID,
    ADS,
    URI,
    Reserved(u8),
}
impl SegmentationUpidType {
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

#[derive(Debug, PartialEq, serde_derive::Serialize)]
pub enum SegmentationTypeId {
    NotIndicated,
    ContentIdentification,
    ProgramStart,
    ProgramEnd,
    ProgramEarlyTermination,
    ProgramBreakaway,
    ProgramResumption,
    ProgramRunoverPlanned,
    ProgramRunoverUnplanned,
    ProgramOverlapStart,
    ProgramBlackoutOverride,
    ProgramStartInProgress,
    ChapterStart,
    ChapterEnd,
    BreakStart,
    BreakEnd,
    ProviderAdvertisementStart,
    ProviderAdvertisementEnd,
    DistributorAdvertisementStart,
    DistributorAdvertisementEnd,
    ProviderPlacementOpportunityStart,
    ProviderPlacementOpportunityEnd,
    DistributorPlacementOpportunityStart,
    DistributorPlacementOpportunityEnd,
    UnscheduledEventStart,
    UnscheduledEventEnd,
    NetworkStart,
    NetworkEnd,
    Reserved(u8),
}
impl SegmentationTypeId {
    pub fn from_id(id: u8) -> SegmentationTypeId {
        match id {
            0 => SegmentationTypeId::NotIndicated,
            1 => SegmentationTypeId::ContentIdentification,
            16 => SegmentationTypeId::ProgramStart,
            17 => SegmentationTypeId::ProgramEnd,
            18 => SegmentationTypeId::ProgramEarlyTermination,
            19 => SegmentationTypeId::ProgramBreakaway,
            20 => SegmentationTypeId::ProgramResumption,
            21 => SegmentationTypeId::ProgramRunoverPlanned,
            22 => SegmentationTypeId::ProgramRunoverUnplanned,
            23 => SegmentationTypeId::ProgramOverlapStart,
            24 => SegmentationTypeId::ProgramBlackoutOverride,
            25 => SegmentationTypeId::ProgramStartInProgress,
            32 => SegmentationTypeId::ChapterStart,
            33 => SegmentationTypeId::ChapterEnd,
            34 => SegmentationTypeId::BreakStart,
            35 => SegmentationTypeId::BreakEnd,
            48 => SegmentationTypeId::ProviderAdvertisementStart,
            49 => SegmentationTypeId::ProviderAdvertisementEnd,
            50 => SegmentationTypeId::DistributorAdvertisementStart,
            51 => SegmentationTypeId::DistributorAdvertisementEnd,
            52 => SegmentationTypeId::ProviderPlacementOpportunityStart,
            53 => SegmentationTypeId::ProviderPlacementOpportunityEnd,
            54 => SegmentationTypeId::DistributorPlacementOpportunityStart,
            55 => SegmentationTypeId::DistributorPlacementOpportunityEnd,
            64 => SegmentationTypeId::UnscheduledEventStart,
            65 => SegmentationTypeId::UnscheduledEventEnd,
            80 => SegmentationTypeId::NetworkStart,
            81 => SegmentationTypeId::NetworkEnd,
            _ => SegmentationTypeId::Reserved(id),
        }
    }
}

#[derive(Debug, serde_derive::Serialize)]
pub enum SegmentationUpid {
    None,
    SegmentationUpid { upid: Vec<u8> },
}

#[derive(Debug, serde_derive::Serialize)]
pub enum DeviceRestrictions {
    RestrictGroup0,
    RestrictGroup1,
    RestrictGroup2,
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

#[derive(Debug, serde_derive::Serialize)]
pub enum DeliveryRestrictionFlags {
    None,
    DeliveryRestrictions {
        web_delivery_allowed_flag: bool,
        no_regional_blackout_flag: bool,
        archive_allowed_flag: bool,
        device_restrictions: DeviceRestrictions,
    },
}

#[derive(Debug, serde_derive::Serialize)]
pub enum SegmentationMode {
    Program,
    Component {
        components: Vec<SegmentationModeComponent>,
    },
}

#[derive(Debug, serde_derive::Serialize)]
pub struct SegmentationModeComponent {
    component_tag: u8,
    pts_offset: u64,
}

#[derive(Debug, serde_derive::Serialize)]
pub enum SegmentationDescriptor {
    Cancel,
    Insert {
        program_segmentation_flag: bool,
        segmentation_duration_flag: bool,
        delivery_not_restricted_flag: bool,
        delivery_restrictions: DeliveryRestrictionFlags,
        segmentation_mode: SegmentationMode,
        segmentation_duration: Option<u64>,
        segmentation_upid_type: SegmentationUpidType,
        segmentation_upid_length: u8,
        segmentation_upid: SegmentationUpid,
        segmentation_type_id: SegmentationTypeId,
        segment_num: u8,
        segments_expected: u8,
        sub_segments: Option<SubSegments>
    },
}

#[derive(Debug, serde_derive::Serialize)]
pub struct SubSegments {
    sub_segment_num: u8,
    sub_segments_expected: u8,
}

#[derive(Debug, serde_derive::Serialize)]
pub struct SpliceDuration {
    return_mode: ReturnMode,
    duration: u64,
}

pub trait SpliceInfoProcessor {
    fn process(
        &self,
        header: SpliceInfoHeader<'_>,
        command: SpliceCommand,
        descriptors: SpliceDescriptors<'_>,
    );
}

#[derive(Debug, serde_derive::Serialize)]
pub enum SpliceDescriptor {
    AvailDescriptor {
        provider_avail_id: u32,
    },
    DTMFDescriptor {
        preroll: u8,
        dtmf_chars: Vec<u8>,
    },
    SegmentationDescriptor {
        segmentation_event_id: u32,
        descriptor_detail: SegmentationDescriptor,
    },
    TimeDescriptor {
        tai_seconds: u64,
        tai_nanoseconds: u32,
        utc_offset: u16,
    },
    Reserved {
        tag: u8,
        identifier: [u8; 4],
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
            let program_segmentation_flag = r.read_bool().named("segmentation_descriptor.program_segmentation_flag")?;
            let segmentation_duration_flag = r.read_bool().named("segmentation_descriptor.segmentation_duration_flag")?;
            let delivery_not_restricted_flag = r.read_bool().named("segmentation_descriptor.delivery_not_restricted_flag")?;
            let delivery_restrictions;
            if !delivery_not_restricted_flag {
                delivery_restrictions = DeliveryRestrictionFlags::DeliveryRestrictions {
                    web_delivery_allowed_flag: r.read_bool().named("segmentation_descriptor.web_delivery_allowed_flag")?,
                    no_regional_blackout_flag: r.read_bool().named("segmentation_descriptor.no_regional_blackout_flag")?,
                    archive_allowed_flag: r.read_bool().named("segmentation_descriptor.archive_allowed_flag")?,
                    device_restrictions: DeviceRestrictions::from_bits(r.read_u8(2).named("segmentation_descriptor.device_restrictions")?),
                }
            } else {
                delivery_restrictions = DeliveryRestrictionFlags::None;
                r.skip(5).named("segmentation_descriptor.reserved")?;
            }
            let segmentation_mode = if !program_segmentation_flag {
                let component_count = r.read_u8(8).named("segmentation_descriptor.component_count")?;
                let mut components = Vec::with_capacity(component_count as usize);

                for _ in 0..component_count {
                    let component_tag = r.read_u8(8).named("segmentation_descriptor.component.component_tag")?;
                    r.skip(7).named("segmentation_descriptor.component.reserved")?;
                    let pts_offset = r.read_u64(33).named("segmentation_descriptor.component.pts_offset")?;
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
                Some(r.read_u64(40).named("segmentation_descriptor.segmentation_duration")?)
            } else {
                None
            };

            let segmentation_upid_type = SegmentationUpidType::from_type(r.read_u8(8).named("segmentation_descriptor.segmentation_upid_type")?);
            let segmentation_upid_length = r.read_u8(8).named("segmentation_descriptor.segmentation_upid_length")?;
            let segmentation_upid = if segmentation_upid_length > 0 {
                let upid_result: Result<Vec<u8>, bitreader::BitReaderError> = (0..segmentation_upid_length)
                    .map(|_| r.read_u8(8))
                    .collect();
                let upid = upid_result.named("segmentation_descriptor.segmentation_upid")?;
                SegmentationUpid::SegmentationUpid { upid }
            } else {
                SegmentationUpid::None
            };

            let segmentation_type_id = SegmentationTypeId::from_id(r.read_u8(8).named("segmentation_type_id")?);
            let segment_num = r.read_u8(8).named("segment_num")?;
            let segments_expected = r.read_u8(8).named("segments_expected")?;

            // The spec notes: "sub_segment_num and sub_segments_expected can form an optional
            // appendix to the segmentation descriptor. The presence or absence of this optional
            // data block is determined by the descriptor loop's descriptor_length."
            let sub_segments = if r.relative_reader().skip(1).is_ok()
            {
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
                segmentation_upid_type,
                segmentation_upid_length,
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

        if buf.len() > (r.position()/8) as usize {
            println!(
                "SCTE35: only {} bytes consumed data in segmentation_descriptor of {} bytes",
                r.position() / 8,
                buf.len()
            );
        }
        Ok(result)
    }

    fn parse_dtmf_descriptor(buf: &[u8]) -> Result<SpliceDescriptor, SpliceDescriptorErr> {
        let mut r = bitreader::BitReader::new(buf);
        let preroll = r.read_u8(8).named("dtmf_descriptor.preroll")?;
        let dtmf_count = r.read_u8(3).named("dtmf_descriptor.dtmf_count")?;
        r.skip(5).named("dtmf_descriptor.reserved")?;
        let dtmf_chars_result: Result<Vec<u8>, BitReaderError> = (0..dtmf_count)
            .map(|_| r.read_u8(8) )
            .collect();
        let dtmf_chars = dtmf_chars_result.named("dtmf_descriptor")?;

        // if we end up without reading to the end of a byte, this must indicate a bug in the
        // parsing routine,
        assert!(r.is_aligned(1));

        if buf.len() > (r.position()/8) as usize {
            println!(
                "SCTE35: only {} bytes consumed data in segmentation_descriptor of {} bytes",
                r.position() / 8,
                buf.len()
            );
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
            return Err(SpliceDescriptorErr::InvalidDescriptorLength(splice_descriptor_len));
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
        Ok(if id != b"CUEI" {
            SpliceDescriptor::Reserved {
                tag: splice_descriptor_tag,
                identifier: [id[0], id[1], id[2], id[3]],
                private_bytes: buf[6..splice_descriptor_end].to_owned(),
            }
        } else {
            match splice_descriptor_tag {
                0x00 => SpliceDescriptor::AvailDescriptor {
                    provider_avail_id: u32::from(buf[6]) << 24
                        | u32::from(buf[7]) << 16
                        | u32::from(buf[8]) << 8
                        | u32::from(buf[9]),
                },
                0x01 => Self::parse_dtmf_descriptor(&buf[6..splice_descriptor_end])?,
                0x02 => Self::parse_segmentation_descriptor(&buf[6..splice_descriptor_end])?,
                0x03 => Self::parse_time_descriptor(&buf[6..splice_descriptor_end])?,
                _ => SpliceDescriptor::Reserved {
                    tag: splice_descriptor_tag,
                    identifier: [id[0], id[1], id[2], id[3]],
                    private_bytes: buf[6..splice_descriptor_end].to_owned(),
                },
            }
        })
    }

    fn parse_time_descriptor(buf: &[u8]) -> Result<SpliceDescriptor, SpliceDescriptorErr> {
        if buf.len() < 12 {
            return Err(SpliceDescriptorErr::NotEnoughData {
                field_name: "time_descriptor",
                expected: 12,
                actual: buf.len(),
            })
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

#[derive(Debug, serde_derive::Serialize)]
pub enum SpliceDescriptorErr {
    InvalidDescriptorLength(usize),
    NotEnoughData { field_name: &'static str, expected: usize, actual: usize },
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
            },
            Err(e) => {
                panic!("scte35-reader bug: {:?}", e)
            },
            Ok(v) => Ok(v),
        }
    }
}

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
        for e in self {
            if let Ok(elem) = e {
                s.serialize_element(&elem)?;
            }
        }
        s.end()
    }
}

pub struct SpliceDescriptorIter<'buf> {
    buf: &'buf [u8],
}
impl<'buf> SpliceDescriptorIter<'buf> {
    fn new(buf: &'buf [u8]) -> SpliceDescriptorIter<'_> {
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

pub struct Scte35SectionProcessor<P, Ctx: demultiplex::DemuxContext>
where
    P: SpliceInfoProcessor,
{
    processor: P,
    phantom: marker::PhantomData<Ctx>,
}
impl<P, Ctx: demultiplex::DemuxContext> psi::WholeCompactSyntaxPayloadParser for Scte35SectionProcessor<P, Ctx>
where
    P: SpliceInfoProcessor,
{
    type Context = Ctx;

    fn section<'a>(
        &mut self,
        _ctx: &mut Self::Context,
        header: &psi::SectionCommonHeader,
        data: &'a [u8],
    ) {
        if header.table_id == 0xfc {
            // no CRC while fuzz-testing, to make it more likely to find parser bugs,
            if !cfg!(fuzzing) {
                let crc = mpeg2ts_reader::mpegts_crc::sum32(data);
                if crc != 0 {
                    println!("SCTE35: section CRC check failed {:#08x}", crc);
                    return;
                }
            }
            let section_data = &data[psi::SectionCommonHeader::SIZE..];
            if section_data.len() < SpliceInfoHeader::HEADER_LENGTH + 4 {
                println!(
                    "SCTE35: section data too short: {} (must be at least {})",
                    section_data.len(),
                    SpliceInfoHeader::HEADER_LENGTH + 4
                );
                return;
            }
            // trim off the 32-bit CRC
            let section_data = &section_data[..section_data.len() - 4];
            let (splice_header, rest) = SpliceInfoHeader::new(section_data);
            //println!("splice header len={}, type={:?}", splice_header.splice_command_length(), splice_header.splice_command_type());
            let command_len = splice_header.splice_command_length() as usize;
            if command_len > rest.len() {
                println!("SCTE35: splice_command_length of {} bytes is too long to fit in remaining {} bytes of section data", command_len, rest.len());
                return;
            }
            let (payload, rest) = rest.split_at(command_len);
            if rest.len() < 2 {
                println!("SCTE35: end of section data while trying to read descriptor_loop_length");
                return;
            }
            let descriptor_loop_length = (u16::from(rest[0]) << 8 | u16::from(rest[1])) as usize;
            if descriptor_loop_length + 2 > rest.len() {
                println!("SCTE35: descriptor_loop_length of {} bytes is too long to fit in remaining {} bytes of section data", descriptor_loop_length, rest.len());
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
                    println!("SCTE35: parse error: {:?}", e);
                }
                None => {
                    println!(
                        "SCTE35: unhandled command {:?}",
                        splice_header.splice_command_type()
                    );
                }
            }
        } else {
            println!(
                "SCTE35: bad table_id for scte35: {:#x} (expected 0xfc)",
                header.table_id
            );
        }
    }
}
impl<P, Ctx: demultiplex::DemuxContext> Scte35SectionProcessor<P, Ctx>
where
    P: SpliceInfoProcessor,
{
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
        let splice_event_cancel_indicator = r.read_bool().named("splice_insert.splice_event_cancel_indicator")?;
        let reserved = r.read_u8(7).named("splice_insert.reserved")?;
        let result = SpliceCommand::SpliceInsert {
            splice_event_id,
            reserved,
            splice_detail: Self::read_splice_detail(&mut r, splice_event_cancel_indicator)?,
        };

        // if we end up without reading to the end of a byte, this must indicate a bug in the
        // parsing routine,
        assert!(r.is_aligned(1));

        if payload.len() > (r.position()/8) as usize {
            println!(
                "SCTE35: only {} bytes consumed data in splice_insert of {} bytes",
                r.position() / 8,
                payload.len()
            );
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

        if payload.len() > (r.position()/8) as usize {
            println!(
                "SCTE35: only {} bytes consumed data in time_signal of {} bytes",
                r.position() / 8,
                payload.len()
            );
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

    fn read_splice_detail(
        r: &mut bitreader::BitReader<'_>,
        splice_event_cancel_indicator: bool,
    ) -> Result<SpliceInsert, SpliceDescriptorErr> {
        if splice_event_cancel_indicator {
            Ok(SpliceInsert::Cancel)
        } else {
            r.relative_reader().skip(1).named("splice_insert.flags")?;
            let network_indicator = NetworkIndicator::from_flag(r.read_u8(1).named("splice_insert.network_indicator")?);
            let program_splice_flag = r.read_bool().named("splice_insert.program_splice_flag")?;
            let duration_flag = r.read_bool().named("splice_insert.duration_flag")?;
            let splice_immediate_flag = r.read_bool().named("splice_insert.splice_immediate_flag")?;
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
            assert_matches!(command, SpliceCommand::SpliceInsert{..});
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
            assert_matches!(command, SpliceCommand::TimeSignal{..});
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
                    segmentation_upid_type: SegmentationUpidType::NotUsed,
                    segmentation_upid_length: 0,
                    segmentation_upid: SegmentationUpid::None,
                    segmentation_type_id: SegmentationTypeId::ProgramStart,
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
            SpliceDescriptor::SegmentationDescriptor { descriptor_detail: SegmentationDescriptor::Insert { segmentation_upid: SegmentationUpid::SegmentationUpid { upid }, .. }, .. } => {
                assert_eq!(upid.len(), 8);
            },
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
        // there are more bytes than expected; this should not panic
        let data = hex!("480000ad7f9f0808000000002cb2d79d350200000000");
        SpliceDescriptor::parse_segmentation_descriptor(&data[..]).unwrap();
    }
}

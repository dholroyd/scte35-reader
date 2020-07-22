# Changelog

## Unreleased
### Fixed
 - Fixed failure to parse marker with `segmentation_descriptor()` that omits the optional `sub_segment_num` and
   `sub_segments_expected` fields.

### Changed
 - Extended error type's `NotEnoughData` variant with a `field_name` describing what data we were trying to parse when
   we ran out of bytes.
 - As a result of fixing sub_segments field handling, the `sub_segment_num` and `sub_segments_expected` fields are
   no longer directly present in `SegmentationDescriptor::Insert`, and have moved to a new, optional `SebSegments`
   struct.

## 0.11.0
### Fixed
 - Avoid panic on unexpectedly small values of `splice_descriptor_len`
 - Fix off-by-one bug in parsing some descriptor data causing an assertion to trigger, per
   [#3](https://github.com/dholroyd/scte35-reader/issues/3).

## 0.10.0
### Changed
 - `Scte35SectionProcessor` implements `WholeCompactSyntaxPayloadParser` rather than `SectionProcessor` so that it can
   now handle SCTE 35 sections that span more than one TS packet
 - Now checks that the CRC in the section data is correct, and will not parse if incorrect.

## 0.9.0
### Changed
 - Bumped `mpeg2ts-reader` to latest 0.10.0 release

### Added
 - More documentation

## 0.7.0
### Changed
 - `SpliceInfoProcessor::process()` signature altered to take new `SpliceDescriptors` type, rather than
   `SpliceDescriptorIterator` directly.  This makes it possible for implementations of `process()` to iterate through
   the descriptors more than once.
 - All interesting types now implement `serde::Serialize` (so `serde` is now a dependency).
 - Now depends on `mpeg2ts-reader` 0.8.

### Added
 - An `is_scte35()` utility function to test if SCTE-35 should be expected.

## 0.6.0
### Changed
 - Made most methods return `Result`, and remove all explicit `unwrap()` calls from within
 - Bumped `mpeg2ts-reader` to latest 0.7.0 release

### Added
 - Support for `time_signal()` and `bandwidth_reservation()` messages, plus `DTMF_descriptor`,
   `segmentation_descriptor` and `time_descriptor` - huge thanks to [@davemevans](https://github.com/davemevans)!

## 0.5.0
### Changed
 - Bumped `mpeg2ts-reader` to latest 0.6.0 release

## 0.4.0
### Fixed
 - Presence of a descriptor in the SCTE data will no longer result in a panic
   due to out of bounds access

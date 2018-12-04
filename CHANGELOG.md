# Changelog

## Unreleased
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

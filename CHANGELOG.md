# Changelog

## 0.7.0
### Changed
 - All interesting types now implement `serde::Serialize` (so `serde` is now a dependency).

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

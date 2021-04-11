# scte35-reader

[![crates.io version](https://img.shields.io/crates/v/scte35-reader.svg)](https://crates.io/crates/scte35-reader)

Parser data formatted according to [SCTE-35](https://scte-cms-resource-storage.s3.amazonaws.com/ANSI_SCTE-35-2019a-1582645390859.pdf).

For an example of usage, see the [scte35dump](https://github.com/dholroyd/scte35dump) tool.

## Supported syntax

A subset of possible SCTE-35 syntax is currently handled:

 - [x] `splice_info_section()`
   - [ ] `encrypted_packet` - ❌ decryption of encrypted SCTE-35 data is not supported

### Commands

 - [x] `splice_null()`
 - [ ] `splice_schedule()`
 - [x] `splice_insert()`
 - [x] `time_signal()`
 - [x] `bandwidth_reservation()`
 - [ ] `private_command()`

### Descriptors

 - [x] `avail_descriptor`
 - [x] `DTMF_descriptor`
 - [x] `segmentation_descriptor`
 - [x] `time_descriptor`
 - [x] _Reserved_ - Descriptors with tags values that are 'reserved' in SCTE-35 are supported in the sense that the application
       gets access to the descriptor byte values, and can parse them with application-specific logic.

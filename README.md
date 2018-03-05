# scte35-reader

[![crates.io version](https://img.shields.io/crates/v/scte35-reader.svg)](https://crates.io/crates/scte35-reader)

Parser data formatted according to [SCTE-35](http://www.scte.org/SCTEDocs/Standards/SCTE%2035%202016.pdf).

For an example of usage, see the [scte35dump](https://github.com/dholroyd/scte35dump) tool.

## Supported syntax

A subset of possible SCTE-35 syntax is currently handled:

### Commands

 - [x] `splice_null()`
 - [ ] `splice_schedule()`
 - [x] `splice_insert()`
 - [ ] `time_signal()`
 - [ ] `bandwidth_reservation()`
 - [ ] `private_command()`

### Descriptors

 - [x] `avail_descriptor`
 - [ ] `DTMF_descriptor`
 - [ ] `segmentation_descriptor`
 - [ ] `time_descriptor`
 - [x] _Reserved_ - Descriptors with tags values that are 'reserved' in SCTE-35 are supported in the sense that the application
       gets access to the descriptor byte values, and can parse them with application-specific logic.

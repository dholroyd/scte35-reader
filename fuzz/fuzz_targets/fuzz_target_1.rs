#![no_main]
use libfuzzer_sys::fuzz_target;
use scte35_reader::*;
use mpeg2ts_reader::demultiplex;
use mpeg2ts_reader::psi;
use mpeg2ts_reader::psi::WholeCompactSyntaxPayloadParser;

mpeg2ts_reader::demux_context!(
        FuzzDemuxContext,
        demultiplex::NullPacketFilter<FuzzDemuxContext>
    );
impl FuzzDemuxContext {
    fn do_construct(
        &mut self,
        _req: demultiplex::FilterRequest<'_, '_>,
    ) -> demultiplex::NullPacketFilter<FuzzDemuxContext> {
        unimplemented!();
    }
}

struct FuzzSpliceInfoProcessor;
impl SpliceInfoProcessor for FuzzSpliceInfoProcessor {
    fn process(
        &self,
        header: SpliceInfoHeader<'_>,
        command: SpliceCommand,
        descriptors: SpliceDescriptors<'_>,
    ) {
        // The debug implementations should call every accessor method under the hood,
        format!("{:?}", header);
        format!("{:?}", command);

        for d in &descriptors {
            format!("{:?}", d);
        }
    }
}
fuzz_target!(|data: &[u8]| {
    if data.len() < psi::SectionCommonHeader::SIZE {
        return;
    }
    let mut parser = Scte35SectionProcessor::new(FuzzSpliceInfoProcessor);
    let header = psi::SectionCommonHeader::new(&data[..psi::SectionCommonHeader::SIZE]);
    let mut ctx = FuzzDemuxContext::new();
    parser.section(&mut ctx, &header, &data[..]);
});

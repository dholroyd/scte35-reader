#![no_main]
use libfuzzer_sys::fuzz_target;
use mpeg2ts_reader::demultiplex;
use mpeg2ts_reader::psi;
use mpeg2ts_reader::psi::WholeCompactSyntaxPayloadParser;
use scte35_reader::*;

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
        std::hint::black_box(format!("{:?}", header));
        std::hint::black_box(format!("{:?}", command));

        for d in &descriptors {
            std::hint::black_box(format!("{:?}", d));
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
    parser.section(&mut ctx, &header, data);
});

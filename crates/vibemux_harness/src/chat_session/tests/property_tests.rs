use super::*;
use proptest::prelude::*;

proptest! {
    #[test]
    fn arbitrary_bounded_vendor_bytes_never_panic(frame in prop::collection::vec(any::<u8>(), 0..2048)) {
        let mut machine = codex_ready();
        codex_turn(&mut machine, 1, "vendor_turn");
        let _ = machine.receive(machine.snapshot().records_received + 1, &frame);
        prop_assert!(machine.snapshot().turns_started == 1);
    }

    #[test]
    fn arbitrary_json_text_survives_small_utf8_chunk_boundaries(text in ".{0,128}", chunk_size in 1usize..32) {
        let json = json!({"text":text}).to_string();
        let wire = format!("{json}\r\n");
        let mut decoder = SessionDecoder::new(&SessionLimits::default()).expect("decoder");
        let mut frames = Vec::new();
        for chunk in wire.as_bytes().chunks(chunk_size) { frames.extend(decoder.push(chunk).expect("valid chunk")); }
        prop_assert_eq!(frames, vec![json]);
        prop_assert!(decoder.finish().expect("EOF").is_none());
    }
}

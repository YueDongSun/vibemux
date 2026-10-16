#![forbid(unsafe_code)]
//! Repeatable bounded fuzz cases for private wire and VT parsing boundaries.
use vibemux_terminal::{
    model::TerminalModel,
    protocol,
    wire::{Command, Notice, Size},
};

#[test]
fn malformed_frames_and_terminal_bytes_remain_bounded_and_never_panic() {
    let mut seed = 0x36a0_98f1_u32;
    for case_index in 0..1024 {
        let payload: Vec<u8> = (0..case_index % 512)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&payload);
        if let Ok(Some(command)) =
            protocol::read_message::<Command>(&mut std::io::Cursor::new(&frame))
        {
            let _ = protocol::validate_command(&command);
        }
        if let Ok(Some(notice)) =
            protocol::read_message::<Notice>(&mut std::io::Cursor::new(&frame))
        {
            let _ = protocol::validate_notice(&notice);
        }
        let mut model = TerminalModel::new(Size {
            columns: 2 + case_index % 80,
            rows: 1 + case_index % 24,
        })
        .unwrap();
        let _ = model.process(&payload);
        protocol::validate_screen(&model.screen()).unwrap();
        model
            .resize(Size {
                columns: 2,
                rows: 1,
            })
            .unwrap();
        protocol::validate_screen(&model.screen()).unwrap();
    }
}

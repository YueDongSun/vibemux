#![forbid(unsafe_code)]
//! Offline real-PTY fixture, never a simulated Claude transcript.
use std::io::{Read, Write};
fn main() {
    let _ = crossterm::terminal::enable_raw_mode();
    let mut output = std::io::stdout();
    let _ = write!(
        output,
        "\x1b[2J\x1b[H\x1b[36mVibeMux PTY fixture\x1b[0m\r\nInput is echoed below. Ctrl+Q exits.\r\n"
    );
    let _ = output.flush();
    let mut input = std::io::stdin();
    let mut buffer = [0_u8; 256];
    while let Ok(length) = input.read(&mut buffer) {
        if length == 0 || buffer[..length].contains(&17) {
            break;
        }
        let _ = output.write_all(&buffer[..length]);
        let _ = output.flush();
    }
}

//! Bounded private Protobuf framing for owned host pipes.
use crate::wire::{Command, Notice, Screen, Size, command, notice};
use prost::Message;
use std::io::{Read, Write};

pub const VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_INPUT_BYTES: usize = 16 * 1024;
pub const MAX_COLUMNS: u32 = 300;
pub const MAX_ROWS: u32 = 120;
pub const QUEUE_CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalError(pub &'static str);
impl std::fmt::Display for TerminalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for TerminalError {}
pub type Result<T> = std::result::Result<T, TerminalError>;

pub fn read_message<M: Message + Default>(reader: &mut impl Read) -> Result<Option<M>> {
    let mut header = [0_u8; 4];
    let first = reader
        .read(&mut header[..1])
        .map_err(|_| TerminalError("terminal_pipe_read_failed"))?;
    if first == 0 {
        return Ok(None);
    }
    reader
        .read_exact(&mut header[1..])
        .map_err(|_| TerminalError("terminal_frame_truncated"))?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(TerminalError("terminal_frame_size_invalid"));
    }
    let mut bytes = vec![0; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| TerminalError("terminal_frame_truncated"))?;
    M::decode(bytes.as_slice())
        .map(Some)
        .map_err(|_| TerminalError("terminal_frame_invalid"))
}

pub fn write_message(writer: &mut impl Write, message: &impl Message) -> Result<()> {
    let bytes = message.encode_to_vec();
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        return Err(TerminalError("terminal_frame_size_invalid"));
    }
    let length =
        u32::try_from(bytes.len()).map_err(|_| TerminalError("terminal_frame_size_invalid"))?;
    writer
        .write_all(&length.to_be_bytes())
        .and_then(|_| writer.write_all(&bytes))
        .and_then(|_| writer.flush())
        .map_err(|_| TerminalError("terminal_pipe_write_failed"))
}

pub fn validate_size(size: &Size) -> Result<()> {
    if !(2..=MAX_COLUMNS).contains(&size.columns) || !(1..=MAX_ROWS).contains(&size.rows) {
        return Err(TerminalError("terminal_dimensions_invalid"));
    }
    Ok(())
}

pub fn validate_command(message: &Command) -> Result<()> {
    if message.version != VERSION {
        return Err(TerminalError("terminal_version_unsupported"));
    }
    match &message.operation {
        Some(command::Operation::Open(open)) => {
            for value in [&open.project_root, &open.executable, &open.git_executable] {
                if value.is_empty() || value.len() > 32768 || value.contains('\0') {
                    return Err(TerminalError("terminal_launch_invalid"));
                }
            }
            validate_size(
                open.size
                    .as_ref()
                    .ok_or(TerminalError("terminal_dimensions_invalid"))?,
            )
        }
        Some(command::Operation::Input(bytes))
            if !bytes.is_empty() && bytes.len() <= MAX_INPUT_BYTES =>
        {
            Ok(())
        }
        Some(command::Operation::Resize(size)) => validate_size(size),
        Some(command::Operation::Close(true)) => Ok(()),
        Some(command::Operation::Scroll(lines)) if lines.unsigned_abs() <= 2000 => Ok(()),
        _ => Err(TerminalError("terminal_command_invalid")),
    }
}

pub fn validate_screen(screen: &Screen) -> Result<()> {
    let size = screen
        .size
        .as_ref()
        .ok_or(TerminalError("terminal_screen_invalid"))?;
    validate_size(size)?;
    let count = usize::try_from(size.rows * size.columns)
        .map_err(|_| TerminalError("terminal_screen_invalid"))?;
    if screen.cells.len() != count
        || screen.cursor_row >= size.rows
        || screen.cursor_column >= size.columns
    {
        return Err(TerminalError("terminal_screen_invalid"));
    }
    if screen.cells.iter().any(|cell| {
        cell.text.len() > 64
            || cell.foreground > 0xffffff
            || cell.background > 0xffffff
            || cell.flags > 15
    }) {
        return Err(TerminalError("terminal_screen_invalid"));
    }
    Ok(())
}

pub fn validate_notice(message: &Notice) -> Result<()> {
    if message.version != VERSION {
        return Err(TerminalError("terminal_version_unsupported"));
    }
    match &message.payload {
        Some(notice::Payload::Screen(screen)) => validate_screen(screen),
        Some(notice::Payload::Status(status))
            if matches!(
                status.phase.as_str(),
                "starting" | "running" | "fixture" | "failed" | "exited"
            ) && status.code.len() <= 64
                && status
                    .code
                    .bytes()
                    .all(|value| value.is_ascii_lowercase() || value == b'_')
                && status.worktree.len() <= 32768
                && !status.worktree.contains('\0')
                && status.branch.len() <= 128
                && !status.branch.chars().any(char::is_control) =>
        {
            Ok(())
        }
        _ => Err(TerminalError("terminal_notice_invalid")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_refuses_oversize_and_truncation_before_decode() {
        assert!(
            read_message::<Command>(&mut std::io::Cursor::new(u32::MAX.to_be_bytes())).is_err()
        );
        assert!(read_message::<Command>(&mut std::io::Cursor::new(vec![0, 0, 0, 4, 1])).is_err());
    }
    #[test]
    fn input_is_data_and_version_and_size_are_enforced() {
        let command = Command {
            version: VERSION,
            operation: Some(command::Operation::Input(
                b"$(echo private); & x\n".to_vec(),
            )),
        };
        let mut bytes = Vec::new();
        write_message(&mut bytes, &command).unwrap();
        let decoded = read_message::<Command>(&mut std::io::Cursor::new(bytes))
            .unwrap()
            .unwrap();
        assert_eq!(decoded, command);
        assert!(validate_command(&decoded).is_ok());
        let mut wrong_version = decoded;
        wrong_version.version += 1;
        assert_eq!(
            validate_command(&wrong_version),
            Err(TerminalError("terminal_version_unsupported"))
        );
        assert!(
            validate_command(&Command {
                version: VERSION,
                operation: Some(command::Operation::Input(vec![1; MAX_INPUT_BYTES + 1])),
            })
            .is_err()
        );
        assert!(
            validate_size(&Size {
                columns: MAX_COLUMNS + 1,
                rows: 20
            })
            .is_err()
        );
    }
}

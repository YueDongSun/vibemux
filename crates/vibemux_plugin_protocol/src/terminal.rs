//! Versioned, bounded observation payloads. No terminal text or input commands.

use crate::wire::{TerminalInventory, TerminalPane};
impl Eq for TerminalInventory {}

pub const INVENTORY_METHOD: &str = "terminal:inventory_v1";
pub const FOCUS_METHOD: &str = "terminal:focus_v1";
pub const OBSERVE_PERMISSION: &str = "terminal:observe";
pub const FOCUS_PERMISSION: &str = "terminal:focus";
// Four maximum-length paths plus a binding still fit the 64 KiB JSON control
// envelope even when every permitted character requires JSON escaping.
pub const MAX_TERMINAL_PANES: usize = 4;
pub const MAX_TERMINAL_PAYLOAD_BYTES: usize = 40 * 1024;
pub const MAX_CWD_BYTES: usize = 4096;

pub fn valid_inventory(inventory: &TerminalInventory) -> bool {
    valid_label(&inventory.instance_id, 128)
        && inventory.panes.len() <= MAX_TERMINAL_PANES
        && inventory.panes.iter().all(valid_pane)
        && inventory.panes.iter().enumerate().all(|(index, pane)| {
            !inventory.panes[..index]
                .iter()
                .any(|old| old.pane_id == pane.pane_id)
        })
}

pub fn valid_pane(pane: &TerminalPane) -> bool {
    valid_pane_id(&pane.pane_id)
        && valid_label(&pane.cwd, MAX_CWD_BYTES)
        && pane.workspace.len() <= 256
        && !pane.workspace.chars().any(char::is_control)
        && valid_pane_id(&pane.window_id)
        && valid_pane_id(&pane.tab_id)
}

pub fn valid_pane_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 20 && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub fn valid_label(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

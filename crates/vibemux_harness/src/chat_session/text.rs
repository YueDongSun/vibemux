//! Bounded replacement snapshots; never protocol or tool-input truncation.

use super::{AssistantText, SessionError, SessionLimits, TurnSummary};

#[derive(Default)]
pub(super) struct TextBuffer {
    items: Vec<TextItem>,
    revision: u64,
}

struct TextItem {
    id: String,
    text: String,
    original_bytes: usize,
}

impl TextBuffer {
    pub(super) fn update(
        &mut self,
        id: &str,
        value: &str,
        append: bool,
        limits: &SessionLimits,
    ) -> Result<(), SessionError> {
        let known = self.items.iter().position(|item| item.id == id);
        if known.is_none() && self.items.len() >= limits.max_text_items {
            return Err(SessionError::TextItemLimit);
        }
        let used = self
            .items
            .iter()
            .enumerate()
            .filter(|(index, _)| append || Some(*index) != known)
            .map(|(_, item)| item.text.len())
            .sum::<usize>();
        let separators = (self.items.len() + usize::from(known.is_none())).saturating_sub(1) * 2;
        let available = limits
            .max_reply_bytes
            .saturating_sub(used.saturating_add(separators));
        let clipped = clip(value, available);
        match known {
            Some(index) if append => {
                self.items[index].original_bytes = self.items[index]
                    .original_bytes
                    .checked_add(value.len())
                    .ok_or(SessionError::RecordBudgetExhausted)?;
                self.items[index].text.push_str(clipped);
            }
            Some(index) => {
                self.items[index].text = clipped.to_owned();
                self.items[index].original_bytes = value.len();
            }
            None => self.items.push(TextItem {
                id: id.to_owned(),
                text: clipped.to_owned(),
                original_bytes: value.len(),
            }),
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(SessionError::RecordBudgetExhausted)?;
        Ok(())
    }

    pub(super) fn final_text(
        &mut self,
        value: &str,
        limits: &SessionLimits,
    ) -> Result<(), SessionError> {
        self.items.clear();
        self.update("result", value, false, limits)
    }

    pub(super) fn snapshot(&self, turn: &TurnSummary, limits: &SessionLimits) -> AssistantText {
        let joined = self
            .items
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let original_bytes = self
            .items
            .iter()
            .map(|item| item.original_bytes)
            .sum::<usize>()
            + self.items.len().saturating_sub(1) * 2;
        let text = clip(&joined, limits.max_reply_bytes).to_owned();
        AssistantText {
            request_id: turn.request_id,
            turn_id: turn.turn_id,
            revision: self.revision,
            truncated: original_bytes > text.len(),
            original_bytes,
            text,
        }
    }
}

fn clip(value: &str, available: usize) -> &str {
    let mut end = available.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

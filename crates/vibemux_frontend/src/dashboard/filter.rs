//! Console list filters: a case-insensitive substring per list. A filter only
//! changes which rows are drawn; it never reads or changes daemon state.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::Tab;

/// Longest query, in characters.
pub const MAX_FILTER_CHARS: usize = 64;

/// The lists a filter can narrow. Overview and Harnesses share the agent list
/// and its selection, so they share its filter too.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterList {
    Agents,
    Branches,
    Tasks,
    Decisions,
}

impl FilterList {
    #[must_use]
    pub const fn for_view(view: Tab) -> Option<Self> {
        match view {
            Tab::Overview | Tab::Harnesses => Some(Self::Agents),
            Tab::Workspace => Some(Self::Branches),
            Tab::Tasks => Some(Self::Tasks),
            Tab::Decisions => Some(Self::Decisions),
            Tab::Daemon | Tab::Sessions => None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FilterState {
    agents: String,
    branches: String,
    tasks: String,
    decisions: String,
    /// The list whose query the keyboard is editing.
    pub editing: Option<FilterList>,
}

impl FilterState {
    /// The query narrowing `view`'s list; empty when it has none.
    #[must_use]
    pub fn query(&self, view: Tab) -> &str {
        FilterList::for_view(view).map_or("", |list| self.list_query(list))
    }

    fn list_query(&self, list: FilterList) -> &str {
        match list {
            FilterList::Agents => &self.agents,
            FilterList::Branches => &self.branches,
            FilterList::Tasks => &self.tasks,
            FilterList::Decisions => &self.decisions,
        }
    }

    fn list_query_mut(&mut self, list: FilterList) -> &mut String {
        match list {
            FilterList::Agents => &mut self.agents,
            FilterList::Branches => &mut self.branches,
            FilterList::Tasks => &mut self.tasks,
            FilterList::Decisions => &mut self.decisions,
        }
    }

    /// Starts editing `view`'s filter; views without a list ignore it.
    pub fn start(&mut self, view: Tab) {
        self.editing = FilterList::for_view(view);
    }

    /// Clears `view`'s filter. Returns whether there was one to clear.
    pub fn clear(&mut self, view: Tab) -> bool {
        let Some(list) = FilterList::for_view(view) else {
            return false;
        };
        let query = self.list_query_mut(list);
        let had_query = !query.is_empty();
        query.clear();
        had_query
    }

    /// Applies one key while editing. Returns false when not editing, so the
    /// caller handles the key as navigation.
    pub fn handle_editing_key(&mut self, key: KeyEvent) -> bool {
        let Some(list) = self.editing else {
            return false;
        };
        match key.code {
            KeyCode::Esc => {
                self.list_query_mut(list).clear();
                self.editing = None;
            }
            KeyCode::Enter => self.editing = None,
            KeyCode::Backspace => {
                self.list_query_mut(list).pop();
            }
            KeyCode::Char(character)
                if !character.is_control()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let query = self.list_query_mut(list);
                if query.chars().count() < MAX_FILTER_CHARS {
                    query.push(character);
                }
            }
            // Arrows still move the selection while the query is open.
            KeyCode::Up | KeyCode::Down => return false,
            _ => {}
        }
        true
    }
}

/// Whether any field contains `query`, ignoring case. An empty query matches
/// every row.
#[must_use]
pub fn matches(query: &str, fields: &[&str]) -> bool {
    if query.is_empty() {
        return true;
    }
    let query = query.to_lowercase();
    fields
        .iter()
        .any(|field| field.to_lowercase().contains(&query))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_is_case_insensitive_substring() {
        assert!(matches("cod", &["Codex"]));
        assert!(matches("CODEX", &["Claude", "codex"]));
        assert!(!matches("x", &["Claude"]));
        assert!(matches("", &[]));
        assert!(!matches("a", &[]));
    }

    #[test]
    fn overview_and_harnesses_share_the_agent_filter() {
        let mut filter = FilterState::default();
        filter.start(Tab::Overview);
        for character in "gro".chars() {
            assert!(filter.handle_editing_key(KeyCode::Char(character).into()));
        }
        assert_eq!(filter.query(Tab::Harnesses), "gro");
        assert_eq!(filter.query(Tab::Tasks), "");
        assert!(filter.handle_editing_key(KeyCode::Enter.into()));
        assert_eq!(filter.editing, None);
        assert!(!filter.handle_editing_key(KeyCode::Char('x').into()));
        filter.start(Tab::Daemon);
        assert_eq!(filter.editing, None, "the daemon view has no list");
    }

    #[test]
    fn the_query_is_bounded() {
        let mut filter = FilterState::default();
        filter.start(Tab::Tasks);
        for _ in 0..MAX_FILTER_CHARS + 10 {
            filter.handle_editing_key(KeyCode::Char('a').into());
        }
        assert_eq!(filter.query(Tab::Tasks).chars().count(), MAX_FILTER_CHARS);
    }
}

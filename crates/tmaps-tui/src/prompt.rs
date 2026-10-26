use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tmaps_core::geocode::Place;

/// A box over the map. While it is open, it takes the keys.
#[derive(Debug)]
pub enum Prompt {
    Search(Search),
    SavePlace { name: String, invalid: bool },
    Places { selected: usize },
}

#[derive(Debug, Default)]
pub struct Search {
    pub text: String,
    pub results: Vec<Place>,
    pub selected: usize,
    /// The number of the last search request, until its results arrive.
    pub pending: Option<u64>,
}

impl Search {
    pub fn chosen(&self) -> Option<&Place> {
        self.results.get(self.selected)
    }
}

/// Types a character or removes the last one. Returns `true` when `text` changed.
pub fn edit(text: &mut String, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            text.push(c);
            true
        }
        KeyCode::Backspace => text.pop().is_some(),
        _ => false,
    }
}

/// Moves a selection in a list of `len` items with ↑ and ↓. Returns `None` for other keys.
pub fn select(selected: usize, len: usize, key: KeyCode) -> Option<usize> {
    match key {
        KeyCode::Up => Some(selected.saturating_sub(1)),
        KeyCode::Down => Some((selected + 1).min(len.saturating_sub(1))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn edit_types_shifted_characters() {
        let mut text = "du".to_owned();
        edit(&mut text, key(KeyCode::Char('O'), KeyModifiers::SHIFT));
        assert_eq!(text, "duO");
    }

    #[test]
    fn edit_ignores_control_characters() {
        let mut text = String::new();
        assert!(!edit(
            &mut text,
            key(KeyCode::Char('w'), KeyModifiers::CONTROL)
        ));
    }

    #[test]
    fn backspace_on_empty_text_changes_nothing() {
        assert!(!edit(
            &mut String::new(),
            key(KeyCode::Backspace, KeyModifiers::NONE)
        ));
    }

    #[test]
    fn down_stops_at_the_last_item() {
        assert_eq!(select(2, 3, KeyCode::Down), Some(2));
    }

    #[test]
    fn down_in_an_empty_list_selects_nothing_new() {
        assert_eq!(select(0, 0, KeyCode::Down), Some(0));
    }
}

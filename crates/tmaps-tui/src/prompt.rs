use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tmaps_core::geocode::Place;

/// A box over the map. While it is open, it takes the keys.
#[derive(Debug)]
pub enum Prompt {
    Search(Search),
    SavePlace { name: String, invalid: bool },
    Places { selected: usize },
}

impl Prompt {
    /// The selected item of a prompt with a list.
    pub fn selected_mut(&mut self) -> Option<&mut usize> {
        match self {
            Self::Search(search) => Some(&mut search.selected),
            Self::Places { selected } => Some(selected),
            Self::SavePlace { .. } => None,
        }
    }
}

#[derive(Debug, Default)]
pub struct Search {
    pub text: String,
    pub results: Vec<Place>,
    pub selected: usize,
    /// The number of the last search request, until its results arrive.
    pub pending: Option<u64>,
    pub error: Option<String>,
}

impl Search {
    pub fn chosen(&self) -> Option<&Place> {
        self.results.get(self.selected)
    }
}

/// Types a character or removes text at the end, with the keys of a shell. Returns `true` when
/// `text` changed.
pub fn edit(text: &mut String, key: KeyEvent) -> bool {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let before = text.len();
    match key.code {
        KeyCode::Char('u') if control => text.clear(),
        KeyCode::Char('w') if control => remove_word(text),
        KeyCode::Backspace if alt => remove_word(text),
        KeyCode::Char(c) if !control && !alt => text.push(c),
        KeyCode::Backspace => {
            text.pop();
        }
        _ => return false,
    }
    text.len() != before
}

/// Removes the spaces at the end, then the word before them.
fn remove_word(text: &mut String) {
    let word_end = text.trim_end().len();
    let word_start = text[..word_end]
        .trim_end_matches(|c: char| !c.is_whitespace())
        .len();
    text.truncate(word_start);
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
        let mut text = "duomo".to_owned();
        assert!(!edit(
            &mut text,
            key(KeyCode::Char('a'), KeyModifiers::CONTROL)
        ));
    }

    #[test]
    fn ctrl_u_clears_the_text() {
        let mut text = "piazza duomo".to_owned();
        edit(&mut text, key(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(text, "");
    }

    #[test]
    fn ctrl_w_removes_the_last_word_and_its_spaces() {
        let mut text = "piazza del duomo  ".to_owned();
        edit(&mut text, key(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(text, "piazza del ");
    }

    #[test]
    fn alt_backspace_removes_the_last_word() {
        let mut text = "via è".to_owned();
        edit(&mut text, key(KeyCode::Backspace, KeyModifiers::ALT));
        assert_eq!(text, "via ");
    }

    #[test]
    fn ctrl_w_on_empty_text_changes_nothing() {
        assert!(!edit(
            &mut String::new(),
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

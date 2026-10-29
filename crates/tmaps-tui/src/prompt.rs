use std::ops::Range;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tmaps_core::LatLon;
use tmaps_core::geocode::Place;
use unicode_segmentation::UnicodeSegmentation;

/// A box over the map. While it is open, it takes the keys.
#[derive(Debug)]
pub enum Prompt {
    Search(Search),
    SavePlace {
        name: Input,
        invalid: bool,
        location: LatLon,
        /// The label of a search result, or `None` for the crosshair.
        label: Option<String>,
        /// The search that `Esc` goes back to.
        search: Option<Search>,
    },
}

impl Prompt {
    /// The selected item of a prompt with a list.
    pub fn selected_mut(&mut self) -> Option<&mut usize> {
        match self {
            Self::Search(search) => Some(&mut search.selected),
            Self::SavePlace { .. } => None,
        }
    }

    pub fn take_removal(&mut self) -> Option<String> {
        match self {
            Self::Search(search) => search.removing.take(),
            Self::SavePlace { .. } => None,
        }
    }
}

#[derive(Debug, Default)]
pub struct Search {
    pub text: Input,
    pub results: Vec<Place>,
    pub selected: usize,
    /// The number of the last search request, until its results arrive.
    pub pending: Option<u64>,
    pub error: Option<String>,
    /// The saved place to remove when the user confirms.
    pub removing: Option<String>,
}

impl Search {
    pub fn chosen(&self) -> Option<&Place> {
        self.results.get(self.selected)
    }

    /// The text after `@`, for a search of the saved places only.
    pub fn saved_query(&self) -> Option<&str> {
        self.text
            .as_str()
            .trim()
            .strip_prefix('@')
            .map(str::trim_start)
    }
}

/// A line of text that the user types, with a cursor. It takes the readline keys of a shell.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Input {
    text: String,
    /// A byte index on a char boundary.
    cursor: usize,
}

/// What a key changed in an [`Input`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Nothing,
    Cursor,
    Text,
}

impl From<&str> for Input {
    fn from(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            cursor: text.len(),
        }
    }
}

impl Input {
    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn before_cursor(&self) -> &str {
        &self.text[..self.cursor]
    }

    pub fn after_cursor(&self) -> &str {
        &self.text[self.cursor..]
    }

    pub fn edit(&mut self, key: KeyEvent) -> Change {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let (len, cursor) = (self.text.len(), self.cursor);
        match key.code {
            KeyCode::Left if control || alt => self.cursor = self.previous_word(),
            KeyCode::Right if control || alt => self.cursor = self.next_word(),
            KeyCode::Char('b') if alt => self.cursor = self.previous_word(),
            KeyCode::Char('f') if alt => self.cursor = self.next_word(),
            KeyCode::Left => self.cursor = self.previous_grapheme(),
            KeyCode::Right => self.cursor = self.next_grapheme(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::Char('a') if control => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            KeyCode::Char('e') if control => self.cursor = self.text.len(),
            KeyCode::Char('u') if control => self.remove(0..self.cursor),
            KeyCode::Char('k') if control => self.remove(self.cursor..self.text.len()),
            KeyCode::Char('w') if control => self.remove(self.previous_word()..self.cursor),
            KeyCode::Backspace if alt => self.remove(self.previous_word()..self.cursor),
            KeyCode::Backspace => self.remove(self.previous_grapheme()..self.cursor),
            KeyCode::Delete => self.remove(self.cursor..self.next_grapheme()),
            KeyCode::Char(c) if !control && !alt => {
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            _ => return Change::Nothing,
        }
        if self.text.len() != len {
            Change::Text
        } else if self.cursor != cursor {
            Change::Cursor
        } else {
            Change::Nothing
        }
    }

    fn remove(&mut self, range: Range<usize>) {
        self.cursor = range.start;
        self.text.replace_range(range, "");
    }

    /// A grapheme is what the user sees as one character, such as `❤️` or `👨‍👩‍👧`.
    fn previous_grapheme(&self) -> usize {
        self.before_cursor()
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_grapheme(&self) -> usize {
        self.cursor
            + self
                .after_cursor()
                .graphemes(true)
                .next()
                .map_or(0, str::len)
    }

    /// The start of the word before the cursor, after the spaces between them.
    fn previous_word(&self) -> usize {
        self.before_cursor()
            .trim_end()
            .trim_end_matches(|c: char| !c.is_whitespace())
            .len()
    }

    /// The end of the word after the cursor, after the spaces between them.
    fn next_word(&self) -> usize {
        let rest = self
            .after_cursor()
            .trim_start()
            .trim_start_matches(|c: char| !c.is_whitespace());
        self.text.len() - rest.len()
    }
}

/// Moves a selection in a list of `len` items with ↑ and ↓, or `Ctrl-P` and `Ctrl-N`. Returns
/// `None` for other keys.
pub fn select(selected: usize, len: usize, key: KeyEvent) -> Option<usize> {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match (key.code, control) {
        (KeyCode::Up, _) | (KeyCode::Char('p'), true) => Some(selected.saturating_sub(1)),
        (KeyCode::Down, _) | (KeyCode::Char('n'), true) => {
            Some((selected + 1).min(len.saturating_sub(1)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn control(c: char) -> KeyEvent {
        key(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn alt(code: KeyCode) -> KeyEvent {
        key(code, KeyModifiers::ALT)
    }

    fn plain(code: KeyCode) -> KeyEvent {
        key(code, KeyModifiers::NONE)
    }

    /// `text` with `|` at the cursor.
    fn input(text: &str) -> Input {
        let cursor = text.find('|').unwrap();
        Input {
            text: text.replace('|', ""),
            cursor,
        }
    }

    fn shown(input: &Input) -> String {
        format!("{}|{}", input.before_cursor(), input.after_cursor())
    }

    fn edited(text: &str, key: KeyEvent) -> String {
        let mut input = input(text);
        input.edit(key);
        shown(&input)
    }

    #[test]
    fn edit_types_shifted_characters() {
        assert_eq!(
            edited("du|", key(KeyCode::Char('O'), KeyModifiers::SHIFT)),
            "duO|"
        );
    }

    #[test]
    fn edit_types_at_the_cursor() {
        assert_eq!(
            edited("pzza|duomo", plain(KeyCode::Char(' '))),
            "pzza |duomo"
        );
        assert_eq!(edited("p|zza", plain(KeyCode::Char('i'))), "pi|zza");
    }

    #[test]
    fn edit_ignores_control_characters() {
        assert_eq!(input("duomo|").edit(control('g')), Change::Nothing);
    }

    #[test]
    fn edit_tells_a_cursor_move_from_a_text_change() {
        assert_eq!(input("duomo|").edit(plain(KeyCode::Left)), Change::Cursor);
        assert_eq!(
            input("duomo|").edit(plain(KeyCode::Backspace)),
            Change::Text
        );
    }

    #[test]
    fn arrows_move_one_character() {
        assert_eq!(edited("via è|", plain(KeyCode::Left)), "via |è");
        assert_eq!(edited("via |è", plain(KeyCode::Right)), "via è|");
    }

    #[test]
    fn arrows_and_removal_take_a_whole_emoji() {
        assert_eq!(edited("a❤\u{fe0f}|", plain(KeyCode::Left)), "a|❤\u{fe0f}");
        assert_eq!(
            edited("a|👨\u{200d}👧", plain(KeyCode::Right)),
            "a👨\u{200d}👧|"
        );
        assert_eq!(edited("a❤\u{fe0f}|", plain(KeyCode::Backspace)), "a|");
        assert_eq!(edited("|👨\u{200d}👧a", plain(KeyCode::Delete)), "|a");
    }

    #[test]
    fn ctrl_b_and_ctrl_f_do_nothing() {
        assert_eq!(input("via| è").edit(control('b')), Change::Nothing);
        assert_eq!(input("via| è").edit(control('f')), Change::Nothing);
    }

    #[test]
    fn arrows_stop_at_the_ends() {
        assert_eq!(input("|duomo").edit(plain(KeyCode::Left)), Change::Nothing);
        assert_eq!(input("duomo|").edit(plain(KeyCode::Right)), Change::Nothing);
    }

    #[test]
    fn alt_arrows_move_one_word() {
        assert_eq!(
            edited("piazza del duomo  |", alt(KeyCode::Left)),
            "piazza del |duomo  "
        );
        assert_eq!(
            edited("piazza del |duomo", alt(KeyCode::Char('b'))),
            "piazza |del duomo"
        );
        assert_eq!(
            edited("|  piazza del", alt(KeyCode::Right)),
            "  piazza| del"
        );
        assert_eq!(
            edited("piazza| del", alt(KeyCode::Char('f'))),
            "piazza del|"
        );
        assert_eq!(
            edited("piazza| del", key(KeyCode::Right, KeyModifiers::CONTROL)),
            "piazza del|"
        );
    }

    #[test]
    fn home_and_end_go_to_the_ends() {
        assert_eq!(edited("pia|zza", plain(KeyCode::Home)), "|piazza");
        assert_eq!(edited("pia|zza", control('a')), "|piazza");
        assert_eq!(edited("pia|zza", plain(KeyCode::End)), "piazza|");
        assert_eq!(edited("pia|zza", control('e')), "piazza|");
    }

    #[test]
    fn ctrl_u_and_ctrl_k_remove_to_the_ends() {
        assert_eq!(edited("piazza |duomo", control('u')), "|duomo");
        assert_eq!(edited("piazza |duomo", control('k')), "piazza |");
    }

    #[test]
    fn ctrl_w_removes_the_word_before_the_cursor_and_its_spaces() {
        assert_eq!(edited("piazza del duomo  |", control('w')), "piazza del |");
        assert_eq!(edited("piazza del| duomo", control('w')), "piazza | duomo");
    }

    #[test]
    fn alt_backspace_removes_the_word_before_the_cursor() {
        assert_eq!(edited("via è|", alt(KeyCode::Backspace)), "via |");
    }

    #[test]
    fn backspace_and_delete_remove_one_character() {
        assert_eq!(edited("via è|x", plain(KeyCode::Backspace)), "via |x");
        assert_eq!(edited("via |èx", plain(KeyCode::Delete)), "via |x");
    }

    #[test]
    fn removal_at_the_ends_changes_nothing() {
        for (text, key) in [
            ("|", control('w')),
            ("|", plain(KeyCode::Backspace)),
            ("|duomo", control('u')),
            ("duomo|", control('k')),
            ("duomo|", plain(KeyCode::Delete)),
        ] {
            assert_eq!(input(text).edit(key), Change::Nothing, "{text}");
        }
    }

    #[test]
    fn down_stops_at_the_last_item() {
        assert_eq!(select(2, 3, plain(KeyCode::Down)), Some(2));
    }

    #[test]
    fn down_in_an_empty_list_selects_nothing_new() {
        assert_eq!(select(0, 0, plain(KeyCode::Down)), Some(0));
    }

    #[test]
    fn ctrl_n_and_ctrl_p_select_like_the_arrows() {
        assert_eq!(select(1, 3, control('n')), Some(2));
        assert_eq!(select(1, 3, control('p')), Some(0));
        assert_eq!(select(1, 3, plain(KeyCode::Char('n'))), None);
    }
}

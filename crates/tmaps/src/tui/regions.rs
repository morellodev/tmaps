use ratatui::layout::{Position, Rect};

/// Where the last frame drew the parts that take mouse clicks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Regions {
    pub map: Rect,
    pub panel: Option<Rect>,
    pub steps: Option<ListRegion>,
    pub prompt_items: Option<ListRegion>,
    pub help: Option<Rect>,
}

/// The items of a list on the screen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListRegion {
    pub area: Rect,
    /// The index of the first visible item.
    pub offset: usize,
    /// The height in rows of each item, also of the items above the visible part.
    pub heights: Vec<usize>,
}

impl ListRegion {
    /// A list draws only the items that fit completely, so the rows under them are empty.
    pub fn item_at(&self, position: Position) -> Option<usize> {
        if !self.area.contains(position) {
            return None;
        }
        let row = usize::from(position.y - self.area.y);
        let mut bottom = 0;
        for (index, height) in self.heights.iter().enumerate().skip(self.offset) {
            bottom += height;
            if row < bottom {
                return (bottom <= usize::from(self.area.height)).then_some(index);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> ListRegion {
        ListRegion {
            area: Rect::new(10, 5, 20, 6),
            offset: 1,
            heights: vec![3, 2, 1, 2],
        }
    }

    #[test]
    fn item_at_starts_at_the_first_visible_item() {
        assert_eq!(list().item_at(Position::new(12, 6)), Some(1));
    }

    #[test]
    fn item_at_finds_a_tall_item_on_its_last_row() {
        assert_eq!(list().item_at(Position::new(12, 9)), Some(3));
    }

    #[test]
    fn item_at_ignores_the_rows_under_the_last_item() {
        assert_eq!(list().item_at(Position::new(12, 10)), None);
    }

    #[test]
    fn item_at_ignores_an_item_that_does_not_fit() {
        let list = ListRegion {
            area: Rect::new(0, 0, 20, 5),
            offset: 0,
            heights: vec![2, 2, 2],
        };
        assert_eq!(list.item_at(Position::new(5, 4)), None);
    }

    #[test]
    fn item_at_ignores_a_click_beside_the_list() {
        assert_eq!(list().item_at(Position::new(30, 6)), None);
    }
}

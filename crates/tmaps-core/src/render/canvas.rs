use super::style::Class;
use super::{DOTS_PER_CELL_X, DOTS_PER_CELL_Y};

/// A grid of terminal cells. Lines set braille dots and the foreground class; areas set the
/// background class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canvas {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cell {
    /// Braille dot bits in Unicode order, see <https://en.wikipedia.org/wiki/Braille_Patterns>.
    pub dots: u8,
    pub fg: Option<Class>,
    pub bg: Option<Class>,
}

impl Cell {
    /// The braille character for the dots, or a space when no dot is set.
    #[must_use]
    pub fn symbol(self) -> char {
        if self.dots == 0 {
            ' '
        } else {
            char::from_u32(0x2800 + u32::from(self.dots)).unwrap_or(' ')
        }
    }
}

/// Bit of each dot, indexed by `[row][column]` inside the cell.
const DOT_BITS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

impl Canvas {
    #[must_use]
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            cells: vec![Cell::default(); usize::from(width) * usize::from(height)],
        }
    }

    #[must_use]
    pub fn width(&self) -> u16 {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> u16 {
        self.height
    }

    #[must_use]
    pub fn width_dots(&self) -> f64 {
        f64::from(self.width) * f64::from(DOTS_PER_CELL_X)
    }

    #[must_use]
    pub fn height_dots(&self) -> f64 {
        f64::from(self.height) * f64::from(DOTS_PER_CELL_Y)
    }

    /// The rows of cells, from top to bottom.
    pub fn rows(&self) -> impl Iterator<Item = &[Cell]> {
        self.cells.chunks(usize::from(self.width.max(1)))
    }

    pub(super) fn set_dot(&mut self, x: i64, y: i64, class: Class) {
        let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) else {
            return;
        };
        let (column, row) = (x / DOTS_PER_CELL_X, y / DOTS_PER_CELL_Y);
        if let Some(cell) = self.cell_mut(column, row) {
            cell.dots |=
                DOT_BITS[usize::from(y % DOTS_PER_CELL_Y)][usize::from(x % DOTS_PER_CELL_X)];
            cell.fg = Some(class);
        }
    }

    pub(super) fn fill(&mut self, column: u16, row: u16, class: Class) {
        if let Some(cell) = self.cell_mut(column, row) {
            cell.bg = Some(class);
        }
    }

    fn cell_mut(&mut self, column: u16, row: u16) -> Option<&mut Cell> {
        if column >= self.width || row >= self.height {
            return None;
        }
        self.cells
            .get_mut(usize::from(row) * usize::from(self.width) + usize::from(column))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dots_build_the_braille_character() {
        let mut canvas = Canvas::new(1, 1);
        canvas.set_dot(0, 0, Class::MinorRoad);
        canvas.set_dot(1, 3, Class::MinorRoad);
        assert_eq!(canvas.rows().next().unwrap()[0].symbol(), '⢁');
    }

    #[test]
    fn dot_outside_the_canvas_is_ignored() {
        let mut canvas = Canvas::new(1, 1);
        canvas.set_dot(2, 0, Class::MinorRoad);
        canvas.set_dot(-1, 0, Class::MinorRoad);
        assert_eq!(canvas, Canvas::new(1, 1));
    }
}

use tmaps_core::PreparedTile;
use tmaps_core::label::Anchor;

use crate::label::{display_width, wrap};

/// A prepared tile with the cell widths of its labels. Grapheme segmentation is slow, so the
/// widths are measured once, with the tile, off the UI thread.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BrailleTile {
    prepared: PreparedTile,
    /// One entry for each label candidate, in the same order. `None` for a text too wide to measure.
    texts: Vec<Option<MeasuredText>>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MeasuredText {
    pub(crate) width: i32,
    /// Two lines of a point label, for when one line does not fit.
    pub(crate) wrapped: Option<[(String, i32); 2]>,
}

impl BrailleTile {
    #[must_use]
    pub fn prepare(prepared: PreparedTile) -> Self {
        let texts = prepared
            .labels()
            .iter()
            .map(|candidate| {
                Some(MeasuredText {
                    width: i32::try_from(display_width(&candidate.text)).ok()?,
                    wrapped: matches!(candidate.anchor, Anchor::Point(_))
                        .then(|| wrap(&candidate.text))
                        .flatten(),
                })
            })
            .collect();
        Self { prepared, texts }
    }

    pub(crate) fn text(&self, index: usize) -> Option<&MeasuredText> {
        self.texts.get(index)?.as_ref()
    }
}

impl AsRef<PreparedTile> for BrailleTile {
    fn as_ref(&self) -> &PreparedTile {
        &self.prepared
    }
}

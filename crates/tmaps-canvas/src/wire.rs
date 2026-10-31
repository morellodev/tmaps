//! Label candidates as bytes, so that a worker can prepare a tile and send its labels to the
//! main thread in a transferable buffer. All numbers are little-endian.

use tmaps_core::label::{Anchor, LabelCandidate, LabelClass};
use tmaps_core::scene::Part;

/// The classes in the order of their tags.
const CLASSES: [LabelClass; 8] = [
    LabelClass::Country,
    LabelClass::City,
    LabelClass::State,
    LabelClass::Town,
    LabelClass::Village,
    LabelClass::Suburb,
    LabelClass::Water,
    LabelClass::Road,
];

const POINT: u8 = 0;
const LINES: u8 = 1;

#[must_use]
pub fn encode(labels: &[LabelCandidate]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for label in labels {
        bytes.push(class_tag(label.class));
        bytes.extend(label.rank.to_le_bytes());
        bytes.extend(label.min_zoom.to_le_bytes());
        bytes.extend(label.max_zoom.to_le_bytes());
        put_len(&mut bytes, label.text.len());
        bytes.extend(label.text.as_bytes());
        match &label.anchor {
            Anchor::Point(point) => {
                bytes.push(POINT);
                put_point(&mut bytes, *point);
            }
            Anchor::Lines(lines) => {
                bytes.push(LINES);
                put_len(&mut bytes, lines.len());
                for line in lines {
                    put_len(&mut bytes, line.points().len());
                    for &point in line.points() {
                        put_point(&mut bytes, point);
                    }
                }
            }
        }
    }
    bytes
}

/// The labels in `bytes` from [`encode`], or `None` if `bytes` has another form.
#[must_use]
pub fn decode(bytes: &[u8]) -> Option<Vec<LabelCandidate>> {
    let mut reader = Reader(bytes);
    let mut labels = Vec::new();
    while !reader.0.is_empty() {
        let class = *CLASSES.get(usize::from(reader.byte()?))?;
        let rank = i64::from_le_bytes(reader.array()?);
        let min_zoom = f64::from_le_bytes(reader.array()?);
        let max_zoom = f64::from_le_bytes(reader.array()?);
        let length = reader.len()?;
        let text = String::from_utf8(reader.take(length)?.to_vec()).ok()?;
        let anchor = match reader.byte()? {
            POINT => Anchor::Point(reader.point()?),
            LINES => {
                let count = reader.len()?;
                let mut lines = Vec::new();
                for _ in 0..count {
                    let length = reader.len()?;
                    let points = (0..length).map(|_| reader.point()).collect::<Option<_>>()?;
                    lines.push(Part::new(points));
                }
                Anchor::Lines(lines)
            }
            _ => return None,
        };
        labels.push(LabelCandidate {
            class,
            rank,
            min_zoom,
            max_zoom,
            text,
            anchor,
        });
    }
    Some(labels)
}

fn class_tag(class: LabelClass) -> u8 {
    match class {
        LabelClass::Country => 0,
        LabelClass::City => 1,
        LabelClass::State => 2,
        LabelClass::Town => 3,
        LabelClass::Village => 4,
        LabelClass::Suburb => 5,
        LabelClass::Water => 6,
        LabelClass::Road => 7,
    }
}

fn put_len(bytes: &mut Vec<u8>, length: usize) {
    let length = u32::try_from(length).unwrap_or(u32::MAX);
    bytes.extend(length.to_le_bytes());
}

fn put_point(bytes: &mut Vec<u8>, [x, y]: [f32; 2]) {
    bytes.extend(x.to_le_bytes());
    bytes.extend(y.to_le_bytes());
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (taken, rest) = self.0.split_at_checked(count)?;
        self.0 = rest;
        Some(taken)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    fn byte(&mut self) -> Option<u8> {
        self.array::<1>().map(|[byte]| byte)
    }

    fn len(&mut self) -> Option<usize> {
        usize::try_from(u32::from_le_bytes(self.array()?)).ok()
    }

    fn point(&mut self) -> Option<[f32; 2]> {
        Some([
            f32::from_le_bytes(self.array()?),
            f32::from_le_bytes(self.array()?),
        ])
    }
}

#[cfg(test)]
mod tests {
    use tmaps_core::PreparedTile;

    use super::*;

    fn fixture(name: &str) -> PreparedTile {
        let path = format!(
            "{}/../tmaps-core/tests/fixtures/tiles/{name}.pbf",
            env!("CARGO_MANIFEST_DIR")
        );
        PreparedTile::decode(&std::fs::read(path).unwrap(), "it").unwrap()
    }

    #[test]
    fn labels_of_a_tile_survive_the_bytes() {
        for name in ["5-16-11", "14-8610-5863"] {
            let labels = fixture(name).labels().to_vec();
            assert_eq!(decode(&encode(&labels)), Some(labels), "tile {name}");
        }
    }

    #[test]
    fn every_class_has_its_own_tag() {
        for (tag, class) in CLASSES.into_iter().enumerate() {
            assert_eq!(usize::from(class_tag(class)), tag);
        }
    }

    #[test]
    fn cut_bytes_give_no_labels() {
        let bytes = encode(fixture("14-8610-5863").labels());
        assert_eq!(decode(&bytes[..bytes.len() - 1]), None);
    }
}

//! Label text that follows a line. The text breaks into runs, one for each segment of the line
//! under it, as in `ol/geom/flat/textpath.js` of `OpenLayers`. A run draws in one call, so a label
//! on a straight part of a line keeps the kerning and the shaping of the browser. The check of
//! the corners is that of `src/symbol/check_max_angle.ts` of `MapLibre`.

use std::collections::VecDeque;
use std::f64::consts::{FRAC_PI_4, PI, TAU};
use std::ops::Range;

use tmaps_core::scene::{Rect, ScreenPoint};

/// The most that a line can turn in [`WINDOW`], in radians: the default `text-max-angle` of
/// `MapLibre`.
const MAX_TURN: f64 = FRAC_PI_4;

/// The length of line in which the turns add up, in text heights, as in `MapLibre`.
const WINDOW: f64 = 0.6;

/// A grapheme cluster of a label text, and its width in CSS pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Cluster {
    pub range: Range<usize>,
    pub width: f64,
}

/// A part of a label text that draws in one call: the text in `range`, centered at `offset` from
/// the label position, and turned clockwise by `angle` radians.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub range: Range<usize>,
    pub offset: ScreenPoint,
    pub angle: f64,
}

impl Run {
    /// The whole `text`, centered on the label position and not turned.
    #[must_use]
    pub fn level(text: &str) -> Self {
        Self {
            range: 0..text.len(),
            offset: (0.0, 0.0),
            angle: 0.0,
        }
    }
}

/// A label text along a line.
#[derive(Debug, Clone, PartialEq)]
pub struct LineText {
    pub runs: Vec<Run>,
    /// The box around each cluster on the screen, without padding.
    pub boxes: Vec<Rect>,
}

/// Lays `clusters` along `path`, centered `along` CSS pixels from its start. The clusters are
/// `height` high. The text reads from left to right, so it is not upside down.
///
/// Returns `None` if the text goes past an end of the line, or if the line turns too much under
/// the text.
#[must_use]
pub fn lay_out(
    path: &Path,
    along: f64,
    clusters: &[Cluster],
    height: f64,
    right_to_left: bool,
) -> Option<LineText> {
    let width: f64 = clusters.iter().map(|cluster| cluster.width).sum();
    let (start, end) = (along - width / 2.0, along + width / 2.0);
    if path.points.len() < 2
        || width <= 0.0
        || start < 0.0
        || end > path.length()
        || path.turns_too_much(start, end, WINDOW * height)
    {
        return None;
    }
    let flip = path.point_at(end).0 < path.point_at(start).0;
    let on_line = |offset: f64| if flip { end - offset } else { start + offset };
    // The clusters in their order on the screen. Each run of right-to-left text draws its own
    // clusters from right to left.
    let visual: Vec<&Cluster> = if right_to_left {
        clusters.iter().rev().collect()
    } else {
        clusters.iter().collect()
    };
    let mut spans: Vec<Span> = Vec::new();
    let mut boxes = Vec::with_capacity(clusters.len());
    let mut offset = 0.0;
    for cluster in visual {
        let center = on_line(offset + cluster.width / 2.0);
        let segment = path.segment_at(center);
        let angle = path.angle(segment, flip);
        boxes.push(turned_box(
            path.point_at(center),
            angle,
            cluster.width,
            height,
        ));
        let next = offset + cluster.width;
        match spans.last_mut() {
            Some(span) if span.segment == segment => {
                span.range = span.range.start.min(cluster.range.start)
                    ..span.range.end.max(cluster.range.end);
                span.to = next;
            }
            _ => spans.push(Span {
                segment,
                range: cluster.range.clone(),
                from: offset,
                to: next,
            }),
        }
        offset = next;
    }
    let origin = path.point_at(along);
    let runs = spans
        .into_iter()
        .map(|span| {
            let (x, y) = path.point_at(on_line(f64::midpoint(span.from, span.to)));
            Run {
                range: span.range,
                offset: (x - origin.0, y - origin.1),
                angle: path.angle(span.segment, flip),
            }
        })
        .collect();
    Some(LineText { runs, boxes })
}

/// The clusters of a run, from offset `from` to `to` along the text, on one segment of the line.
struct Span {
    segment: usize,
    range: Range<usize>,
    from: f64,
    to: f64,
}

/// A line on the screen, with the distance along it to each point.
pub struct Path {
    points: Vec<ScreenPoint>,
    /// The distance of each point from the first point.
    starts: Vec<f64>,
}

impl Path {
    pub fn new(points: impl IntoIterator<Item = ScreenPoint>) -> Self {
        let points: Vec<_> = points.into_iter().collect();
        let mut starts = Vec::with_capacity(points.len());
        let mut start = 0.0;
        for (index, &point) in points.iter().enumerate() {
            if let Some(index) = index.checked_sub(1) {
                start += distance(points[index], point);
            }
            starts.push(start);
        }
        Self { points, starts }
    }

    fn length(&self) -> f64 {
        self.starts.last().copied().unwrap_or_default()
    }

    /// The index of the first segment that ends after `at`, or of the last segment. Needs two
    /// points or more.
    fn segment_at(&self, at: f64) -> usize {
        self.starts[1..]
            .partition_point(|&start| start <= at)
            .min(self.points.len() - 2)
    }

    fn point_at(&self, at: f64) -> ScreenPoint {
        let segment = self.segment_at(at);
        let (a, b) = (self.points[segment], self.points[segment + 1]);
        let length = self.starts[segment + 1] - self.starts[segment];
        let t = if length > 0.0 {
            (at - self.starts[segment]) / length
        } else {
            0.0
        };
        (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1))
    }

    /// The direction of `segment` in radians, or the opposite direction if `flip`.
    fn angle(&self, segment: usize, flip: bool) -> f64 {
        let (a, b) = (self.points[segment], self.points[segment + 1]);
        let sign = if flip { -1.0 } else { 1.0 };
        (sign * (b.1 - a.1)).atan2(sign * (b.0 - a.0))
    }

    /// Whether the turns at the corners between `start` and `end` add up to more than
    /// [`MAX_TURN`] in a length of `window`.
    fn turns_too_much(&self, start: f64, end: f64, window: f64) -> bool {
        let mut recent = VecDeque::new();
        let mut sum = 0.0;
        let mut previous = None;
        let first = self.starts[1..].partition_point(|&start_of_next| start_of_next <= start);
        for segment in first..self.points.len().saturating_sub(1) {
            let (from, to) = (self.starts[segment], self.starts[segment + 1]);
            if from >= end {
                break;
            }
            if to <= start || to <= from {
                continue;
            }
            let direction = self.angle(segment, false);
            if let Some(previous) = previous {
                let turn = wrap(direction - previous).abs();
                recent.push_back((from, turn));
                sum += turn;
                while let Some(&(at, old)) = recent.front()
                    && from - at > window
                {
                    sum -= old;
                    recent.pop_front();
                }
                if sum > MAX_TURN {
                    return true;
                }
            }
            previous = Some(direction);
        }
        false
    }
}

fn distance(a: ScreenPoint, b: ScreenPoint) -> f64 {
    (b.0 - a.0).hypot(b.1 - a.1)
}

/// `angle` in the range from -π to π.
fn wrap(angle: f64) -> f64 {
    (angle + PI).rem_euclid(TAU) - PI
}

/// The box around a `width` × `height` rectangle that is centered on `center` and turned by
/// `angle`.
fn turned_box(center: ScreenPoint, angle: f64, width: f64, height: f64) -> Rect {
    let (sin, cos) = angle.sin_cos();
    let box_width = width * cos.abs() + height * sin.abs();
    let box_height = width * sin.abs() + height * cos.abs();
    Rect {
        left: center.0 - box_width / 2.0,
        top: center.1 - box_height / 2.0,
        right: center.0 + box_width / 2.0,
        bottom: center.1 + box_height / 2.0,
    }
}

/// The blocks of the right-to-left scripts, as in the `rtlRegEx` of `OpenLayers`.
pub fn is_right_to_left(c: char) -> bool {
    matches!(
        c,
        '\u{591}'..='\u{8FF}'
            | '\u{FB1D}'..='\u{FDFF}'
            | '\u{FE70}'..='\u{FEFC}'
            | '\u{10800}'..='\u{10FFF}'
            | '\u{1E800}'..='\u{1EFFF}'
    )
}

#[cfg(test)]
mod tests {
    use unicode_segmentation::UnicodeSegmentation;

    use super::*;

    /// Each cluster is 10 pixels wide and high.
    fn lay(points: &[ScreenPoint], along: f64, text: &str) -> Option<LineText> {
        let clusters: Vec<_> = text
            .grapheme_indices(true)
            .map(|(start, cluster)| Cluster {
                range: start..start + cluster.len(),
                width: 10.0,
            })
            .collect();
        let path = Path::new(points.iter().copied());
        lay_out(
            &path,
            along,
            &clusters,
            10.0,
            text.chars().any(is_right_to_left),
        )
    }

    /// The point `length` pixels from `from`, in the direction `degrees` clockwise from the x axis.
    fn step(from: ScreenPoint, degrees: f64, length: f64) -> ScreenPoint {
        let (sin, cos) = degrees.to_radians().sin_cos();
        (from.0 + length * cos, from.1 + length * sin)
    }

    fn runs(text: &LineText) -> Vec<(Range<usize>, f64)> {
        text.runs
            .iter()
            .map(|run| (run.range.clone(), run.angle))
            .collect()
    }

    #[test]
    fn text_on_one_segment_is_one_run_on_the_line() {
        let text = lay(&[(0.0, 100.0), (100.0, 0.0)], 50.0, "Via Po").unwrap();
        assert_eq!(runs(&text), [(0..6, -FRAC_PI_4)]);
        let (x, y) = text.runs[0].offset;
        assert!(x.abs() < 1e-9 && y.abs() < 1e-9);
    }

    #[test]
    fn text_on_a_line_that_goes_left_is_not_upside_down() {
        let text = lay(&[(300.0, 100.0), (0.0, 100.0)], 150.0, "Via Po").unwrap();
        assert_eq!(runs(&text), [(0..6, 0.0)]);
    }

    #[test]
    fn text_breaks_into_one_run_for_each_segment() {
        let text = lay(
            &[(0.0, 0.0), (100.0, 0.0), (200.0, 10.0)],
            100.0,
            "Via Roma",
        )
        .unwrap();
        let angle = 10.0_f64.atan2(100.0);
        assert_eq!(runs(&text), [(0..4, 0.0), (4..8, angle)]);
    }

    #[test]
    fn run_is_centered_on_its_clusters() {
        let text = lay(
            &[(0.0, 0.0), (100.0, 0.0), (200.0, 10.0)],
            100.0,
            "Via Roma",
        )
        .unwrap();
        assert_eq!(text.runs[0].offset, (-20.0, 0.0));
    }

    #[test]
    fn text_over_a_sharp_corner_is_left_out() {
        let text = lay(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0)], 100.0, "Via Po");
        assert_eq!(text, None);
    }

    #[test]
    fn turns_far_apart_do_not_add_up() {
        let points = [(0.0, 0.0), (40.0, 0.0), step((40.0, 0.0), 30.0, 30.0)];
        let points = [points[0], points[1], points[2], step(points[2], 0.0, 40.0)];
        assert!(lay(&points, 55.0, "Via Lodi").is_some());
    }

    #[test]
    fn turns_close_together_add_up() {
        let corner = step((50.0, 0.0), 30.0, 3.0);
        let points = [(0.0, 0.0), (50.0, 0.0), corner, step(corner, 60.0, 50.0)];
        assert_eq!(lay(&points, 51.5, "Via Po"), None);
    }

    #[test]
    fn text_that_goes_past_the_line_end_is_left_out() {
        assert_eq!(lay(&[(0.0, 0.0), (50.0, 0.0)], 25.0, "Via Po"), None);
    }

    #[test]
    fn runs_of_right_to_left_text_go_from_right_to_left() {
        let text = "שדרות רוטשילד";
        let laid = lay(&[(0.0, 0.0), (100.0, 0.0), (200.0, 10.0)], 100.0, text).unwrap();
        let ranges: Vec<_> = laid.runs.iter().map(|run| run.range.clone()).collect();
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].end, text.len());
        assert_eq!(ranges[1].start, 0);
    }

    #[test]
    fn each_cluster_has_a_box_around_its_turned_rectangle() {
        let text = lay(&[(0.0, 100.0), (100.0, 0.0)], 50.0, "ab").unwrap();
        let side = 2.0 * 10.0 * FRAC_PI_4.cos();
        assert!(
            text.boxes
                .iter()
                .all(|rect| (rect.right - rect.left - side).abs() < 1e-9)
        );
    }
}

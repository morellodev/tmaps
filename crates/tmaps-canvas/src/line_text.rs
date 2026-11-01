//! Label text that follows a line. The text breaks into runs, one for each straight part of the
//! line under it and each direction of the text, as in `ol/geom/flat/textpath.js` of
//! `OpenLayers`. A run draws in one call, so a label on a straight part of a line keeps the
//! kerning and the shaping of the browser. The check of the corners is that of
//! `src/symbol/check_max_angle.ts` of `MapLibre`.

use std::collections::VecDeque;
use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};
use std::ops::Range;

use tmaps_core::scene::{Rect, ScreenPoint};
use unicode_bidi::{Direction, ParagraphBidiInfo};

/// The most that a line can turn in [`WINDOW`], in radians: the default `text-max-angle` of
/// `MapLibre`.
const MAX_TURN: f64 = FRAC_PI_4;

/// The most that a run can turn past vertical, in radians. A run that turns more is upside
/// down, but a label on a wavy vertical line can have runs a little past vertical.
const MAX_TILT: f64 = PI / 18.0;

/// The length of line in which the turns add up, in text heights, as in `MapLibre`.
const WINDOW: f64 = 0.6;

/// The most that a segment can turn from the first segment of its straight part, in radians.
/// Lines often have extra points on a straight part, and a run must not break there: the
/// browser cannot join Arabic letters across two runs.
const STRAIGHT: f64 = PI / 60.0;

/// A grapheme cluster of a label text, its width in CSS pixels, and the direction of its text.
#[derive(Debug, Clone, PartialEq)]
pub struct Cluster {
    pub range: Range<usize>,
    pub width: f64,
    pub right_to_left: bool,
}

/// A part of a label text that draws in one call: the text in `range`, centered at `offset` from
/// the label position, and turned clockwise by `angle` radians.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub range: Range<usize>,
    pub offset: ScreenPoint,
    pub angle: f64,
    pub right_to_left: bool,
}

impl Run {
    /// The whole `text`, centered on the label position and not turned.
    #[must_use]
    pub fn level(text: &str, right_to_left: bool) -> Self {
        Self {
            range: 0..text.len(),
            offset: (0.0, 0.0),
            angle: 0.0,
            right_to_left,
        }
    }
}

/// Whether the first letter of `text` is in a right-to-left script, as the base direction of a
/// paragraph in UAX #9 (rules P2 and P3): <https://www.unicode.org/reports/tr9/#P2>.
#[must_use]
pub fn is_right_to_left(text: &str) -> bool {
    matches!(unicode_bidi::get_base_direction(text), Direction::Rtl)
}

/// Whether the base direction of `text` is right to left, and its `clusters` in their order on
/// the screen, from left to right. The order is that of the Unicode Bidirectional Algorithm
/// (UAX #9), so a number in a right-to-left text keeps its digits from left to right.
///
/// `clusters` are in the order of the text.
#[must_use]
pub fn in_visual_order(text: &str, mut clusters: Vec<Cluster>) -> (bool, Vec<Cluster>) {
    let bidi = ParagraphBidiInfo::new(text, None);
    let base = bidi.paragraph_level.is_rtl();
    if bidi.is_pure_ltr {
        return (base, clusters);
    }
    let (levels, runs) = bidi.visual_runs(0..text.len());
    let mut visual = Vec::with_capacity(clusters.len());
    for run in runs {
        let right_to_left = levels[run.start].is_rtl();
        let from = clusters.partition_point(|cluster| cluster.range.start < run.start);
        let to = clusters.partition_point(|cluster| cluster.range.start < run.end);
        let part = &mut clusters[from..to];
        for cluster in part.iter_mut() {
            cluster.right_to_left = right_to_left;
        }
        if right_to_left {
            visual.extend(part.iter().rev().cloned());
        } else {
            visual.extend(part.iter().cloned());
        }
    }
    (base, visual)
}

/// A label text along a line.
#[derive(Debug, Clone, PartialEq)]
pub struct LineText {
    pub runs: Vec<Run>,
    /// The box around each cluster on the screen, without padding.
    pub boxes: Vec<Rect>,
}

/// Lays `clusters` along `path`, centered `along` CSS pixels from its start. The clusters are
/// `height` high, and in their order on the screen from [`in_visual_order`]. The first cluster
/// goes at the left end of the text, so the text is not upside down. A text on one straight part
/// draws in one run, in the base direction `right_to_left`.
///
/// Returns `None` if the text goes past an end of the line, if the line turns too much under the
/// text, or if a part of the text is upside down. The window of the turn check is short, so a
/// long bend can pass it and still turn the end of the text upside down.
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
    let mut spans: Vec<Span> = Vec::new();
    let mut boxes = Vec::with_capacity(clusters.len());
    let mut offset = 0.0;
    for cluster in clusters {
        let center = on_line(offset + cluster.width / 2.0);
        let segment = path.segment_at(center);
        boxes.push(turned_box(
            path.point_at(center),
            path.angle(segment, flip),
            cluster.width,
            height,
        ));
        let part = path.parts[segment];
        let next = offset + cluster.width;
        match spans.last_mut() {
            Some(span) if span.takes(part, cluster) => {
                span.range = span.range.start.min(cluster.range.start)
                    ..span.range.end.max(cluster.range.end);
                span.to = next;
            }
            _ => spans.push(Span {
                part,
                range: cluster.range.clone(),
                right_to_left: cluster.right_to_left,
                from: offset,
                to: next,
            }),
        }
        offset = next;
    }
    // On one straight part, the browser orders a text of two directions itself.
    if let [first, .., last] = spans.as_slice()
        && first.part == last.part
    {
        spans = vec![Span {
            part: first.part,
            range: spans.iter().map(|span| span.range.start).min().unwrap_or(0)
                ..spans.iter().map(|span| span.range.end).max().unwrap_or(0),
            right_to_left,
            from: 0.0,
            to: width,
        }];
    }
    let origin = path.point_at(along);
    let runs: Vec<Run> = spans
        .into_iter()
        .map(|span| {
            let (a, b) = (
                path.point_at(on_line(span.from)),
                path.point_at(on_line(span.to)),
            );
            Run {
                range: span.range,
                offset: (
                    f64::midpoint(a.0, b.0) - origin.0,
                    f64::midpoint(a.1, b.1) - origin.1,
                ),
                angle: direction(a, b),
                right_to_left: span.right_to_left,
            }
        })
        .collect();
    let upright = |run: &Run| run.angle.abs() <= FRAC_PI_2 + MAX_TILT;
    runs.iter().all(upright).then_some(LineText { runs, boxes })
}

/// The clusters of a run, from offset `from` to `to` along the text, on one straight part of the
/// line.
struct Span {
    part: usize,
    range: Range<usize>,
    right_to_left: bool,
    from: f64,
    to: f64,
}

impl Span {
    /// Whether `cluster` on `part` can draw in the same call as the span: it has the same
    /// direction, and it comes next in the text.
    fn takes(&self, part: usize, cluster: &Cluster) -> bool {
        let next = if self.right_to_left {
            cluster.range.end == self.range.start
        } else {
            cluster.range.start == self.range.end
        };
        self.part == part && self.right_to_left == cluster.right_to_left && next
    }
}

/// A line, with the distance along it to each point.
pub struct Path {
    points: Vec<ScreenPoint>,
    /// The distance of each point from the first point.
    starts: Vec<f64>,
    /// The straight part of each segment, by number from the start of the line.
    parts: Vec<usize>,
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
        let mut parts = Vec::with_capacity(points.len().saturating_sub(1));
        let mut part = 0;
        let mut first = None;
        for (segment, pair) in points.windows(2).enumerate() {
            if starts[segment + 1] > starts[segment] {
                let angle = direction(pair[0], pair[1]);
                match first {
                    Some(first) if wrap(angle - first).abs() <= STRAIGHT => {}
                    Some(_) => {
                        part += 1;
                        first = Some(angle);
                    }
                    None => first = Some(angle),
                }
            }
            parts.push(part);
        }
        Self {
            points,
            starts,
            parts,
        }
    }

    pub fn length(&self) -> f64 {
        self.starts.last().copied().unwrap_or_default()
    }

    /// The index of the first segment that ends after `at`, or of the last segment. Needs two
    /// points or more.
    fn segment_at(&self, at: f64) -> usize {
        self.starts[1..]
            .partition_point(|&start| start <= at)
            .min(self.points.len() - 2)
    }

    /// The point at the distance `at` from the first point. Needs two points or more.
    pub fn point_at(&self, at: f64) -> ScreenPoint {
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
        if flip {
            direction(b, a)
        } else {
            direction(a, b)
        }
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

/// The direction from `a` to `b`, in radians clockwise from the x axis.
fn direction(a: ScreenPoint, b: ScreenPoint) -> f64 {
    (b.1 - a.1).atan2(b.0 - a.0)
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

#[cfg(test)]
mod tests {
    use unicode_segmentation::UnicodeSegmentation;

    use super::*;

    /// Each cluster is 10 pixels wide and high.
    fn lay(points: &[ScreenPoint], along: f64, text: &str) -> Option<LineText> {
        let clusters = text
            .grapheme_indices(true)
            .map(|(start, cluster)| Cluster {
                range: start..start + cluster.len(),
                width: 10.0,
                right_to_left: false,
            })
            .collect();
        let (right_to_left, clusters) = in_visual_order(text, clusters);
        let path = Path::new(points.iter().copied());
        lay_out(&path, along, &clusters, 10.0, right_to_left)
    }

    /// The point `length` pixels from `from`, in the direction `degrees` clockwise from the x axis.
    fn step(from: ScreenPoint, degrees: f64, length: f64) -> ScreenPoint {
        let (sin, cos) = degrees.to_radians().sin_cos();
        (from.0 + length * cos, from.1 + length * sin)
    }

    /// The range of each run, and its angle in whole degrees.
    fn runs(text: &LineText) -> Vec<(Range<usize>, i32)> {
        text.runs
            .iter()
            .map(|run| (run.range.clone(), degrees(run.angle)))
            .collect()
    }

    #[expect(clippy::cast_possible_truncation, reason = "test angles are small")]
    fn degrees(angle: f64) -> i32 {
        angle.to_degrees().round() as i32
    }

    fn directions(text: &LineText) -> Vec<(Range<usize>, bool)> {
        text.runs
            .iter()
            .map(|run| (run.range.clone(), run.right_to_left))
            .collect()
    }

    #[test]
    fn text_on_one_segment_is_one_run_on_the_line() {
        let text = lay(&[(0.0, 100.0), (100.0, 0.0)], 50.0, "Via Po").unwrap();
        assert_eq!(runs(&text), [(0..6, -45)]);
        let (x, y) = text.runs[0].offset;
        assert!(x.abs() < 1e-9 && y.abs() < 1e-9);
    }

    #[test]
    fn text_on_a_line_that_goes_left_is_not_upside_down() {
        let text = lay(&[(300.0, 100.0), (0.0, 100.0)], 150.0, "Via Po").unwrap();
        assert_eq!(runs(&text), [(0..6, 0)]);
    }

    #[test]
    fn text_breaks_into_one_run_for_each_straight_part() {
        let text = lay(
            &[(0.0, 0.0), (100.0, 0.0), (200.0, 10.0)],
            100.0,
            "Via Roma",
        )
        .unwrap();
        assert_eq!(runs(&text), [(0..4, 0), (4..8, 6)]);
    }

    #[test]
    fn text_over_a_point_on_a_straight_part_is_one_run() {
        let points = [(0.0, 0.0), (100.0, 0.0), (200.0, 2.0)];
        let text = lay(&points, 100.0, "محمد").unwrap();
        assert_eq!(directions(&text), [(0..8, true)]);
    }

    #[test]
    fn run_over_a_straight_part_follows_its_chord() {
        let text = lay(&[(0.0, 0.0), (100.0, 0.0), (200.0, 2.0)], 100.0, "Via Po").unwrap();
        let (x, y) = text.runs[0].offset;
        assert!(x.abs() < 0.01 && (y - 0.3).abs() < 0.01, "{x}, {y}");
    }

    #[test]
    fn number_in_right_to_left_text_keeps_its_digits_from_left_to_right() {
        let text = lay(&[(0.0, 0.0), (100.0, 0.0), (200.0, 10.0)], 95.0, "אב 1234").unwrap();
        assert_eq!(directions(&text), [(5..9, false), (0..5, true)]);
    }

    #[test]
    fn text_of_two_directions_on_a_straight_line_is_one_run_in_its_base_direction() {
        let name = "Rue 6 - شارع 6";
        let text = lay(&[(0.0, 0.0), (200.0, 0.0)], 100.0, name).unwrap();
        assert_eq!(directions(&text), [(0..name.len(), false)]);
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
    fn text_around_a_long_bend_is_left_out() {
        let mut points = vec![(0.0, 0.0)];
        for turn in 0..19 {
            let last = points[points.len() - 1];
            points.push(step(last, f64::from(turn) * 10.0, 5.0));
        }
        assert_eq!(lay(&points, 47.5, "Via Lodi"), None);
    }

    #[test]
    fn base_direction_comes_from_the_first_letter() {
        let found: Vec<_> = ["שדרות 6", "6 שדרות", "Rue 6 - شارع 6", "6"]
            .into_iter()
            .map(|text| (is_right_to_left(text), in_visual_order(text, Vec::new()).0))
            .collect();
        assert_eq!(
            found,
            [(true, true), (true, true), (false, false), (false, false)]
        );
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

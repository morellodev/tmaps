use std::collections::HashMap;

use tmaps_core::label::LabelClass;

/// The time that a label takes to fade in or out, in milliseconds.
const FADE_MS: f64 = 200.0;

/// The longest time step of a fade, in milliseconds. After an idle time, the first frame of a
/// fade comes long after the last frame, and a longer step would skip the fade.
const MAX_STEP_MS: f64 = 50.0;

/// The labels on the map, with their opacity. `P` is a position on the map.
pub struct Fades<P> {
    labels: HashMap<String, Fade<P>>,
    /// Labels that fade out at their old position, because their text moved to a new position.
    left: Vec<(String, Fade<P>)>,
    time: Option<f64>,
}

pub struct Fade<P> {
    pub class: LabelClass,
    pub at: P,
    pub opacity: f64,
    placed: bool,
}

impl<P> Default for Fades<P> {
    fn default() -> Self {
        Self {
            labels: HashMap::new(),
            left: Vec::new(),
            time: None,
        }
    }
}

impl<P> Fades<P> {
    /// The position of each text on the map, also of a text that fades out.
    pub fn positions(&self) -> impl Iterator<Item = (&str, &P)> {
        self.labels
            .iter()
            .map(|(text, fade)| (text.as_str(), &fade.at))
    }

    /// All labels, also the labels that fade out, so that the caller can update their positions.
    pub fn labels_mut(&mut self) -> impl Iterator<Item = &mut Fade<P>> {
        self.labels
            .values_mut()
            .chain(self.left.iter_mut().map(|(_, fade)| fade))
    }

    /// Sets the labels to fade in: the `placed` labels. The other labels fade out. A placed label
    /// is `kept` when it is at its last position. Else its text fades in at the new position, and
    /// fades out at the old one.
    pub fn place<'a>(&mut self, placed: impl IntoIterator<Item = (&'a str, LabelClass, P, bool)>) {
        for fade in self.labels.values_mut() {
            fade.placed = false;
        }
        for (text, class, at, kept) in placed {
            let new = Fade {
                class,
                at,
                opacity: 0.0,
                placed: true,
            };
            match self.labels.get_mut(text) {
                Some(fade) if kept => {
                    *fade = Fade {
                        opacity: fade.opacity,
                        ..new
                    };
                }
                Some(fade) => {
                    let old = std::mem::replace(fade, new);
                    self.left.push((text.to_owned(), old));
                }
                None => {
                    self.labels.insert(text.to_owned(), new);
                }
            }
        }
    }

    /// Moves the opacity of each label to its end. `now` is the frame time in milliseconds.
    ///
    /// Returns `true` while a fade is not complete.
    pub fn step(&mut self, now: f64) -> bool {
        let step = self
            .time
            .map_or(MAX_STEP_MS, |time| (now - time).clamp(0.0, MAX_STEP_MS))
            / FADE_MS;
        self.time = Some(now);
        for fade in self.labels_mut() {
            let change = if fade.placed { step } else { -step };
            fade.opacity = (fade.opacity + change).clamp(0.0, 1.0);
        }
        self.labels
            .retain(|_, fade| fade.placed || fade.opacity > 0.0);
        self.left.retain(|(_, fade)| fade.opacity > 0.0);
        !self.left.is_empty()
            || self
                .labels
                .values()
                .any(|fade| fade.opacity < 1.0 || !fade.placed)
    }

    /// The labels to draw: the placed labels and the labels that fade out.
    pub fn labels(&self) -> impl Iterator<Item = (&str, &Fade<P>)> {
        self.labels
            .iter()
            .map(|(text, fade)| (text.as_str(), fade))
            .chain(self.left.iter().map(|(text, fade)| (text.as_str(), fade)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update<'a>(
        fades: &mut Fades<u8>,
        placed: impl IntoIterator<Item = (&'a str, LabelClass, u8, bool)>,
        now: f64,
    ) -> bool {
        fades.place(placed);
        fades.step(now)
    }

    fn opacities(fades: &Fades<u8>, text: &str) -> Vec<(u8, f64)> {
        fades
            .labels()
            .filter(|&(each, _)| each == text)
            .map(|(_, fade)| (fade.at, fade.opacity))
            .collect()
    }

    #[test]
    fn placed_label_fades_in() {
        let mut fades = Fades::default();
        let roma = [("Roma", LabelClass::City, 0, true)];
        assert!(update(
            &mut fades,
            [("Roma", LabelClass::City, 0, false)],
            1000.0
        ));
        assert_eq!(opacities(&fades, "Roma"), [(0, 0.25)]);
        assert!(update(&mut fades, roma, 1050.0));
        assert!(update(&mut fades, roma, 1100.0));
        assert!(!update(&mut fades, roma, 1150.0));
        assert_eq!(opacities(&fades, "Roma"), [(0, 1.0)]);
    }

    #[test]
    fn label_that_is_not_placed_fades_out_and_leaves() {
        let mut fades = Fades::default();
        update(&mut fades, [("Roma", LabelClass::City, 0, false)], 0.0);
        assert!(update(&mut fades, [], 25.0));
        assert_eq!(opacities(&fades, "Roma"), [(0, 0.125)]);
        assert_eq!(fades.positions().count(), 1);
        assert!(!update(&mut fades, [], 50.0));
        assert_eq!(opacities(&fades, "Roma"), []);
        assert_eq!(fades.positions().count(), 0);
    }

    #[test]
    fn label_at_a_new_position_fades_in_there_and_out_at_the_old_one() {
        let mut fades = Fades::default();
        for time in [0.0, 50.0, 100.0, 150.0] {
            update(
                &mut fades,
                [("Roma", LabelClass::City, 0, time > 0.0)],
                time,
            );
        }
        assert!(update(
            &mut fades,
            [("Roma", LabelClass::City, 1, false)],
            200.0
        ));
        assert_eq!(opacities(&fades, "Roma"), [(1, 0.25), (0, 0.75)]);
        assert_eq!(fades.positions().collect::<Vec<_>>(), [("Roma", &1)]);
    }

    #[test]
    fn label_fades_in_between_placements() {
        let mut fades = Fades::default();
        fades.place([("Roma", LabelClass::City, 0, false)]);
        for time in [0.0, 50.0, 100.0] {
            assert!(fades.step(time));
        }
        assert!(!fades.step(150.0));
        assert_eq!(opacities(&fades, "Roma"), [(0, 1.0)]);
    }

    #[test]
    fn fade_after_an_idle_time_takes_short_steps() {
        let mut fades = Fades::default();
        update(&mut fades, [], 0.0);
        update(&mut fades, [("Roma", LabelClass::City, 0, false)], 60_000.0);
        assert_eq!(opacities(&fades, "Roma"), [(0, 0.25)]);
    }
}

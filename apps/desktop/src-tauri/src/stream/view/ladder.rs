//! Which of the stream's two layers a viewer takes (ADR-0065). Only time and the arrival of
//! pictures go in, only what to do comes out: the room is the caller's.
//!
//! LiveKit stops sending the full layer when the viewer's line cannot carry it, and does not
//! start again by itself. So a silence of the full layer means going to the light one, and
//! from there the full one is tried again now and then, with the light one still on screen.

use std::time::Duration;

/// No pictures for this long: the layer is not coming.
const QUIET: Duration = Duration::from_secs(3);
/// The first pictures of a layer take a key frame and a subscription.
const FIRST_PICTURE: Duration = Duration::from_secs(6);
/// A layer just asked for counts as having shown a picture this far ahead: with `QUIET`,
/// that makes `FIRST_PICTURE`.
const FIRST_PICTURE_AHEAD: Duration = Duration::from_secs(3);
/// The full layer has this long to bring its first picture during a try. A line that cannot
/// carry it is stopped by the server sooner than this.
const TRY_FIRST_PICTURE: Duration = Duration::from_secs(4);
/// Pictures of the full layer for this long and the try worked.
const TRY_LENGTH: Duration = Duration::from_secs(5);
/// A gap in pictures this long ends a try that had begun.
const TRY_QUIET: Duration = Duration::from_secs(2);
/// The first wait before a try, and the longest one: a failed try doubles it. A try that fails
/// is seen and heard, a few seconds of a still picture, so they are not frequent.
const FIRST_WAIT: Duration = Duration::from_secs(60);
const LONGEST_WAIT: Duration = Duration::from_secs(600);
/// The full layer holding for this long means the waits start over.
const HELD: Duration = Duration::from_secs(120);
/// How often the light layer is asked again when even it does not come.
const LOW_AGAIN: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// As the streamer chose.
    Full,
    Light,
}

/// What to do in the room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Subscribe(Layer),
    Unsubscribe(Layer),
    /// Tell the streamer whether this viewer needs the light layer.
    AskLight(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Full,
    Light {
        try_at: Duration,
    },
    Trying {
        began: Duration,
        first: Option<Duration>,
    },
}

/// One viewer's choice. `now` is any clock that only goes forward.
#[derive(Debug)]
pub struct Ladder {
    mode: Mode,
    /// Since when the full layer has been on screen.
    full_since: Duration,
    /// The time of the last picture, or when a picture can first be expected.
    full_until: Duration,
    light_until: Duration,
    /// Unsubscribed from a silent light layer, to subscribe at the next tick.
    light_off: bool,
    light_again_at: Duration,
    wait: Duration,
}

impl Ladder {
    /// Subscribed to the full layer just now.
    pub fn new(now: Duration) -> Self {
        Self {
            mode: Mode::Full,
            full_since: now,
            full_until: now + FIRST_PICTURE_AHEAD,
            light_until: now,
            light_off: false,
            light_again_at: now,
            wait: FIRST_WAIT,
        }
    }

    /// A picture came. Whether to show it: during a try, the full layer stays out of sight.
    pub fn picture(&mut self, layer: Layer, now: Duration) -> bool {
        match layer {
            Layer::Full => {
                self.full_until = self.full_until.max(now);
                if let Mode::Trying { first, .. } = &mut self.mode {
                    first.get_or_insert(now);
                }
                self.mode == Mode::Full
            }
            Layer::Light => {
                self.light_until = self.light_until.max(now);
                self.mode != Mode::Full
            }
        }
    }

    /// On the light layer, or about to be: what the panel says.
    pub const fn reduced(&self) -> bool {
        !matches!(self.mode, Mode::Full)
    }

    /// The light layer is what this viewer is after: for when its track shows up.
    pub const fn wants_light(&self) -> bool {
        self.reduced() && !self.light_off
    }

    /// Call about once a second. `jitter` spreads the tries of viewers on one line, so that
    /// they do not all overload it at the same moment.
    pub fn tick(&mut self, now: Duration, jitter: Duration) -> Vec<Step> {
        match self.mode {
            Mode::Full => self.tick_full(now, jitter),
            Mode::Light { try_at } => {
                if now >= try_at {
                    self.mode = Mode::Trying {
                        began: now,
                        first: None,
                    };
                    self.full_until = now;
                    let mut steps = vec![Step::Subscribe(Layer::Full)];
                    self.keep_light(now, &mut steps);
                    steps
                } else {
                    let mut steps = Vec::new();
                    self.keep_light(now, &mut steps);
                    steps
                }
            }
            Mode::Trying { began, first } => self.tick_try(now, jitter, began, first),
        }
    }

    fn tick_full(&mut self, now: Duration, jitter: Duration) -> Vec<Step> {
        if now.saturating_sub(self.full_until) < QUIET {
            return Vec::new();
        }
        if now.saturating_sub(self.full_since) >= HELD {
            self.wait = FIRST_WAIT;
        }
        self.mode = Mode::Light {
            try_at: now + self.wait + jitter,
        };
        self.light_until = now + FIRST_PICTURE_AHEAD;
        self.light_again_at = now + FIRST_PICTURE;
        vec![
            Step::AskLight(true),
            Step::Subscribe(Layer::Light),
            Step::Unsubscribe(Layer::Full),
        ]
    }

    fn tick_try(
        &mut self,
        now: Duration,
        jitter: Duration,
        began: Duration,
        first: Option<Duration>,
    ) -> Vec<Step> {
        let worked = first.is_some_and(|first| now.saturating_sub(first) >= TRY_LENGTH);
        let failed = match first {
            Some(_) => now.saturating_sub(self.full_until) >= TRY_QUIET,
            None => now.saturating_sub(began) >= TRY_FIRST_PICTURE,
        };
        if failed {
            self.wait = (self.wait * 2).min(LONGEST_WAIT);
            self.mode = Mode::Light {
                try_at: now + self.wait + jitter,
            };
            let mut steps = vec![Step::Unsubscribe(Layer::Full)];
            self.keep_light(now, &mut steps);
            steps
        } else if worked {
            self.mode = Mode::Full;
            self.full_since = now;
            self.light_off = false;
            vec![Step::Unsubscribe(Layer::Light), Step::AskLight(false)]
        } else {
            let mut steps = Vec::new();
            self.keep_light(now, &mut steps);
            steps
        }
    }

    /// The server stops the light layer too when even it does not fit, and a subscription
    /// made anew is what makes it try.
    fn keep_light(&mut self, now: Duration, steps: &mut Vec<Step>) {
        if self.light_off {
            self.light_off = false;
            self.light_until = now + FIRST_PICTURE_AHEAD;
            self.light_again_at = now + LOW_AGAIN;
            steps.push(Step::Subscribe(Layer::Light));
        } else if now.saturating_sub(self.light_until) >= QUIET && now >= self.light_again_at {
            self.light_off = true;
            steps.push(Step::Unsubscribe(Layer::Light));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn s(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    /// Ticks from `from` to `to`, a picture of `layers` each second; the steps that came.
    fn run(ladder: &mut Ladder, from: u64, to: u64, layers: &[Layer]) -> Vec<(u64, Step)> {
        let mut steps = Vec::new();
        for now in from..=to {
            for layer in layers {
                ladder.picture(*layer, s(now));
            }
            steps.extend(
                ladder
                    .tick(s(now), s(0))
                    .into_iter()
                    .map(|step| (now, step)),
            );
        }
        steps
    }

    /// A viewer who just went to the light layer at 10 seconds.
    fn on_light() -> Ladder {
        let mut ladder = Ladder::new(s(0));
        run(&mut ladder, 0, 7, &[Layer::Full]);
        assert_eq!(
            ladder.tick(s(10), s(0)),
            [
                Step::AskLight(true),
                Step::Subscribe(Layer::Light),
                Step::Unsubscribe(Layer::Full),
            ]
        );
        ladder
    }

    #[test]
    fn a_stream_that_comes_stays_on_the_full_layer() {
        let mut ladder = Ladder::new(s(0));
        assert_eq!(run(&mut ladder, 0, 600, &[Layer::Full]), []);
        assert!(!ladder.reduced());
        assert!(ladder.picture(Layer::Full, s(601)), "it is on screen");
    }

    #[test]
    fn the_first_picture_gets_its_time() {
        let mut ladder = Ladder::new(s(0));
        assert_eq!(
            run(&mut ladder, 0, 5, &[]),
            [],
            "a key frame can take a while"
        );
        assert_eq!(ladder.tick(s(6), s(0)).len(), 3, "this is too long");
        assert!(ladder.reduced());
    }

    #[test]
    fn silence_of_the_full_layer_means_the_light_one() {
        let mut ladder = on_light();
        assert!(ladder.reduced());
        assert!(ladder.wants_light());
        assert!(
            ladder.picture(Layer::Light, s(11)),
            "the light one is shown"
        );
    }

    #[test]
    fn a_try_is_out_of_sight_and_keeps_the_light_layer_until_it_works() {
        let mut ladder = on_light();
        assert_eq!(
            run(&mut ladder, 11, 69, &[Layer::Light]),
            [],
            "the first wait"
        );
        assert_eq!(
            run(&mut ladder, 70, 70, &[Layer::Light]),
            [(70, Step::Subscribe(Layer::Full))]
        );
        assert!(!ladder.picture(Layer::Full, s(71)), "not shown yet");
        assert!(ladder.picture(Layer::Light, s(71)));
        assert_eq!(
            run(&mut ladder, 71, 75, &[Layer::Full, Layer::Light]),
            [],
            "not long enough"
        );
        assert_eq!(
            run(&mut ladder, 76, 76, &[Layer::Full, Layer::Light]),
            [
                (76, Step::Unsubscribe(Layer::Light)),
                (76, Step::AskLight(false))
            ]
        );
        assert!(!ladder.reduced());
        assert!(ladder.picture(Layer::Full, s(77)), "now the full one is");
    }

    #[test]
    fn a_failed_try_goes_back_and_waits_longer() {
        let mut ladder = on_light();
        run(&mut ladder, 11, 70, &[Layer::Light]);
        assert_eq!(
            run(&mut ladder, 71, 74, &[Layer::Light]),
            [(74, Step::Unsubscribe(Layer::Full))],
            "no picture came"
        );
        assert_eq!(
            run(&mut ladder, 75, 193, &[Layer::Light]),
            [],
            "twice as long now"
        );
        assert_eq!(
            run(&mut ladder, 194, 194, &[Layer::Light]),
            [(194, Step::Subscribe(Layer::Full))]
        );
    }

    #[test]
    fn pictures_that_stop_end_a_try() {
        let mut ladder = on_light();
        run(&mut ladder, 11, 70, &[Layer::Light]);
        run(&mut ladder, 71, 72, &[Layer::Full, Layer::Light]);
        assert_eq!(
            run(&mut ladder, 73, 74, &[Layer::Light]),
            [(74, Step::Unsubscribe(Layer::Full))],
            "the server stopped it"
        );
    }

    #[test]
    fn the_wait_has_a_limit_and_starts_over_after_the_full_layer_held() {
        let mut ladder = on_light();
        let mut now = 11;
        for _ in 0..6 {
            // Up to and through a try that brings nothing.
            while !run(&mut ladder, now, now, &[Layer::Light])
                .iter()
                .any(|(_, step)| *step == Step::Unsubscribe(Layer::Full))
            {
                now += 1;
            }
            now += 1;
        }
        assert_eq!(ladder.wait, LONGEST_WAIT);

        let at = now;
        let try_at = (at..at + 700)
            .find(|t| !run(&mut ladder, *t, *t, &[Layer::Light]).is_empty())
            .expect("another try");
        assert_eq!(try_at, at - 1 + 600, "no longer than the limit");
        run(
            &mut ladder,
            try_at + 1,
            try_at + 6,
            &[Layer::Full, Layer::Light],
        );
        assert!(!ladder.reduced(), "this one worked");

        let long = try_at + 6 + 120;
        run(&mut ladder, try_at + 7, long, &[Layer::Full]);
        ladder.tick(s(long + 3), s(0));
        assert_eq!(ladder.wait, FIRST_WAIT);
    }

    #[test]
    fn a_full_layer_that_keeps_failing_does_not_shorten_the_wait() {
        let mut ladder = on_light();
        run(&mut ladder, 11, 70, &[Layer::Light]);
        run(&mut ladder, 71, 74, &[Layer::Light]);
        assert_eq!(ladder.wait, s(120));
        run(&mut ladder, 75, 194, &[Layer::Light]);
        run(&mut ladder, 195, 200, &[Layer::Full, Layer::Light]);
        assert!(!ladder.reduced());
        run(&mut ladder, 201, 209, &[Layer::Full]);
        ladder.tick(s(212), s(0));
        assert!(ladder.reduced(), "it fell again in no time");
        assert_eq!(ladder.wait, s(120), "so the wait stays");
    }

    #[test]
    fn a_light_layer_that_stops_is_asked_again() {
        let mut ladder = on_light();
        // Asked at 10; its first picture has until 16.
        assert_eq!(run(&mut ladder, 11, 15, &[]), []);
        assert_eq!(
            run(&mut ladder, 16, 17, &[]),
            [
                (16, Step::Unsubscribe(Layer::Light)),
                (17, Step::Subscribe(Layer::Light))
            ]
        );
        assert_eq!(run(&mut ladder, 18, 26, &[]), [], "not too often");
        assert_eq!(
            run(&mut ladder, 27, 27, &[]),
            [(27, Step::Unsubscribe(Layer::Light))]
        );
    }

    #[test]
    fn jitter_moves_the_try() {
        let mut ladder = Ladder::new(s(0));
        run(&mut ladder, 0, 7, &[Layer::Full]);
        ladder.tick(s(10), s(7));
        assert_eq!(run(&mut ladder, 11, 76, &[Layer::Light]), []);
        assert_eq!(
            run(&mut ladder, 77, 77, &[Layer::Light]),
            [(77, Step::Subscribe(Layer::Full))]
        );
    }
}

//! Frame timeline: maps recording time to frame ticks after trim, idle limit and speed.

use mt_record::{Event, EventKind};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    pub fps: u32,
    pub speed: f64,
    /// Seconds; 0 = off.
    pub idle_limit: f64,
    pub trim_start: f64,
    pub trim_end: f64,
}

/// One event the replay applies. `tick` is `None` for events before `trim_start`: they build
/// the first frame's screen without being shown.
#[derive(Debug, Clone, PartialEq)]
pub struct Step<'a> {
    pub tick: Option<u64>,
    pub kind: &'a EventKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Timeline<'a> {
    pub steps: Vec<Step<'a>>,
    /// Virtual end of the recording in seconds (after trim, idle limit and speed).
    pub end: f64,
}

impl Timing {
    pub fn tick_of(&self, secs: f64) -> u64 {
        (secs * f64::from(self.fps) + 1e-9).floor() as u64
    }

    /// Centiseconds from tick 0 to `secs`, rounded (frame delays carry the remainder).
    pub fn cs(secs: f64) -> u64 {
        (secs * 100.0).round() as u64
    }
}

/// Output and resize events on the virtual clock. Input and markers are not drawn.
pub fn timeline<'a>(events: &'a [Event], t: &Timing) -> Timeline<'a> {
    let last = events.last().map_or(0.0, |e| e.time);
    let end_real = (last - t.trim_end).max(t.trim_start);
    let mut steps = Vec::new();
    let (mut prev, mut v) = (t.trim_start, 0.0f64);
    for e in events {
        if !matches!(e.kind, EventKind::Output(_) | EventKind::Resize { .. }) {
            continue;
        }
        if e.time > end_real {
            break;
        }
        if e.time < t.trim_start {
            steps.push(Step {
                tick: None,
                kind: &e.kind,
            });
            continue;
        }
        v += gap(e.time - prev, t);
        prev = e.time;
        steps.push(Step {
            tick: Some(t.tick_of(v)),
            kind: &e.kind,
        });
    }
    let end = v + gap(end_real - prev, t);
    Timeline { steps, end }
}

fn gap(real: f64, t: &Timing) -> f64 {
    let real = real.max(0.0);
    let capped = if t.idle_limit > 0.0 {
        real.min(t.idle_limit)
    } else {
        real
    };
    capped / t.speed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(time: f64) -> Event {
        Event {
            time,
            kind: EventKind::Output("x".into()),
        }
    }

    fn timing() -> Timing {
        Timing {
            fps: 10,
            speed: 1.0,
            idle_limit: 2.0,
            trim_start: 0.0,
            trim_end: 0.0,
        }
    }

    fn ticks(tl: &Timeline) -> Vec<Option<u64>> {
        tl.steps.iter().map(|s| s.tick).collect()
    }

    #[test]
    fn idle_limit_caps_pauses() {
        let ev = [out(0.0), out(0.5), out(30.5), out(31.0)];
        let tl = timeline(&ev, &timing());
        assert_eq!(ticks(&tl), [Some(0), Some(5), Some(25), Some(30)]);
        assert!((tl.end - 3.0).abs() < 1e-9);
        let off = Timing {
            idle_limit: 0.0,
            ..timing()
        };
        assert_eq!(timeline(&ev, &off).steps[2].tick, Some(305));
    }

    #[test]
    fn speed_scales_time() {
        let ev = [out(0.0), out(1.0), out(2.0)];
        let fast = Timing {
            speed: 2.0,
            ..timing()
        };
        assert_eq!(ticks(&timeline(&ev, &fast)), [Some(0), Some(5), Some(10)]);
    }

    #[test]
    fn trim_hides_the_start_and_drops_the_end() {
        let ev = [out(0.0), out(1.0), out(2.0), out(3.0), out(4.0)];
        let t = Timing {
            trim_start: 1.5,
            trim_end: 1.5,
            ..timing()
        };
        let tl = timeline(&ev, &t);
        assert_eq!(ticks(&tl), [None, None, Some(5)]);
        assert!((tl.end - 1.0).abs() < 1e-9);
    }

    #[test]
    fn input_and_markers_are_skipped() {
        let ev = [
            out(0.0),
            Event {
                time: 0.1,
                kind: EventKind::Input("a".into()),
            },
            Event {
                time: 0.2,
                kind: EventKind::Marker("m".into()),
            },
        ];
        assert_eq!(timeline(&ev, &timing()).steps.len(), 1);
    }
}

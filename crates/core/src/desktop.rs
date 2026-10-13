//! Screen-edge geometry for two-machine switching.
//!
//! All coordinates are Quartz global display coordinates: origin at the
//! top-left of the main display, y down, in points. A `Desktop` is the set of
//! active display rects; positions are kept inside them and crossing is
//! detected against the outermost left/right extent.

use osm_model::{LogicalPoint, LogicalRect};

/// Distance from the edge where an entering or returning cursor is placed,
/// so the very next movement does not immediately cross again.
pub const EDGE_INSET: f64 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Desktop {
    displays: Vec<LogicalRect>,
    bounds: LogicalRect,
}

impl Desktop {
    /// Returns `None` when there is no usable display.
    pub fn new(displays: Vec<LogicalRect>) -> Option<Self> {
        let displays: Vec<_> = displays
            .into_iter()
            .filter(|r| r.w > 0.0 && r.h > 0.0)
            .collect();
        let first = *displays.first()?;
        let (mut l, mut t, mut r, mut b) =
            (first.left(), first.top(), first.right(), first.bottom());
        for d in &displays[1..] {
            l = l.min(d.left());
            t = t.min(d.top());
            r = r.max(d.right());
            b = b.max(d.bottom());
        }
        let bounds = LogicalRect {
            x: l,
            y: t,
            w: r - l,
            h: b - t,
        };
        Some(Self { displays, bounds })
    }

    pub fn bounds(&self) -> LogicalRect {
        self.bounds
    }

    pub fn displays(&self) -> &[LogicalRect] {
        &self.displays
    }

    /// The OS pins the cursor at the outermost pixel, so "at the edge" means
    /// within one point of the extent while still moving outwards.
    pub fn at_edge(&self, side: Side, p: LogicalPoint, dx: f64) -> bool {
        match side {
            Side::Left => dx < 0.0 && p.x <= self.bounds.left() + 1.0,
            Side::Right => dx > 0.0 && p.x >= self.bounds.right() - 1.0,
        }
    }

    /// For a tracked (injected) cursor: whether moving to `p` leaves the
    /// desktop through `side`.
    pub fn beyond_edge(&self, side: Side, p: LogicalPoint) -> bool {
        match side {
            Side::Left => p.x < self.bounds.left(),
            Side::Right => p.x >= self.bounds.right(),
        }
    }

    /// Vertical position as a fraction of the desktop height, in `[0, 1]`.
    pub fn y_ratio(&self, p: LogicalPoint) -> f64 {
        ((p.y - self.bounds.top()) / self.bounds.h).clamp(0.0, 1.0)
    }

    /// Where a cursor arriving through `side` at `y_ratio` should appear.
    pub fn entry_point(&self, side: Side, y_ratio: f64) -> LogicalPoint {
        let ratio = if y_ratio.is_finite() {
            y_ratio.clamp(0.0, 1.0)
        } else {
            0.5
        };
        let x = match side {
            Side::Left => self.bounds.left() + EDGE_INSET,
            Side::Right => self.bounds.right() - 1.0 - EDGE_INSET,
        };
        let y = self.bounds.top() + ratio * self.bounds.h;
        self.clamp(LogicalPoint { x, y })
    }

    /// Nearest point that lies on an actual display. Displays of different
    /// sizes leave gaps in the bounding box that the cursor must not enter.
    pub fn clamp(&self, p: LogicalPoint) -> LogicalPoint {
        let mut best = p;
        let mut best_dist = f64::INFINITY;
        for d in &self.displays {
            let c = LogicalPoint {
                x: p.x.clamp(d.left(), d.right() - 1.0),
                y: p.y.clamp(d.top(), d.bottom() - 1.0),
            };
            let dist = (c.x - p.x).powi(2) + (c.y - p.y).powi(2);
            if dist < best_dist {
                best_dist = dist;
                best = c;
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> LogicalRect {
        LogicalRect { x, y, w, h }
    }

    fn pt(x: f64, y: f64) -> LogicalPoint {
        LogicalPoint { x, y }
    }

    fn laptop() -> Desktop {
        Desktop::new(vec![rect(0.0, 0.0, 1512.0, 982.0)]).unwrap()
    }

    #[test]
    fn empty_desktop_is_none() {
        assert!(Desktop::new(vec![]).is_none());
        assert!(Desktop::new(vec![rect(0.0, 0.0, 0.0, 10.0)]).is_none());
    }

    #[test]
    fn edge_requires_outward_motion() {
        let d = laptop();
        assert!(d.at_edge(Side::Right, pt(1511.0, 500.0), 3.0));
        assert!(!d.at_edge(Side::Right, pt(1511.0, 500.0), -3.0));
        assert!(!d.at_edge(Side::Right, pt(1400.0, 500.0), 3.0));
        assert!(d.at_edge(Side::Left, pt(0.0, 10.0), -1.0));
        assert!(!d.at_edge(Side::Left, pt(0.0, 10.0), 1.0));
    }

    #[test]
    fn entry_point_is_inset_and_keeps_ratio() {
        let d = laptop();
        let p = d.entry_point(Side::Left, 0.5);
        assert_eq!(p, pt(EDGE_INSET, 491.0));
        let p = d.entry_point(Side::Right, 1.0);
        assert_eq!(p, pt(1511.0 - EDGE_INSET, 981.0));
        // Entering does not count as being at the edge again.
        assert!(!d.at_edge(Side::Right, p, 1.0));
    }

    #[test]
    fn entry_point_survives_bad_ratio() {
        let p = laptop().entry_point(Side::Left, f64::NAN);
        assert_eq!(p.y, 491.0);
    }

    #[test]
    fn y_ratio_round_trips_between_different_heights() {
        let small = laptop();
        let big = Desktop::new(vec![rect(0.0, 0.0, 2560.0, 1440.0)]).unwrap();
        let ratio = small.y_ratio(pt(1511.0, 245.5));
        let p = big.entry_point(Side::Left, ratio);
        assert!((p.y - 360.0).abs() < 0.01);
    }

    #[test]
    fn clamp_avoids_gap_between_unequal_displays() {
        // Laptop with a taller external display on its right, top-aligned.
        let d = Desktop::new(vec![
            rect(0.0, 0.0, 1512.0, 982.0),
            rect(1512.0, 0.0, 2560.0, 1440.0),
        ])
        .unwrap();
        assert_eq!(d.bounds(), rect(0.0, 0.0, 4072.0, 1440.0));
        // Below the laptop is empty space; the cursor snaps back onto a screen.
        let p = d.clamp(pt(100.0, 1200.0));
        assert_eq!(p, pt(100.0, 981.0));
        // A point on the external display is untouched.
        assert_eq!(d.clamp(pt(2000.0, 1200.0)), pt(2000.0, 1200.0));
    }

    #[test]
    fn beyond_edge_uses_outer_extent() {
        let d = Desktop::new(vec![
            rect(-1920.0, 0.0, 1920.0, 1080.0),
            rect(0.0, 0.0, 1512.0, 982.0),
        ])
        .unwrap();
        assert!(d.beyond_edge(Side::Left, pt(-1921.0, 10.0)));
        assert!(!d.beyond_edge(Side::Left, pt(-1.0, 10.0)));
        assert!(d.beyond_edge(Side::Right, pt(1512.0, 10.0)));
    }
}

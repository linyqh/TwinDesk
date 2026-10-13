//! Coordinate contract (report §5.2).
//!
//! Core works in one logical desktop space per device: origin top-left, x to
//! the right, y down, values may be negative. Each display's adapter owns the
//! transform to native API coordinates; AppKit points, Quartz coordinates and
//! physical pixels never mix in here. On the wire a position is a display id
//! plus a normalized point in `[0, 1)`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogicalPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl LogicalRect {
    pub fn left(&self) -> f64 {
        self.x
    }
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    pub fn top(&self) -> f64 {
        self.y
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    /// Half-open containment: the right and bottom edges belong to the
    /// neighbour, so adjacent rects never both contain a point.
    pub fn contains(&self, p: LogicalPoint) -> bool {
        p.x >= self.left() && p.x < self.right() && p.y >= self.top() && p.y < self.bottom()
    }
}

#[derive(Debug, Error, PartialEq)]
#[error("normalized coordinate out of range [0, 1): ({x}, {y})")]
pub struct OutOfRange {
    pub x: f64,
    pub y: f64,
}

/// A position inside one display, independent of its resolution and scale.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NormalizedPoint {
    x: f64,
    y: f64,
}

impl NormalizedPoint {
    pub fn new(x: f64, y: f64) -> Result<Self, OutOfRange> {
        let ok = |v: f64| (0.0..1.0).contains(&v);
        if ok(x) && ok(y) {
            Ok(Self { x, y })
        } else {
            Err(OutOfRange { x, y })
        }
    }

    pub fn x(&self) -> f64 {
        self.x
    }
    pub fn y(&self) -> f64 {
        self.y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_containment_is_half_open() {
        let r = LogicalRect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        };
        assert!(r.contains(LogicalPoint { x: 0.0, y: 0.0 }));
        assert!(!r.contains(LogicalPoint { x: 10.0, y: 5.0 }));
    }

    #[test]
    fn normalized_point_rejects_one_and_nan() {
        assert!(NormalizedPoint::new(0.999, 0.0).is_ok());
        assert!(NormalizedPoint::new(1.0, 0.0).is_err());
        assert!(NormalizedPoint::new(f64::NAN, 0.0).is_err());
    }
}

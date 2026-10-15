//! Target-local Caps Lock decisions. Time and OS state are supplied by callers.

use std::time::Duration;

pub const HOLD_THRESHOLD: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapsAction {
    ShortPress,
    ToggleLock,
}

#[derive(Debug, Default)]
pub struct CapsGesture {
    pressed: Option<Duration>,
    fired: bool,
}

impl CapsGesture {
    pub fn edge(&mut self, down: bool, now: Duration) -> Option<CapsAction> {
        if down {
            if self.pressed.is_none() {
                self.pressed = Some(now);
                self.fired = false;
            }
            None
        } else {
            let started = self.pressed.take()?;
            if self.fired {
                None
            } else if now.saturating_sub(started) >= HOLD_THRESHOLD {
                Some(CapsAction::ToggleLock)
            } else {
                Some(CapsAction::ShortPress)
            }
        }
    }

    pub fn poll(&mut self, now: Duration) -> Option<CapsAction> {
        if !self.fired && now.saturating_sub(self.pressed?) >= HOLD_THRESHOLD {
            self.fired = true;
            Some(CapsAction::ToggleLock)
        } else {
            None
        }
    }

    pub fn cancel(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Clone)]
pub struct InputSource {
    pub id: String,
    pub ascii: bool,
}

/// Last used source in each group; IDs are always revalidated against the
/// target's enabled sources, never received from the controlling computer.
#[derive(Debug, Default)]
pub struct SourceHistory {
    ascii: Option<String>,
    non_ascii: Option<String>,
}

impl SourceHistory {
    pub fn target(&mut self, current: &str, sources: &[InputSource]) -> Option<String> {
        let active = sources.iter().find(|s| s.id == current)?;
        let (remember, previous) = if active.ascii {
            (&mut self.ascii, &self.non_ascii)
        } else {
            (&mut self.non_ascii, &self.ascii)
        };
        *remember = Some(active.id.clone());
        sources
            .iter()
            .find(|s| s.ascii != active.ascii && previous.as_ref() == Some(&s.id))
            .or_else(|| sources.iter().find(|s| s.ascii != active.ascii))
            .map(|s| s.id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn short_press_runs_once_on_release_and_ignores_repeat() {
        let mut g = CapsGesture::default();
        assert_eq!(g.edge(true, ms(0)), None);
        assert_eq!(g.edge(true, ms(50)), None);
        assert_eq!(g.edge(false, ms(100)), Some(CapsAction::ShortPress));
        assert_eq!(g.edge(false, ms(110)), None);
    }

    #[test]
    fn hold_toggles_once_without_switching_source_on_release() {
        let mut g = CapsGesture::default();
        g.edge(true, ms(0));
        assert_eq!(g.poll(ms(499)), None);
        assert_eq!(g.poll(ms(500)), Some(CapsAction::ToggleLock));
        assert_eq!(g.poll(ms(800)), None);
        assert_eq!(g.edge(false, ms(900)), None);
        g.edge(true, ms(1000));
        assert_eq!(g.edge(false, ms(1600)), Some(CapsAction::ToggleLock));
    }

    #[test]
    fn disconnect_cancels_pending_press() {
        let mut g = CapsGesture::default();
        g.edge(true, ms(0));
        g.cancel();
        assert_eq!(g.poll(ms(1000)), None);
        assert_eq!(g.edge(false, ms(1100)), None);
    }

    #[test]
    fn source_selection_uses_target_history_and_handles_removed_sources() {
        let sources: Vec<_> = [("ABC", true), ("Pinyin", false), ("WeType", false)]
            .into_iter()
            .map(|(id, ascii)| InputSource {
                id: id.into(),
                ascii,
            })
            .collect();
        let mut h = SourceHistory::default();
        assert_eq!(h.target("WeType", &sources).as_deref(), Some("ABC"));
        assert_eq!(h.target("ABC", &sources).as_deref(), Some("WeType"));
        assert_eq!(h.target("ABC", &sources[..2]).as_deref(), Some("Pinyin"));
        assert_eq!(h.target("ABC", &sources[..1]), None);
        assert_eq!(h.target("unknown", &sources), None);
    }
}

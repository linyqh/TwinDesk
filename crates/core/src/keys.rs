//! macOS virtual keycodes that need special handling when replaying input.
//!
//! Modifier keys arrive as `kCGEventFlagsChanged` with no up/down bit; the
//! direction is whether the key's flag is set in the new flags.

pub const KEY_ESCAPE: u16 = 53;

pub const FLAG_ALPHA_SHIFT: u64 = 0x0001_0000;
pub const FLAG_SHIFT: u64 = 0x0002_0000;
pub const FLAG_CONTROL: u64 = 0x0004_0000;
pub const FLAG_ALTERNATE: u64 = 0x0008_0000;
pub const FLAG_COMMAND: u64 = 0x0010_0000;
pub const FLAG_SECONDARY_FN: u64 = 0x0080_0000;

/// Flag bit owned by a modifier keycode, or `None` for ordinary keys.
pub fn modifier_flag(keycode: u16) -> Option<u64> {
    match keycode {
        56 | 60 => Some(FLAG_SHIFT),
        59 | 62 => Some(FLAG_CONTROL),
        58 | 61 => Some(FLAG_ALTERNATE),
        55 | 54 => Some(FLAG_COMMAND),
        57 => Some(FLAG_ALPHA_SHIFT),
        63 => Some(FLAG_SECONDARY_FN),
        _ => None,
    }
}

/// Direction of a flags-changed event for `keycode`, given the new flags.
pub fn modifier_is_down(keycode: u16, flags: u64) -> bool {
    modifier_flag(keycode).is_some_and(|f| flags & f != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_and_right_shift_share_a_flag() {
        assert_eq!(modifier_flag(56), modifier_flag(60));
        assert!(modifier_is_down(56, FLAG_SHIFT | FLAG_COMMAND));
        assert!(!modifier_is_down(56, FLAG_COMMAND));
    }

    #[test]
    fn ordinary_keys_are_not_modifiers() {
        assert_eq!(modifier_flag(0), None); // 'a'
        assert_eq!(modifier_flag(KEY_ESCAPE), None);
        assert!(!modifier_is_down(0, u64::MAX));
    }
}

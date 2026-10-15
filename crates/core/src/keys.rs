//! macOS virtual keycodes that need special handling when replaying input.
//!
//! Modifier keys arrive as `kCGEventFlagsChanged` with no up/down bit; the
//! direction is whether the key's flag is set in the new flags.

pub const KEY_ESCAPE: u16 = 53;
pub const KEY_CAPS_LOCK: u16 = 57;
pub const KEY_FUNCTION: u16 = 63;

pub const FLAG_ALPHA_SHIFT: u64 = 0x0001_0000;
pub const FLAG_SHIFT: u64 = 0x0002_0000;
pub const FLAG_CONTROL: u64 = 0x0004_0000;
pub const FLAG_ALTERNATE: u64 = 0x0008_0000;
pub const FLAG_COMMAND: u64 = 0x0010_0000;
pub const FLAG_SECONDARY_FN: u64 = 0x0080_0000;
/// Physical Caps Lock state, distinct from the latched AlphaShift state.
pub const FLAG_CAPS_LOCK_HELD: u64 = 0x0100_0080;

/// Preserve the target's lock state, never copy it from the controller.
pub fn target_caps_flags(remote: u64, target: u64) -> u64 {
    (remote & !(FLAG_ALPHA_SHIFT | FLAG_CAPS_LOCK_HELD)) | (target & FLAG_ALPHA_SHIFT)
}

/// Flag bit owned by a modifier keycode, or `None` for ordinary keys.
pub fn modifier_flag(keycode: u16) -> Option<u64> {
    match keycode {
        56 | 60 => Some(FLAG_SHIFT),
        59 | 62 => Some(FLAG_CONTROL),
        58 | 61 => Some(FLAG_ALTERNATE),
        55 | 54 => Some(FLAG_COMMAND),
        KEY_CAPS_LOCK => Some(FLAG_CAPS_LOCK_HELD),
        KEY_FUNCTION => Some(FLAG_SECONDARY_FN),
        _ => None,
    }
}

/// Direction of a flags-changed event for `keycode`, given the new flags.
pub fn modifier_is_down(keycode: u16, flags: u64) -> bool {
    modifier_flag(keycode).is_some_and(|f| flags & f != 0)
}

/// A key event as captured, before forwarding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyStroke {
    pub keycode: u16,
    pub down: bool,
    pub repeat: bool,
    pub flags: u64,
}

/// Undoes the local Fn remap while forwarding.
///
/// Physical Fn is remapped to `remapped_keycode` (F19) below the event tap
/// chain so local apps never see it. This turns that key back into Fn for
/// the peer and keeps the Fn flag on every key pressed while it is held,
/// since the HID system no longer sets it once Fn is remapped.
#[derive(Clone, Copy, Debug, Default)]
pub struct FnRestore {
    held: bool,
}

impl FnRestore {
    pub fn held(&self) -> bool {
        self.held
    }

    pub fn reset(&mut self) {
        self.held = false;
    }

    pub fn apply(&mut self, k: KeyStroke, remapped_keycode: u16) -> KeyStroke {
        if k.keycode == remapped_keycode {
            if !k.repeat {
                self.held = k.down;
            }
            let flags = if k.down {
                k.flags | FLAG_SECONDARY_FN
            } else {
                k.flags & !FLAG_SECONDARY_FN
            };
            return KeyStroke {
                keycode: KEY_FUNCTION,
                down: k.down,
                repeat: k.repeat,
                flags,
            };
        }
        if self.held {
            return KeyStroke {
                flags: k.flags | FLAG_SECONDARY_FN,
                ..k
            };
        }
        k
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const F19: u16 = 80;

    fn key(keycode: u16, down: bool, flags: u64) -> KeyStroke {
        KeyStroke {
            keycode,
            down,
            repeat: false,
            flags,
        }
    }

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

    #[test]
    fn fn_is_a_modifier() {
        assert!(modifier_is_down(KEY_FUNCTION, FLAG_SECONDARY_FN));
        assert!(!modifier_is_down(KEY_FUNCTION, 0));
    }

    #[test]
    fn caps_lock_pressed_state_is_not_the_latched_lock_state() {
        assert!(!modifier_is_down(KEY_CAPS_LOCK, FLAG_ALPHA_SHIFT));
        assert!(modifier_is_down(KEY_CAPS_LOCK, FLAG_CAPS_LOCK_HELD));
        assert!(modifier_is_down(
            KEY_CAPS_LOCK,
            FLAG_CAPS_LOCK_HELD | FLAG_ALPHA_SHIFT
        ));
    }

    #[test]
    fn controller_caps_state_never_overrides_target_lock() {
        assert_eq!(
            target_caps_flags(FLAG_ALPHA_SHIFT | FLAG_SHIFT | FLAG_CAPS_LOCK_HELD, 0),
            FLAG_SHIFT
        );
        assert_eq!(
            target_caps_flags(FLAG_COMMAND, FLAG_ALPHA_SHIFT),
            FLAG_COMMAND | FLAG_ALPHA_SHIFT
        );
        assert_eq!(target_caps_flags(0, FLAG_ALPHA_SHIFT), FLAG_ALPHA_SHIFT);
    }

    #[test]
    fn remapped_key_becomes_fn_with_flag() {
        let mut r = FnRestore::default();
        let down = r.apply(key(F19, true, 0x100), F19);
        assert_eq!(down, key(KEY_FUNCTION, true, 0x100 | FLAG_SECONDARY_FN));
        assert!(r.held());
        let up = r.apply(key(F19, false, 0x100 | FLAG_SECONDARY_FN), F19);
        assert_eq!(up, key(KEY_FUNCTION, false, 0x100));
        assert!(!r.held());
    }

    #[test]
    fn keys_pressed_while_fn_held_carry_the_flag() {
        let mut r = FnRestore::default();
        r.apply(key(F19, true, 0), F19);
        let a = r.apply(key(0, true, 0), F19);
        assert_eq!(a.flags, FLAG_SECONDARY_FN);
        r.apply(key(F19, false, 0), F19);
        assert_eq!(r.apply(key(0, true, 0), F19).flags, 0);
    }

    #[test]
    fn repeat_does_not_toggle_held() {
        let mut r = FnRestore::default();
        r.apply(key(F19, true, 0), F19);
        r.apply(
            KeyStroke {
                keycode: F19,
                down: true,
                repeat: true,
                flags: 0,
            },
            F19,
        );
        assert!(r.held());
    }

    #[test]
    fn other_keys_untouched_when_fn_not_held() {
        let mut r = FnRestore::default();
        let k = key(12, true, FLAG_COMMAND);
        assert_eq!(r.apply(k, F19), k);
    }
}

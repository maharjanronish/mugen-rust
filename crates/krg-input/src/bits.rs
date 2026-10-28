//! Ikemen GO `InputBits` port — verified against `src/input.go:358-375`.
//!
//! ```go
//! type InputBits int16
//! const (
//!     IB_PU InputBits = 1 << iota  // 1<<0 pad up
//!     IB_PD                         // 1<<1 pad down
//!     IB_PL                         // 1<<2 pad left
//!     IB_PR                         // 1<<3 pad right
//!     IB_A                          // 1<<4
//!     IB_B                          // 1<<5
//!     IB_C                          // 1<<6
//!     IB_X                          // 1<<7
//!     IB_Y                          // 1<<8
//!     IB_Z                          // 1<<9
//!     IB_S                          // 1<<10
//!     IB_D                          // 1<<11 (Ikemen extra button)
//!     IB_W                          // 1<<12 (Ikemen extra button)
//!     IB_M                          // 1<<13 (Ikemen extra button)
//! )
//! ```
//!
//! Directions are PHYSICAL (up/down/left/right on the pad) — the engine's
//! command layer converts L/R into F/B relative to the character's facing,
//! exactly like MUGEN.

/// Pad up.
pub const PU: u32 = 1 << 0;
/// Pad down.
pub const PD: u32 = 1 << 1;
/// Pad left.
pub const PL: u32 = 1 << 2;
/// Pad right.
pub const PR: u32 = 1 << 3;

/// Kick buttons.
pub const A: u32 = 1 << 4;
pub const B: u32 = 1 << 5;
pub const C: u32 = 1 << 6;
/// Punch buttons.
pub const X: u32 = 1 << 7;
pub const Y: u32 = 1 << 8;
pub const Z: u32 = 1 << 9;
/// Start.
pub const S: u32 = 1 << 10;
/// Ikemen GO extra buttons.
pub const D: u32 = 1 << 11;
pub const W: u32 = 1 << 12;
pub const M: u32 = 1 << 13;

/// `IB_anybutton` (input.go:375).
pub const ANY_BUTTON: u32 = A | B | C | X | Y | Z | S | D | W | M;

/// Human name for a single bit (for HUDs/logs). Empty for unknown/none.
pub fn name(bit: u32) -> &'static str {
    match bit {
        PU => "↑",
        PD => "↓",
        PL => "←",
        PR => "→",
        A => "A",
        B => "B",
        C => "C",
        X => "X",
        Y => "Y",
        Z => "Z",
        S => "S",
        D => "D",
        W => "W",
        M => "M",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 14 values must match Ikemen GO bit-for-bit (input.go:361-374).
    #[test]
    fn values_match_ikemen() {
        assert_eq!(PU, 0b0000_0000_0000_0001);
        assert_eq!(PD, 0b10);
        assert_eq!(PL, 0b100);
        assert_eq!(PR, 0b1000);
        assert_eq!(A, 0b1_0000);
        assert_eq!(B, 0b10_0000);
        assert_eq!(C, 0b100_0000);
        assert_eq!(X, 0b1000_0000);
        assert_eq!(Y, 0b1_0000_0000);
        assert_eq!(Z, 0b10_0000_0000);
        assert_eq!(S, 0b100_0000_0000);
        assert_eq!(D, 0b1000_0000_0000);
        assert_eq!(W, 0b1_0000_0000_0000);
        assert_eq!(M, 0b10_0000_0000_0000);
        // 14 bits fit an i16 like Ikemen's InputBits.
        assert!(ANY_BUTTON < 1 << 15);
    }
}

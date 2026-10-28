//! Kurogane core — faithful port of Ikemen GO's `src/common.go` text helpers.
//!
//! These parsers are deliberately *lenient* and saturating, NOT like `str::parse`:
//! MUGEN content files are 25+ years of hand-edited text and every quirk of
//! `Atoi`/`Atof` below is load-bearing for byte-exact behavior with the Go
//! engine. Each function references the Go original it ports.
//!
//! Port origin: https://github.com/ikemen-engine/Ikemen-GO (MIT), `src/common.go`

pub mod text;

pub use text::{atof, atoi, is_numeric, section_name};

/// Port of `image.go` `TransType` (blend modes used by sprites/animations).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum TransType {
    #[default]
    None = 0,
    Add = 1,
    Sub = 2,
    SubAdd = 3,
    Default = 4,
}

/// `IMax` — Ikemen GO common.go:26. Saturation value for positive overflow in `atoi`.
pub const I_MAX: i32 = i32::MAX;
/// `IErr` — Ikemen GO common.go:27 (`^IMax`). Saturation value for negative overflow.
pub const I_ERR: i32 = i32::MIN;

/// Port of `NormalizeRect` (common.go:1999) — sorts rect corners into (left, top, right, bottom).
pub fn normalize_rect<T: PartialOrd>(mut rect: [T; 4]) -> [T; 4] {
    if rect[0] > rect[2] {
        rect.swap(0, 2);
    }
    if rect[1] > rect[3] {
        rect.swap(1, 3);
    }
    rect
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rect_swaps() {
        assert_eq!(normalize_rect([30, 40, 10, 20]), [10, 20, 30, 40]);
        assert_eq!(normalize_rect([1, 2, 3, 4]), [1, 2, 3, 4]);
    }

    #[test]
    fn trans_type_values_match_go() {
        assert_eq!(TransType::None as i32, 0);
        assert_eq!(TransType::Add as i32, 1);
        assert_eq!(TransType::Sub as i32, 2);
        assert_eq!(TransType::SubAdd as i32, 3);
        assert_eq!(TransType::Default as i32, 4);
    }
}

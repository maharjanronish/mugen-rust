//! Kurogane format layer — faithful Rust port of the Ikemen GO (MIT) file
//! format handling for MUGEN content.
//!
//! Modules:
//! * [`decode`] — pixel codecs (PCX RLE, RLE8, RLE5, LZ5)
//! * [`sff`] — SFF v1/v2 sprite containers, lazy decode, palettes
//! * [`sprite`] — premultiply + the LRU [`sprite::SpriteCache`]
//! * [`anim`] — AIR animation compiler
//!
//! Every ported function carries a reference to the Go original so behavior
//! can be audited line-by-line against
//! https://github.com/ikemen-engine/Ikemen-GO.

pub mod anim;
pub mod decode;
pub mod sff;
pub mod sprite;

pub use anim::{AnimCompiler, AnimFrame, AnimWarnings, Animation, AnimationTable};
pub use sff::{PaletteList, Pixels, Sff, SffData, SffError, SffHeader, SffType, SpriteEntry};
pub use sprite::{premultiply_rgba, DecodedSprite, SpriteCache};

//! Sprite pixel decoders — faithful ports of Ikemen GO `src/image.go`.
//!
//! Every decoder reproduces the exact Go algorithm including its quirks:
//! input-index clamping at `len-1` (the Go `if i < len(rle)-1 { i++ }` guard),
//! output clamping, and the PCX bytes-per-line padding-stripping behavior.
//! These quirks occur in real-world MUGEN content and must not be "fixed".

/// Port of `RlePcxDecode` (image.go:1011) — PCX RLE with bpl padding stripping.
///
/// `rle` is the raw PCX image data (after the 128-byte PCX header), `w`/`h` are
/// the sprite dimensions, `bpl` is the PCX "bytes per line" — the row stride
/// which may exceed `w`. Bytes emitted beyond `w` within a stride are padding
/// and get dropped, which is exactly what the `k == bpl → n = 1` dance does.
#[allow(unused_assignments)] // faithful transliteration of Go's variable flow
pub fn rle_pcx_decode(rle: &[u8], w: u32, h: u32, bpl: u32) -> Vec<u8> {
    if rle.is_empty() || bpl == 0 {
        return rle.to_vec();
    }
    let len_p = (w as usize) * (h as usize);
    let mut p = vec![0u8; len_p];
    let w = w as usize;
    let bpl = bpl as i64;
    let (mut i, mut j, mut k) = (0usize, 0usize, 0i64);
    while j < len_p {
        let (mut n, mut d) = (1i64, rle[i]);
        if i < rle.len() - 1 {
            i += 1;
        }
        if d >= 0xc0 {
            n = (d & 0x3f) as i64;
            d = rle[i];
            if i < rle.len() - 1 {
                i += 1;
            }
        }
        while n > 0 {
            if k < w as i64 && j < len_p {
                p[j] = d;
                j += 1;
            }
            k += 1;
            if k == bpl {
                k = 0;
                n = 1; // Go trick: forces loop exit after this iteration (drops stride padding)
            }
            n -= 1;
        }
    }
    p
}

/// Port of `Rle8Decode` (image.go:1209) — SFFv2 format 2.
pub fn rle8_decode(rle: &[u8], w: u32, h: u32) -> Vec<u8> {
    if rle.is_empty() {
        return rle.to_vec();
    }
    let len_p = (w as usize) * (h as usize);
    let mut p = vec![0u8; len_p];
    let (mut i, mut j) = (0usize, 0usize);
    while j < len_p {
        let (mut n, mut d) = (1usize, rle[i]);
        if i < rle.len() - 1 {
            i += 1;
        }
        if d & 0xc0 == 0x40 {
            n = (d & 0x3f) as usize;
            d = rle[i];
            if i < rle.len() - 1 {
                i += 1;
            }
        }
        while n > 0 {
            if j < len_p {
                p[j] = d;
                j += 1;
            }
            n -= 1;
        }
    }
    p
}

/// Port of `Rle5Decode` (image.go:1237) — SFFv2 format 3.
pub fn rle5_decode(rle: &[u8], w: u32, h: u32) -> Vec<u8> {
    if rle.is_empty() {
        return rle.to_vec();
    }
    let len_p = (w as usize) * (h as usize);
    let mut p = vec![0u8; len_p];
    let (mut i, mut j) = (0usize, 0usize);
    while j < len_p {
        // packet header: rl byte, dl byte (bit7 = an explicit color byte follows)
        let mut rl = rle[i] as i64;
        if i < rle.len() - 1 {
            i += 1;
        }
        let mut dl = (rle[i] & 0x7f) as i64;
        let mut c = 0u8;
        if rle[i] >> 7 != 0 {
            if i < rle.len() - 1 {
                i += 1;
            }
            c = rle[i];
        }
        if i < rle.len() - 1 {
            i += 1;
        }
        // do-while: at least one pixel is emitted per packet (Go `for { ... }`)
        loop {
            if j < len_p {
                p[j] = c;
                j += 1;
            }
            rl -= 1;
            if rl < 0 {
                dl -= 1;
                if dl < 0 {
                    break;
                }
                c = rle[i] & 0x1f;
                rl = (rle[i] >> 5) as i64;
                if i < rle.len() - 1 {
                    i += 1;
                }
            }
        }
    }
    p
}

/// Port of `Lz5Decode` (image.go:1281) — SFFv2 format 4 (LZ-ish back-reference codec).
#[allow(unused_assignments)] // faithful transliteration of Go's variable flow
pub fn lz5_decode(rle: &[u8], w: u32, h: u32) -> Vec<u8> {
    if rle.is_empty() {
        return rle.to_vec();
    }
    let len_p = (w as usize) * (h as usize);
    let mut p = vec![0u8; len_p];
    let (mut i, mut j, mut n) = (0usize, 0usize, 0i64);
    let mut ct = rle[i];
    let mut cts = 0u32;
    let mut rb = 0u8;
    let mut rbc = 0u32;
    if i < rle.len() - 1 {
        i += 1;
    }
    while j < len_p {
        let mut d = rle[i] as i64;
        if i < rle.len() - 1 {
            i += 1;
        }
        if ct & (1u8 << cts) != 0 {
            if d & 0x3f == 0 {
                d = (d << 2 | rle[i] as i64) + 1;
                if i < rle.len() - 1 {
                    i += 1;
                }
                n = rle[i] as i64 + 2;
                if i < rle.len() - 1 {
                    i += 1;
                }
            } else {
                rb |= ((d as u8) & 0xc0) >> rbc;
                rbc += 2;
                n = d & 0x3f;
                if rbc < 8 {
                    d = rle[i] as i64 + 1;
                    if i < rle.len() - 1 {
                        i += 1;
                    }
                } else {
                    d = rb as i64 + 1;
                    rb = 0;
                    rbc = 0;
                }
            }
            loop {
                if j < len_p {
                    if j >= d as usize {
                        p[j] = p[j - d as usize];
                    }
                    j += 1;
                }
                n -= 1;
                if n < 0 {
                    break;
                }
            }
        } else {
            if d & 0xe0 == 0 {
                n = rle[i] as i64 + 8;
                if i < rle.len() - 1 {
                    i += 1;
                }
            } else {
                n = d >> 5;
                d &= 0x1f;
            }
            while n > 0 {
                if j < len_p {
                    p[j] = d as u8;
                    j += 1;
                }
                n -= 1;
            }
        }
        cts += 1;
        if cts >= 8 {
            ct = rle[i];
            cts = 0;
            if i < rle.len() - 1 {
                i += 1;
            }
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rle8_roundtrip_known_vector() {
        // 4x1 all color 7: packet (0x44, 0x07) = 4 copies of 7
        let px = rle8_decode(&[0x44, 0x07], 4, 1);
        assert_eq!(px, vec![7, 7, 7, 7]);
        // Bytes without the 0x40 marker are single literals (Go: n stays 1).
        let px = rle8_decode(&[0x09, 0x01], 2, 1);
        assert_eq!(px, vec![9, 1]);
    }

    #[test]
    fn rle8_clamps_output() {
        // More run data than the sprite size: extra writes are dropped.
        let px = rle8_decode(&[0x47, 0x05], 3, 1); // 7 copies of 5 into 3x1
        assert_eq!(px, vec![5, 5, 5]);
    }

    #[test]
    fn rle_pcx_strips_stride_padding() {
        // 2x2 image, bpl=4: rows carry pad bytes in the stream; pads are dropped.
        // PCX RLE is pure RLE (no literal runs): 0xC1,v = one copy of v.
        let data = [
            0xC1, 0x01, 0xC1, 0x02, 0xC1, 0x00, 0xC1, 0x00, // row 0: 1,2 + padding
            0xC1, 0x03, 0xC1, 0x04, 0xC1, 0x00, 0xC1, 0x00, // row 1: 3,4 + padding
        ];
        let px = rle_pcx_decode(&data, 2, 2, 4);
        assert_eq!(px, vec![1, 2, 3, 4]);
    }

    #[test]
    fn rle_pcx_plain_when_bpl_zero() {
        assert_eq!(rle_pcx_decode(&[9, 9], 2, 1, 0), vec![9, 9]);
    }

    #[test]
    fn lz5_basic_literals() {
        // ct=0 → all literals: byte with n in high 3 bits, value in low 5
        // 0x21 = n=1, d=1 ; 0x22 = n=1, d=2 ; 0x23 = n=1, d=3
        let px = lz5_decode(&[0x00, 0x21, 0x22, 0x23], 3, 1);
        assert_eq!(px, vec![1, 2, 3]);
    }

    #[test]
    fn rle5_basic() {
        // RLE5: rl byte, then dl byte (bit7 = extra color byte follows)
        // rl=3, dl(bit7=0)=1, color comes from... first packet: rl copies of c where c defaults 0
        let px = rle5_decode(&[0x03, 0x01], 3, 1);
        assert_eq!(px, vec![0, 0, 0]);
    }
}

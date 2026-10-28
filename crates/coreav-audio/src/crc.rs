//! The CRCs of FLAC frames (RFC 9639 9.1.8 and 9.3): CRC-8 with polynomial
//! x^8 + x^2 + x + 1 and CRC-16 with x^16 + x^15 + x^2 + 1, both most
//! significant bit first, initialized with 0, without final inversion.

/// CRC-8 of `data`.
pub(crate) fn crc8(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |crc, &byte| {
        CRC8.get(usize::from(crc ^ byte)).copied().unwrap_or(0)
    })
}

/// CRC-16 of `data`, eight bytes at a time ("slicing by eight").
pub(crate) fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    let mut chunks = data.chunks_exact(8);
    for chunk in &mut chunks {
        let [b0, b1, b2, b3, b4, b5, b6, b7] = <[u8; 8]>::try_from(chunk).unwrap_or([0; 8]);
        let [high, low] = crc.to_be_bytes();
        crc = lookup(7, b0 ^ high)
            ^ lookup(6, b1 ^ low)
            ^ lookup(5, b2)
            ^ lookup(4, b3)
            ^ lookup(3, b4)
            ^ lookup(2, b5)
            ^ lookup(1, b6)
            ^ lookup(0, b7);
    }
    for &byte in chunks.remainder() {
        crc = (crc << 8) ^ lookup(0, (crc >> 8) as u8 ^ byte);
    }
    crc
}

#[inline(always)]
fn lookup(table: usize, byte: u8) -> u16 {
    CRC16
        .get(table)
        .and_then(|t| t.get(usize::from(byte)))
        .copied()
        .unwrap_or(0)
}

const CRC8: [u8; 256] = crc8_table();
const CRC16: [[u16; 256]; 8] = crc16_tables();

// Evaluated only for the `const` items above, at compile time, where an
// index out of bounds fails the build instead of panicking.
#[expect(clippy::indexing_slicing, reason = "compile-time table construction")]
const fn crc8_table() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

// Evaluated only for the `const` items above, at compile time, where an
// index out of bounds fails the build instead of panicking.
#[expect(clippy::indexing_slicing, reason = "compile-time table construction")]
const fn crc16_tables() -> [[u16; 256]; 8] {
    let mut tables = [[0u16; 256]; 8];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u16) << 8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
            bit += 1;
        }
        tables[0][i] = crc;
        i += 1;
    }
    let mut t = 1;
    while t < 8 {
        let mut i = 0;
        while i < 256 {
            let previous = tables[t - 1][i];
            tables[t][i] = (previous << 8) ^ tables[0][(previous >> 8) as usize];
            i += 1;
        }
        t += 1;
    }
    tables
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crc16_bitwise(data: &[u8]) -> u16 {
        let mut crc: u16 = 0;
        for &byte in data {
            crc ^= u16::from(byte) << 8;
            for _ in 0..8 {
                crc = if crc & 0x8000 != 0 {
                    (crc << 1) ^ 0x8005
                } else {
                    crc << 1
                };
            }
        }
        crc
    }

    #[test]
    fn slicing_matches_bitwise() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 131 + 7) as u8).collect();
        for len in [0, 1, 7, 8, 9, 15, 16, 17, 100, 1000] {
            assert_eq!(
                crc16(&data[..len]),
                crc16_bitwise(&data[..len]),
                "len {len}"
            );
        }
    }

    #[test]
    fn rfc_9639_example_header() {
        // Appendix D.1.4: frame header 0xFFF8 6918 00 00 has CRC-8 0xBF.
        assert_eq!(crc8(&[0xFF, 0xF8, 0x69, 0x18, 0x00, 0x00]), 0xBF);
    }
}

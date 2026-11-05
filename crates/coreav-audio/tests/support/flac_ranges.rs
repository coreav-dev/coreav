#![allow(clippy::unwrap_used)]

struct Bits {
    bytes: Vec<u8>,
    length: usize,
}

impl Bits {
    fn put(&mut self, width: u8, value: i64) {
        for bit in (0..width).rev() {
            if self.length % 8 == 0 {
                self.bytes.push(0);
            }
            *self.bytes.last_mut().unwrap() |=
                (((value as u64 >> bit) & 1) as u8) << (7 - self.length % 8);
            self.length += 1;
        }
    }
}

fn crc(bytes: &[u8], width: u8, polynomial: u16) -> u16 {
    let mut value = 0u16;
    for &byte in bytes {
        value ^= u16::from(byte) << (width - 8);
        for _ in 0..8 {
            let high = value & (1 << (width - 1));
            value = value.wrapping_shl(1) ^ if high == 0 { 0 } else { polynomial };
        }
    }
    if width == 8 { value & 255 } else { value }
}

fn frame(bits: u8, assignment: u8, values: [i64; 2], fixed_zero: bool, number: u8) -> Vec<u8> {
    let mut bytes = vec![
        0xff,
        0xf8,
        0x69,
        (assignment << 4) | if bits == 16 { 8 } else { 12 },
        number,
        15,
    ];
    bytes.push(crc(&bytes, 8, 7) as u8);
    let channels = if assignment == 0 { 1 } else { 2 };
    let mut out = Bits {
        bytes: vec![],
        length: 0,
    };
    for (channel, value) in values.into_iter().enumerate().take(channels) {
        let side = matches!((assignment, channel), (8 | 10, 1) | (9, 0));
        let depth = bits + u8::from(side);
        out.put(8, if fixed_zero { 16 } else { 0 });
        if fixed_zero {
            out.put(6, 0);
            out.put(4, 15);
            out.put(5, i64::from(depth + 1));
            for _ in 0..16 {
                out.put(depth + 1, value);
            }
        } else {
            out.put(depth, value);
        }
    }
    bytes.extend(out.bytes);
    bytes.extend(crc(&bytes, 16, 0x8005).to_be_bytes());
    bytes
}

pub fn file(bits: u8, assignment: u8, values: [i64; 2], fixed_zero: bool, late: bool) -> Vec<u8> {
    let channels = if assignment == 0 { 1 } else { 2 };
    let mut bytes = b"fLaC\x80\0\0\x22".to_vec();
    bytes.extend([0, 16, 0, 16, 0, 0, 0, 0, 0, 0]);
    let packed = (44_100u64 << 44)
        | ((channels - 1) << 41)
        | (u64::from(bits - 1) << 36)
        | if late { 32 } else { 16 };
    bytes.extend(packed.to_be_bytes());
    bytes.extend([0; 16]);
    if late {
        bytes.extend(frame(bits, assignment, [0, 0], false, 0));
    }
    bytes.extend(frame(bits, assignment, values, fixed_zero, u8::from(late)));
    bytes
}

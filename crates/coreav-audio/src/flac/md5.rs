pub(super) struct Md5 {
    state: [u32; 4],
    pending: Vec<u8>,
    bytes: u64,
}

impl Md5 {
    pub(super) fn new() -> Self {
        Self {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            pending: Vec::with_capacity(64),
            bytes: 0,
        }
    }

    pub(super) fn update(&mut self, mut input: &[u8]) {
        self.bytes = self.bytes.wrapping_add(input.len() as u64);
        if !self.pending.is_empty() {
            let take = (64 - self.pending.len()).min(input.len());
            let (prefix, rest) = input.split_at(take);
            self.pending.extend_from_slice(prefix);
            input = rest;
            if self.pending.len() == 64 {
                let block = std::mem::take(&mut self.pending);
                self.block(&block);
                self.pending = block;
                self.pending.clear();
            }
        }
        let mut chunks = input.chunks_exact(64);
        for block in &mut chunks {
            self.block(block);
        }
        self.pending.extend_from_slice(chunks.remainder());
    }

    pub(super) fn finish(mut self) -> [u8; 16] {
        let bits = self.bytes.wrapping_mul(8);
        let padding = if self.pending.len() < 56 {
            56 - self.pending.len()
        } else {
            120 - self.pending.len()
        };
        let mut tail = vec![0; padding];
        if let Some(first) = tail.first_mut() {
            *first = 0x80;
        }
        tail.extend_from_slice(&bits.to_le_bytes());
        self.update(&tail);
        let mut result = [0; 16];
        for (bytes, word) in result.chunks_exact_mut(4).zip(self.state) {
            bytes.copy_from_slice(&word.to_le_bytes());
        }
        result
    }

    fn block(&mut self, bytes: &[u8]) {
        let mut words = [0u32; 16];
        for (word, bytes) in words.iter_mut().zip(bytes.chunks_exact(4)) {
            *word = u32::from_le_bytes(bytes.try_into().unwrap_or_default());
        }
        let [mut a, mut b, mut c, mut d] = self.state;
        for (i, (&constant, &rotation)) in CONSTANTS.iter().zip(&ROTATIONS).enumerate() {
            let (function, word) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let sum = a
                .wrapping_add(function)
                .wrapping_add(constant)
                .wrapping_add(words.get(word).copied().unwrap_or_default());
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(sum.rotate_left(rotation));
        }
        for (word, addition) in self.state.iter_mut().zip([a, b, c, d]) {
            *word = word.wrapping_add(addition);
        }
    }
}

const ROTATIONS: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const CONSTANTS: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

#[derive(Default)]
pub struct Writer(Vec<bool>);

impl Writer {
    pub fn bit(&mut self, bit: bool) {
        self.0.push(bit);
    }
    pub fn bits(&mut self, value: u32, width: u8) {
        for shift in (0..width).rev() {
            self.bit(value >> shift & 1 != 0);
        }
    }
    pub fn ue(&mut self, value: u32) {
        let word = u64::from(value) + 1;
        let width = (64 - word.leading_zeros()) as u8;
        for _ in 1..width {
            self.bit(false);
        }
        for shift in (0..width).rev() {
            self.bit(word >> shift & 1 != 0);
        }
    }
    pub fn se(&mut self, value: i32) {
        let word = if value <= 0 {
            -i64::from(value) * 2
        } else {
            i64::from(value) * 2 - 1
        };
        self.ue(word as u32);
    }
    pub fn nal(mut self, header: u8) -> Vec<u8> {
        self.bit(true);
        while self.0.len() % 8 != 0 {
            self.bit(false);
        }
        let mut nal = vec![header];
        let mut zeros = 0;
        for chunk in self.0.chunks(8) {
            let byte = chunk.iter().fold(0_u8, |a, &b| (a << 1) | u8::from(b));
            if zeros == 2 && byte <= 3 {
                nal.push(3);
                zeros = 0;
            }
            nal.push(byte);
            zeros = if byte == 0 { zeros + 1 } else { 0 };
        }
        nal
    }
}

#[derive(Clone)]
pub struct Sequence {
    pub profile: u8,
    pub constraints: u8,
    pub level: u8,
    pub id: u32,
    pub chroma: u32,
    pub depth: u32,
    pub scaling: bool,
    pub frame_bits: u32,
    pub poc: u32,
    pub references: u32,
    pub gaps: bool,
    pub width_mbs: u32,
    pub height_mbs: u32,
    pub progressive: bool,
    pub crop: Option<[u32; 4]>,
    pub vui: bool,
    pub reorder: u32,
    pub cpb: u32,
}

impl Default for Sequence {
    fn default() -> Self {
        Self {
            profile: 100,
            constraints: 0,
            level: 31,
            id: 0,
            chroma: 1,
            depth: 0,
            scaling: false,
            frame_bits: 0,
            poc: 0,
            references: 2,
            gaps: false,
            width_mbs: 2,
            height_mbs: 2,
            progressive: true,
            crop: None,
            vui: false,
            reorder: 0,
            cpb: 0,
        }
    }
}

impl Sequence {
    pub fn nal(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.bits(u32::from(self.profile), 8);
        w.bits(u32::from(self.constraints), 8);
        w.bits(u32::from(self.level), 8);
        w.ue(self.id);
        w.ue(self.chroma);
        if self.chroma == 3 {
            w.bit(false);
        }
        w.ue(self.depth);
        w.ue(self.depth);
        w.bit(false);
        w.bit(self.scaling);
        if self.scaling {
            for _ in 0..8 {
                w.bit(false);
            }
        }
        w.ue(self.frame_bits);
        w.ue(self.poc);
        if self.poc == 0 {
            w.ue(0);
        }
        if self.poc == 1 {
            w.bit(true);
            w.se(0);
            w.se(0);
            w.ue(0);
        }
        w.ue(self.references);
        w.bit(self.gaps);
        w.ue(self.width_mbs - 1);
        w.ue(self.height_mbs - 1);
        w.bit(self.progressive);
        if !self.progressive {
            w.bit(false);
        }
        w.bit(true);
        w.bit(self.crop.is_some());
        if let Some(crop) = self.crop {
            for value in crop {
                w.ue(value);
            }
        }
        w.bit(self.vui);
        if self.vui {
            w.bit(true);
            w.bits(255, 8);
            w.bits(1, 16);
            w.bits(1, 16);
            w.bit(true);
            w.bit(false);
            w.bit(true);
            w.bits(5, 3);
            w.bit(false);
            w.bit(true);
            for _ in 0..3 {
                w.bits(1, 8);
            }
            w.bit(true);
            w.ue(0);
            w.ue(0);
            w.bit(true);
            w.bits(1001, 32);
            w.bits(60000, 32);
            w.bit(true);
            w.bit(true);
            w.ue(self.cpb);
            w.bits(0, 4);
            w.bits(0, 4);
            for _ in 0..=self.cpb.min(32) {
                w.ue(0);
                w.ue(0);
                w.bit(true);
            }
            for _ in 0..4 {
                w.bits(23, 5);
            }
            w.bit(false);
            w.bit(false);
            w.bit(false);
            w.bit(true);
            w.bit(true);
            for _ in 0..4 {
                w.ue(0);
            }
            w.ue(self.reorder);
            w.ue(self.references);
        }
        w.nal(0x67)
    }
}

#[derive(Default)]
pub struct Picture {
    pub id: u32,
    pub sequence: u32,
    pub bottom: bool,
    pub groups: u32,
    pub redundant: bool,
    pub extension: bool,
    pub scaling: bool,
    pub qp: i32,
}

impl Picture {
    pub fn nal(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.ue(self.id);
        w.ue(self.sequence);
        w.bit(true);
        w.bit(self.bottom);
        w.ue(self.groups);
        w.ue(0);
        w.ue(0);
        w.bit(false);
        w.bits(0, 2);
        w.se(self.qp);
        w.se(0);
        w.se(0);
        w.bit(true);
        w.bit(false);
        w.bit(self.redundant);
        if self.extension {
            w.bit(true);
            w.bit(self.scaling);
            w.se(0);
        }
        w.nal(0x68)
    }
}

#[derive(Clone)]
pub struct Slice {
    pub idr: bool,
    pub kind: u32,
    pub pps: u32,
    pub first_mb: u32,
    pub frame: u32,
    pub idr_id: u32,
    pub poc: u32,
    pub poc_type: u32,
    pub bottom: Option<i32>,
}

impl Default for Slice {
    fn default() -> Self {
        Self {
            idr: true,
            kind: 2,
            pps: 0,
            first_mb: 0,
            frame: 0,
            idr_id: 0,
            poc: 0,
            poc_type: 0,
            bottom: None,
        }
    }
}

impl Slice {
    pub fn nal(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.ue(self.first_mb);
        w.ue(self.kind);
        w.ue(self.pps);
        w.bits(self.frame, 4);
        if self.idr {
            w.ue(self.idr_id);
        }
        if self.poc_type == 0 {
            w.bits(self.poc, 4);
            if let Some(delta) = self.bottom {
                w.se(delta);
            }
        }
        w.bits(0x25, 6);
        w.nal(if self.idr { 0x65 } else { 0x41 })
    }
}

pub fn unit(nals: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in nals {
        out.extend((nal.len() as u32).to_be_bytes());
        out.extend(*nal);
    }
    out
}

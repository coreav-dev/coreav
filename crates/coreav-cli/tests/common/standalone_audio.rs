pub fn wave() -> Vec<u8> {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend(52u32.to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(8000u32.to_le_bytes());
    bytes.extend(16000u32.to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend([0; 16]);
    bytes
}

pub fn flac() -> Vec<u8> {
    let mut bytes = b"fLaC\x80\0\0\x22".to_vec();
    bytes.extend(4096u16.to_be_bytes());
    bytes.extend(4096u16.to_be_bytes());
    bytes.extend([0; 6]);
    let packed = (8000u64 << 44) | (15u64 << 36) | 4096;
    bytes.extend(packed.to_be_bytes());
    bytes.extend([0; 16]);
    bytes
}

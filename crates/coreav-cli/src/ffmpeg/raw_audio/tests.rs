use super::*;

#[test]
fn mpeg1_admission_checks_every_complete_frame() {
    let mut frame = vec![0; 417];
    if let Some(header) = frame.get_mut(..4) {
        header.copy_from_slice(&[0xff, 0xfb, 0x90, 0x64]);
    }
    assert!(mpeg1_frames(&frame));
    let mut pair = frame.clone();
    pair.extend_from_slice(&frame);
    assert!(mpeg1_frames(&pair));
    pair.extend_from_slice(&[0xff, 0xf3, 0x80, 0x64]);
    assert!(!mpeg1_frames(&pair));
    frame.pop();
    assert!(!mpeg1_frames(&frame));
    assert!(!mpeg1_frames(&[]));
    assert!(!mpeg1_frames(&[0xff, 0xfb, 0, 0]));
}

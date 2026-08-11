use super::*;

#[test]
fn test_write_and_read() {
    let mut rom = TrackedRom::new(vec![0u8; 256]);
    rom.write("test", 0, &[0xAA, 0xBB]).unwrap();
    assert_eq!(rom.read(0, 2).unwrap(), &[0xAA, 0xBB]);
}

#[test]
fn test_collision_detection() {
    let mut rom = TrackedRom::new(vec![0u8; 256]);
    rom.write("first", 10, &[1, 2, 3]).unwrap();
    let err = rom.write("second", 11, &[4, 5]);
    assert!(err.is_err());
    assert!(err.unwrap_err().to_string().contains("collision"));
}

#[test]
fn test_no_collision_adjacent() {
    let mut rom = TrackedRom::new(vec![0u8; 256]);
    rom.write("first", 10, &[1, 2, 3]).unwrap();
    rom.write("second", 13, &[4, 5]).unwrap(); // adjacent, no overlap
}

#[test]
fn test_write_reports() {
    let mut rom = TrackedRom::new(vec![0u8; 256]);
    rom.write("alpha", 0, &[1]).unwrap();
    rom.write("beta", 100, &[2]).unwrap();
    let reports = rom.write_reports();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].label, "alpha");
    assert_eq!(reports[1].label, "beta");
}

#[test]
fn test_out_of_bounds() {
    let mut rom = TrackedRom::new(vec![0u8; 256]);
    let err = rom.write("oob", 255, &[1, 2]);
    assert!(err.is_err());
}

#[test]
fn test_untouched_bytes_preserved() {
    let original = vec![0xFFu8; 256];
    let mut rom = TrackedRom::new(original.clone());
    rom.write("patch", 10, &[0, 0, 0]).unwrap();
    for i in 0..256 {
        if !(10..13).contains(&i) {
            assert_eq!(rom.data()[i], 0xFF, "byte at {i} was modified");
        }
    }
}

#[test]
fn test_read_out_of_bounds_returns_none() {
    let rom = TrackedRom::new(vec![0xAA; 16]);
    assert!(rom.read(14, 4).is_none());
    assert!(rom.read(0, 17).is_none());
    assert!(rom.read(usize::MAX, 1).is_none());
    assert!(rom.read(0, 16).is_some());
    assert_eq!(rom.read(0, 1).unwrap(), &[0xAA]);
}

#[test]
fn deref_allows_read_indexing() {
    let mut rom = TrackedRom::new(vec![0u8; 0x200]);
    rom.write("t", 0x100, &[0x11, 0x22, 0x33]).unwrap();
    assert_eq!(rom[0x100], 0x11); // Index via Deref<[u8]>
    assert_eq!(&rom[0x100..0x103], &[0x11, 0x22, 0x33]);
}

#[test]
fn write_bank_computes_physical_offset() {
    let mut rom = TrackedRom::new(vec![0u8; 0x10000]);
    // bank 3, off 0x36C0 -> physical 3*0x4000 + 0x36C0 = 0xF6C0
    rom.write_bank("portrait", 3, 0x36C0, &[0xAA]).unwrap();
    assert_eq!(rom[0xF6C0], 0xAA);
}

#[test]
fn write_expect_freespace_passes_on_ff() {
    let mut rom = TrackedRom::new(vec![0xFFu8; 0x100]);
    rom.write_expect("t", 0x10, &[1, 2], &Expect::FreeSpace(0xFF))
        .unwrap();
    assert_eq!(&rom[0x10..0x12], &[1, 2]);
}

#[test]
fn write_expect_freespace_errors_on_nonfree() {
    let mut rom = TrackedRom::new(vec![0u8; 0x100]); // 0x00, not 0xFF
    let err = rom
        .write_expect("t", 0x10, &[1], &Expect::FreeSpace(0xFF))
        .unwrap_err();
    assert!(matches!(err, TrackedRomError::Expectation { .. }));
}

#[test]
fn write_expect_bytes_passes_on_match() {
    let mut rom = TrackedRom::new(vec![0u8; 0x100]);
    // seed via direct data (no tracked write) so the region is free for write_expect
    // (use a fresh region; here 0x10 is 0x00,0x00 which equals the expected)
    rom.write_expect("t", 0x10, &[0xBE, 0xEF], &Expect::Bytes(&[0x00, 0x00]))
        .unwrap();
    assert_eq!(&rom[0x10..0x12], &[0xBE, 0xEF]);
}

#[test]
fn write_expect_bytes_errors_on_mismatch() {
    let mut rom = TrackedRom::new(vec![0u8; 0x100]);
    let err = rom
        .write_expect("t", 0x10, &[1], &Expect::Bytes(&[0xAB]))
        .unwrap_err();
    assert!(matches!(err, TrackedRomError::Expectation { .. }));
}

#[test]
fn write_expect_bytes_errors_on_length_mismatch() {
    let mut rom = TrackedRom::new(vec![0u8; 0x100]);
    // expected len 1, data len 2 — must error, not partial-validate
    let err = rom
        .write_expect("t", 0x10, &[1, 2], &Expect::Bytes(&[0x00]))
        .unwrap_err();
    assert!(matches!(err, TrackedRomError::Expectation { .. }));
    // and the region must NOT have been written
    assert_eq!(&rom[0x10..0x12], &[0x00, 0x00]);
}

#[test]
fn tracked_write_passes_untracked_scan() {
    let original = vec![0u8; 0x100];
    let mut rom = TrackedRom::new(original.clone());
    rom.write("intended", 0x10, &[0xAB]).unwrap();
    assert!(rom.check_untracked_writes(&original).is_ok());
}

#[test]
fn untracked_change_is_detected() {
    let mut original = vec![0u8; 0x100];
    original[0x50] = 0x99;
    let mut rom = TrackedRom::new(vec![0u8; 0x100]);
    rom.write("intended", 0x10, &[0xAB]).unwrap();
    let err = rom.check_untracked_writes(&original).unwrap_err();
    assert!(err.contains("UNTRACKED"), "{err}");
    assert!(err.contains("0x000050"));
}

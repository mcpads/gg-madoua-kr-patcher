use super::*;

#[test]
fn test_slot0_fixed() {
    assert_eq!(SegaMapper::logical_to_physical(0x0000, 5, 8), 0x00000);
    assert_eq!(SegaMapper::logical_to_physical(0x3FFF, 5, 8), 0x03FFF);
}

#[test]
fn test_slot1() {
    assert_eq!(SegaMapper::logical_to_physical(0x4000, 5, 8), 0x14000);
    assert_eq!(SegaMapper::logical_to_physical(0x7FFF, 5, 8), 0x17FFF);
}

#[test]
fn test_slot2() {
    assert_eq!(SegaMapper::logical_to_physical(0x8000, 5, 8), 0x20000);
    assert_eq!(SegaMapper::logical_to_physical(0xBFFF, 5, 8), 0x23FFF);
}

#[test]
fn test_ram_mirror() {
    // $C000-$FFFF is RAM, not ROM.
    // We return the address as-is (no panic).
    let result = SegaMapper::logical_to_physical(0xC000, 5, 8);
    // Just verify it doesn't panic and returns some value
    let _ = result;
}

#[test]
fn test_physical_to_bank() {
    assert_eq!(SegaMapper::physical_to_bank(0x00000), (0, 0x0000));
    assert_eq!(SegaMapper::physical_to_bank(0x0C200), (3, 0x0200));
    assert_eq!(SegaMapper::physical_to_bank(0x14000), (5, 0x0000));
    assert_eq!(SegaMapper::physical_to_bank(0x80000), (32, 0x0000));
    assert_eq!(SegaMapper::physical_to_bank(0x232C0), (8, 0x32C0));
}

#[test]
fn test_bank_range() {
    assert_eq!(SegaMapper::bank_range(0), 0x00000..0x04000);
    assert_eq!(SegaMapper::bank_range(3), 0x0C000..0x10000);
    assert_eq!(SegaMapper::bank_range(32), 0x80000..0x84000);
}

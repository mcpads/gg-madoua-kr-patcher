use super::*;

#[test]
fn test_parse_magic() {
    let mut rom = vec![0u8; 0x8000];
    rom[0x7FF0..0x7FF8].copy_from_slice(b"TMR SEGA");
    let header = TmrSegaHeader::parse(&rom).unwrap();
    assert_eq!(&header.magic, b"TMR SEGA");
}

#[test]
fn test_checksum_computation() {
    let mut rom = vec![0u8; 0x8000];
    // Set some bytes so checksum isn't zero
    rom[0] = 0x01;
    rom[1] = 0x02;
    let expected: u16 = 0x01 + 0x02; // only two non-zero bytes before header
    assert_eq!(TmrSegaHeader::compute_checksum(&rom), expected);
}

#[test]
fn test_checksum_round_trip() {
    let mut rom = vec![0x42u8; 0x8000];
    rom[0x7FF0..0x7FF8].copy_from_slice(b"TMR SEGA");
    let checksum = TmrSegaHeader::compute_checksum(&rom);
    TmrSegaHeader::update_checksum(&mut rom);
    // Verify the stored checksum matches
    let stored = u16::from_le_bytes([rom[0x7FFA], rom[0x7FFB]]);
    assert_eq!(stored, checksum);
}

#[test]
fn test_invalid_magic() {
    let rom = vec![0u8; 0x8000];
    assert!(TmrSegaHeader::parse(&rom).is_err());
}

#[test]
fn test_parse_region_romsize() {
    let mut rom = vec![0u8; 0x8000];
    rom[0x7FF0..0x7FF8].copy_from_slice(b"TMR SEGA");
    // Set region=7 (0x7C upper nibble) and rom_size=C (0x7C lower nibble)
    rom[0x7FFF] = 0x7C;
    let header = TmrSegaHeader::parse(&rom).unwrap();
    assert_eq!(header.region, 0x7);
    assert_eq!(header.rom_size, 0xC);
}

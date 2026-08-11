use super::*;

#[test]
fn test_tile_vram_addr() {
    assert_eq!(Tile4bpp::tile_vram_addr(0), 0);
    assert_eq!(Tile4bpp::tile_vram_addr(1), 32);
    assert_eq!(Tile4bpp::tile_vram_addr(0x70), 0x70 * 32);
}

#[test]
fn test_pixel_set_get_round_trip() {
    let mut tile = Tile4bpp { data: [0; 32] };
    tile.set_pixel(0, 0, 15);
    assert_eq!(tile.get_pixel(0, 0), 15);
    assert_eq!(tile.get_pixel(1, 0), 0);
}

#[test]
fn test_pixel_all_colors() {
    let mut tile = Tile4bpp { data: [0; 32] };
    for color in 0..16u8 {
        tile.set_pixel(color % 8, color / 8, color);
        assert_eq!(tile.get_pixel(color % 8, color / 8), color);
    }
}

#[test]
fn test_all_bits_set() {
    let tile = Tile4bpp { data: [0xFF; 32] };
    for y in 0..8 {
        for x in 0..8 {
            assert_eq!(tile.get_pixel(x, y), 15);
        }
    }
}

//! UI 베이크드 그래픽 텍스트(압축) 인벤토리와 코덱 검증 게이트 (UI 트랙 B).
//!
//! A의 UI 그래픽은 `lzss` LZSS로 압축돼 있다. 이 모듈은 각 UI 요소의 JP 압축 블록 위치와
//! uncompressed 크기(EN 패치 `dumpgrp.sh`/`asm/main.s`에서 확정)를 고정하고, `lzss` 코덱이
//! ROM의 실제 블록을 무손실 라운드트립하는지 static gate로 검사한다. 재조판(retype)·재압축
//! 패치는 이 코덱 위에 올린다.
//!
//! 실제 렌더 경로·타일 지오메트리·팔레트는 fresh scene에서 재확인해야 하며, 여기 주소는
//! JP 원본(`7EC95282`) 기준 EN 소스 대조값이다.

use crate::lzss;
use anyhow::{Context, Result};
use gg_sms::header::TmrSegaHeader;
use gg_sms::rom::{Expect, TrackedRom};
use retro_z80::{Assembler, Instruction, Register16};
use std::path::Path;

/// GG/SMS 4bpp 타일 = 32바이트.
pub const TILE_BYTES: usize = 32;

/// 일반 버튼의 JP 글리프를 지울 내부 사각형 `(row0, row1, col0, col1)`(half-open).
/// 원본 9세트 전수 스캔에서 glyph/shadow가 rows 3..13, cols 3..21까지 닿는다. 이 범위는
/// 둥근 프레임의 안쪽이며, rows 3/12의 cols 2/21과 rows 4..11의 cols 1/22는 보존한다.
const DEFAULT_INTERIOR_BOX: (usize, usize, usize, usize) = (3, 13, 3, 21);

/// 나침반 방향석 전용 내부 사각형. 일반 버튼과 프레임 폭이 달라 cols 3/20도 프레임이므로
/// rows 3..13을 지우되, cols 4..20만 지워 좌우 프레임을 보존한다.
const COMPASS_INTERIOR_BOX: (usize, usize, usize, usize) = (3, 13, 4, 20);

/// 확장 ROM의 UI relocation 전용 리소스 뱅크. bank 32는 글리프, 33~43은 region repack이
/// 사용하므로 그 다음 bank 44를 쓴다.
const UI_RELOC_BANK: u8 = 44;
const UI_RELOC_BANK_BASE: usize = UI_RELOC_BANK as usize * 0x4000;
const UI_RELOC_FIRST_DESC_OFFSET: usize = 0x10;
const SLOT1_BASE: u16 = 0x4000;

/// JP 원본의 `ld hl,$0217`(main-menu save) / `ld hl,$0218`(battle flee) 호출부.
/// `$0079 -> $16BA` 리소스 로더가 HL의 상위 바이트를 bank, 하위 바이트를 리소스 id로 쓴다.
const SAVE_ASSET_LOAD_SITE: usize = 0x1A2F5;
const SAVE_ASSET_LOAD_LOGICAL: u16 = 0xA2F5;
const SAVE_ASSET_LOAD_ORIG: [u8; 3] = [0x21, 0x17, 0x02];
const FLEE_ASSET_LOAD_SITE: usize = 0x1A33E;
const FLEE_ASSET_LOAD_LOGICAL: u16 = 0xA33E;
const FLEE_ASSET_LOAD_ORIG: [u8; 3] = [0x21, 0x18, 0x02];
const SAVE_RELOC_ASSET_ID: u16 = (UI_RELOC_BANK as u16) << 8;
const FLEE_RELOC_ASSET_ID: u16 = ((UI_RELOC_BANK as u16) << 8) | 1;

// ─────────────────────────────────────────────────────────────────────────
// 4bpp planar 타일 codec (GG/SMS 공통). 행당 4바이트(=4 bitplane), MSB=좌측 픽셀.
// ─────────────────────────────────────────────────────────────────────────

/// 타일의 `(x, y)` 픽셀 팔레트 인덱스(0..15)를 읽는다.
pub fn get_index(tile: &[u8], x: usize, y: usize) -> u8 {
    let b = 7 - x;
    let mut v = 0u8;
    for p in 0..4 {
        v |= ((tile[y * 4 + p] >> b) & 1) << p;
    }
    v
}

/// 타일의 `(x, y)` 픽셀을 팔레트 인덱스 `idx`(0..15)로 쓴다.
pub fn set_index(tile: &mut [u8], x: usize, y: usize, idx: u8) {
    let b = 7 - x;
    for p in 0..4 {
        if (idx >> p) & 1 == 1 {
            tile[y * 4 + p] |= 1 << b;
        } else {
            tile[y * 4 + p] &= !(1u8 << b);
        }
    }
}

/// 32바이트 4bpp 타일을 8×8 팔레트 인덱스 그리드로 디코드한다.
pub fn decode_tile(tile: &[u8]) -> [[u8; 8]; 8] {
    let mut g = [[0u8; 8]; 8];
    for (y, row) in g.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate() {
            *cell = get_index(tile, x, y);
        }
    }
    g
}

/// UI 압축 그래픽 블록 하나의 서술자.
pub struct UiBlob {
    /// 요소 이름.
    pub name: &'static str,
    /// JP 원본 ROM의 압축 블록 물리 오프셋.
    pub compressed_addr: usize,
    /// 디컴프 후 uncompressed 크기(바이트). `tiles * TILE_BYTES`와 일치해야 한다.
    pub uncompressed_len: usize,
    /// 타일 수(각 32바이트 4bpp).
    pub tiles: usize,
    /// JP 원문(참고).
    pub jp: &'static str,
    /// 한글 제안(참고, 미확정).
    pub ko_hint: &'static str,
    /// JP 글리프를 지울 버튼 내부 사각형 `(row0, row1, col0, col1)`(half-open).
    pub interior_box: (usize, usize, usize, usize),
}

/// A의 압축 UI 그래픽 블록 인벤토리. 주소·크기는 EN 패치 소스(`madouaggtools`) 대조값이며,
/// `lzss` 라운드트립으로 검증된다(`cmd_check_ui_graphics`).
pub const UI_BLOBS: &[UiBlob] = &[
    UiBlob {
        name: "compass",
        compressed_addr: 0x92DA,
        uncompressed_len: 768,
        tiles: 24,
        jp: "東西南北",
        ko_hint: "동/서/남/북",
        interior_box: COMPASS_INTERIOR_BOX,
    },
    UiBlob {
        name: "buttons_map",
        compressed_addr: 0x8C30,
        uncompressed_len: 192,
        tiles: 6,
        jp: "マップ",
        ko_hint: "지도",
        interior_box: DEFAULT_INTERIOR_BOX,
    },
    UiBlob {
        name: "buttons_magic_item",
        compressed_addr: 0x8CCA,
        uncompressed_len: 384,
        tiles: 12,
        jp: "じゅもん/アイテム",
        ko_hint: "주문/아이템",
        interior_box: DEFAULT_INTERIOR_BOX,
    },
    UiBlob {
        name: "buttons_save",
        compressed_addr: 0x8DD3,
        uncompressed_len: 192,
        tiles: 6,
        jp: "セーブ",
        ko_hint: "저장",
        interior_box: DEFAULT_INTERIOR_BOX,
    },
    UiBlob {
        name: "buttons_flee_lipemco",
        compressed_addr: 0x8E6E,
        uncompressed_len: 384,
        tiles: 12,
        jp: "ダッシュ/にげる",
        ko_hint: "대시/도망",
        interior_box: DEFAULT_INTERIOR_BOX,
    },
    UiBlob {
        name: "buttons_file",
        compressed_addr: 0x944D,
        uncompressed_len: 768,
        tiles: 24,
        jp: "ファイル1~4",
        ko_hint: "파일1~4",
        interior_box: DEFAULT_INTERIOR_BOX,
    },
    UiBlob {
        name: "buttons_yes_no",
        compressed_addr: 0x9578,
        uncompressed_len: 384,
        tiles: 12,
        jp: "はい/いいえ",
        ko_hint: "예/아니오",
        interior_box: DEFAULT_INTERIOR_BOX,
    },
    UiBlob {
        name: "button_cursor",
        compressed_addr: 0x9526,
        uncompressed_len: 192,
        tiles: 6,
        jp: "(커서)",
        ko_hint: "(원본 유지)",
        interior_box: DEFAULT_INTERIOR_BOX,
    },
    UiBlob {
        name: "buttons_buy_sell_leave",
        compressed_addr: 0x25CEA,
        uncompressed_len: 576,
        tiles: 18,
        jp: "かう/うる/やめる",
        ko_hint: "사기/팔기/가기",
        interior_box: DEFAULT_INTERIOR_BOX,
    },
    UiBlob {
        name: "buttons_title",
        compressed_addr: 0x23D22,
        uncompressed_len: 384,
        tiles: 12,
        jp: "はじめ/つづき",
        ko_hint: "시작/계속",
        interior_box: DEFAULT_INTERIOR_BOX,
    },
];

/// UI 압축 블록 코덱 static gate.
///
/// 각 블록을 ROM에서 디컴프해 (1) uncompressed 크기가 인벤토리와 일치하고 `tiles*32`인지,
/// (2) `decompress(compress(dec)) == dec` 무손실인지, (3) 재압축 크기가 원본 이하라 in-place가
/// 가능한지 검사한다. 하나라도 실패하면 게이트 실패다. 이는 트랙 B 재조판 패치의 코덱 전제를
/// 고정한다(렌더 경로/지오메트리 증명은 별도 fresh scene 과제).
pub fn cmd_check_ui_graphics(rom_path: &Path) -> Result<()> {
    let rom = std::fs::read(rom_path)
        .with_context(|| format!("ROM 읽기 실패: {}", rom_path.display()))?;

    let mut failures = 0usize;
    println!("UI 압축 그래픽 코덱 검사: {}", rom_path.display());
    for blob in UI_BLOBS {
        anyhow::ensure!(
            blob.uncompressed_len == blob.tiles * TILE_BYTES,
            "{}: 인벤토리 오류 uncompressed_len {} != tiles {} * {}",
            blob.name,
            blob.uncompressed_len,
            blob.tiles,
            TILE_BYTES
        );
        anyhow::ensure!(
            blob.compressed_addr < rom.len(),
            "{}: 압축 주소 0x{:05X}가 ROM 범위를 벗어남",
            blob.name,
            blob.compressed_addr
        );

        let (dec, consumed) = lzss::decompress(&rom, blob.compressed_addr);
        let size_ok = dec.len() == blob.uncompressed_len;
        let recomp = lzss::compress(&dec);
        let (dec2, _) = lzss::decompress(&recomp, 0);
        let lossless = dec2 == dec;
        let fits = recomp.len() <= consumed;

        let ok = size_ok && lossless && fits;
        if !ok {
            failures += 1;
        }
        println!(
            "  {} {:24} addr=0x{:05X} {}t dec={}B(exp {}) comp={}B recomp={}B  size_ok={} lossless={} fits={}  [{} → {}]",
            if ok { "OK  " } else { "FAIL" },
            blob.name,
            blob.compressed_addr,
            blob.tiles,
            dec.len(),
            blob.uncompressed_len,
            consumed,
            recomp.len(),
            size_ok,
            lossless,
            fits,
            blob.jp,
            blob.ko_hint,
        );
    }

    anyhow::ensure!(
        failures == 0,
        "UI 압축 블록 {}개가 코덱 검사에 실패함",
        failures
    );
    println!(
        "UI blobs: {} (전부 무손실 라운드트립·in-place fit)",
        UI_BLOBS.len()
    );
    Ok(())
}

/// 블록 이름으로 `UI_BLOBS`에서 서술자를 찾는다.
pub fn blob_by_name(name: &str) -> Option<&'static UiBlob> {
    UI_BLOBS.iter().find(|b| b.name == name)
}

// ─────────────────────────────────────────────────────────────────────────
// compose (retype): 디컴프 4bpp 버퍼에서 JP 글리프를 지우고 한글을 스탬프한다.
// A 실측: 모든 UI 블록은 ColMajor(EN structure "0 2 4 / 1 3 5"), 버튼 면색 인덱스 1,
// 글리프 획 인덱스 15, 프레임 d(13)/e(14). interior_box는 코너 프레임을 보존한다.
// ─────────────────────────────────────────────────────────────────────────

/// 버튼 면(배경) 팔레트 인덱스.
const BG_INDEX: u8 = 1;
/// 글리프 획 팔레트 인덱스.
const INK_INDEX: u8 = 15;

/// ColMajor: 24×16 버튼 안의 `(gx, gy)` 픽셀이 속한 6타일 중 하나의 인덱스.
/// EN structure "0 2 4 / 1 3 5" = `tiles[(gx/8)*2 + (gy/8)]`.
fn colmajor_tile(tiles: &[usize; 6], gx: usize, gy: usize) -> usize {
    tiles[(gx / 8) * 2 + (gy / 8)]
}

/// 디컴프 4bpp 버퍼의 버튼 하나를 재조판한다: 내부 박스를 면색으로 클리어(프레임 보존) →
/// 한글 글리프를 가로 중앙, rows 4..12에 획색으로 스탬프.
pub fn retype_button(
    buf: &mut [u8],
    tiles: &[usize; 6],
    glyphs: &[[u8; 8]],
    xs: Option<&[usize]>,
    interior_box: (usize, usize, usize, usize),
) {
    let tile_off = |gx: usize, gy: usize| colmajor_tile(tiles, gx, gy) * TILE_BYTES;
    let (r0, r1, c0, c1) = interior_box;
    for gy in r0..r1 {
        for gx in c0..c1 {
            let off = tile_off(gx, gy);
            set_index(&mut buf[off..off + TILE_BYTES], gx % 8, gy % 8, BG_INDEX);
        }
    }
    let k = glyphs.len();
    // 글리프 x위치: `xs`가 주어지면 그대로(예: 3글자 "일기N"을 GG1식 우측 시프트 [3,10,18]로
    // 좌측 프레임에서 뗌), 없으면 블록 중앙정렬(2글자 기본).
    let x_start = 24usize.saturating_sub(k * 8) / 2;
    for (gi, glyph) in glyphs.iter().enumerate() {
        let x0 = xs
            .and_then(|xs| xs.get(gi))
            .copied()
            .unwrap_or(x_start + gi * 8);
        for (ry, &row) in glyph.iter().enumerate() {
            let gy = 4 + ry;
            if gy >= 16 {
                break;
            }
            for rx in 0..8 {
                if (row >> (7 - rx)) & 1 == 1 {
                    let gx = x0 + rx;
                    if gx < 24 {
                        let off = tile_off(gx, gy);
                        set_index(&mut buf[off..off + TILE_BYTES], gx % 8, gy % 8, INK_INDEX);
                    }
                }
            }
        }
    }
}

/// UI 압축 블록을 디컴프해 각 타일을 8×8 팔레트 인덱스(hex) 그리드로 렌더한다.
///
/// 지오메트리(타일 배치·interior_box)·팔레트 인덱스(frame/bg/ink)를 A에서 재측정하기 위한
/// 도구다. `--assemble rowmajor|colmajor`를 주면 6타일씩 24×16 버튼으로 조립해 보여 준다.
pub fn cmd_dump_ui_graphic(rom_path: &Path, name: &str, assemble: Option<&str>) -> Result<()> {
    let rom = std::fs::read(rom_path)
        .with_context(|| format!("ROM 읽기 실패: {}", rom_path.display()))?;
    let blob = blob_by_name(name)
        .with_context(|| format!("알 수 없는 UI blob: {name} (see check-ui-graphics)"))?;
    let (dec, consumed) = lzss::decompress(&rom, blob.compressed_addr);
    println!(
        "{} @0x{:05X}  {}타일  dec={}B comp={}B  [{} → {}]",
        blob.name,
        blob.compressed_addr,
        blob.tiles,
        dec.len(),
        consumed,
        blob.jp,
        blob.ko_hint
    );

    // 인덱스 히스토그램(전체 블록).
    let mut hist = [0usize; 16];
    for t in 0..blob.tiles {
        let tile = &dec[t * TILE_BYTES..(t + 1) * TILE_BYTES];
        for y in 0..8 {
            for x in 0..8 {
                hist[get_index(tile, x, y) as usize] += 1;
            }
        }
    }
    print!("index histogram:");
    for (i, &c) in hist.iter().enumerate() {
        if c > 0 {
            print!(" {i:X}={c}");
        }
    }
    println!();

    if let Some(mode) = assemble {
        // 6타일 = 24×16 버튼으로 조립. 각 direction/button마다 출력.
        let colmajor = match mode {
            "rowmajor" => false,
            "colmajor" => true,
            other => anyhow::bail!("--assemble는 rowmajor|colmajor (got {other})"),
        };
        let buttons = blob.tiles / 6;
        for b in 0..buttons {
            let base = b * 6;
            println!(
                "-- button {b} (tiles {base}..{}) {} --",
                base + 5,
                if colmajor { "colmajor" } else { "rowmajor" }
            );
            for gy in 0..16usize {
                let mut line = String::new();
                for gx in 0..24usize {
                    let ti = if colmajor {
                        (gx / 8) * 2 + (gy / 8)
                    } else {
                        (gy / 8) * 3 + (gx / 8)
                    };
                    let tile = &dec[(base + ti) * TILE_BYTES..(base + ti + 1) * TILE_BYTES];
                    let v = get_index(tile, gx % 8, gy % 8);
                    line.push(if v == 0 {
                        '.'
                    } else {
                        std::char::from_digit(v as u32, 16).unwrap()
                    });
                }
                println!("  {line}");
            }
        }
    } else {
        // 타일별 8×8 그리드.
        for t in 0..blob.tiles {
            let tile = &dec[t * TILE_BYTES..(t + 1) * TILE_BYTES];
            println!("-- tile {t} --");
            for row in decode_tile(tile) {
                let line: String = row
                    .iter()
                    .map(|&v| {
                        if v == 0 {
                            '.'
                        } else {
                            std::char::from_digit(v as u32, 16).unwrap()
                        }
                    })
                    .collect();
                println!("  {line}");
            }
        }
    }
    Ok(())
}

/// 블록을 디컴프해 `labels`(버튼당 하나)로 재조판한 뒤, 재조판된 uncompressed 버퍼를
/// 반환한다. `labels`는 버튼 수와 같아야 한다.
pub fn retype_blob(
    rom: &[u8],
    blob: &UiBlob,
    labels: &[String],
    xs: Option<&[usize]>,
    font: &crate::font::GlyphFont,
) -> Result<Vec<u8>> {
    let (mut dec, _consumed) = lzss::decompress(rom, blob.compressed_addr);
    anyhow::ensure!(
        dec.len() == blob.uncompressed_len,
        "{}: 디컴프 크기 {} != 예상 {}",
        blob.name,
        dec.len(),
        blob.uncompressed_len
    );
    let buttons = blob.tiles / 6;
    anyhow::ensure!(
        labels.len() == buttons,
        "{}: 라벨 {}개 != 버튼 {}개",
        blob.name,
        labels.len(),
        buttons
    );
    for (b, label) in labels.iter().enumerate() {
        let base = b * 6;
        let tiles = [base, base + 1, base + 2, base + 3, base + 4, base + 5];
        let glyphs: Vec<[u8; 8]> = label.chars().map(|ch| font.render(ch)).collect();
        // 24px 버튼: 2글자는 중앙정렬, 3글자는 `xs` 명시 x위치(예: 일기N)로만 허용한다.
        anyhow::ensure!(
            glyphs.len() <= 2 || (glyphs.len() == 3 && xs.is_some()),
            "{}: 라벨 '{}'은 {}글자 — 24px 버튼은 2글자(중앙) 또는 3글자(xs 명시)까지",
            blob.name,
            label,
            glyphs.len()
        );
        if let Some(xs) = xs {
            anyhow::ensure!(
                xs.len() == glyphs.len(),
                "{}: 라벨 '{}' 글리프 {}개 != xs {}개",
                blob.name,
                label,
                glyphs.len(),
                xs.len()
            );
        }
        retype_button(&mut dec, &tiles, &glyphs, xs, blob.interior_box);
    }
    Ok(dec)
}

/// 한 UI 블록의 한글 라벨 세트(버튼당 하나).
pub struct UiLabelSet {
    /// `UI_BLOBS`의 블록 이름.
    pub blob: &'static str,
    /// 버튼당 한글 라벨(2글자 중앙정렬, 또는 `xs`와 함께 3글자).
    pub labels: &'static [&'static str],
    /// 버튼별 글리프 x위치 override. 3글자 라벨(예: "일기N")을 GG1식 우측 시프트 `[3,10,18]`로
    /// 좌측 프레임에서 떼는 데 쓴다. None이면 블록 중앙정렬(2글자 기본).
    pub xs: Option<&'static [usize]>,
}

/// in-place fit이 확인된 UI 라벨 세트(재조판 후 재압축 크기 ≤ 원본이라 포인터 수정 불필요).
///
/// 라벨은 자매 `gg_madou_3` 관례대로 2음절(버튼 내부 16px = 글리프 2개)로 맞췄다. 버튼
/// 순서(멀티버튼 correctness)는 EN 패치 `madoua/build.sh`의 rawgrpconv 타일 오프셋 + rsrc PNG로
/// 검증했다: compass tile0/6/12/18 = E/W/S/N(compass_e/w/s/n.png), magic_item tile0/6 = MAGIC/ITEM
/// (button03/04), yes_no tile0/6 = YES/NO(button11/12), buy_sell_leave tile0/6/12 = Buy/Sell/Leave
/// (button14="Buy"/13="Sell"/15="Leave"), title tile0/6 = start/continue(title_button01/02).
///
/// `buttons_file`(세이브 슬롯 = 일기1~4)은 GG1 관례(`FILE1~4 → 일기1~4`)를 따라 3글자 라벨을
/// `xs=[3,10,18]`(우측 2px 시프트)로 조판한다. A는 GG1과 달리 4개 독립 풀버튼(24타일)이라
/// 공유-타일 조립이 불필요하고, in-place fit이면 압축-로드 포인터 수정도 필요 없다.
/// 원본 슬롯을 넘는 `buttons_save`(저장)·`buttons_flee_lipemco`(대시/도망)는
/// `UI_RELOCATED_LABELS`에서 bank 44 리소스로 별도 처리한다.
pub const UI_LABELS: &[UiLabelSet] = &[
    UiLabelSet {
        blob: "buttons_map",
        labels: &["지도"],
        xs: None,
    },
    UiLabelSet {
        blob: "buttons_magic_item",
        labels: &["주문", "물건"],
        xs: None,
    },
    UiLabelSet {
        blob: "buttons_buy_sell_leave",
        labels: &["사기", "팔기", "가기"],
        xs: None,
    },
    UiLabelSet {
        blob: "buttons_yes_no",
        labels: &["예", "아니"],
        xs: None,
    },
    UiLabelSet {
        blob: "buttons_title",
        labels: &["시작", "계속"],
        xs: None,
    },
    UiLabelSet {
        blob: "compass",
        labels: &["동", "서", "남", "북"],
        xs: None,
    },
    UiLabelSet {
        blob: "buttons_file",
        labels: &["일기1", "일기2", "일기3", "일기4"],
        // 일=3(좌프레임에서 뗌), 기=9, 숫자=15. GG1 기본 [3,10,18]은 숫자가 우측 프레임에 너무
        // 붙어(x=18) A 프레임에선 밀려 보임 → 간격을 6/6으로 좁혀 숫자를 안쪽으로 당긴다.
        xs: Some(&[3, 9, 15]),
    },
];

/// 원본 압축 슬롯보다 커서 확장 리소스 뱅크로 옮기는 라벨.
const UI_RELOCATED_LABELS: &[UiLabelSet] = &[
    UiLabelSet {
        blob: "buttons_save",
        labels: &["저장"],
        xs: None,
    },
    UiLabelSet {
        blob: "buttons_flee_lipemco",
        labels: &["대시", "도망"],
        xs: None,
    },
];

/// bank 44에 들어갈 최소 리소스 뱅크를 만든다.
///
/// 게임의 `$1DD7/$1DE3` 로더 포맷을 그대로 따른다.
/// `[4B header][save ptr][flee ptr][padding to $10][02 C0 07][save LZSS]
///  [02 00 07][flee LZSS]`. `0x02` 디스크립터는 뒤의 16-bit VRAM 목적지로 압축 블록을
/// 전송한다. 포인터는 slot1 절대 주소다(header 첫 바이트 bit5=0).
fn build_relocated_ui_resource_bank(
    save_recomp: &[u8],
    flee_recomp: &[u8],
) -> Result<(Vec<u8>, usize, usize)> {
    let save_desc = UI_RELOC_FIRST_DESC_OFFSET;
    let save_stream = save_desc + 3;
    let flee_desc = save_stream + save_recomp.len();
    let flee_stream = flee_desc + 3;
    let end = flee_stream + flee_recomp.len();
    anyhow::ensure!(
        end <= 0x4000,
        "UI relocation 리소스 {}B가 16KB bank를 초과",
        end
    );

    let mut bank = vec![0xFF; end];
    // region repack과 같은 리소스 뱅크 매직. bit5=0이므로 table pointer는 slot1 절대주소다.
    bank[0..4].copy_from_slice(&[0x42, 0x30, 0x37, 0x07]);
    let save_ptr = SLOT1_BASE + save_desc as u16;
    let flee_ptr = SLOT1_BASE + flee_desc as u16;
    bank[4..6].copy_from_slice(&save_ptr.to_le_bytes());
    bank[6..8].copy_from_slice(&flee_ptr.to_le_bytes());

    bank[save_desc..save_stream].copy_from_slice(&[0x02, 0xC0, 0x07]);
    bank[save_stream..flee_desc].copy_from_slice(save_recomp);
    bank[flee_desc..flee_stream].copy_from_slice(&[0x02, 0x00, 0x07]);
    bank[flee_stream..end].copy_from_slice(flee_recomp);
    Ok((bank, save_stream, flee_stream))
}

fn assemble_ld_hl_asset_id(id: u16, origin: u16) -> [u8; 3] {
    let mut assembler = Assembler::new();
    assembler.emit(Instruction::LdRRImm(Register16::Hl, id));
    assembler
        .assemble(origin)
        .expect("LD HL, nn must assemble and decode exactly")
        .into_bytes()
        .try_into()
        .expect("LD HL, nn must encode to three bytes")
}

/// save/flee를 재조판·재압축해 확장 bank 44의 새 리소스로 만들고, 두 호출부의 asset id를
/// `$2C00/$2C01`로 바꾼다. 원본 bank 2 블록은 그대로 남겨 다른 경로를 손상시키지 않는다.
fn apply_relocated_ui_labels(rom: &mut TrackedRom, font: &crate::font::GlyphFont) -> Result<usize> {
    anyhow::ensure!(
        rom.len() >= UI_RELOC_BANK_BASE + 0x4000,
        "UI relocation은 1MB 확장 ROM이 필요함"
    );

    let save_set = &UI_RELOCATED_LABELS[0];
    let flee_set = &UI_RELOCATED_LABELS[1];
    let save_blob = blob_by_name(save_set.blob).expect("buttons_save inventory");
    let flee_blob = blob_by_name(flee_set.blob).expect("buttons_flee_lipemco inventory");
    let save_labels = save_set
        .labels
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let flee_labels = flee_set
        .labels
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let save_retyped = retype_blob(rom.data(), save_blob, &save_labels, save_set.xs, font)?;
    let flee_retyped = retype_blob(rom.data(), flee_blob, &flee_labels, flee_set.xs, font)?;
    let save_recomp = lzss::compress(&save_retyped);
    let flee_recomp = lzss::compress(&flee_retyped);

    let (bank, save_stream, flee_stream) =
        build_relocated_ui_resource_bank(&save_recomp, &flee_recomp)?;
    rom.write_expect(
        "ui relocated resource bank 44",
        UI_RELOC_BANK_BASE,
        &bank,
        &Expect::FreeSpace(0xFF),
    )?;
    rom.write_expect(
        "ui save asset id 0217 -> 2C00",
        SAVE_ASSET_LOAD_SITE,
        &assemble_ld_hl_asset_id(SAVE_RELOC_ASSET_ID, SAVE_ASSET_LOAD_LOGICAL),
        &Expect::Bytes(&SAVE_ASSET_LOAD_ORIG),
    )?;
    rom.write_expect(
        "ui flee asset id 0218 -> 2C01",
        FLEE_ASSET_LOAD_SITE,
        &assemble_ld_hl_asset_id(FLEE_RELOC_ASSET_ID, FLEE_ASSET_LOAD_LOGICAL),
        &Expect::Bytes(&FLEE_ASSET_LOAD_ORIG),
    )?;

    // 새 리소스가 실제 한글 타일로 다시 풀리는지 빌드 안에서 검증한다.
    let (save_back, _) = lzss::decompress(rom.data(), UI_RELOC_BANK_BASE + save_stream);
    let (flee_back, _) = lzss::decompress(rom.data(), UI_RELOC_BANK_BASE + flee_stream);
    anyhow::ensure!(save_back == save_retyped, "relocated save LZSS 검증 실패");
    anyhow::ensure!(flee_back == flee_retyped, "relocated flee LZSS 검증 실패");
    Ok(UI_RELOCATED_LABELS.len())
}

/// UI 라벨을 ROM에 적용한다. `UI_LABELS`는 원위치 안전쓰기하고, save/flee는 확장 bank 44
/// 리소스로 옮긴 뒤 호출부 asset id를 재지정한다.
///
/// 각 블록: 디컴프 → 버튼별 retype → 재압축 → 라운드트립 검증 → 재압축 크기 ≤ 원본 확인 →
/// `write_expect(Expect::Bytes(원본 앞부분))`로 in-place. 디코더는 `0x00` 종료 바이트에서
/// 멈추므로 더 짧은 재압축을 원위치에 써도 뒤 잔여 원본 바이트는 무해하다. 성공 시 패치한
/// 블록 수를 반환한다.
pub fn apply_ui_labels(rom: &mut TrackedRom, font: &crate::font::GlyphFont) -> Result<usize> {
    let mut patched = 0usize;
    for set in UI_LABELS {
        let blob = blob_by_name(set.blob)
            .with_context(|| format!("UI_LABELS: 알 수 없는 blob {}", set.blob))?;
        let labels: Vec<String> = set.labels.iter().map(|s| s.to_string()).collect();
        let retyped = retype_blob(rom.data(), blob, &labels, set.xs, font)?;
        let recomp = lzss::compress(&retyped);
        let (back, _) = lzss::decompress(&recomp, 0);
        anyhow::ensure!(
            back == retyped,
            "{}: 재조판 재압축 라운드트립 실패",
            blob.name
        );

        let (_, orig_clen) = lzss::decompress(rom.data(), blob.compressed_addr);
        anyhow::ensure!(
            recomp.len() <= orig_clen,
            "{}: 재조판 후 {}B > 원본 {}B — relocate 필요(UI_LABELS에서 제외돼야 함)",
            blob.name,
            recomp.len(),
            orig_clen
        );
        let original =
            rom.data()[blob.compressed_addr..blob.compressed_addr + recomp.len()].to_vec();
        rom.write_expect(
            &format!(
                "ui_label_inplace_{}_{:#07X}",
                blob.name, blob.compressed_addr
            ),
            blob.compressed_addr,
            &recomp,
            &Expect::Bytes(&original),
        )?;
        patched += 1;
    }
    patched += apply_relocated_ui_labels(rom, font)?;
    Ok(patched)
}

/// UI 라벨만 적용한 ROM을 빌드한다(트랙 B PoC 산출물). 대사/money와 독립적으로 UI 트랙을
/// end-to-end 검증한다.
pub fn cmd_build_ui(
    rom_path: &Path,
    font_path: &Path,
    output_path: &Path,
    bps_output_path: Option<&Path>,
    allow_noncanonical: bool,
) -> Result<()> {
    let data = std::fs::read(rom_path)
        .with_context(|| format!("ROM 읽기 실패: {}", rom_path.display()))?;
    crate::rom::require_canonical_crc(crc32fast::hash(&data), allow_noncanonical)?;
    let source = data.clone();
    let mut data = data;
    crate::rom::expand_rom_for_ko(&mut data);
    let original = data.clone();
    let mut rom = TrackedRom::new(data);
    let font = crate::font::GlyphFont::load(font_path, 8.0, 128, 0, 0)?;

    crate::rom::mark_rom_size_1mb(&mut rom, &original)?;
    let patched = apply_ui_labels(&mut rom, &font)?;
    rom.check_untracked_writes(&original)
        .map_err(|e| anyhow::anyhow!(e))?;

    let mut rom_data = rom.into_data();
    TmrSegaHeader::update_checksum(&mut rom_data);
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(output_path, &rom_data)
        .with_context(|| format!("출력 쓰기 실패: {}", output_path.display()))?;
    if let Some(path) = bps_output_path {
        crate::bps::write_bps(&source, &rom_data, path)?;
    }

    // 검증: 패치된 ROM의 각 블록이 재조판 타일로 디코드되는지.
    for set in UI_LABELS {
        let blob = blob_by_name(set.blob).unwrap();
        let (dec, _) = lzss::decompress(&rom_data, blob.compressed_addr);
        anyhow::ensure!(
            dec.len() == blob.uncompressed_len,
            "{}: 패치 후 디컴프 크기 불일치",
            blob.name
        );
    }

    println!("UI 빌드 완료: {}", output_path.display());
    if let Some(path) = bps_output_path {
        println!("BPS 패치: {}", path.display());
    }
    println!(
        "patched UI blobs: {patched} / {}",
        UI_LABELS.len() + UI_RELOCATED_LABELS.len()
    );
    println!("CRC32: {:08X}", crc32fast::hash(&rom_data));
    Ok(())
}

/// 재조판 결과를 조립 렌더하고 재압축 fit을 보고한다(빌드 전 시각 검증).
pub fn cmd_preview_ui_retype(
    rom_path: &Path,
    name: &str,
    labels_csv: &str,
    font_path: &Path,
) -> Result<()> {
    let rom = std::fs::read(rom_path)
        .with_context(|| format!("ROM 읽기 실패: {}", rom_path.display()))?;
    let blob = blob_by_name(name).with_context(|| format!("알 수 없는 UI blob: {name}"))?;
    let font = crate::font::GlyphFont::load(font_path, 8.0, 128, 0, 0)?;
    let labels: Vec<String> = labels_csv
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();

    let (_, orig_clen) = lzss::decompress(&rom, blob.compressed_addr);
    let retyped = retype_blob(&rom, blob, &labels, None, &font)?;
    let recomp = lzss::compress(&retyped);
    let (back, _) = lzss::decompress(&recomp, 0);
    anyhow::ensure!(back == retyped, "재조판 블록 재압축이 라운드트립 실패");

    println!(
        "{} 재조판 [{}]  recomp={}B vs orig={}B  in-place={}",
        blob.name,
        labels_csv,
        recomp.len(),
        orig_clen,
        if recomp.len() <= orig_clen {
            "OK"
        } else {
            "NO(relocate 필요)"
        }
    );
    let buttons = blob.tiles / 6;
    for b in 0..buttons {
        let base = b * 6;
        println!("-- button {b} [{}] --", labels[b]);
        for gy in 0..16usize {
            let mut line = String::new();
            for gx in 0..24usize {
                let ti = (gx / 8) * 2 + (gy / 8);
                let tile = &retyped[(base + ti) * TILE_BYTES..(base + ti + 1) * TILE_BYTES];
                let v = get_index(tile, gx % 8, gy % 8);
                line.push(if v == 0 {
                    '.'
                } else {
                    std::char::from_digit(v as u32, 16).unwrap()
                });
            }
            println!("  {line}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_codec_roundtrips_index() {
        let mut tile = [0u8; TILE_BYTES];
        for y in 0..8 {
            for x in 0..8 {
                set_index(&mut tile, x, y, ((x + y * 8) % 16) as u8);
            }
        }
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(get_index(&tile, x, y), ((x + y * 8) % 16) as u8);
            }
        }
    }

    #[test]
    fn inventory_sizes_are_consistent() {
        for blob in UI_BLOBS {
            assert_eq!(
                blob.uncompressed_len,
                blob.tiles * TILE_BYTES,
                "{} uncompressed_len must equal tiles*32",
                blob.name
            );
        }
    }

    #[test]
    fn ui_labels_match_blob_button_counts() {
        for set in UI_LABELS.iter().chain(UI_RELOCATED_LABELS) {
            let blob = blob_by_name(set.blob).expect("UI_LABELS blob must exist");
            assert_eq!(
                set.labels.len(),
                blob.tiles / 6,
                "{}: label count must equal button count",
                blob.name
            );
            for label in set.labels {
                let n = label.chars().count();
                // 24px 버튼: 2글자는 중앙정렬, 3글자는 xs 명시 x위치(예: 일기N)로만 허용.
                assert!(
                    n <= 2 || (n == 3 && set.xs.is_some()),
                    "{}: label '{label}' exceeds 2 glyphs without xs",
                    blob.name
                );
                if let Some(xs) = set.xs {
                    assert_eq!(
                        xs.len(),
                        n,
                        "{}: label '{label}' glyphs {n} != xs {}",
                        blob.name,
                        xs.len()
                    );
                }
            }
        }
    }

    #[test]
    fn relocated_resource_bank_has_valid_table_descriptors_and_streams() {
        let save_raw = vec![0x11; 192];
        let flee_raw = (0..384).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        let save = lzss::compress(&save_raw);
        let flee = lzss::compress(&flee_raw);
        let (bank, save_stream, flee_stream) =
            build_relocated_ui_resource_bank(&save, &flee).unwrap();

        assert_eq!(&bank[0..4], &[0x42, 0x30, 0x37, 0x07]);
        assert_eq!(u16::from_le_bytes([bank[4], bank[5]]), 0x4010);
        let flee_desc = flee_stream - 3;
        assert_eq!(
            u16::from_le_bytes([bank[6], bank[7]]),
            SLOT1_BASE + flee_desc as u16
        );
        assert_eq!(&bank[save_stream - 3..save_stream], &[0x02, 0xC0, 0x07]);
        assert_eq!(&bank[flee_stream - 3..flee_stream], &[0x02, 0x00, 0x07]);
        assert_eq!(lzss::decompress(&bank, save_stream).0, save_raw);
        assert_eq!(lzss::decompress(&bank, flee_stream).0, flee_raw);
    }

    #[test]
    fn retype_clears_interior_and_stamps_ink() {
        // 6타일(1버튼) 버퍼를 임의 인덱스로 채운 뒤 retype: 내부 박스는 BG, 스탬프한 획은 INK.
        let mut buf = vec![0x00u8; 6 * TILE_BYTES];
        for t in 0..6 {
            for y in 0..8 {
                for x in 0..8 {
                    set_index(&mut buf[t * TILE_BYTES..(t + 1) * TILE_BYTES], x, y, 7);
                }
            }
        }
        // 전 픽셀이 켜진 글리프 하나 → 스탬프 영역이 INK가 된다.
        let glyph = [0xFFu8; 8];
        retype_button(
            &mut buf,
            &[0, 1, 2, 3, 4, 5],
            &[glyph],
            None,
            DEFAULT_INTERIOR_BOX,
        );

        // interior_box 안의 한 픽셀은 BG 또는 INK(원래 7이 아님).
        let (r0, _r1, c0, _c1) = DEFAULT_INTERIOR_BOX;
        let ti = (c0 / 8) * 2 + (r0 / 8);
        let v = get_index(&buf[ti * TILE_BYTES..(ti + 1) * TILE_BYTES], c0 % 8, r0 % 8);
        assert!(
            v == BG_INDEX || v == INK_INDEX,
            "interior must be retyped, got {v}"
        );

        // 프레임 영역(col0,row0)은 손대지 않음(여전히 7).
        assert_eq!(
            get_index(&buf[0..TILE_BYTES], 0, 0),
            7,
            "frame corner preserved"
        );
    }

    #[test]
    fn compass_retype_clears_full_jp_height_and_preserves_side_frames() {
        let compass = blob_by_name("compass").expect("compass inventory");
        assert_eq!(compass.interior_box, COMPASS_INTERIOR_BOX);

        let mut buf = vec![0x00u8; 6 * TILE_BYTES];
        for t in 0..6 {
            for y in 0..8 {
                for x in 0..8 {
                    set_index(&mut buf[t * TILE_BYTES..(t + 1) * TILE_BYTES], x, y, 7);
                }
            }
        }
        retype_button(
            &mut buf,
            &[0, 1, 2, 3, 4, 5],
            &[],
            None,
            compass.interior_box,
        );

        for gy in 3..13 {
            for gx in 4..20 {
                let ti = (gx / 8) * 2 + (gy / 8);
                assert_eq!(
                    get_index(&buf[ti * TILE_BYTES..(ti + 1) * TILE_BYTES], gx % 8, gy % 8),
                    BG_INDEX,
                    "compass interior must be cleared at ({gx},{gy})"
                );
            }
            for gx in [3usize, 20] {
                let ti = (gx / 8) * 2 + (gy / 8);
                assert_eq!(
                    get_index(&buf[ti * TILE_BYTES..(ti + 1) * TILE_BYTES], gx % 8, gy % 8),
                    7,
                    "compass side frame must be preserved at ({gx},{gy})"
                );
            }
        }
    }

    #[test]
    fn default_retype_clears_full_source_bounds_and_preserves_outer_band() {
        let mut buf = vec![0x00u8; 6 * TILE_BYTES];
        for t in 0..6 {
            for y in 0..8 {
                for x in 0..8 {
                    set_index(&mut buf[t * TILE_BYTES..(t + 1) * TILE_BYTES], x, y, 7);
                }
            }
        }
        retype_button(
            &mut buf,
            &[0, 1, 2, 3, 4, 5],
            &[],
            None,
            DEFAULT_INTERIOR_BOX,
        );

        for gy in 3..13 {
            for gx in 3..21 {
                let ti = (gx / 8) * 2 + (gy / 8);
                assert_eq!(
                    get_index(&buf[ti * TILE_BYTES..(ti + 1) * TILE_BYTES], gx % 8, gy % 8),
                    BG_INDEX,
                    "default interior must be cleared at ({gx},{gy})"
                );
            }
            for gx in [2usize, 21] {
                let ti = (gx / 8) * 2 + (gy / 8);
                assert_eq!(
                    get_index(&buf[ti * TILE_BYTES..(ti + 1) * TILE_BYTES], gx % 8, gy % 8),
                    7,
                    "default outer band must be preserved at ({gx},{gy})"
                );
            }
        }
    }
}

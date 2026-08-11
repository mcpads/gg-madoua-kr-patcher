//! 게임기어 ROM 식별·헤더 파싱.
//!
//! TMR SEGA 헤더는 오프셋 0x7FF0(16바이트). 0x7FFA에 16비트 체크섬,
//! 0x7FFF 상위 니블=리전(0x5=GG Japan, 0x7=GG International), 하위 니블=크기 코드.

use crate::glyph;
use anyhow::{Context, Result};
use gg_sms::header::TmrSegaHeader;
use gg_sms::rom::{Expect, TrackedRom};
use std::collections::BTreeMap;
use std::path::Path;

/// 분석의 그라운드 트루스가 되는 JP 원본(No-Intro: Madou Monogatari A - Dokidoki Vacation (Japan)).
pub const JP_CRC32: u32 = 0x7EC9_5282;
pub const JP_MD5: &str = "ab0d1eb20ac63a984d874a885ca2588d";
pub const JP_SIZE: usize = 512 * 1024;

/// KO 글리프 뱅크(bank 32)를 담기 위한 확장 ROM 크기(1MB). A의 EN 패치·gg_madou_1도 1MB.
pub(crate) const EXPANDED_ROM_SIZE: usize = 0x10_0000;

/// TMR SEGA 헤더의 ROM 크기 코드 바이트(0x7FFF, 하위 니블=크기, 상위 니블=리전).
const ROM_SIZE_CODE_OFFSET: usize = 0x7FFF;

/// ROM 데이터를 1MB로 확장한다(0xFF padding). 글리프 뱅크 이하 자유공간 확보.
pub(crate) fn expand_rom_for_ko(data: &mut Vec<u8>) {
    if data.len() < EXPANDED_ROM_SIZE {
        data.resize(EXPANDED_ROM_SIZE, 0xFF);
    }
}

/// 1MB로 확장한 ROM의 TMR SEGA 크기 코드를 갱신한다(리전 상위 니블 보존).
pub(crate) fn mark_rom_size_1mb(rom: &mut TrackedRom, original: &[u8]) -> Result<()> {
    let size_byte = original[ROM_SIZE_CODE_OFFSET];
    rom.write_expect(
        "rom size code 1MB",
        ROM_SIZE_CODE_OFFSET,
        &[(size_byte & 0xF0) | 0x02],
        &Expect::Bytes(&[size_byte]),
    )
    .map_err(|e| anyhow::anyhow!(e))
}

pub fn cmd_info(path: &Path) -> Result<()> {
    let data = std::fs::read(path).with_context(|| format!("ROM 읽기 실패: {}", path.display()))?;

    let crc = crc32fast::hash(&data);
    let md5 = {
        use md5::{Digest, Md5};
        let mut h = Md5::new();
        h.update(&data);
        h.finalize()
    };
    let md5_hex: String = md5.iter().map(|b| format!("{b:02x}")).collect();

    println!("파일      : {}", path.display());
    println!(
        "크기      : {} bytes ({} KB)",
        data.len(),
        data.len() / 1024
    );
    println!("CRC32     : {crc:08X}");
    println!("MD5       : {md5_hex}");

    if data.len() != JP_SIZE {
        println!("주의      : 크기가 JP 원본({JP_SIZE} bytes)과 다름");
    }
    let jp_match = crc == JP_CRC32 && md5_hex == JP_MD5;
    println!(
        "JP 원본   : {}",
        if jp_match {
            "일치 ✓"
        } else {
            "불일치 — 오프셋 전제가 깨질 수 있음"
        }
    );

    if data.len() >= 0x8000 {
        let magic = &data[0x7FF0..0x7FF8];
        let header_ok = magic == b"TMR SEGA";
        println!(
            "TMR SEGA  : {}",
            if header_ok {
                "확인 ✓".into()
            } else {
                format!("미확인 ({magic:02X?})")
            }
        );
        let checksum = u16::from_le_bytes([data[0x7FFA], data[0x7FFB]]);
        let region_size = data[0x7FFF];
        println!("헤더 체크섬: {checksum:04X}");
        println!(
            "리전/크기 : {:X} / {:X}",
            region_size >> 4,
            region_size & 0x0F
        );
    }

    Ok(())
}

/// PoC: 폰트 슬롯 하나를 한글 '가'로 교체한 ROM을 만든다.
pub fn cmd_poc_patch(rom_path: &Path, out_path: &Path, index: usize) -> Result<()> {
    let data = std::fs::read(rom_path)
        .with_context(|| format!("ROM 읽기 실패: {}", rom_path.display()))?;
    let mut data = data;
    expand_rom_for_ko(&mut data);
    let original = data.clone();
    let mut rom = TrackedRom::new(data);

    let crc = crc32fast::hash(rom.data());
    if crc != JP_CRC32 {
        eprintln!(
            "주의: 입력 CRC32 {crc:08X} 가 JP 원본({JP_CRC32:08X})과 다름 — 오프셋 전제가 깨질 수 있음"
        );
    }

    let off = glyph::FONT_BASE + index * glyph::GLYPH_BYTES;
    anyhow::ensure!(
        off + glyph::GLYPH_BYTES <= rom.len(),
        "폰트 슬롯이 ROM 범위를 벗어남"
    );

    let before: [u8; glyph::GLYPH_BYTES] = rom[off..off + glyph::GLYPH_BYTES].try_into().unwrap();
    rom.write_expect(
        "poc1 glyph replacement",
        off,
        &glyph::POC_GA,
        &Expect::Bytes(&before),
    )?;
    ensure_all_writes_tracked(&rom, &original)?;

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(out_path, rom.data())
        .with_context(|| format!("출력 쓰기 실패: {}", out_path.display()))?;

    println!("폰트 슬롯 0x{index:02X} (ROM 0x{off:05X}) 교체");
    println!("--- 교체 전 ---\n{}", glyph::to_ascii(&before));
    println!("--- 교체 후 (가) ---\n{}", glyph::to_ascii(&glyph::POC_GA));
    println!("출력: {}", out_path.display());
    Ok(())
}

/// 인트로 대사 "いう"가 들어 있는 스크립트 오프셋(JP 코드 0x10 0x11).
const POC2_SCRIPT_EDIT: usize = 0x1A694;

/// 기대값 검증 후 쓰기 — 분석이 틀렸으면 즉시 실패시킨다(conventions §5.2).
fn write_checked(
    rom: &mut TrackedRom,
    label: &str,
    off: usize,
    expect: &[u8],
    new: &[u8],
) -> Result<()> {
    rom.write_expect(label, off, new, &Expect::Bytes(expect))?;
    Ok(())
}

/// 자유공간(0xFF)임을 확인한 뒤 쓰기.
fn write_free(rom: &mut TrackedRom, label: &str, off: usize, new: &[u8]) -> Result<()> {
    rom.write_expect(label, off, new, &Expect::FreeSpace(0xFF))?;
    Ok(())
}

fn ensure_all_writes_tracked(rom: &TrackedRom, original: &[u8]) -> Result<()> {
    rom.check_untracked_writes(original)
        .map_err(|e| anyhow::anyhow!(e))
}

/// 프리픽스 디스패치 훅(핸들러 + call-site 패치)을 설치한다. PoC 공용.
fn install_ko_hook(rom: &mut TrackedRom) -> Result<()> {
    write_free(
        rom,
        "poc ko handler",
        glyph::KO_HANDLER_PHYS,
        &glyph::ko_handler_bytes(),
    )?;
    let replacement = glyph::assemble_ko_handler_call();
    write_checked(
        rom,
        "poc hook call-site",
        glyph::HOOK_CALL_SITE,
        &[0xCD, 0x3E, 0x9A],
        &replacement,
    )
}

/// 2-iteration 프리픽스 훅을 설치한다.
///
/// - `$99A4`의 `cp FB; jr nz,$99F0`를 확장 디스패처로 보낸다.
/// - `$99F1`의 원본 폰트 확장 호출은 `C bit6`가 켜진 경우만 한글 뱅크를 읽는다.
fn install_ko_prefix2_hook(rom: &mut TrackedRom) -> Result<()> {
    write_free(
        rom,
        "ko glyph hook handler",
        glyph::KO_HANDLER_PHYS,
        &glyph::ko_prefix2_handler_bytes(),
    )?;
    // 공유 글리프 렌더 $9A3E 엔트리를 핸들러로 후킹(모든 렌더 루프 커버).
    let replacement = glyph::assemble_ko_handler_jump();
    write_checked(
        rom,
        "ko glyph render hook",
        glyph::GLYPH_RENDER_HOOK_SITE,
        &glyph::GLYPH_RENDER_HOOK_ORIG,
        &replacement,
    )
}

/// 2차 PoC: 프리픽스 디스패치 훅으로 한글 전용 코드포인트를 검증한다.
pub fn cmd_poc2_patch(rom_path: &Path, out_path: &Path) -> Result<()> {
    let data = std::fs::read(rom_path)
        .with_context(|| format!("ROM 읽기 실패: {}", rom_path.display()))?;
    let mut data = data;
    expand_rom_for_ko(&mut data);
    let original = data.clone();
    let mut rom = TrackedRom::new(data);
    if crc32fast::hash(rom.data()) != JP_CRC32 {
        eprintln!("주의: 입력이 JP 원본과 다름");
    }
    write_free(
        &mut rom,
        "poc2 ko glyph bank",
        glyph::KO_BANK_PHYS,
        &glyph::POC_GA,
    )?; // 인덱스 0 = '가'
    install_ko_hook(&mut rom)?;
    write_checked(
        &mut rom,
        "poc2 script edit",
        POC2_SCRIPT_EDIT,
        &[0x10, 0x11],
        &[glyph::KO_PREFIX, 0x00],
    )?;
    ensure_all_writes_tracked(&rom, &original)?;

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(out_path, rom.data())?;
    println!("2차 PoC 완료 — 0x{POC2_SCRIPT_EDIT:05X} いう → [0xA3 0x00] (손도안 '가')");
    println!("출력: {}", out_path.display());
    Ok(())
}

/// 2차 fixed PoC: 탁점처럼 prefix/base를 2 iteration으로 처리한다.
pub fn cmd_poc2_fixed_patch(rom_path: &Path, out_path: &Path) -> Result<()> {
    let data = std::fs::read(rom_path)
        .with_context(|| format!("ROM 읽기 실패: {}", rom_path.display()))?;
    let mut data = data;
    expand_rom_for_ko(&mut data);
    let original = data.clone();
    let mut rom = TrackedRom::new(data);
    if crc32fast::hash(rom.data()) != JP_CRC32 {
        eprintln!("주의: 입력이 JP 원본과 다름");
    }
    write_free(
        &mut rom,
        "poc2 fixed ko glyph bank",
        glyph::KO_BANK_PHYS,
        &glyph::POC_GA,
    )?;
    install_ko_prefix2_hook(&mut rom)?;
    write_checked(
        &mut rom,
        "poc2 fixed script edit",
        POC2_SCRIPT_EDIT,
        &[0x10, 0x11],
        &[glyph::KO_PREFIX, 0x01],
    )?;
    ensure_all_writes_tracked(&rom, &original)?;

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(out_path, rom.data())?;
    println!("2차 fixed PoC 완료 — 0x{POC2_SCRIPT_EDIT:05X} いう → [0xA3 0x01] (2-iteration '가')");
    println!("출력: {}", out_path.display());
    Ok(())
}

/// 3차 PoC: 실제 폰트(Galmuri)로 한글 단어를 렌더해 인게임에 표시(TTF→화면 end-to-end).
/// 인트로 2행 "コトきく"(4바이트)를 한글 2음절로 치환한다.
pub fn cmd_poc3_patch(rom_path: &Path, out_path: &Path, ttf: &Path, word: &str) -> Result<()> {
    let data = std::fs::read(rom_path)
        .with_context(|| format!("ROM 읽기 실패: {}", rom_path.display()))?;
    let mut data = data;
    expand_rom_for_ko(&mut data);
    let original = data.clone();
    let mut rom = TrackedRom::new(data);
    if crc32fast::hash(rom.data()) != JP_CRC32 {
        eprintln!("주의: 입력이 JP 원본과 다름");
    }

    let chars: Vec<char> = word.chars().collect();
    anyhow::ensure!(
        chars.len() == 2,
        "이 PoC는 2음절 단어를 받는다 (4바이트 치환)"
    );

    // 실제 폰트로 2글자 렌더 → 한글 뱅크 인덱스 0,1
    let f = crate::font::GlyphFont::load(ttf, 8.0, 128, 0, 0)?;
    let mut bank = Vec::new();
    for &c in &chars {
        bank.extend_from_slice(&f.render(c));
    }
    write_free(&mut rom, "poc3 ko glyph bank", glyph::KO_BANK_PHYS, &bank)?;
    install_ko_hook(&mut rom)?;

    // 인트로 2행 "コトきく" (4F 59 15 16) → [A3 00][A3 01]
    write_checked(
        &mut rom,
        "poc3 script edit",
        0x1A697,
        &[0x4F, 0x59, 0x15, 0x16],
        &[glyph::KO_PREFIX, 0x00, glyph::KO_PREFIX, 0x01],
    )?;
    ensure_all_writes_tracked(&rom, &original)?;

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(out_path, rom.data())?;
    println!("3차 PoC 완료 — 0x1A697 \"コトきく\" → \"{word}\" (Galmuri 실폰트)");
    for (i, &c) in chars.iter().enumerate() {
        println!(
            "--- {c} (인덱스 {i}) ---\n{}",
            crate::font::to_ascii(&f.render(c))
        );
    }
    println!("출력: {}", out_path.display());
    Ok(())
}

/// 3차 fixed PoC: 실제 폰트 2음절을 2-iteration prefix/base 모델로 표시한다.
pub fn cmd_poc3_fixed_patch(
    rom_path: &Path,
    out_path: &Path,
    ttf: &Path,
    word: &str,
) -> Result<()> {
    let data = std::fs::read(rom_path)
        .with_context(|| format!("ROM 읽기 실패: {}", rom_path.display()))?;
    let mut data = data;
    expand_rom_for_ko(&mut data);
    let original = data.clone();
    let mut rom = TrackedRom::new(data);
    if crc32fast::hash(rom.data()) != JP_CRC32 {
        eprintln!("주의: 입력이 JP 원본과 다름");
    }

    let chars: Vec<char> = word.chars().collect();
    anyhow::ensure!(
        chars.len() == 2,
        "이 PoC는 2음절 단어를 받는다 (4바이트 치환)"
    );

    let f = crate::font::GlyphFont::load(ttf, 8.0, 128, 0, 0)?;
    let mut bank = Vec::new();
    for &c in &chars {
        bank.extend_from_slice(&f.render(c));
    }
    write_free(
        &mut rom,
        "poc3 fixed ko glyph bank",
        glyph::KO_BANK_PHYS,
        &bank,
    )?;
    install_ko_prefix2_hook(&mut rom)?;

    write_checked(
        &mut rom,
        "poc3 fixed script edit",
        0x1A697,
        &[0x4F, 0x59, 0x15, 0x16],
        &[glyph::KO_PREFIX, 0x01, glyph::KO_PREFIX, 0x02],
    )?;
    ensure_all_writes_tracked(&rom, &original)?;

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(out_path, rom.data())?;
    println!("3차 fixed PoC 완료 — 0x1A697 \"コトきく\" → \"{word}\" (2-iteration)");
    for (i, &c) in chars.iter().enumerate() {
        println!(
            "--- {c} (base byte 0x{:02X}) ---\n{}",
            i + 1,
            crate::font::to_ascii(&f.render(c))
        );
    }
    println!("출력: {}", out_path.display());
    Ok(())
}

/// HUD/상점 돈 단위 `金`(한자) 글리프를 한글 `금`으로 교체한다(UI 트랙 A).
///
/// `0x19B55`의 1bpp 8바이트를 게임이 1bpp→4bpp 확장해 배경 nametable 타일 0x25로
/// 직접 업로드한다(대사 char 엔진 밖 HUD 그래픽 경로 → 텍스트 번역이 못 닿음). 상점
/// 텍스트 byte 0x0C와 소스 공유라 한 번 교체로 둘 다 갱신된다. 0원칙: 한글 패치에 JP
/// 한자 노출은 블로커. 모든 빌드의 baseline 로컬라이제이션 패치다.
fn patch_money_glyph(rom: &mut TrackedRom) -> Result<()> {
    write_checked(
        rom,
        "money kanji 金 → hangul 금",
        glyph::MONEY_KANJI_GLYPH_ADDR,
        &glyph::MONEY_KANJI_JP,
        &glyph::MONEY_GLYPH_KO_GEUM,
    )
}

/// region repack 목적지 뱅크 시작(확장 ROM). region N → bank `REGION_REPACK_BASE + N`. bank 32는
/// 글리프 뱅크, 33~43이 11개 region.
const REGION_REPACK_BASE: u8 = 33;
/// repack 뱅크에서 문자열 포인터 테이블이 시작하는 슬롯 1 오프셋(리소스 헤더 영역 뒤).
const REGION_REPACK_TABLE_OFFSET: u16 = 0x40;

/// region 대사를 EN 패치 포맷대로 전용 확장 뱅크로 옮긴다(데이터-only base-bank redirect).
///
/// **EN 오라클 검증 포맷**(`madoua/asm/main.s` `scriptRegionN`): fresh 뱅크 =
/// `[42 30 37 07 매직][.dw 테이블ptr=0x4010][0xFF×10 filler][compact bin: 테이블+문자열]`,
/// loc 테이블 = `(rsrc_id=0, 새 뱅크)`. 매직 첫 바이트 bit5=0 → pointer 테이블
/// (`getPointerFromMultiTable $1DE3`가 판별). 스톡 로드 코드는 `bank+4+rsrc_id*2`(=bank+4)에서
/// 테이블 포인터를 읽는다.
///
/// **공유 리소스**(메뉴/아이템/마법명 등, 원본 뱅크 5·7의 rsrc 0-20 등)는 **활성 region의 loc[1]로
/// 읽히지 않는다** — EN이 그 뱅크를 안 옮겨도 정상 동작하는 것으로 확정. 따라서 fresh 뱅크엔 대사만
/// 넣고 공유는 원본 뱅크에 그대로 둔다. 16KB 전용 뱅크라 용량·fragmentation 문제가 없다.
fn repack_region(
    rom: &mut TrackedRom,
    region: usize,
    strings: &[Vec<u8>],
    target_bank: u8,
) -> Result<()> {
    let fresh_base = target_bank as usize * 0x4000;
    // fresh 뱅크를 메모리에서 조립(마지막에 한 번 write_free — 대사만, 공유 리소스 없음).
    let mut fresh = vec![0xFFu8; 0x4000];
    // EN 리소스 테이블 매직(첫 바이트 bit5=0 → pointer 테이블).
    fresh[0..4].copy_from_slice(&[0x42, 0x30, 0x37, 0x07]);
    // rsrc_id=0 헤더 슬롯 → 테이블 포인터(슬롯1+0x10). 6..0x10은 filler 0xFF 유지.
    let table_slot = crate::script::SLOT1_BASE + 0x10;
    fresh[4..6].copy_from_slice(&table_slot.to_le_bytes());
    // compact bin(테이블 count*2 + 문자열)을 0x10에 배치. 빈 엔트리의 원본 포인터(money WRAM
    // $C8BF 등)를 verbatim 보존하려고 원본 테이블 포인터를 함께 넘긴다. loc redirect는 아래에서
    // 하므로 이 시점 loc[region]은 원본이고, region 뱅크 테이블도 pristine이다.
    let orig_pointers = crate::script::region_original_pointers(rom.data(), region)?;
    let bin = crate::script::build_region_repack_bin(strings, &orig_pointers, 0x10)?;
    anyhow::ensure!(
        0x10 + bin.len() <= 0x4000,
        "region {region} 대사 bin({}B)이 16KB 뱅크를 초과",
        bin.len()
    );
    fresh[0x10..0x10 + bin.len()].copy_from_slice(&bin);
    write_free(
        rom,
        &format!("region {region} repack bank"),
        fresh_base,
        &fresh,
    )?;

    // loc 테이블 = (rsrc_id=0, 새 뱅크). 원본 2바이트를 expect로 검증.
    let loc = crate::script::REGION_LOC_TABLE + region * 2;
    let orig = [rom.data()[loc], rom.data()[loc + 1]];
    write_checked(
        rom,
        &format!("region {region} loc redirect"),
        loc,
        &orig,
        &[0x00, target_bank],
    )?;
    Ok(())
}

/// cutscene 첫 문자열 물리 오프셋(seg A 시작). JP 구조.
const CUTSCENE_SEG_A_START: usize = 0x1A66D;
/// cutscene reloc 구간(seg B, 핸들러 뒤 bank 6 자유공간).
const CUTSCENE_SEG_B_END: usize = 0x1C000;

/// 현재 바이트를 기대값으로 삼아 덮어쓴다(비-0xFF 영역 재배치용, tracked).
fn overwrite_tracked(rom: &mut TrackedRom, label: &str, off: usize, new: &[u8]) -> Result<()> {
    let cur = rom.data()[off..off + new.len()].to_vec();
    rom.write_expect(label, off, new, &Expect::Bytes(&cur))?;
    Ok(())
}

/// cutscene 텍스트를 bank 6 안에서 compact 재배치한다. cutscene는 코드와 슬롯 2를 공유해 다른
/// 뱅크로 옮길 수 없으므로(로더가 `LD DE,$B162`로 슬롯 2 테이블을 읽음), bank 6의 두 자유
/// 구간(문자열영역 seg A: 첫 문자열~테이블, reloc영역 seg B: 핸들러 뒤)에 문자열을 채우고
/// 테이블(제자리)을 재작성한다. 코드영역(테이블 뒤~핸들러)은 건드리지 않는다.
fn repack_cutscenes(
    rom: &mut TrackedRom,
    entries_by_id: &BTreeMap<String, crate::script::ScriptEntry>,
    plan_by_id: &BTreeMap<&str, &crate::translation::EncodedOverride>,
) -> Result<usize> {
    let count = crate::script::CUTSCENE_COUNT;
    let table_addr = crate::script::CUTSCENE_POINTER_TABLE;
    let seg_b_start = glyph::KO_CUTSCENE_RELOC_START;
    let segs = [
        (CUTSCENE_SEG_A_START, table_addr),
        (seg_b_start, CUTSCENE_SEG_B_END),
    ];

    let mut strings: Vec<Vec<u8>> = Vec::with_capacity(count);
    for i in 0..count {
        let id = format!("cutscene/{i:03X}");
        let bytes = if let Some(enc) = plan_by_id.get(id.as_str()) {
            let entry = entries_by_id
                .get(&id)
                .with_context(|| format!("cutscene repack: 알 수 없는 id {id}"))?;
            validate_translation_source(entry, enc)?;
            enc.bytes.clone()
        } else if let Some(entry) = entries_by_id.get(&id) {
            crate::script::raw_entry_bytes(entry)?
        } else {
            Vec::new()
        };
        strings.push(bytes);
    }

    // 세그먼트에 순서대로 채우며 각 인덱스의 물리 주소를 기록.
    let mut addrs = vec![0usize; count];
    let mut seg = 0usize;
    let mut cur = segs[0].0;
    for (i, s) in strings.iter().enumerate() {
        if s.is_empty() {
            continue;
        }
        if cur + s.len() > segs[seg].1 {
            seg += 1;
            anyhow::ensure!(seg < segs.len(), "cutscene repack: bank 6 자유공간 초과");
            cur = segs[seg].0;
        }
        anyhow::ensure!(
            cur + s.len() <= segs[seg].1,
            "cutscene repack: 문자열 {i:03X}이 세그먼트를 초과"
        );
        addrs[i] = cur;
        cur += s.len();
    }

    // 문자열 쓰기(seg A는 덮어쓰기, seg B는 자유공간).
    for (i, s) in strings.iter().enumerate() {
        if s.is_empty() {
            continue;
        }
        let off = addrs[i];
        if off >= seg_b_start {
            write_free(rom, &format!("cutscene {i:03X} repack"), off, s)?;
        } else {
            overwrite_tracked(rom, &format!("cutscene {i:03X} repack"), off, s)?;
        }
    }

    // 테이블 재작성(제자리 덮어쓰기): 슬롯 2 포인터.
    let mut table = Vec::with_capacity(count * 2);
    for i in 0..count {
        let ptr = if strings[i].is_empty() {
            0u16
        } else {
            crate::script::cutscene_pointer_for_physical(addrs[i])?
        };
        table.extend_from_slice(&ptr.to_le_bytes());
    }
    overwrite_tracked(rom, "cutscene table repack", table_addr, &table)?;
    Ok(count)
}

/// 입력 ROM CRC가 canonical JP 원본인지 판정한다. 다르면 `allow_noncanonical`이 참일 때만
/// 경고하고 통과하고, 아니면 실패한다. JP 고정 오프셋 전제를 쓰는 산출물 생성 명령의 공통 가드다.
pub(crate) fn require_canonical_crc(crc: u32, allow_noncanonical: bool) -> Result<()> {
    if crc == JP_CRC32 {
        return Ok(());
    }
    if allow_noncanonical {
        eprintln!(
            "주의: 입력 CRC32 {crc:08X}가 JP 원본({JP_CRC32:08X})과 다름 — --allow-noncanonical-source로 계속함"
        );
        return Ok(());
    }
    anyhow::bail!(
        "입력 CRC32 {crc:08X}가 JP 원본({JP_CRC32:08X})과 다름. JP 원본으로 빌드하거나 --allow-noncanonical-source를 명시하라"
    )
}

/// KO 글리프 뱅크(bank 32)를 담아야 하는 빌드(has_glyphs)일 때만 ROM을 확장 크기로 늘린다.
/// 글리프가 없는 빌드는 원본 크기를 유지한다 — 1MB로 부풀리면서 크기 헤더는 안 고치는 불일치를 막는다.
fn expanded_source(mut data: Vec<u8>, has_glyphs: bool) -> Vec<u8> {
    if has_glyphs {
        expand_rom_for_ko(&mut data);
    }
    data
}

pub fn cmd_build(
    rom_path: &Path,
    translations_path: &Path,
    font_path: &Path,
    output_path: &Path,
    bps_output_path: Option<&Path>,
    allow_noncanonical: bool,
    preview_human_review: bool,
) -> Result<()> {
    let data = std::fs::read(rom_path)
        .with_context(|| format!("ROM 읽기 실패: {}", rom_path.display()))?;
    let jp_original = data.clone();
    require_canonical_crc(crc32fast::hash(&jp_original), allow_noncanonical)?;

    let plan = if preview_human_review {
        crate::translation::load_human_review_preview_plan(translations_path)?
    } else {
        crate::translation::load_translation_plan(translations_path)?
    };
    // 디렉토리(배포) 빌드는 모든 엔트리의 원문 대조 source 메타데이터를 강제한다(source-drift 방지).
    // source 없는 overlay는 단일 JSON PoC 파일 경로에서만 허용된다.
    if translations_path.is_dir() {
        crate::translation::require_source_metadata(&plan.encoded)?;
    }
    let has_glyphs = plan.encoding.glyph_count() > 0;

    // KO 글리프가 있으면 글리프 뱅크(bank 32, 0x80000)를 담기 위해 ROM을 1MB로 확장한다.
    let data = expanded_source(data, has_glyphs);
    let original = data.clone();
    let mut rom = TrackedRom::new(data);

    let entries = crate::script::extract_entries(rom.data())?;
    let entries_by_id: BTreeMap<String, crate::script::ScriptEntry> = entries
        .into_iter()
        .map(|entry| (entry.id.clone(), entry))
        .collect();

    // KO 글리프 뱅크와 UI 트랙 B 라벨 재조판이 공유하는 폰트를 한 번만 로드한다.
    let font = if has_glyphs {
        Some(crate::font::GlyphFont::load(font_path, 8.0, 128, 0, 0)?)
    } else {
        None
    };

    let mut glyph_bank_len = 0usize;
    if has_glyphs {
        let font = font.as_ref().expect("has_glyphs면 폰트가 로드됨");
        let mut glyph_bank = Vec::new();
        for &ch in plan.encoding.glyph_chars() {
            let rendered = glyph::dynamic_punctuation_glyph(ch).unwrap_or_else(|| font.render(ch));
            glyph_bank.extend_from_slice(&rendered);
        }
        glyph_bank_len = glyph_bank.len();
        write_free(
            &mut rom,
            "build ko glyph bank",
            glyph::KO_BANK_PHYS,
            &glyph_bank,
        )?;
        install_ko_prefix2_hook(&mut rom)?;
        // 헤더 ROM 크기 코드(0x7FFF 하위 니블)를 1MB(0x2, EN 1MB 관례)로 갱신. 리전 니블 보존.
        mark_rom_size_1mb(&mut rom, &original)?;
    }

    // region 전용 뱅크 repack: 모든 region 텍스트(번역/원문)를 확장 뱅크로 옮겨 overlength
    // 용량을 확보한다. has_glyphs(ROM 1MB 확장)일 때만.
    let plan_by_id: BTreeMap<&str, &crate::translation::EncodedOverride> =
        plan.encoded.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut repacked_regions = 0usize;
    if has_glyphs {
        for region in 0..crate::script::REGION_COUNT {
            let count = crate::script::region_string_count(region);
            let mut strings: Vec<Vec<u8>> = Vec::with_capacity(count);
            for i in 0..count {
                let id = format!("region/{region}/{i:03X}");
                let bytes = if let Some(enc) = plan_by_id.get(id.as_str()) {
                    let entry = entries_by_id
                        .get(&id)
                        .with_context(|| format!("region repack: 알 수 없는 id {id}"))?;
                    validate_translation_source(entry, enc)?;
                    enc.bytes.clone()
                } else if let Some(entry) = entries_by_id.get(&id) {
                    crate::script::raw_entry_bytes(entry)?
                } else {
                    Vec::new()
                };
                strings.push(bytes);
            }
            repack_region(
                &mut rom,
                region,
                &strings,
                REGION_REPACK_BASE + region as u8,
            )?;
            repacked_regions += 1;
        }
    }

    // cutscene 텍스트도 bank 6 안에서 compact 재배치(코드와 슬롯 공유라 fresh 뱅크 불가).
    let relocated_cutscenes = if has_glyphs {
        repack_cutscenes(&mut rom, &entries_by_id, &plan_by_id)?
    } else {
        0
    };
    let _ = glyph_bank_len;

    let translated_entries = plan.encoded.len();
    let relocated_regions = repacked_regions;
    for encoded in &plan.encoded {
        let entry = entries_by_id
            .get(&encoded.id)
            .with_context(|| format!("알 수 없는 translation id: {}", encoded.id))?;
        // region·cutscene 엔트리는 위 repack에서 처리했으므로 loop에서 건너뛴다.
        if matches!(
            entry.kind,
            crate::script::ScriptKind::Region | crate::script::ScriptKind::Cutscene
        ) {
            continue;
        }
        validate_translation_source(entry, encoded)?;
        anyhow::ensure!(
            !(entry.kind == crate::script::ScriptKind::Shop && encoded.bytes.len() != entry.len),
            "{}: shop 문자열은 relocation 전까지 원본 길이와 정확히 같아야 함 ({} != {})",
            entry.id,
            encoded.bytes.len(),
            entry.len
        );
        match entry.kind {
            _ => {
                anyhow::ensure!(
                    encoded.bytes.len() <= entry.len,
                    "{}: 인코딩 결과 {} bytes가 원본 slot {} bytes를 초과함 (relocation 미지원 kind)",
                    entry.id,
                    encoded.bytes.len(),
                    entry.len
                );
                let original_entry = crate::script::raw_entry_bytes(entry)?;
                let mut replacement = encoded.bytes.clone();
                replacement.resize(entry.len, 0x00);
                write_checked(
                    &mut rom,
                    &format!("translation {}", entry.id),
                    entry.offset,
                    &original_entry,
                    &replacement,
                )?;
            }
        }
    }

    // baseline UI 트랙 A: 돈 단위 한자 → 한글 (대사 번역 유무와 무관하게 항상 적용).
    patch_money_glyph(&mut rom)?;

    // UI 트랙 B: KO 텍스트 빌드(has_glyphs)면 UI 버튼 라벨도 한글로 교체한다.
    // 7세트는 원위치에 재압축하고, save/flee 2세트는 확장 bank 44로 옮겨 asset id를 재지정한다.
    let ui_labels_patched = if let Some(font) = font.as_ref() {
        crate::ui_graphics::apply_ui_labels(&mut rom, font)?
    } else {
        0
    };

    ensure_all_writes_tracked(&rom, &original)?;
    let mut rom_data = rom.into_data();
    TmrSegaHeader::update_checksum(&mut rom_data);

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(output_path, &rom_data)
        .with_context(|| format!("출력 쓰기 실패: {}", output_path.display()))?;
    if let Some(path) = bps_output_path {
        // BPS source는 실제 JP 512KB 원본(패치 적용 대상). target은 1MB 확장본일 수 있다.
        crate::bps::write_bps(&jp_original, &rom_data, path)?;
    }

    let crc = crc32fast::hash(&rom_data);
    println!("빌드 완료: {}", output_path.display());
    if preview_human_review {
        println!("build mode: needs_human_review QA preview (not release-ready)");
    }
    if let Some(path) = bps_output_path {
        println!("BPS 패치: {}", path.display());
    }
    println!("translations: {}", translated_entries);
    println!("relocated regions: {}", relocated_regions);
    println!("relocated cutscenes: {}", relocated_cutscenes);
    println!("ui labels (트랙 B): {ui_labels_patched}");
    println!(
        "money 金→금 glyph: 8 bytes at {:#07X}",
        glyph::MONEY_KANJI_GLYPH_ADDR
    );
    println!("KO glyphs: {}", plan.encoding.glyph_count());
    println!("CRC32: {crc:08X}");
    Ok(())
}

fn relocate_region_entry(
    rom: &mut TrackedRom,
    entry: &crate::script::ScriptEntry,
    bytes: &[u8],
    cursor: &mut usize,
) -> Result<()> {
    let region = entry
        .region
        .with_context(|| format!("{} missing region number", entry.id))?;
    let dst = *cursor;
    let ptr = crate::script::region_pointer_for_physical(rom.data(), region, dst)?;
    write_free(
        rom,
        &format!("region relocation data {}", entry.id),
        dst,
        bytes,
    )?;

    let ptr_off = crate::script::region_pointer_offset(rom.data(), region, entry.index)?;
    let old_ptr = crate::script::region_pointer_for_physical(rom.data(), region, entry.offset)?;
    write_checked(
        rom,
        &format!("region relocation pointer {}", entry.id),
        ptr_off,
        &old_ptr.to_le_bytes(),
        &ptr.to_le_bytes(),
    )?;

    *cursor = dst + bytes.len();
    Ok(())
}

fn validate_translation_source(
    entry: &crate::script::ScriptEntry,
    encoded: &crate::translation::EncodedOverride,
) -> Result<()> {
    let Some(source) = &encoded.source else {
        return Ok(());
    };
    let actual = crate::script::raw_entry_bytes(entry)?;
    anyhow::ensure!(
        actual == source.bytes,
        "{}: translation source bytes do not match current ROM extract",
        entry.id
    );
    if let Some(expected) = source.crc32 {
        let actual_crc = crc32fast::hash(&actual);
        anyhow::ensure!(
            actual_crc == expected,
            "{}: translation source CRC32 mismatch: metadata {expected:08X} vs ROM {actual_crc:08X}",
            entry.id
        );
    }
    Ok(())
}

fn relocate_cutscene_entry(
    rom: &mut TrackedRom,
    entry: &crate::script::ScriptEntry,
    bytes: &[u8],
    cursor: &mut usize,
) -> Result<()> {
    let dst = *cursor;
    let ptr = crate::script::cutscene_pointer_for_physical(dst)?;
    write_free(
        rom,
        &format!("cutscene relocation data {}", entry.id),
        dst,
        bytes,
    )?;

    let ptr_off = crate::script::cutscene_pointer_offset(entry.index)?;
    let old_ptr = crate::script::cutscene_pointer_for_physical(entry.offset)?;
    write_checked(
        rom,
        &format!("cutscene relocation pointer {}", entry.id),
        ptr_off,
        &old_ptr.to_le_bytes(),
        &ptr.to_le_bytes(),
    )?;

    *cursor = dst + bytes.len();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_poc_rom() -> Vec<u8> {
        // 글리프 뱅크가 bank 32(0x80000)로 옮겨졌으므로 1MB 확장 ROM으로 만든다.
        let mut data = vec![0xFF; EXPANDED_ROM_SIZE];
        data[glyph::HOOK_CALL_SITE..glyph::HOOK_CALL_SITE + 3].copy_from_slice(&[0xCD, 0x3E, 0x9A]);
        data[glyph::GLYPH_RENDER_HOOK_SITE..glyph::GLYPH_RENDER_HOOK_SITE + 3]
            .copy_from_slice(&glyph::GLYPH_RENDER_HOOK_ORIG);
        data[POC2_SCRIPT_EDIT..POC2_SCRIPT_EDIT + 2].copy_from_slice(&[0x10, 0x11]);
        data[glyph::FONT_BASE..glyph::FONT_BASE + glyph::GLYPH_BYTES]
            .copy_from_slice(&[0xAA; glyph::GLYPH_BYTES]);
        data
    }

    #[test]
    fn canonical_crc_accepted() {
        require_canonical_crc(JP_CRC32, false).unwrap();
    }

    #[test]
    fn noncanonical_crc_rejected_without_override() {
        assert!(require_canonical_crc(0xDEAD_BEEF, false).is_err());
    }

    #[test]
    fn noncanonical_crc_allowed_with_override() {
        require_canonical_crc(0xDEAD_BEEF, true).unwrap();
    }

    #[test]
    fn no_glyph_source_size_unchanged() {
        let src = vec![0xFF; EXPANDED_ROM_SIZE / 2];
        assert_eq!(expanded_source(src, false).len(), EXPANDED_ROM_SIZE / 2);
    }

    #[test]
    fn glyph_build_expands_source() {
        let src = vec![0xFF; EXPANDED_ROM_SIZE / 2];
        assert_eq!(expanded_source(src, true).len(), EXPANDED_ROM_SIZE);
    }

    #[test]
    fn poc2_writes_are_tracked_and_labelled() {
        let data = synthetic_poc_rom();
        let original = data.clone();
        let mut rom = TrackedRom::new(data);

        write_free(
            &mut rom,
            "poc2 ko glyph bank",
            glyph::KO_BANK_PHYS,
            &glyph::POC_GA,
        )
        .unwrap();
        install_ko_hook(&mut rom).unwrap();
        write_checked(
            &mut rom,
            "poc2 script edit",
            POC2_SCRIPT_EDIT,
            &[0x10, 0x11],
            &[glyph::KO_PREFIX, 0x00],
        )
        .unwrap();

        ensure_all_writes_tracked(&rom, &original).unwrap();
        let labels: Vec<&str> = rom
            .write_reports()
            .iter()
            .map(|w| w.label.as_str())
            .collect();
        assert_eq!(
            labels,
            vec![
                "poc2 ko glyph bank",
                "poc ko handler",
                "poc hook call-site",
                "poc2 script edit"
            ]
        );
    }

    #[test]
    fn poc2_fixed_writes_are_tracked_and_labelled() {
        let data = synthetic_poc_rom();
        let original = data.clone();
        let mut rom = TrackedRom::new(data);

        write_free(
            &mut rom,
            "poc2 fixed ko glyph bank",
            glyph::KO_BANK_PHYS,
            &glyph::POC_GA,
        )
        .unwrap();
        install_ko_prefix2_hook(&mut rom).unwrap();
        write_checked(
            &mut rom,
            "poc2 fixed script edit",
            POC2_SCRIPT_EDIT,
            &[0x10, 0x11],
            &[glyph::KO_PREFIX, 0x01],
        )
        .unwrap();

        ensure_all_writes_tracked(&rom, &original).unwrap();
        let labels: Vec<&str> = rom
            .write_reports()
            .iter()
            .map(|w| w.label.as_str())
            .collect();
        assert_eq!(
            labels,
            vec![
                "poc2 fixed ko glyph bank",
                "ko glyph hook handler",
                "ko glyph render hook",
                "poc2 fixed script edit"
            ]
        );
    }

    #[test]
    fn checked_write_fails_on_wrong_base_bytes() {
        let mut data = synthetic_poc_rom();
        data[POC2_SCRIPT_EDIT] = 0x99;
        let mut rom = TrackedRom::new(data);

        let err = write_checked(
            &mut rom,
            "bad script edit",
            POC2_SCRIPT_EDIT,
            &[0x10, 0x11],
            &[glyph::KO_PREFIX, 0x00],
        )
        .unwrap_err();
        assert!(err.to_string().contains("expectation failed"));
    }

    #[test]
    fn free_write_fails_on_nonfree_region() {
        let mut data = synthetic_poc_rom();
        data[glyph::KO_BANK_PHYS] = 0x00;
        let mut rom = TrackedRom::new(data);

        let err =
            write_free(&mut rom, "glyph bank", glyph::KO_BANK_PHYS, &glyph::POC_GA).unwrap_err();
        assert!(err.to_string().contains("expectation failed"));
    }

    #[test]
    fn build_like_translation_writes_are_tracked() {
        let mut data = synthetic_poc_rom();
        data[0x120..0x124].copy_from_slice(&[0x10, 0x11, 0x12, 0x00]);
        let original = data.clone();
        let mut rom = TrackedRom::new(data);

        write_free(
            &mut rom,
            "build ko glyph bank",
            glyph::KO_BANK_PHYS,
            &[0x11; glyph::GLYPH_BYTES * 2],
        )
        .unwrap();
        install_ko_prefix2_hook(&mut rom).unwrap();
        write_checked(
            &mut rom,
            "translation cutscene/002",
            0x120,
            &[0x10, 0x11, 0x12, 0x00],
            &[glyph::KO_PREFIX, 0x01, glyph::KO_PREFIX, 0x02],
        )
        .unwrap();

        ensure_all_writes_tracked(&rom, &original).unwrap();
    }

    #[test]
    fn money_glyph_patch_replaces_kanji_with_hangul_and_is_tracked() {
        let mut data = vec![0xFF; 0x20000];
        data[glyph::MONEY_KANJI_GLYPH_ADDR..glyph::MONEY_KANJI_GLYPH_ADDR + glyph::GLYPH_BYTES]
            .copy_from_slice(&glyph::MONEY_KANJI_JP);
        let original = data.clone();
        let mut rom = TrackedRom::new(data);

        patch_money_glyph(&mut rom).unwrap();

        assert_eq!(
            &rom[glyph::MONEY_KANJI_GLYPH_ADDR..glyph::MONEY_KANJI_GLYPH_ADDR + glyph::GLYPH_BYTES],
            &glyph::MONEY_GLYPH_KO_GEUM,
            "money glyph should become hangul 금"
        );
        ensure_all_writes_tracked(&rom, &original).unwrap();
        assert_eq!(
            rom.write_reports()
                .iter()
                .map(|w| w.label.as_str())
                .collect::<Vec<_>>(),
            vec!["money kanji 金 → hangul 금"]
        );
    }

    #[test]
    fn money_glyph_patch_fails_on_already_patched_rom() {
        // 입력이 JP 원본이 아니면(이미 금) 안전쓰기가 실패해야 한다(conventions §5.2).
        let mut data = vec![0xFF; 0x20000];
        data[glyph::MONEY_KANJI_GLYPH_ADDR..glyph::MONEY_KANJI_GLYPH_ADDR + glyph::GLYPH_BYTES]
            .copy_from_slice(&glyph::MONEY_GLYPH_KO_GEUM);
        let mut rom = TrackedRom::new(data);

        let err = patch_money_glyph(&mut rom).unwrap_err();
        assert!(err.to_string().contains("expectation failed"));
    }

    #[test]
    fn cutscene_relocation_writes_data_and_pointer() {
        let mut data = synthetic_poc_rom();
        data[0x1B166..0x1B168].copy_from_slice(&0xA68Cu16.to_le_bytes());
        data[0x1A68C..0x1A68F].copy_from_slice(&[0x10, 0x11, 0x00]);
        let original = data.clone();
        let mut rom = TrackedRom::new(data);
        let entry = crate::script::ScriptEntry {
            id: "cutscene/002".to_string(),
            kind: crate::script::ScriptKind::Cutscene,
            region: None,
            index: 2,
            slot: 2,
            offset: 0x1A68C,
            len: 3,
            bytes_hex: "10 11 00".to_string(),
            source_crc32: String::new(),
            jp_preview: String::new(),
            ko: String::new(),
            skip: false,
        };
        let mut cursor = glyph::KO_CUTSCENE_RELOC_START;

        relocate_cutscene_entry(
            &mut rom,
            &entry,
            &[glyph::KO_PREFIX, 0x01, glyph::KO_PREFIX, 0x02, 0x00],
            &mut cursor,
        )
        .unwrap();

        assert_eq!(
            &rom[0x1B166..0x1B168],
            &crate::script::cutscene_pointer_for_physical(glyph::KO_CUTSCENE_RELOC_START)
                .unwrap()
                .to_le_bytes()
        );
        assert_eq!(
            &rom[glyph::KO_CUTSCENE_RELOC_START..glyph::KO_CUTSCENE_RELOC_START + 5],
            &[glyph::KO_PREFIX, 0x01, glyph::KO_PREFIX, 0x02, 0x00]
        );
        ensure_all_writes_tracked(&rom, &original).unwrap();
    }

    #[test]
    fn translation_source_guard_rejects_mismatched_bytes() {
        let entry = crate::script::ScriptEntry {
            id: "cutscene/002".to_string(),
            kind: crate::script::ScriptKind::Cutscene,
            region: None,
            index: 2,
            slot: 2,
            offset: 0x1A68C,
            len: 3,
            bytes_hex: "10 11 00".to_string(),
            source_crc32: String::new(),
            jp_preview: String::new(),
            ko: String::new(),
            skip: false,
        };
        let encoded = crate::translation::EncodedOverride {
            id: entry.id.clone(),
            bytes: vec![glyph::KO_PREFIX, 0x01, 0x00],
            source: Some(crate::translation::TranslationSource {
                bytes: vec![0x10, 0x12, 0x00],
                crc32: None,
            }),
        };

        let err = validate_translation_source(&entry, &encoded).unwrap_err();
        assert!(err.to_string().contains("source bytes do not match"));
    }

    #[test]
    fn region_relocation_writes_data_and_pointer() {
        let mut data = synthetic_poc_rom();
        data[0x1A069] = 0x15;
        data[0x1A06A] = 0x07;
        data[0x1C02E..0x1C030].copy_from_slice(&0x4800u16.to_le_bytes());
        data[0x1C802..0x1C804].copy_from_slice(&0x4030u16.to_le_bytes());
        data[0x1C030..0x1C032].copy_from_slice(&[0x10, 0x00]);
        let original = data.clone();
        let mut rom = TrackedRom::new(data);
        let entry = crate::script::ScriptEntry {
            id: "region/0/001".to_string(),
            kind: crate::script::ScriptKind::Region,
            region: Some(0),
            index: 1,
            slot: 1,
            offset: 0x1C030,
            len: 2,
            bytes_hex: "10 00".to_string(),
            source_crc32: String::new(),
            jp_preview: String::new(),
            ko: String::new(),
            skip: false,
        };
        let mut cursor = 0x1FE00;

        relocate_region_entry(
            &mut rom,
            &entry,
            &[glyph::KO_PREFIX, 0x01, glyph::KO_PREFIX, 0x02, 0x00],
            &mut cursor,
        )
        .unwrap();

        assert_eq!(
            &rom[0x1C802..0x1C804],
            &crate::script::region_pointer_for_physical(rom.data(), 0, 0x1FE00)
                .unwrap()
                .to_le_bytes()
        );
        assert_eq!(
            &rom[0x1FE00..0x1FE05],
            &[glyph::KO_PREFIX, 0x01, glyph::KO_PREFIX, 0x02, 0x00]
        );
        ensure_all_writes_tracked(&rom, &original).unwrap();
    }
}

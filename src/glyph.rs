//! 1bpp 8×8 글리프와 폰트 슬롯 패치.
//!
//! 폰트는 ROM `FONT_BASE`에 글리프당 8바이트(행당 1바이트, MSB=좌측 픽셀, 1=전경)로
//! 비압축 저장된다. 런타임에 게임 루틴이 1bpp→4bpp로 확장해 VRAM에 올린다.
//! 글리프 인덱스 ↔ 텍스트 코드 관계: `font_index = text_code - 1`.

use retro_z80::{
    AluOperation, Assembler, ByteOperand, Condition, Instruction, Register16, StackRegister,
};

/// JP 원본의 메인 폰트 데이터 시작 오프셋.
pub const FONT_BASE: usize = 0x19AFD;
pub const GLYPH_BYTES: usize = 8;

/// 작은 TTF 래스터에서 대시처럼 뭉개지지 않도록 만든 8×8 서양식 물결표.
/// GG1·GG3에서 실사용한 crest-trough-crest 도안을 공유한다.
pub const KO_TILDE: [u8; GLYPH_BYTES] = [0x00, 0x00, 0x00, 0x61, 0x92, 0x0C, 0x00, 0x00];

/// 일본식 `、`와 구분되는 8×8 서양식 콤마. 2×2 머리와 좌하향 꼬리를 쓴다.
/// GG1·GG3에서 실사용한 도안을 공유한다.
pub const KO_COMMA: [u8; GLYPH_BYTES] = [0x00, 0x00, 0x00, 0x00, 0x30, 0x30, 0x20, 0x40];

/// 한글 동적 글리프 뱅크에서 손도안으로 공급할 문장부호.
pub fn dynamic_punctuation_glyph(ch: char) -> Option<[u8; GLYPH_BYTES]> {
    match ch {
        '~' => Some(KO_TILDE),
        ',' => Some(KO_COMMA),
        _ => None,
    }
}

/// 텍스트 코드 → 폰트 글리프 인덱스. (본 인코딩 구현에서 사용 예정)
#[allow(dead_code)]
pub fn font_index(text_code: u8) -> usize {
    (text_code as usize).wrapping_sub(1)
}

/// 8×8 1bpp 글리프를 ASCII 아트로(`#`/`.`) 렌더한다 — 디버그·검증용.
pub fn to_ascii(glyph: &[u8; GLYPH_BYTES]) -> String {
    let mut s = String::new();
    for &b in glyph {
        for c in 0..8 {
            s.push(if (b >> (7 - c)) & 1 == 1 { '#' } else { '.' });
        }
        s.push('\n');
    }
    s
}

// ─────────────────────────────────────────────────────────────────────────
// 2차 PoC: 프리픽스 디스패치 훅 (인코딩 공간 검증)
// ─────────────────────────────────────────────────────────────────────────

/// 한글 프리픽스 바이트(다중 프리픽스의 첫 번째). JP 텍스트에 글리프가 배정되지 않은 미사용
/// 대역(0xA3~). dakuten/dispatch 참조로도 쓴다.
pub const KO_PREFIX: u8 = 0xA3;

/// 다중 프리픽스 바이트들(연속). JP 스크립트에서 0xA3~0xAF는 미사용 확인됨. 각 프리픽스는
/// `PER_PREFIX`개 base를 addressing한다: 음절 rank i → prefix `KO_PREFIXES[i/PER_PREFIX]`,
/// base `0x01 + i%PER_PREFIX`. glyph# = prefix_index*PER_PREFIX + (base-1) = rank.
pub const KO_PREFIXES: [u8; 4] = [0xA3, 0xA4, 0xA5, 0xA6];

/// 프리픽스당 base 용량. base = `0x01..=0xFA`(0x00 종료·0xFB-0xFF 제어 제외) = 250.
pub const PER_PREFIX: usize = 250;

/// 다중 프리픽스 총 글리프 용량(`KO_PREFIXES.len() * PER_PREFIX`).
pub const KO_MULTI_PREFIX_CAP: usize = KO_PREFIXES.len() * PER_PREFIX;

/// (레거시) $9981 렌더 루프의 글리프 렌더 호출 지점 (물리). 단일 프리픽스 PoC(`install_ko_hook`)
/// 전용. 새 경로는 `GLYPH_RENDER_HOOK_SITE`를 쓴다.
pub const HOOK_CALL_SITE: usize = 0x199F1;

/// (레거시) `FC/FB` 프리픽스 디스패치 위치 (물리, $9981 루프). 새 경로는 안 쓴다.
pub const PREFIX_DISPATCH_SITE: usize = 0x199A4;

/// 공유 글리프 렌더 `$9A3E`(물리 0x19A3E)를 후킹한다. 스토리 텍스트의 여러 렌더 루프
/// ($9981·$A573 등)가 모두 이 `call $9A3E`를 A=char, DE→다음바이트, HL=VRAM dst, C=plane
/// mask로 호출하므로, 여기 한 곳만 훅하면 모든 텍스트 모드의 한글 렌더를 커버한다.
/// (이전 $99A4/$99F1 훅은 $9981 루프에만 걸려 스토리 텍스트($A573)를 놓쳤다 — journal
/// 2026-07-07 참조.) 원본 첫 3바이트 `dec a; push de; ex de,hl`(3D D5 EB)를 `jp`로 덮고,
/// 핸들러가 비-프리픽스 경로에서 그 3명령을 재현한 뒤 `$9A41`로 복귀한다.
pub const GLYPH_RENDER_HOOK_SITE: usize = 0x19A3E;
pub const GLYPH_RENDER_HOOK_ORIG: [u8; 3] = [0x3D, 0xD5, 0xEB];
const ORIGINAL_FONT_EXPAND_LOGICAL: u16 = 0x9A3E;
const GLYPH_RENDER_CONT_LOGICAL: u16 = ORIGINAL_FONT_EXPAND_LOGICAL + 3; // $9A41

/// 한글 글리프 훅 핸들러 배치 위치 (물리). bank 6 자유공간, 논리 $B726. 핸들러는 슬롯 2에서
/// 실행되고(bank 6), 글리프 뱅크는 슬롯 1(bank 32)에서 읽으므로 서로 겹치지 않는다.
pub const KO_HANDLER_PHYS: usize = 0x1B726;
pub const KO_HANDLER_LOGICAL: u16 = 0xB726;

/// bank 6에서 핸들러 뒤 cutscene relocation 시작(물리). 글리프 뱅크가 bank 32로 빠져 bank 6
/// 여유가 다시 열렸다. 핸들러는 `KO_HANDLER_PHYS`부터 이 주소까지(256바이트) 안에 들어간다.
pub const KO_CUTSCENE_RELOC_START: usize = 0x1B826;

/// 한글 글리프 뱅크 시작. 물리 `0x80000`(bank 32, 확장 ROM), 논리 `$4000`(슬롯 1). glyph#
/// i 글리프 = `+i*8`. 다중 프리픽스 최대 1000 글리프 = 8000바이트가 16KB bank 32에 들어간다.
pub const KO_BANK_PHYS: usize = 0x80000;
pub const KO_BANK_LOGICAL: u16 = 0x4000;

/// 글리프 뱅크의 ROM bank 번호(슬롯 1에 매핑). 물리 `0x80000 / 0x4000 = 32`.
pub const KO_BANK_BANK: u8 = 32;

/// 슬롯 1(0x4000-0x7FFF) 맵퍼 레지스터(Sega 맵퍼). 핸들러가 여기 bank 32를 매핑해 글리프를
/// 읽고 원래 bank로 복원한다.
pub const KO_SLOT1_MAPPER: u16 = 0xFFFE;

/// 한글 프리픽스 디스패치 핸들러 (Z80, 65바이트).
///
/// 진입: `A`=현재 char, `DE`=char 다음 바이트, `HL`=합성버퍼 dst, `C`=플레인 마스크.
/// `A != KO_PREFIX` 이면 원본 폰트 디컴프(0x9A3E)로 tail-jump(거기서 ret→루프 복귀).
/// 프리픽스면 다음 바이트를 인덱스로 읽어 한글 뱅크에서 글리프를 fetch,
/// 원본 0x9A3E와 동일한 1bpp→4bpp 확장(plane0=0xFF 배경, glyph는 C가 고른 플레인)을 수행.
///
/// 확장 루프 본문(`ld a,(de)`부터 djnz까지 38바이트)은 원본 0x9A53~0x9A78과 동일.
pub fn ko_handler_bytes() -> Vec<u8> {
    let mut assembler = Assembler::new();
    assembler
        .emit(Instruction::AluImm(AluOperation::Cp, KO_PREFIX))
        .emit(Instruction::JpCond(
            Condition::Nz,
            ORIGINAL_FONT_EXPAND_LOGICAL,
        ))
        .emit(Instruction::LdADE)
        .emit(Instruction::IncRR(Register16::De))
        .emit(Instruction::Push(StackRegister::De))
        .emit(Instruction::Push(StackRegister::Hl))
        .emit(Instruction::LdRR(ByteOperand::L, ByteOperand::A))
        .emit(Instruction::LdRImm(ByteOperand::H, 0x00))
        .emit(Instruction::AddHLRR(Register16::Hl))
        .emit(Instruction::AddHLRR(Register16::Hl))
        .emit(Instruction::AddHLRR(Register16::Hl))
        .emit(Instruction::LdRRImm(Register16::De, KO_BANK_LOGICAL))
        .emit(Instruction::AddHLRR(Register16::De))
        .emit(Instruction::ExDEHL)
        .emit(Instruction::Pop(StackRegister::Hl))
        .emit(Instruction::Push(StackRegister::Bc))
        .emit(Instruction::LdRImm(ByteOperand::B, GLYPH_BYTES as u8))
        .label("row")
        .emit(Instruction::LdADE);
    emit_plane_write(&mut assembler, 0);
    assembler
        .emit(Instruction::LdRImm(ByteOperand::IndirectHl, 0xFF))
        .emit(Instruction::IncRR(Register16::Hl));
    for bit in 1..=3 {
        emit_plane_write(&mut assembler, bit);
        assembler.emit(Instruction::IncRR(Register16::Hl));
    }
    assembler
        .emit(Instruction::IncRR(Register16::De))
        .djnz("row")
        .emit(Instruction::Pop(StackRegister::Bc))
        .emit(Instruction::Pop(StackRegister::De))
        .emit(Instruction::Ret);
    assemble_glyph_code(assembler)
}

/// 공유 글리프 렌더 `$9A3E` 훅 핸들러.
///
/// 진입: `A`=현재 char, `DE`=char 다음 바이트(char가 프리픽스면 base), `HL`=VRAM tile dst,
/// `C`=plane mask. `$9A3E`(글리프 렌더) 엔트리를 `jp`로 덮어 여기로 온다.
///
/// - `A`가 프리픽스 범위 [0xA3, 0xA7)이면 한글: `prefix_index = A - 0xA3`을 `C`의 bits 4-5에
///   싣고, `base = (DE)`를 읽어 소비(`inc de`)한 뒤 한 번에 다중 프리픽스 확장을 수행한다.
///   `$9981`·`$A573` 두 렌더 루프 모두 `inc de` 후 `call $9A3E`라 `DE`는 base를 가리키므로
///   프리픽스+base 2바이트를 한 호출에서 처리한다(cross-iteration 플래그 불필요).
/// - 아니면 원본 첫 3바이트(`dec a; push de; ex de,hl`)를 재현하고 `$9A41`로 tail-jump한다.
pub fn ko_prefix2_handler_bytes() -> Vec<u8> {
    let mut assembler = Assembler::new();
    let first = KO_PREFIXES[0];
    let past = KO_PREFIXES[KO_PREFIXES.len() - 1] + 1;

    // A가 프리픽스 범위 [first, past)인가? 아니면 원본 glyph 경로(orig).
    assembler
        .emit(Instruction::AluImm(AluOperation::Cp, first))
        .jr_cond(Condition::C, "orig") // A < first
        .emit(Instruction::AluImm(AluOperation::Cp, past))
        .jr_cond(Condition::Nc, "orig"); // A >= past

    // 프리픽스: prefix_index = A - first → C bits 4-5에 병합.
    // **B는 렌더 루프($A573·$9981)의 문자 카운터이므로 임시로도 건드리면 안 된다.** temp reg 없이
    // C bits 4-5를 직접 clear(`res`)한 뒤 병합해, C의 초기 bits 4-5 상태와 무관하게 견고하다.
    // emit_ko_multi_expand가 뒤에서 bits 4-6을 다시 clear해 plane mask를 복원한다.
    assembler
        .emit(Instruction::AluImm(AluOperation::Sub, first))
        .emit(Instruction::Rlca)
        .emit(Instruction::Rlca)
        .emit(Instruction::Rlca)
        .emit(Instruction::Rlca) // A = prefix_index << 4
        .emit(Instruction::Res(4, ByteOperand::C)) // C bit4 clear
        .emit(Instruction::Res(5, ByteOperand::C)) // C bit5 clear
        .emit(Instruction::AluR(AluOperation::Or, ByteOperand::C)) // A = (prefix_index<<4) | C
        .emit(Instruction::LdRR(ByteOperand::C, ByteOperand::A)) // C = plane mask | prefix_index<<4 (B 보존)
        // base 읽고 소비(DE→다음 char).
        .emit(Instruction::LdADE) // A = base
        .emit(Instruction::IncRR(Register16::De));
    emit_ko_multi_expand(&mut assembler); // A=base, C bits4-5=prefix_index, HL=dst; DE 보존; ret

    // orig: 원본 $9A3E 첫 3바이트 재현 후 $9A41로 복귀.
    assembler
        .label("orig")
        .emit(Instruction::DecR(ByteOperand::A)) // dec a
        .emit(Instruction::Push(StackRegister::De)) // push de
        .emit(Instruction::ExDEHL) // ex de,hl
        .emit(Instruction::Jp(GLYPH_RENDER_CONT_LOGICAL));

    assemble_glyph_code(assembler)
}

fn assemble_glyph_code(assembler: Assembler) -> Vec<u8> {
    let bytes = assembler
        .assemble(KO_HANDLER_LOGICAL)
        .expect("typed Z80 glyph code must assemble and decode exactly")
        .into_bytes();
    let reserve = KO_CUTSCENE_RELOC_START - KO_HANDLER_PHYS;
    assert!(
        bytes.len() <= reserve,
        "한글 글리프 훅 핸들러가 예약 공간({reserve}B)을 초과: {}B",
        bytes.len()
    );
    bytes
}

/// 다중 프리픽스 글리프 확장(Entry 2 본문). (prefix, base) → rank → 글리프 뱅크(bank 32,
/// 슬롯 1) 주소 → 8행 1bpp→4bpp 확장(plane0=0xFF, glyph는 C가 고른 plane).
fn emit_ko_multi_expand(assembler: &mut Assembler) {
    // 호출자의 BC 보존: B는 렌더 루프($A573)의 문자 카운터인데 아래에서 B를 prefix_index·
    // row 카운터로 쓴다(내부 push/pop bc는 오염된 B를 보존하므로 여기 바깥쪽에서 원본을 지킨다).
    assembler.emit(Instruction::Push(StackRegister::Bc));
    // base 저장(A를 prefix_index 추출에 쓰므로).
    assembler.emit(Instruction::Push(StackRegister::Af));
    // prefix_index = (C >> 4) & 3 → B.
    assembler
        .emit(Instruction::LdRR(ByteOperand::A, ByteOperand::C))
        .emit(Instruction::AluImm(AluOperation::And, 0x30))
        .emit(Instruction::Rrca)
        .emit(Instruction::Rrca)
        .emit(Instruction::Rrca)
        .emit(Instruction::Rrca)
        .emit(Instruction::LdRR(ByteOperand::B, ByteOperand::A)); // B = prefix_index
    // C의 플래그 비트 4-6 클리어(bit7 + plane bits 0-3 보존).
    assembler
        .emit(Instruction::LdRR(ByteOperand::A, ByteOperand::C))
        .emit(Instruction::AluImm(AluOperation::And, 0x8F))
        .emit(Instruction::LdRR(ByteOperand::C, ByteOperand::A))
        .emit(Instruction::Pop(StackRegister::Af)) // A = base
        .emit(Instruction::Push(StackRegister::De)) // char ptr
        .emit(Instruction::Push(StackRegister::Hl)) // dst
        // rank = prefix_index * PER_PREFIX + (base - 1) → HL.
        .emit(Instruction::DecR(ByteOperand::A)) // A = base - 1
        .emit(Instruction::LdRR(ByteOperand::L, ByteOperand::A))
        .emit(Instruction::LdRImm(ByteOperand::H, 0x00)) // HL = base - 1
        .emit(Instruction::LdRR(ByteOperand::A, ByteOperand::B))
        .emit(Instruction::AluR(AluOperation::Or, ByteOperand::A)) // or a (B==0 검사)
        .jr_cond(Condition::Z, "no_add")
        .emit(Instruction::LdRRImm(Register16::De, PER_PREFIX as u16))
        .label("add_loop")
        .emit(Instruction::AddHLRR(Register16::De))
        .djnz("add_loop") // prefix_index번 250 더함
        .label("no_add")
        .emit(Instruction::AddHLRR(Register16::Hl))
        .emit(Instruction::AddHLRR(Register16::Hl))
        .emit(Instruction::AddHLRR(Register16::Hl)) // HL = rank * 8
        .emit(Instruction::LdRRImm(Register16::De, KO_BANK_LOGICAL))
        .emit(Instruction::AddHLRR(Register16::De)) // HL = 글리프 src(슬롯 1 논리)
        .emit(Instruction::ExDEHL) // DE = src
        .emit(Instruction::Pop(StackRegister::Hl)); // HL = dst
    // 슬롯 1에 글리프 뱅크(bank 32) 매핑, 기존 bank 저장.
    assembler
        .emit(Instruction::LdAAddr(KO_SLOT1_MAPPER))
        .emit(Instruction::Push(StackRegister::Af)) // save slot1 bank
        .emit(Instruction::LdRImm(ByteOperand::A, KO_BANK_BANK))
        .emit(Instruction::LdAddrA(KO_SLOT1_MAPPER))
        .emit(Instruction::Push(StackRegister::Bc))
        .emit(Instruction::LdRImm(ByteOperand::B, GLYPH_BYTES as u8))
        .label("row")
        .emit(Instruction::LdADE);
    emit_plane_write(assembler, 0);
    assembler
        .emit(Instruction::LdRImm(ByteOperand::IndirectHl, 0xFF))
        .emit(Instruction::IncRR(Register16::Hl));
    emit_plane_write(assembler, 1);
    assembler.emit(Instruction::IncRR(Register16::Hl));
    emit_plane_write(assembler, 2);
    assembler.emit(Instruction::IncRR(Register16::Hl));
    emit_plane_write(assembler, 3);
    assembler
        .emit(Instruction::IncRR(Register16::Hl))
        .emit(Instruction::IncRR(Register16::De))
        .djnz("row")
        .emit(Instruction::Pop(StackRegister::Bc))
        .emit(Instruction::Pop(StackRegister::Af)) // A = 저장된 slot1 bank
        .emit(Instruction::LdAddrA(KO_SLOT1_MAPPER)) // 슬롯 1 복원
        .emit(Instruction::Pop(StackRegister::De)) // char ptr
        .emit(Instruction::Pop(StackRegister::Bc)) // 호출자 BC(문자 카운터) 복원
        .emit(Instruction::Ret);
}

fn emit_plane_write(assembler: &mut Assembler, bit: u8) {
    let label = format!("plane{bit}_skip");
    assembler
        .emit(Instruction::LdRImm(ByteOperand::IndirectHl, 0x00))
        .emit(Instruction::Bit(bit, ByteOperand::C))
        .jr_cond(Condition::Z, &label)
        .emit(Instruction::LdRR(ByteOperand::IndirectHl, ByteOperand::A))
        .label(&label);
}

pub(crate) fn assemble_ko_handler_call() -> [u8; 3] {
    assemble_three_byte_instruction(Instruction::Call(KO_HANDLER_LOGICAL), 0x99F1)
}

pub(crate) fn assemble_ko_handler_jump() -> [u8; 3] {
    assemble_three_byte_instruction(
        Instruction::Jp(KO_HANDLER_LOGICAL),
        ORIGINAL_FONT_EXPAND_LOGICAL,
    )
}

fn assemble_three_byte_instruction(instruction: Instruction, origin: u16) -> [u8; 3] {
    let mut assembler = Assembler::new();
    assembler.emit(instruction);
    assembler
        .assemble(origin)
        .expect("fixed typed Z80 instruction must assemble and decode exactly")
        .into_bytes()
        .try_into()
        .expect("CALL/JP nn must encode to three bytes")
}

/// PoC용 한글 글리프 '가' (ㄱ + ㅏ), 8×8 1bpp 손도안.
///
/// ```text
/// ####.#..
/// ...#.#..
/// ..#..#..
/// .#...##.
/// .#...#..
/// #....#..
/// .....#..
/// .....#..
/// ```
pub const POC_GA: [u8; GLYPH_BYTES] = [
    0b1111_0100,
    0b0001_0100,
    0b0010_0100,
    0b0100_0110,
    0b0100_0100,
    0b1000_0100,
    0b0000_0100,
    0b0000_0100,
];

// ─────────────────────────────────────────────────────────────────────────
// UI 베이크드 그래픽: HUD/상점 돈 단위 글리프 (트랙 A)
// ─────────────────────────────────────────────────────────────────────────

/// 돈 단위 `金`(한자) 폰트 글리프의 물리 ROM 주소.
///
/// 게임이 이 1bpp 8바이트를 1bpp→4bpp 확장해 배경 nametable 타일 `0x25`로 직접
/// 업로드한다(대사 char 엔진 밖 HUD 그래픽 경로 → 텍스트 번역이 못 닿음). 상점 텍스트
/// byte `0x0C`와 소스를 공유한다. fresh-scene VRAM으로 소스 확정
/// (`docs/analysis/shop-hud-graphics.md` RESOLVED).
pub const MONEY_KANJI_GLYPH_ADDR: usize = 0x19B55;

/// `MONEY_KANJI_GLYPH_ADDR`의 JP 원본 바이트(`金`). 안전쓰기 기대값.
///
/// ```text
/// ...#....
/// ..#.#...
/// .#####..
/// #..#..#.
/// .#####..
/// ...#....
/// .#.#.#..
/// #######.
/// ```
pub const MONEY_KANJI_JP: [u8; GLYPH_BYTES] = [0x10, 0x28, 0x7C, 0x92, 0x7C, 0x10, 0x54, 0xFE];

/// 돈 단위 한글 `금`(ㄱ+ㅡ+ㅁ), 8×8 1bpp 손도안.
///
/// Galmuri11을 8px로 렌더하면 다운스케일로 뭉개지므로(자모 소실) 손도안을 쓴다.
/// 자매 `gg_madou_1`이 프레임-980 화면 검증까지 통과시킨 검증된 픽셀 디자인을 이식했다
/// (주소가 아닌 글리프 아트라 이식 안전). 0원칙: 한글 패치에 JP 한자 노출은 블로커.
///
/// ```text
/// ........
/// .#####..
/// .....#..
/// #######.
/// ........
/// .#####..
/// .#...#..
/// .#####..
/// ```
pub const MONEY_GLYPH_KO_GEUM: [u8; GLYPH_BYTES] = [0x00, 0x7C, 0x04, 0xFE, 0x00, 0x7C, 0x44, 0x7C];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn western_punctuation_uses_series_synth_glyphs() {
        assert_eq!(dynamic_punctuation_glyph('~'), Some(KO_TILDE));
        assert_eq!(dynamic_punctuation_glyph(','), Some(KO_COMMA));
        assert_eq!(dynamic_punctuation_glyph('、'), None);
        assert_ne!(KO_COMMA, KO_TILDE);
    }

    #[test]
    fn glyph_hook_handler_has_expected_shape() {
        let bytes = ko_prefix2_handler_bytes();
        assert!(bytes.len() <= KO_CUTSCENE_RELOC_START - KO_HANDLER_PHYS);
        // 진입은 프리픽스 범위 검사(cp 0xA3)로 시작한다.
        assert_eq!(
            &bytes[..2],
            &[0xFE, KO_PREFIXES[0]],
            "handler starts with cp first-prefix"
        );
        // 비-프리픽스 경로는 원본 첫 3바이트 재현 후 $9A41로 tail-jump(jp = C3 41 9A).
        let cont = GLYPH_RENDER_CONT_LOGICAL;
        let tail = [
            0x3D,
            0xD5,
            0xEB,
            0xC3,
            (cont & 0xFF) as u8,
            (cont >> 8) as u8,
        ];
        assert!(
            bytes.windows(tail.len()).any(|w| w == tail),
            "orig path replays dec a; push de; ex de,hl then jp $9A41"
        );
    }
}

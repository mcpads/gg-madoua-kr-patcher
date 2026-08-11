use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

const BANK_SIZE: usize = 0x4000;
pub const SLOT1_BASE: u16 = 0x4000;
const SLOT2_BASE: u16 = 0x8000;
pub const REGION_LOC_TABLE: usize = 0x1A069;
pub const CUTSCENE_POINTER_TABLE: usize = 0x1B162;
pub const CUTSCENE_COUNT: usize = 0xA8;
const SHOP_START: usize = 0x25BB7;
const SHOP_COUNT: usize = 12;
const REGION_STRING_COUNTS: [usize; 11] = [
    0x73, 0x35, 0x76, 0x2C, 0x0F, 0x16, 0x9F, 0x7C, 0x37, 0x48, 0x05,
];

#[derive(Debug, Serialize, Deserialize)]
struct ExtractFile {
    format: String,
    rom: ExtractRomInfo,
    counts: ExtractCounts,
    entries: Vec<ScriptEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ExtractRomInfo {
    path: String,
    size: usize,
    crc32: String,
    md5: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ExtractCounts {
    total: usize,
    regions: usize,
    cutscenes: usize,
    shop: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptEntry {
    pub id: String,
    pub kind: ScriptKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<usize>,
    pub index: usize,
    pub slot: u8,
    pub offset: usize,
    pub len: usize,
    pub bytes_hex: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_crc32: String,
    pub jp_preview: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ko: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub skip: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptKind {
    Region,
    Cutscene,
    Shop,
}

pub fn cutscene_pointer_offset(index: usize) -> Result<usize> {
    anyhow::ensure!(
        index < CUTSCENE_COUNT,
        "cutscene index out of range: {index} >= {CUTSCENE_COUNT}"
    );
    Ok(CUTSCENE_POINTER_TABLE + index * 2)
}

pub fn cutscene_pointer_for_physical(offset: usize) -> Result<u16> {
    let bank_base = cutscene_bank_base();
    physical_to_slot_pointer(bank_base, offset, SLOT2_BASE)
}

pub fn cutscene_relocation_start(_glyph_bank_len: usize) -> usize {
    // 글리프 뱅크는 이제 bank 32(0x80000)로 옮겨졌으므로 cutscene relocation은 bank 6의
    // 핸들러 뒤 여유공간을 쓴다(글리프 길이와 무관). cutscene는 slot 2(bank 6)에서 읽히므로
    // bank 32에는 둘 수 없다.
    crate::glyph::KO_CUTSCENE_RELOC_START
}

pub fn region_bank_base_for_region(rom: &[u8], region: usize) -> Result<usize> {
    region_bank_base(rom, region)
}

pub fn region_pointer_offset(rom: &[u8], region: usize, index: usize) -> Result<usize> {
    let entries = *REGION_STRING_COUNTS
        .get(region)
        .with_context(|| format!("region out of range: {region}"))?;
    anyhow::ensure!(
        index < entries,
        "region {region} index out of range: {index} >= {entries}"
    );
    Ok(region_table_addr(rom, region)? + index * 2)
}

pub fn region_pointer_for_physical(rom: &[u8], region: usize, offset: usize) -> Result<u16> {
    let bank_base = region_bank_base(rom, region)?;
    physical_to_slot_pointer(bank_base, offset, SLOT1_BASE)
}

pub fn region_relocation_start(rom: &[u8], region: usize) -> Result<usize> {
    let bank_base = region_bank_base(rom, region)?;
    find_largest_ff_run(rom, bank_base, bank_base + BANK_SIZE)
        .map(|(start, _end)| start)
        .with_context(|| format!("region {region} bank has no 0xFF relocation run"))
}

pub fn cmd_extract_text(rom_path: &Path, output_path: &Path) -> Result<()> {
    let rom = std::fs::read(rom_path)
        .with_context(|| format!("ROM 읽기 실패: {}", rom_path.display()))?;
    let entries = extract_entries(&rom)?;
    let region_count = entries
        .iter()
        .filter(|e| e.kind == ScriptKind::Region)
        .count();
    let cutscene_count = entries
        .iter()
        .filter(|e| e.kind == ScriptKind::Cutscene)
        .count();
    let shop_count = entries
        .iter()
        .filter(|e| e.kind == ScriptKind::Shop)
        .count();

    let file = ExtractFile {
        format: "madoua-text-extract-v1".to_string(),
        rom: ExtractRomInfo {
            path: rom_path.display().to_string(),
            size: rom.len(),
            crc32: format!("{:08X}", crc32fast::hash(&rom)),
            md5: md5_hex(&rom),
        },
        counts: ExtractCounts {
            total: entries.len(),
            regions: region_count,
            cutscenes: cutscene_count,
            shop: shop_count,
        },
        entries,
    };

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let json = serde_json::to_string_pretty(&file)?;
    std::fs::write(output_path, json)
        .with_context(|| format!("출력 쓰기 실패: {}", output_path.display()))?;

    println!("추출 완료: {}", output_path.display());
    println!(
        "entries: total {} = regions {} + cutscenes {} + shop {}",
        file.counts.total, file.counts.regions, file.counts.cutscenes, file.counts.shop
    );
    Ok(())
}

pub fn cmd_roundtrip_text(rom_path: &Path, input_path: &Path, out_dir: &Path) -> Result<()> {
    let rom = std::fs::read(rom_path)
        .with_context(|| format!("ROM 읽기 실패: {}", rom_path.display()))?;
    let text = std::fs::read_to_string(input_path)
        .with_context(|| format!("JSON 읽기 실패: {}", input_path.display()))?;
    let extract: ExtractFile = serde_json::from_str(&text)
        .with_context(|| format!("JSON 파싱 실패: {}", input_path.display()))?;
    anyhow::ensure!(
        extract.format == "madoua-text-extract-v1",
        "지원하지 않는 extract format: {}",
        extract.format
    );
    validate_counts(&extract)?;
    validate_entries_against_rom(&extract.entries, &rom)?;

    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("출력 디렉토리 생성 실패: {}", out_dir.display()))?;

    let mut wrote_files = 0usize;
    let mut wrote_bytes = 0usize;
    for region in 0..REGION_STRING_COUNTS.len() {
        let entries = entries_for_region(&extract.entries, region)?;
        let bin = build_slot_tabled_bin(&entries, SLOT1_BASE, 0x10)?;
        verify_slot_tabled_bin(&bin, &entries, SLOT1_BASE, 0x10)?;
        let path = out_dir.join(format!("region{region}.bin"));
        wrote_bytes += bin.len();
        wrote_files += 1;
        std::fs::write(&path, bin)
            .with_context(|| format!("출력 쓰기 실패: {}", path.display()))?;
    }

    let cutscenes = entries_for_kind(&extract.entries, ScriptKind::Cutscene, CUTSCENE_COUNT)?;
    let cutscene_bin = build_tabled_bin(&cutscenes)?;
    verify_tabled_bin(&cutscene_bin, &cutscenes)?;
    let cutscene_path = out_dir.join("cutscenes.bin");
    wrote_bytes += cutscene_bin.len();
    wrote_files += 1;
    std::fs::write(&cutscene_path, cutscene_bin)
        .with_context(|| format!("출력 쓰기 실패: {}", cutscene_path.display()))?;

    let shops = entries_for_kind(&extract.entries, ScriptKind::Shop, SHOP_COUNT)?;
    for entry in shops {
        let bytes = entry_bytes(entry)?;
        let path = out_dir.join(format!("shop_{:02}.bin", entry.index));
        wrote_bytes += bytes.len();
        wrote_files += 1;
        std::fs::write(&path, bytes)
            .with_context(|| format!("출력 쓰기 실패: {}", path.display()))?;
    }

    println!("라운드트립 완료: {}", out_dir.display());
    println!(
        "verified entries: {} (ROM raw match + rebuilt bin self-roundtrip)",
        extract.entries.len()
    );
    println!("wrote files: {wrote_files}, bytes: {wrote_bytes}");
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScriptRoutine {
    name: &'static str,
    logical: u16,
    physical: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScriptCallerHit {
    routine: ScriptRoutine,
    opcode: &'static str,
    physical: usize,
    bank: usize,
    logical: u16,
    slot: &'static str,
    immediate_clues: Vec<String>,
    context: Vec<String>,
}

const SCRIPT_ROUTINES: [ScriptRoutine; 6] = [
    ScriptRoutine {
        name: "runScript",
        logical: 0x98E0,
        physical: 0x198E0,
    },
    ScriptRoutine {
        name: "renderLoop",
        logical: 0x9979,
        physical: 0x19979,
    },
    ScriptRoutine {
        name: "fontDecode1bppTo4bpp",
        logical: 0x9A3E,
        physical: 0x19A3E,
    },
    ScriptRoutine {
        name: "runTabledScript",
        logical: 0xA00D,
        physical: 0x1A00D,
    },
    ScriptRoutine {
        name: "getScriptPointer",
        logical: 0xA05B,
        physical: 0x1A05B,
    },
    ScriptRoutine {
        name: "runCutsceneScript",
        logical: 0xA54E,
        physical: 0x1A54E,
    },
];

pub fn cmd_scan_script_callers(rom_path: &Path, context_lines: usize) -> Result<()> {
    let rom = std::fs::read(rom_path)
        .with_context(|| format!("ROM 읽기 실패: {}", rom_path.display()))?;
    let hits = scan_script_callers(&rom, context_lines);

    println!("script caller scan: {}", rom_path.display());
    println!(
        "ROM size: {} bytes = {} banks",
        rom.len(),
        rom.len().div_ceil(BANK_SIZE)
    );
    println!(
        "targets: {}",
        SCRIPT_ROUTINES
            .iter()
            .map(|r| format!("{}=${:04X}/0x{:05X}", r.name, r.logical, r.physical))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("hits: {}", hits.len());
    println!(
        "address model: bank0 physical 0x0000..0x3FFF is slot0 logical; later banks are reported as slot2 logical candidates"
    );

    for hit in hits {
        println!();
        println!(
            "{} {} (${:04X}) @ phys 0x{:05X} bank {:02} {} logical ${:04X}",
            hit.opcode,
            hit.routine.name,
            hit.routine.logical,
            hit.physical,
            hit.bank,
            hit.slot,
            hit.logical
        );
        if hit.immediate_clues.is_empty() {
            println!("  immediate clues: none in aligned disasm context");
        } else {
            println!("  immediate clues: {}", hit.immediate_clues.join(", "));
        }
        if hit.context.is_empty() {
            println!("  disasm context: unavailable from linear bank decode");
        } else {
            println!("  disasm context:");
            for line in hit.context {
                println!("    {line}");
            }
        }
    }

    Ok(())
}

fn scan_script_callers(rom: &[u8], context_lines: usize) -> Vec<ScriptCallerHit> {
    let aligned = aligned_disasm_by_physical(rom);
    let mut hits = Vec::new();
    for offset in 0..rom.len().saturating_sub(2) {
        let Some(opcode) = direct_target_opcode(rom[offset]) else {
            continue;
        };
        let logical_target = u16::from_le_bytes([rom[offset + 1], rom[offset + 2]]);
        let Some(routine) = SCRIPT_ROUTINES
            .iter()
            .copied()
            .find(|routine| routine.logical == logical_target)
        else {
            continue;
        };
        let (bank, slot, logical) = physical_to_debug_logical(offset);
        let (context, immediate_clues) = aligned
            .get(&offset)
            .map(|index| {
                (
                    disasm_context(&aligned.lines, *index, context_lines),
                    disasm_immediate_clues(&aligned.lines, *index, context_lines * 2),
                )
            })
            .unwrap_or_else(|| {
                (
                    Vec::new(),
                    immediate_clues_before(rom, offset, 16)
                        .into_iter()
                        .map(|clue| format!("raw {clue}"))
                        .collect(),
                )
            });
        hits.push(ScriptCallerHit {
            routine,
            opcode,
            physical: offset,
            bank,
            logical,
            slot,
            immediate_clues,
            context,
        });
    }
    hits
}

struct AlignedDisasm {
    lines: Vec<AlignedDisasmLine>,
    by_physical: std::collections::BTreeMap<usize, usize>,
}

impl AlignedDisasm {
    fn get(&self, physical: &usize) -> Option<&usize> {
        self.by_physical.get(physical)
    }
}

struct AlignedDisasmLine {
    physical: usize,
    immediate: Option<String>,
    text: String,
}

fn aligned_disasm_by_physical(rom: &[u8]) -> AlignedDisasm {
    let mut lines = Vec::new();
    let mut by_physical = std::collections::BTreeMap::new();

    push_aligned_disasm_window(rom, 0, 0x0000, 0x0000, &mut lines, &mut by_physical);
    for bank in 1..rom.len().div_ceil(BANK_SIZE) {
        let physical_start = bank * BANK_SIZE;
        push_aligned_disasm_window(
            rom,
            physical_start,
            SLOT2_BASE,
            physical_start,
            &mut lines,
            &mut by_physical,
        );
    }

    AlignedDisasm { lines, by_physical }
}

fn push_aligned_disasm_window(
    rom: &[u8],
    physical_start: usize,
    logical_base: u16,
    window_start: usize,
    lines: &mut Vec<AlignedDisasmLine>,
    by_physical: &mut std::collections::BTreeMap<usize, usize>,
) {
    if window_start >= rom.len() {
        return;
    }
    let window_end = (window_start + BANK_SIZE).min(rom.len());
    let window = &rom[window_start..window_end];
    let mut offset = 0usize;
    while offset < window.len() {
        let logical = logical_base + offset as u16;
        let physical = physical_start + offset;
        let (length, text, immediate) = match retro_z80::decode_bytes(&window[offset..]) {
            Ok(decoded) => {
                let length = decoded.length();
                let instruction = *decoded.instruction();
                let bytes = &window[offset..offset + length];
                (
                    length,
                    format!("${logical:04X}: {:<11} {instruction}", format_bytes(bytes)),
                    disasm_immediate_clue(physical, &instruction),
                )
            }
            Err(error) => {
                let byte = window[offset];
                (
                    1,
                    format!("${logical:04X}: {byte:02X}          .DB ${byte:02X} ; {error}"),
                    None,
                )
            }
        };
        by_physical.insert(physical, lines.len());
        lines.push(AlignedDisasmLine {
            physical,
            immediate,
            text,
        });
        offset += length;
    }
}

fn direct_target_opcode(opcode: u8) -> Option<&'static str> {
    let op = match opcode {
        0xC3 => "JP",
        0xC2 => "JP NZ",
        0xCA => "JP Z",
        0xD2 => "JP NC",
        0xDA => "JP C",
        0xE2 => "JP PO",
        0xEA => "JP PE",
        0xF2 => "JP P",
        0xFA => "JP M",
        0xCD => "CALL",
        0xC4 => "CALL NZ",
        0xCC => "CALL Z",
        0xD4 => "CALL NC",
        0xDC => "CALL C",
        0xE4 => "CALL PO",
        0xEC => "CALL PE",
        0xF4 => "CALL P",
        0xFC => "CALL M",
        _ => return None,
    };
    Some(op)
}

fn physical_to_debug_logical(physical: usize) -> (usize, &'static str, u16) {
    let bank = physical / BANK_SIZE;
    let in_bank = physical % BANK_SIZE;
    if bank == 0 {
        (bank, "slot0", in_bank as u16)
    } else {
        (bank, "slot2", SLOT2_BASE + in_bank as u16)
    }
}

fn immediate_clues_before(rom: &[u8], offset: usize, window: usize) -> Vec<String> {
    let start = offset.saturating_sub(window);
    let mut clues = Vec::new();
    for pos in start..offset {
        if pos + 1 < offset {
            if let Some(reg) = ld_r_imm_reg(rom[pos]) {
                clues.push(format!("0x{pos:05X}: LD {reg}, ${:02X}", rom[pos + 1]));
            }
        }
        if pos + 2 < offset {
            if let Some(reg) = ld_rr_imm_reg(rom[pos]) {
                let value = u16::from_le_bytes([rom[pos + 1], rom[pos + 2]]);
                clues.push(format!("0x{pos:05X}: LD {reg}, ${value:04X}"));
            }
        }
    }
    clues
}

fn ld_r_imm_reg(opcode: u8) -> Option<&'static str> {
    match opcode {
        0x06 => Some("B"),
        0x0E => Some("C"),
        0x16 => Some("D"),
        0x1E => Some("E"),
        0x26 => Some("H"),
        0x2E => Some("L"),
        0x3E => Some("A"),
        _ => None,
    }
}

fn ld_rr_imm_reg(opcode: u8) -> Option<&'static str> {
    match opcode {
        0x01 => Some("BC"),
        0x11 => Some("DE"),
        0x21 => Some("HL"),
        0x31 => Some("SP"),
        _ => None,
    }
}

fn disasm_context(lines: &[AlignedDisasmLine], index: usize, context_lines: usize) -> Vec<String> {
    let start = index.saturating_sub(context_lines);
    let end = (index + context_lines + 1).min(lines.len());
    lines[start..end]
        .iter()
        .map(|line| {
            let marker = if line.physical == lines[index].physical {
                "=>"
            } else {
                "  "
            };
            format!("{marker} phys 0x{:05X} {}", line.physical, line.text)
        })
        .collect()
}

fn disasm_immediate_clues(
    lines: &[AlignedDisasmLine],
    index: usize,
    context_lines: usize,
) -> Vec<String> {
    let start = index.saturating_sub(context_lines);
    lines[start..index]
        .iter()
        .filter_map(|line| line.immediate.clone())
        .collect()
}

fn disasm_immediate_clue(physical: usize, instruction: &retro_z80::Instruction) -> Option<String> {
    use retro_z80::Instruction;

    match instruction {
        Instruction::LdRImm(reg, value) => {
            Some(format!("0x{physical:05X}: LD {reg}, ${value:02X}"))
        }
        Instruction::LdRRImm(reg, value) => {
            Some(format!("0x{physical:05X}: LD {reg}, ${value:04X}"))
        }
        Instruction::LdIxImm(reg, value) => {
            Some(format!("0x{physical:05X}: LD {reg}, ${value:04X}"))
        }
        _ => None,
    }
}

fn format_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn cmd_init_translations(input_path: &Path, output_dir: &Path, force: bool) -> Result<()> {
    let text = std::fs::read_to_string(input_path)
        .with_context(|| format!("JSON 읽기 실패: {}", input_path.display()))?;
    let extract: ExtractFile = serde_json::from_str(&text)
        .with_context(|| format!("JSON 파싱 실패: {}", input_path.display()))?;
    anyhow::ensure!(
        extract.format == "madoua-text-extract-v1",
        "지원하지 않는 extract format: {}",
        extract.format
    );
    validate_counts(&extract)?;

    for stage in [
        "raw",
        "in_progress",
        "needs_review",
        "needs_human_review",
        "complete",
    ] {
        std::fs::create_dir_all(output_dir.join(stage))
            .with_context(|| format!("{stage} 디렉토리 생성 실패"))?;
    }

    let raw_dir = output_dir.join("raw");
    let mut wrote_files = 0usize;
    let mut wrote_entries = 0usize;
    for region in 0..REGION_STRING_COUNTS.len() {
        let entries = entries_for_region(&extract.entries, region)?;
        let path = raw_dir.join(format!("region_{region:02}.json"));
        write_translation_stage_file(
            &path,
            format!("region/{region} raw extract"),
            entries,
            force,
        )?;
        wrote_files += 1;
        wrote_entries += REGION_STRING_COUNTS[region];
    }

    let cutscenes = entries_for_kind(&extract.entries, ScriptKind::Cutscene, CUTSCENE_COUNT)?;
    write_translation_stage_file(
        &raw_dir.join("cutscenes.json"),
        "cutscene raw extract".to_string(),
        cutscenes,
        force,
    )?;
    wrote_files += 1;
    wrote_entries += CUTSCENE_COUNT;

    let shops = entries_for_kind(&extract.entries, ScriptKind::Shop, SHOP_COUNT)?;
    write_translation_stage_file(
        &raw_dir.join("shop.json"),
        "shop raw extract".to_string(),
        shops,
        force,
    )?;
    wrote_files += 1;
    wrote_entries += SHOP_COUNT;

    println!("번역 stage 초기화 완료: {}", output_dir.display());
    println!("stage dirs: raw, in_progress, needs_review, needs_human_review, complete");
    println!("raw files: {wrote_files}, entries: {wrote_entries}");
    println!("build/check directory input reads complete/ only");
    Ok(())
}

pub fn extract_entries(rom: &[u8]) -> Result<Vec<ScriptEntry>> {
    let mut entries = Vec::new();
    extract_regions(rom, &mut entries)?;
    extract_cutscenes(rom, &mut entries)?;
    extract_shop(rom, &mut entries)?;
    Ok(entries)
}

#[derive(Debug, Serialize)]
struct TranslationStageFile<'a> {
    format: &'static str,
    description: String,
    entries: Vec<TranslationStageEntry<'a>>,
}

#[derive(Debug, Serialize)]
struct TranslationStageEntry<'a> {
    id: &'a str,
    kind: ScriptKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    region: Option<usize>,
    index: usize,
    offset: String,
    len: usize,
    jp: &'a str,
    bytes_hex: &'a str,
    source_crc32: &'a str,
    ko: &'a str,
    status: &'a str,
    notes: &'a str,
}

fn write_translation_stage_file(
    path: &Path,
    description: String,
    entries: Vec<&ScriptEntry>,
    force: bool,
) -> Result<()> {
    if path.exists() && !force {
        anyhow::bail!(
            "{} already exists; pass --force to overwrite generated raw stage file",
            path.display()
        );
    }
    let file = TranslationStageFile {
        format: "madoua-translation-v1",
        description,
        entries: entries
            .iter()
            .map(|entry| TranslationStageEntry {
                id: &entry.id,
                kind: entry.kind,
                region: entry.region,
                index: entry.index,
                offset: format!("0x{:05X}", entry.offset),
                len: entry.len,
                jp: &entry.jp_preview,
                bytes_hex: &entry.bytes_hex,
                source_crc32: &entry.source_crc32,
                ko: "",
                status: "untranslated",
                notes: "",
            })
            .collect(),
    };
    let json = serde_json::to_string_pretty(&file)?;
    std::fs::write(path, json).with_context(|| format!("출력 쓰기 실패: {}", path.display()))
}

fn extract_regions(rom: &[u8], entries: &mut Vec<ScriptEntry>) -> Result<()> {
    for (region, &count) in REGION_STRING_COUNTS.iter().enumerate() {
        for index in 0..count {
            if region == 3 && index == 0x25 {
                entries.push(ScriptEntry {
                    id: format!("region/{region}/{index:03X}"),
                    kind: ScriptKind::Region,
                    region: Some(region),
                    index,
                    slot: 1,
                    offset: 0,
                    len: 0,
                    bytes_hex: String::new(),
                    source_crc32: String::new(),
                    jp_preview: String::new(),
                    ko: String::new(),
                    skip: false,
                });
                continue;
            }

            let table_addr = region_table_addr(rom, region)?;
            let ptr = read_u16_le(rom, table_addr + index * 2)
                .with_context(|| format!("region {region} script {index:03X} ptr"))?;
            let bank_base = region_bank_base(rom, region)?;
            let offset = slot_pointer_to_physical(bank_base, ptr, SLOT1_BASE)?;
            let bytes = read_script_bytes(rom, offset)
                .with_context(|| format!("region {region} script {index:03X} at 0x{offset:05X}"))?;
            entries.push(entry(
                ScriptKind::Region,
                Some(region),
                index,
                1,
                offset,
                bytes,
            ));
        }
    }
    Ok(())
}

fn validate_counts(file: &ExtractFile) -> Result<()> {
    let regions = file
        .entries
        .iter()
        .filter(|e| e.kind == ScriptKind::Region)
        .count();
    let cutscenes = file
        .entries
        .iter()
        .filter(|e| e.kind == ScriptKind::Cutscene)
        .count();
    let shop = file
        .entries
        .iter()
        .filter(|e| e.kind == ScriptKind::Shop)
        .count();
    anyhow::ensure!(
        file.counts.total == file.entries.len(),
        "count mismatch: total {} vs entries {}",
        file.counts.total,
        file.entries.len()
    );
    anyhow::ensure!(
        file.counts.regions == regions
            && file.counts.cutscenes == cutscenes
            && file.counts.shop == shop,
        "count mismatch: header regions/cutscenes/shop = {}/{}/{} but entries = {}/{}/{}",
        file.counts.regions,
        file.counts.cutscenes,
        file.counts.shop,
        regions,
        cutscenes,
        shop
    );
    Ok(())
}

fn validate_entries_against_rom(entries: &[ScriptEntry], rom: &[u8]) -> Result<()> {
    for entry in entries {
        let bytes = entry_bytes(entry)?;
        anyhow::ensure!(
            entry.len == bytes.len(),
            "{} len mismatch: metadata {} vs bytes {}",
            entry.id,
            entry.len,
            bytes.len()
        );
        if bytes.is_empty() && entry.offset == 0 {
            continue;
        }
        let end = entry
            .offset
            .checked_add(bytes.len())
            .with_context(|| format!("{} offset overflow", entry.id))?;
        anyhow::ensure!(
            end <= rom.len(),
            "{} range out of ROM: 0x{:05X}..0x{:05X}",
            entry.id,
            entry.offset,
            end
        );
        anyhow::ensure!(
            rom[entry.offset..end] == bytes,
            "{} raw bytes do not match ROM at 0x{:05X}",
            entry.id,
            entry.offset
        );
    }
    Ok(())
}

fn entries_for_region(entries: &[ScriptEntry], region: usize) -> Result<Vec<&ScriptEntry>> {
    let mut filtered: Vec<&ScriptEntry> = entries
        .iter()
        .filter(|e| e.kind == ScriptKind::Region && e.region == Some(region))
        .collect();
    filtered.sort_by_key(|e| e.index);
    let expected = REGION_STRING_COUNTS[region];
    anyhow::ensure!(
        filtered.len() == expected,
        "region {region} count mismatch: {} vs {expected}",
        filtered.len()
    );
    for (expected_index, entry) in filtered.iter().enumerate() {
        anyhow::ensure!(
            entry.index == expected_index,
            "region {region} index mismatch at position {expected_index}: {}",
            entry.index
        );
    }
    Ok(filtered)
}

fn entries_for_kind(
    entries: &[ScriptEntry],
    kind: ScriptKind,
    expected: usize,
) -> Result<Vec<&ScriptEntry>> {
    let mut filtered: Vec<&ScriptEntry> = entries.iter().filter(|e| e.kind == kind).collect();
    filtered.sort_by_key(|e| e.index);
    anyhow::ensure!(
        filtered.len() == expected,
        "{kind:?} count mismatch: {} vs {expected}",
        filtered.len()
    );
    for (expected_index, entry) in filtered.iter().enumerate() {
        anyhow::ensure!(
            entry.index == expected_index,
            "{kind:?} index mismatch at position {expected_index}: {}",
            entry.index
        );
    }
    Ok(filtered)
}

fn build_slot_tabled_bin(
    entries: &[&ScriptEntry],
    slot_base: u16,
    slot_sub_offset: u16,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let table_len = entries.len() * 2;
    let mut data_len = 0usize;
    for entry in entries {
        let ptr = slot_base as usize + slot_sub_offset as usize + table_len + data_len;
        anyhow::ensure!(
            ptr <= u16::MAX as usize,
            "{} pointer overflow: 0x{ptr:X}",
            entry.id
        );
        out.extend_from_slice(&(ptr as u16).to_le_bytes());
        data_len += entry_bytes(entry)?.len();
    }
    append_entry_bytes(&mut out, entries)?;
    Ok(out)
}

fn build_tabled_bin(entries: &[&ScriptEntry]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let table_len = entries.len() * 2;
    let mut data_len = 0usize;
    for entry in entries {
        let ptr = table_len + data_len;
        anyhow::ensure!(
            ptr <= u16::MAX as usize,
            "{} pointer overflow: 0x{ptr:X}",
            entry.id
        );
        out.extend_from_slice(&(ptr as u16).to_le_bytes());
        data_len += entry_bytes(entry)?.len();
    }
    append_entry_bytes(&mut out, entries)?;
    Ok(out)
}

fn append_entry_bytes(out: &mut Vec<u8>, entries: &[&ScriptEntry]) -> Result<()> {
    for entry in entries {
        out.extend_from_slice(&entry_bytes(entry)?);
    }
    Ok(())
}

fn verify_slot_tabled_bin(
    bin: &[u8],
    entries: &[&ScriptEntry],
    slot_base: u16,
    slot_sub_offset: u16,
) -> Result<()> {
    let logical_base = slot_base as usize + slot_sub_offset as usize;
    verify_tabled_bin_with_start(bin, entries, |ptr| {
        anyhow::ensure!(
            ptr as usize >= logical_base,
            "slot pointer ${ptr:04X} below logical base ${logical_base:04X}"
        );
        Ok(ptr as usize - logical_base)
    })
}

fn verify_tabled_bin(bin: &[u8], entries: &[&ScriptEntry]) -> Result<()> {
    verify_tabled_bin_with_start(bin, entries, |ptr| Ok(ptr as usize))
}

fn verify_tabled_bin_with_start<F>(
    bin: &[u8],
    entries: &[&ScriptEntry],
    ptr_to_start: F,
) -> Result<()>
where
    F: Fn(u16) -> Result<usize>,
{
    let table_len = entries.len() * 2;
    anyhow::ensure!(
        bin.len() >= table_len,
        "bin shorter than pointer table: {} < {table_len}",
        bin.len()
    );
    let mut starts = Vec::new();
    for i in 0..entries.len() {
        let ptr = read_u16_le(bin, i * 2)?;
        starts.push(ptr_to_start(ptr)?);
    }
    for (i, entry) in entries.iter().enumerate() {
        let start = starts[i];
        let end = starts.get(i + 1).copied().unwrap_or(bin.len());
        anyhow::ensure!(
            start <= end && end <= bin.len(),
            "{} invalid bin slice {start}..{end} of {}",
            entry.id,
            bin.len()
        );
        let expected = entry_bytes(entry)?;
        anyhow::ensure!(
            bin[start..end] == expected,
            "{} rebuilt bin bytes mismatch",
            entry.id
        );
    }
    Ok(())
}

fn entry_bytes(entry: &ScriptEntry) -> Result<Vec<u8>> {
    parse_hex_bytes(&entry.bytes_hex).with_context(|| format!("{} bytes_hex", entry.id))
}

pub fn raw_entry_bytes(entry: &ScriptEntry) -> Result<Vec<u8>> {
    entry_bytes(entry)
}

fn parse_hex_bytes(s: &str) -> Result<Vec<u8>> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    trimmed
        .split_whitespace()
        .map(|part| {
            anyhow::ensure!(part.len() == 2, "invalid byte token: {part}");
            u8::from_str_radix(part, 16).with_context(|| format!("invalid byte token: {part}"))
        })
        .collect()
}

fn extract_cutscenes(rom: &[u8], entries: &mut Vec<ScriptEntry>) -> Result<()> {
    let bank_base = (CUTSCENE_POINTER_TABLE / BANK_SIZE) * BANK_SIZE;
    for index in 0..CUTSCENE_COUNT {
        let ptr = read_u16_le(rom, CUTSCENE_POINTER_TABLE + index * 2)
            .with_context(|| format!("cutscene {index:03X} ptr"))?;
        let offset = slot_pointer_to_physical(bank_base, ptr, SLOT2_BASE)?;
        let bytes = read_script_bytes(rom, offset)
            .with_context(|| format!("cutscene {index:03X} at 0x{offset:05X}"))?;
        entries.push(entry(ScriptKind::Cutscene, None, index, 2, offset, bytes));
    }
    Ok(())
}

fn extract_shop(rom: &[u8], entries: &mut Vec<ScriptEntry>) -> Result<()> {
    let mut offset = SHOP_START;
    for index in 0..SHOP_COUNT {
        let bytes = read_script_bytes(rom, offset)
            .with_context(|| format!("shop {index:02} at 0x{offset:05X}"))?;
        let len = bytes.len();
        entries.push(entry(ScriptKind::Shop, None, index, 2, offset, bytes));
        offset += len;
    }
    Ok(())
}

fn entry(
    kind: ScriptKind,
    region: Option<usize>,
    index: usize,
    slot: u8,
    offset: usize,
    bytes: Vec<u8>,
) -> ScriptEntry {
    let id = match (kind, region) {
        (ScriptKind::Region, Some(region)) => format!("region/{region}/{index:03X}"),
        (ScriptKind::Cutscene, _) => format!("cutscene/{index:03X}"),
        (ScriptKind::Shop, _) => format!("shop/{index:02}"),
        (ScriptKind::Region, None) => unreachable!("region entries must carry region number"),
    };
    ScriptEntry {
        id,
        kind,
        region,
        index,
        slot,
        offset,
        len: bytes.len(),
        bytes_hex: hex_bytes(&bytes),
        source_crc32: source_crc32(&bytes),
        jp_preview: decode_preview(&bytes),
        ko: String::new(),
        skip: false,
    }
}

/// region 개수(리소스 테이블 크기).
pub const REGION_COUNT: usize = REGION_STRING_COUNTS.len();

/// region의 문자열(엔트리) 개수.
pub fn region_string_count(region: usize) -> usize {
    REGION_STRING_COUNTS[region]
}

/// region의 리소스 id(loc 테이블 loc[0]).
pub fn region_rsrc_id(rom: &[u8], region: usize) -> Result<u8> {
    let loc = REGION_LOC_TABLE + region * 2;
    anyhow::ensure!(
        loc + 1 < rom.len(),
        "region loc table out of ROM for region {region}"
    );
    Ok(rom[loc])
}

/// region 원본 리소스 테이블의 슬롯 포인터 `count`개를 그대로 읽는다. 빈(비-slot1) 엔트리의
/// 특수 포인터(예: money/상태 박스의 WRAM `$C8BF`)를 repack이 verbatim 보존하는 데 쓴다.
pub fn region_original_pointers(rom: &[u8], region: usize) -> Result<Vec<u16>> {
    let table_phys = region_table_addr(rom, region)?;
    let count = region_string_count(region);
    let mut ptrs = Vec::with_capacity(count);
    for i in 0..count {
        let p = read_u16_le(rom, table_phys + i * 2)
            .with_context(|| format!("region {region} 원본 포인터 idx {i}"))?;
        ptrs.push(p);
    }
    Ok(ptrs)
}

/// region repack용 bin(문자열 포인터 테이블 + 문자열)을 만든다. 테이블은 슬롯 1
/// `SLOT1_BASE + table_slot_offset`에 놓이고, 각 포인터는 문자열의 슬롯 1 주소다.
///
/// 빈 문자열(len 0)은 0x0000이 아니라 **원본 포인터(`orig_pointers[i]`)를 verbatim 보존**한다.
/// 이 게임에서 유일한 빈 엔트리는 필드 money/상태 박스의 WRAM 포인터 `$C8BF`(런타임에 RAM에
/// 조립되는 동적 문자열)로, 뱅크와 무관해 그대로 두는 것이 정확하다. 예전처럼 0x0000으로 두면
/// 엔진이 논리주소 0x0000(부트 코드)에서 문자열을 읽어 garbage를 렌더한다. slot1 in-bank
/// 포인터였다면 fresh 뱅크로 옮길 수 없으므로 실패로 드러낸다(현재 그런 엔트리 0건).
/// 반환 bin은 물리 `bank*0x4000 + table_slot_offset`에 배치한다.
pub fn build_region_repack_bin(
    strings: &[Vec<u8>],
    orig_pointers: &[u16],
    table_slot_offset: u16,
) -> Result<Vec<u8>> {
    anyhow::ensure!(
        strings.len() == orig_pointers.len(),
        "build_region_repack_bin: strings({}) != orig_pointers({})",
        strings.len(),
        orig_pointers.len()
    );
    let table_len = strings.len() * 2;
    let mut table = Vec::with_capacity(table_len);
    let mut data = Vec::new();
    for (i, s) in strings.iter().enumerate() {
        if s.is_empty() {
            let orig = orig_pointers[i];
            anyhow::ensure!(
                !(SLOT1_BASE..0x8000).contains(&orig),
                "region repack: 빈 문자열 idx {i}의 원본 포인터 ${orig:04X}가 slot1 in-bank라 \
                 verbatim 보존 불가(fresh 뱅크에 터미네이터 문자열이 필요)"
            );
            table.extend_from_slice(&orig.to_le_bytes());
        } else {
            let ptr = SLOT1_BASE as usize + table_slot_offset as usize + table_len + data.len();
            anyhow::ensure!(
                ptr <= u16::MAX as usize,
                "region repack 포인터 오버플로우: 0x{ptr:X}"
            );
            table.extend_from_slice(&(ptr as u16).to_le_bytes());
            data.extend_from_slice(s);
        }
    }
    table.extend_from_slice(&data);
    Ok(table)
}

fn region_bank_base(rom: &[u8], region: usize) -> Result<usize> {
    let loc = REGION_LOC_TABLE + region * 2;
    anyhow::ensure!(
        loc + 1 < rom.len(),
        "region loc table out of ROM for region {region}"
    );
    Ok(rom[loc + 1] as usize * BANK_SIZE)
}

fn region_table_addr(rom: &[u8], region: usize) -> Result<usize> {
    let loc = REGION_LOC_TABLE + region * 2;
    anyhow::ensure!(
        loc + 1 < rom.len(),
        "region loc table out of ROM for region {region}"
    );
    let rsrc_id = rom[loc] as usize;
    let bank_base = rom[loc + 1] as usize * BANK_SIZE;
    let table_ptr_addr = bank_base + 4 + rsrc_id * 2;
    let table_ptr = read_u16_le(rom, table_ptr_addr)
        .with_context(|| format!("region {region} resource pointer"))?;
    slot_pointer_to_physical(bank_base, table_ptr, SLOT1_BASE)
}

fn slot_pointer_to_physical(bank_base: usize, ptr: u16, slot_base: u16) -> Result<usize> {
    anyhow::ensure!(
        ptr >= slot_base,
        "slot pointer ${ptr:04X} below slot base ${slot_base:04X}"
    );
    Ok(bank_base + (ptr - slot_base) as usize)
}

fn physical_to_slot_pointer(bank_base: usize, offset: usize, slot_base: u16) -> Result<u16> {
    anyhow::ensure!(
        (bank_base..bank_base + BANK_SIZE).contains(&offset),
        "physical offset 0x{offset:05X} outside bank base 0x{bank_base:05X}"
    );
    let ptr = slot_base as usize + (offset - bank_base);
    anyhow::ensure!(
        ptr <= u16::MAX as usize,
        "slot pointer overflow for offset 0x{offset:05X}"
    );
    Ok(ptr as u16)
}

fn cutscene_bank_base() -> usize {
    (CUTSCENE_POINTER_TABLE / BANK_SIZE) * BANK_SIZE
}

pub fn find_largest_ff_run(data: &[u8], start: usize, end: usize) -> Option<(usize, usize)> {
    let end = end.min(data.len());
    let mut best: Option<(usize, usize)> = None;
    let mut pos = start.min(end);
    while pos < end {
        if data[pos] != 0xFF {
            pos += 1;
            continue;
        }
        let run_start = pos;
        while pos < end && data[pos] == 0xFF {
            pos += 1;
        }
        let run_end = pos;
        if best
            .map(|(best_start, best_end)| run_end - run_start > best_end - best_start)
            .unwrap_or(true)
        {
            best = Some((run_start, run_end));
        }
    }
    best
}

fn read_script_bytes(rom: &[u8], offset: usize) -> Result<Vec<u8>> {
    anyhow::ensure!(
        offset < rom.len(),
        "script offset out of ROM: 0x{offset:05X}"
    );
    let mut bytes = Vec::new();
    let mut pos = offset;
    while pos < rom.len() {
        let b = rom[pos];
        bytes.push(b);
        pos += 1;
        if b == 0xFE {
            anyhow::ensure!(pos < rom.len(), "flags op at EOF: 0x{:05X}", pos - 1);
            bytes.push(rom[pos]);
            pos += 1;
            continue;
        }
        if b == 0x00 {
            return Ok(bytes);
        }
    }
    anyhow::bail!("unterminated script at 0x{offset:05X}")
}

pub fn read_u16_le(data: &[u8], offset: usize) -> Result<u16> {
    anyhow::ensure!(
        offset + 1 < data.len(),
        "u16 read out of range at 0x{offset:05X}"
    );
    Ok(u16::from_le_bytes([data[offset], data[offset + 1]]))
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn source_crc32(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        String::new()
    } else {
        format!("{:08X}", crc32fast::hash(bytes))
    }
}

fn md5_hex(data: &[u8]) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_preview(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            0x00 => {
                out.push_str("[end]");
                i += 1;
            }
            0xFD => {
                out.push_str("[wait]");
                i += 1;
            }
            0xFE => {
                out.push_str("[flags]");
                if let Some(&param) = bytes.get(i + 1) {
                    out.push_str(&format!("<${param:02X}>"));
                    i += 2;
                } else {
                    i += 1;
                }
            }
            0xFF => {
                out.push_str("[br]");
                i += 1;
            }
            0xFB | 0xFC => {
                if let Some(&next) = bytes.get(i + 1) {
                    if let Some(s) = dakuten_pair(b, next) {
                        out.push_str(s);
                        i += 2;
                        continue;
                    }
                }
                out.push_str(single_token(b).unwrap_or("<$??>"));
                i += 1;
            }
            _ => {
                if let Some(s) = single_token(b) {
                    if s.is_empty() {
                        out.push_str(&format!("<${b:02X}>"));
                    } else {
                        out.push_str(s);
                    }
                } else {
                    out.push_str(&format!("<${b:02X}>"));
                }
                i += 1;
            }
        }
    }
    out
}

pub fn encode_jp_char(ch: char) -> Option<Vec<u8>> {
    let text = ch.to_string();
    for b in 0x01..=0xFC {
        if single_token(b) == Some(text.as_str()) {
            return Some(vec![b]);
        }
    }
    for prefix in [0xFB, 0xFC] {
        for base in 0x00..=0xFF {
            if dakuten_pair(prefix, base) == Some(text.as_str()) {
                return Some(vec![prefix, base]);
            }
        }
    }
    None
}

fn single_token(b: u8) -> Option<&'static str> {
    Some(match b {
        0x01 => "　",
        0x02 => "０",
        0x03 => "１",
        0x04 => "２",
        0x05 => "３",
        0x06 => "４",
        0x07 => "５",
        0x08 => "６",
        0x09 => "７",
        0x0A => "８",
        0x0B => "９",
        0x0C => "金",
        0x0D => "、",
        0x0E => "。",
        0x0F => "あ",
        0x10 => "い",
        0x11 => "う",
        0x12 => "え",
        0x13 => "お",
        0x14 => "か",
        0x15 => "き",
        0x16 => "く",
        0x17 => "け",
        0x18 => "こ",
        0x19 => "さ",
        0x1A => "し",
        0x1B => "す",
        0x1C => "せ",
        0x1D => "そ",
        0x1E => "た",
        0x1F => "ち",
        0x20 => "つ",
        0x21 => "て",
        0x22 => "と",
        0x23 => "な",
        0x24 => "に",
        0x25 => "ぬ",
        0x26 => "ね",
        0x27 => "の",
        0x28 => "は",
        0x29 => "ひ",
        0x2A => "ふ",
        0x2B => "へ",
        0x2C => "ほ",
        0x2D => "ま",
        0x2E => "み",
        0x2F => "む",
        0x30 => "め",
        0x31 => "も",
        0x32 => "や",
        0x33 => "ゆ",
        0x34 => "よ",
        0x35 => "ら",
        0x36 => "り",
        0x37 => "る",
        0x38 => "れ",
        0x39 => "ろ",
        0x3A => "わ",
        0x3B => "ん",
        0x3C => "を",
        0x3D => "ぁ",
        0x3E => "ぃ",
        0x3F => "ぅ",
        0x40 => "ぇ",
        0x41 => "ぉ",
        0x42 => "ゃ",
        0x43 => "ゅ",
        0x44 => "ょ",
        0x45 => "っ",
        0x46 => "ア",
        0x47 => "イ",
        0x48 => "ウ",
        0x49 => "エ",
        0x4A => "オ",
        0x4B => "カ",
        0x4C => "キ",
        0x4D => "ク",
        0x4E => "ケ",
        0x4F => "コ",
        0x50 => "サ",
        0x51 => "シ",
        0x52 => "ス",
        0x53 => "セ",
        0x54 => "ソ",
        0x55 => "タ",
        0x56 => "チ",
        0x57 => "ツ",
        0x58 => "テ",
        0x59 => "ト",
        0x5A => "ナ",
        0x5B => "ニ",
        0x5C => "ヌ",
        0x5D => "ネ",
        0x5E => "ノ",
        0x5F => "ハ",
        0x60 => "ヒ",
        0x61 => "フ",
        0x62 => "ヘ",
        0x63 => "ホ",
        0x64 => "マ",
        0x65 => "ミ",
        0x66 => "ム",
        0x67 => "メ",
        0x68 => "モ",
        0x69 => "ヤ",
        0x6A => "ユ",
        0x6B => "ヨ",
        0x6C => "ラ",
        0x6D => "リ",
        0x6E => "ル",
        0x6F => "レ",
        0x70 => "ロ",
        0x71 => "ワ",
        0x72 => "ン",
        0x73 => "ヲ",
        0x74 => "ァ",
        0x75 => "ィ",
        0x76 => "ゥ",
        0x77 => "ェ",
        0x78 => "ォ",
        0x79 => "ャ",
        0x7A => "ュ",
        0x7B => "ョ",
        0x7C => "ッ",
        0x7D => "『",
        0x7E => "』",
        0x7F => "！",
        0x80 => "？",
        0x81 => "・",
        0x82 => "ー",
        0x83 => "＆",
        0x84 => "．",
        0x85 => "Ａ",
        0x86 => "Ｂ",
        0x87 => "Ｃ",
        0x88 => "Ｄ",
        0x89 => "Ｅ",
        0x8A => "Ｆ",
        0x8B => "Ｇ",
        0x8C => "Ｈ",
        0x8D => "Ｉ",
        0x8E => "Ｊ",
        0x8F => "Ｋ",
        0x90 => "Ｌ",
        0x91 => "Ｍ",
        0x92 => "Ｎ",
        0x93 => "Ｏ",
        0x94 => "Ｐ",
        0x95 => "Ｑ",
        0x96 => "Ｒ",
        0x97 => "Ｓ",
        0x98 => "Ｔ",
        0x99 => "Ｕ",
        0x9A => "Ｖ",
        0x9B => "Ｗ",
        0x9C => "Ｘ",
        0x9D => "Ｙ",
        0x9E => "Ｚ",
        0x9F => "（",
        0xA0 => "）",
        0xA1 => "[copyright]",
        0xA2 => "…",
        0xFB => "゜",
        0xFC => "゛",
        _ => return None,
    })
}

fn dakuten_pair(prefix: u8, base: u8) -> Option<&'static str> {
    Some(match (prefix, base) {
        (0xFB, 0x28) => "ぱ",
        (0xFB, 0x29) => "ぴ",
        (0xFB, 0x2A) => "ぷ",
        (0xFB, 0x2B) => "ぺ",
        (0xFB, 0x2C) => "ぽ",
        (0xFB, 0x5F) => "パ",
        (0xFB, 0x60) => "ピ",
        (0xFB, 0x61) => "プ",
        (0xFB, 0x62) => "ペ",
        (0xFB, 0x63) => "ポ",
        (0xFC, 0x14) => "が",
        (0xFC, 0x15) => "ぎ",
        (0xFC, 0x16) => "ぐ",
        (0xFC, 0x17) => "げ",
        (0xFC, 0x18) => "ご",
        (0xFC, 0x19) => "ざ",
        (0xFC, 0x1A) => "じ",
        (0xFC, 0x1B) => "ず",
        (0xFC, 0x1C) => "ぜ",
        (0xFC, 0x1D) => "ぞ",
        (0xFC, 0x1E) => "だ",
        (0xFC, 0x1F) => "ぢ",
        (0xFC, 0x20) => "づ",
        (0xFC, 0x21) => "で",
        (0xFC, 0x22) => "ど",
        (0xFC, 0x28) => "ば",
        (0xFC, 0x29) => "び",
        (0xFC, 0x2A) => "ぶ",
        (0xFC, 0x2B) => "べ",
        (0xFC, 0x2C) => "ぼ",
        (0xFC, 0x4B) => "ガ",
        (0xFC, 0x4C) => "ギ",
        (0xFC, 0x4D) => "グ",
        (0xFC, 0x4E) => "ゲ",
        (0xFC, 0x4F) => "ゴ",
        (0xFC, 0x50) => "ザ",
        (0xFC, 0x51) => "ジ",
        (0xFC, 0x52) => "ズ",
        (0xFC, 0x53) => "ゼ",
        (0xFC, 0x54) => "ゾ",
        (0xFC, 0x55) => "ダ",
        (0xFC, 0x56) => "ヂ",
        (0xFC, 0x57) => "ヅ",
        (0xFC, 0x58) => "デ",
        (0xFC, 0x59) => "ド",
        (0xFC, 0x5F) => "バ",
        (0xFC, 0x60) => "ビ",
        (0xFC, 0x61) => "ブ",
        (0xFC, 0x62) => "ベ",
        (0xFC, 0x63) => "ボ",
        (0xFC, 0x76) => "ヴ",
        _ => return None,
    })
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_param_zero_does_not_terminate_script() {
        let rom = [0xFE, 0x00, 0x10, 0x00];
        assert_eq!(read_script_bytes(&rom, 0).unwrap(), rom);
    }

    #[test]
    fn decodes_controls_and_dakuten_pairs() {
        let bytes = [0xFC, 0x14, 0xFF, 0xFE, 0x2A, 0x00];
        assert_eq!(decode_preview(&bytes), "が[br][flags]<$2A>[end]");
    }

    #[test]
    fn computes_region_table_address_from_resource_table() {
        let mut rom = vec![0xFF; 0x20000];
        rom[REGION_LOC_TABLE] = 0x15;
        rom[REGION_LOC_TABLE + 1] = 0x07;
        let bank_base = 0x07 * BANK_SIZE;
        rom[bank_base + 4 + 0x15 * 2..bank_base + 6 + 0x15 * 2]
            .copy_from_slice(&0x4826u16.to_le_bytes());
        assert_eq!(region_table_addr(&rom, 0).unwrap(), 0x1C826);
    }

    #[test]
    fn slot_pointer_conversion_respects_slot_base() {
        assert_eq!(
            slot_pointer_to_physical(0x18000, 0xA66D, SLOT2_BASE).unwrap(),
            0x1A66D
        );
        assert_eq!(cutscene_pointer_for_physical(0x1B790).unwrap(), 0xB790);
        assert_eq!(
            cutscene_pointer_offset(2).unwrap(),
            CUTSCENE_POINTER_TABLE + 4
        );
    }

    #[test]
    fn region_pointer_helpers_resolve_resource_table() {
        let mut rom = vec![0xFF; 0x20000];
        rom[REGION_LOC_TABLE] = 0x15;
        rom[REGION_LOC_TABLE + 1] = 0x07;
        let bank_base = 0x07 * BANK_SIZE;
        rom[bank_base + 4 + 0x15 * 2..bank_base + 6 + 0x15 * 2]
            .copy_from_slice(&0x4800u16.to_le_bytes());
        rom[bank_base + 0x0800..bank_base + 0x0802].copy_from_slice(&0x4030u16.to_le_bytes());

        assert_eq!(region_bank_base_for_region(&rom, 0).unwrap(), bank_base);
        assert_eq!(
            region_pointer_offset(&rom, 0, 0).unwrap(),
            bank_base + 0x0800
        );
        assert_eq!(
            region_pointer_for_physical(&rom, 0, bank_base + 0x3E6C).unwrap(),
            0x7E6C
        );
    }

    #[test]
    fn region_relocation_start_uses_largest_ff_run_in_region_bank() {
        let mut rom = vec![0x00; 0x20000];
        rom[REGION_LOC_TABLE] = 0x15;
        rom[REGION_LOC_TABLE + 1] = 0x07;
        let bank_base = 0x07 * BANK_SIZE;
        rom[bank_base + 4 + 0x15 * 2..bank_base + 6 + 0x15 * 2]
            .copy_from_slice(&0x4800u16.to_le_bytes());
        rom[bank_base + 0x0100..bank_base + 0x0110].fill(0xFF);
        rom[bank_base + 0x0300..bank_base + 0x0330].fill(0xFF);

        assert_eq!(
            region_relocation_start(&rom, 0).unwrap(),
            bank_base + 0x0300
        );
    }

    #[test]
    fn script_caller_scan_finds_bank6_slot2_anchors() {
        let mut rom = vec![0x00; BANK_SIZE * 7];
        let start = 6 * BANK_SIZE + 0x2018;
        rom[start..start + 10].copy_from_slice(&[
            0x3E, 0x02, // LD A,$02
            0x0E, 0x0F, // LD C,$0F
            0xCD, 0xE0, 0x98, // CALL $98E0
            0xC3, 0x0D, 0xA0, // JP $A00D
        ]);

        let hits = scan_script_callers(&rom, 1);

        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].routine.name, "runScript");
        assert_eq!(hits[0].opcode, "CALL");
        assert_eq!(hits[0].physical, start + 4);
        assert_eq!(hits[0].bank, 6);
        assert_eq!(hits[0].slot, "slot2");
        assert_eq!(hits[0].logical, 0xA01C);
        assert!(
            hits[0]
                .immediate_clues
                .iter()
                .any(|clue| clue.ends_with("LD A, $02"))
        );
        assert!(
            hits[0]
                .immediate_clues
                .iter()
                .any(|clue| clue.ends_with("LD C, $0F"))
        );

        assert_eq!(hits[1].routine.name, "runTabledScript");
        assert_eq!(hits[1].opcode, "JP");
        assert_eq!(hits[1].physical, start + 7);
        assert_eq!(hits[1].logical, 0xA01F);
    }

    #[test]
    fn slot_tabled_bin_roundtrips_entry_bytes_and_empty_entries() {
        let entries = vec![
            ScriptEntry {
                id: "region/0/000".to_string(),
                kind: ScriptKind::Region,
                region: Some(0),
                index: 0,
                slot: 1,
                offset: 0x100,
                len: 2,
                bytes_hex: "01 00".to_string(),
                source_crc32: String::new(),
                jp_preview: String::new(),
                ko: String::new(),
                skip: false,
            },
            ScriptEntry {
                id: "region/0/001".to_string(),
                kind: ScriptKind::Region,
                region: Some(0),
                index: 1,
                slot: 1,
                offset: 0,
                len: 0,
                bytes_hex: String::new(),
                source_crc32: String::new(),
                jp_preview: String::new(),
                ko: String::new(),
                skip: false,
            },
            ScriptEntry {
                id: "region/0/002".to_string(),
                kind: ScriptKind::Region,
                region: Some(0),
                index: 2,
                slot: 1,
                offset: 0x102,
                len: 3,
                bytes_hex: "10 FD 00".to_string(),
                source_crc32: String::new(),
                jp_preview: String::new(),
                ko: String::new(),
                skip: false,
            },
        ];
        let refs: Vec<&ScriptEntry> = entries.iter().collect();
        let bin = build_slot_tabled_bin(&refs, SLOT1_BASE, 0x10).unwrap();
        assert_eq!(&bin[..6], &[0x16, 0x40, 0x18, 0x40, 0x18, 0x40]);
        assert_eq!(&bin[6..], &[0x01, 0x00, 0x10, 0xFD, 0x00]);
        verify_slot_tabled_bin(&bin, &refs, SLOT1_BASE, 0x10).unwrap();
    }

    #[test]
    fn tabled_bin_roundtrips_entry_bytes() {
        let entries = vec![
            ScriptEntry {
                id: "cutscene/000".to_string(),
                kind: ScriptKind::Cutscene,
                region: None,
                index: 0,
                slot: 2,
                offset: 0x200,
                len: 2,
                bytes_hex: "01 00".to_string(),
                source_crc32: String::new(),
                jp_preview: String::new(),
                ko: String::new(),
                skip: false,
            },
            ScriptEntry {
                id: "cutscene/001".to_string(),
                kind: ScriptKind::Cutscene,
                region: None,
                index: 1,
                slot: 2,
                offset: 0x202,
                len: 3,
                bytes_hex: "10 FD 00".to_string(),
                source_crc32: String::new(),
                jp_preview: String::new(),
                ko: String::new(),
                skip: false,
            },
        ];
        let refs: Vec<&ScriptEntry> = entries.iter().collect();
        let bin = build_tabled_bin(&refs).unwrap();
        assert_eq!(&bin[..4], &[0x04, 0x00, 0x06, 0x00]);
        assert_eq!(&bin[4..], &[0x01, 0x00, 0x10, 0xFD, 0x00]);
        verify_tabled_bin(&bin, &refs).unwrap();
    }

    #[test]
    fn parse_hex_bytes_rejects_bad_tokens() {
        assert_eq!(parse_hex_bytes("0A ff").unwrap(), vec![0x0A, 0xFF]);
        assert!(parse_hex_bytes("0AF").is_err());
        assert!(parse_hex_bytes("GG").is_err());
    }

    #[test]
    fn encodes_preview_jp_chars_back_to_bytes() {
        assert_eq!(encode_jp_char('お').unwrap(), vec![0x13]);
        assert_eq!(encode_jp_char('ば').unwrap(), vec![0xFC, 0x28]);
        assert_eq!(encode_jp_char('ヴ').unwrap(), vec![0xFC, 0x76]);
    }

    // 회귀: region 3 idx 37은 필드 money/상태 박스의 WRAM 포인터($C8BF, len 0로 추출됨)다.
    // repack이 이걸 0x0000으로 두면 엔진이 0x0000(부트 코드)를 문자열로 읽어 garble이 난다.
    // 빈 엔트리는 원본 특수 포인터를 verbatim 보존해야 한다.
    #[test]
    fn repack_bin_preserves_special_empty_pointer() {
        let strings = vec![vec![0x10u8, 0x00], Vec::new(), vec![0x11u8, 0x00]];
        let orig = vec![0x4020u16, 0xC8BF, 0x4030];
        let bin = build_region_repack_bin(&strings, &orig, 0x10).unwrap();
        let e0 = u16::from_le_bytes([bin[0], bin[1]]);
        let e1 = u16::from_le_bytes([bin[2], bin[3]]);
        let e2 = u16::from_le_bytes([bin[4], bin[5]]);
        // 빈 엔트리는 원본 WRAM 포인터를 보존한다(0x0000이면 이전 버그).
        assert_eq!(e1, 0xC8BF, "빈 엔트리는 원본 특수 포인터를 보존해야 함");
        // 비-빈 엔트리는 테이블(6바이트) 뒤 문자열을 가리킨다.
        // SLOT1_BASE(0x4000)+offset(0x10)+table_len(6) = 0x4016.
        assert_eq!(e0, 0x4016);
        assert_eq!(e2, 0x4018); // idx0 문자열 2바이트 뒤

        // slot1 in-bank 포인터를 빈 엔트리로 주면 실패해야 한다(안전 가드).
        let bad = build_region_repack_bin(&[Vec::new()], &[0x4500u16], 0x10);
        assert!(bad.is_err(), "slot1 in-bank 빈 포인터는 거부돼야 함");
    }
}

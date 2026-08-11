//! 마도물어 A (Game Gear) 한글 패치 빌드 도구.
//!
//! 서브커맨드 동사 규약(project-conventions §3.1): info / scan / extract / check / build.

use clap::{Parser, Subcommand};

mod bps;
mod font;
mod glyph;
mod lzss;
mod rom;
mod runtime;
mod scan;
mod script;
mod translation;
mod ui_graphics;

#[derive(Parser)]
#[command(name = "madoua_kr", about = "마도물어 A 한글 패치 빌드 도구")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 입력 ROM의 메타데이터(크기·해시·헤더)를 출력한다.
    Info {
        /// 원본 ROM 경로
        #[arg(long)]
        rom: std::path::PathBuf,
    },
    /// 선두 바이트 출현 빈도를 집계해 한글 프리픽스 후보(0회 대역)를 찾는다.
    ScanPrefix {
        #[arg(long)]
        rom: std::path::PathBuf,
        /// madouaggtools 형식의 script txt 디렉토리. 지정하면 #STARTMSG 범위만 집계한다.
        #[arg(long)]
        script_dir: Option<std::path::PathBuf>,
    },
    /// ROM 안의 0xFF/0x00 연속 자유공간 후보를 뱅크별로 스캔한다.
    ScanFreespace {
        #[arg(long)]
        rom: std::path::PathBuf,
        #[arg(long, default_value_t = 128)]
        min_run: usize,
    },
    /// 원본에서 텍스트를 추출해 JSON으로 떨군다.
    ExtractText {
        #[arg(long)]
        rom: std::path::PathBuf,
        #[arg(long)]
        output: std::path::PathBuf,
    },
    /// 추출 JSON의 raw bytes를 빌드용 region/cutscene/shop bin으로 재생성하고 검증한다.
    RoundtripText {
        #[arg(long)]
        rom: std::path::PathBuf,
        #[arg(long)]
        input: std::path::PathBuf,
        #[arg(long)]
        out_dir: std::path::PathBuf,
    },
    /// 추출 JSON을 번역 workflow stage 디렉토리(raw/in_progress/.../complete)로 펼친다.
    InitTranslations {
        #[arg(long)]
        input: std::path::PathBuf,
        #[arg(long)]
        output: std::path::PathBuf,
        /// 기존 raw 파일 덮어쓰기.
        #[arg(long)]
        force: bool,
    },
    /// 번역 텍스트의 글리프 커버리지를 검사한다(ROM 없이 dry-run 가능).
    CheckGlyphs {
        #[arg(long)]
        translations: std::path::PathBuf,
    },
    /// 번역 용어집이 raw 추출 엔트리를 실제로 참조하는지 검사한다.
    CheckTerms {
        #[arg(long)]
        terms: std::path::PathBuf,
        #[arg(long)]
        raw: std::path::PathBuf,
    },
    /// 번역 stage 파일이 raw 기준선·상태 규칙·용어집을 지키는지 검사한다.
    CheckStage {
        #[arg(long)]
        stage: std::path::PathBuf,
        #[arg(long)]
        raw: std::path::PathBuf,
        #[arg(long)]
        terms: std::path::PathBuf,
        /// 단일 prefix 용량(249) 초과를 오류로 만들지 않고 정확성만 검증한다(다중 프리픽스 대기).
        #[arg(long)]
        defer_glyph_cap: bool,
    },
    /// shop/HUD/graphics text surface catalog가 raw 추출 기준선을 덮는지 검사한다.
    CheckSurfaces {
        #[arg(long)]
        catalog: std::path::PathBuf,
        #[arg(long)]
        raw: std::path::PathBuf,
        /// Release-readiness mode: fail if any cataloged surface is still unverified or blocked.
        #[arg(long)]
        require_release_ready: bool,
    },
    /// shop money byte와 메인 폰트 후보 글리프 소스를 정적 검증한다.
    CheckMoneySources {
        #[arg(long)]
        rom: std::path::PathBuf,
        #[arg(long)]
        raw: std::path::PathBuf,
    },
    /// UI 압축 그래픽 블록이 LZSS 코덱으로 무손실 라운드트립·in-place fit하는지 검증한다(트랙 B).
    CheckUiGraphics {
        #[arg(long)]
        rom: std::path::PathBuf,
    },
    /// UI 압축 블록을 디컴프해 타일을 팔레트 인덱스 그리드로 렌더한다(지오메트리 재측정).
    DumpUiGraphic {
        #[arg(long)]
        rom: std::path::PathBuf,
        #[arg(long)]
        name: String,
        /// 6타일씩 24×16 버튼으로 조립: rowmajor | colmajor.
        #[arg(long)]
        assemble: Option<String>,
    },
    /// UI 블록을 한글 라벨로 재조판해 결과를 렌더하고 재압축 fit을 보고한다(빌드 전 검증).
    PreviewUiRetype {
        #[arg(long)]
        rom: std::path::PathBuf,
        #[arg(long)]
        name: String,
        /// 버튼당 한글 라벨(콤마 구분, 각 최대 2글자).
        #[arg(long)]
        labels: String,
        #[arg(long)]
        font: std::path::PathBuf,
    },
    /// in-place UI 라벨(UI_LABELS)만 적용한 ROM을 빌드한다(트랙 B PoC).
    BuildUi {
        #[arg(long)]
        rom: std::path::PathBuf,
        #[arg(long)]
        font: std::path::PathBuf,
        #[arg(long)]
        output: std::path::PathBuf,
        #[arg(long)]
        bps_output: Option<std::path::PathBuf>,
        /// JP 원본이 아닌 소스로 빌드를 허용한다(기본은 CRC 불일치 시 실패).
        #[arg(long)]
        allow_noncanonical_source: bool,
    },
    /// 알려진 텍스트 엔진 루틴을 호출하는 Z80 CALL/JP 앵커를 스캔한다.
    ScanScriptCallers {
        #[arg(long)]
        rom: std::path::PathBuf,
        /// 호출점 주변 disasm 문맥 줄 수.
        #[arg(long, default_value_t = 5)]
        context: usize,
    },
    /// RetroArch headless screenshot gate를 실행한다.
    CheckRuntime {
        #[arg(long)]
        rom: std::path::PathBuf,
        #[arg(long)]
        screenshot: std::path::PathBuf,
        #[arg(long, default_value_t = 980)]
        frames: u32,
        #[arg(long)]
        retroarch: Option<std::path::PathBuf>,
        #[arg(long)]
        core: Option<std::path::PathBuf>,
        #[arg(long)]
        replay: Option<std::path::PathBuf>,
        #[arg(long)]
        expected_crc32: Option<String>,
    },
    /// Runtime scene manifest의 active gate들을 실행하고 pending gate들을 검증한다.
    CheckRuntimeScenes {
        #[arg(long)]
        manifest: std::path::PathBuf,
        #[arg(long)]
        retroarch: Option<std::path::PathBuf>,
        #[arg(long)]
        core: Option<std::path::PathBuf>,
        /// Release-readiness mode: fail if any runtime scene is still pending.
        #[arg(long)]
        require_all_active: bool,
    },
    /// Runtime scene manifest의 pending route/probe를 emucap 실행 계획 JSON으로 출력한다.
    ExportRuntimeProbePlan {
        #[arg(long)]
        manifest: std::path::PathBuf,
        /// 특정 scene id만 출력한다. 생략하면 pending scene 전체를 출력한다.
        #[arg(long)]
        scene: Option<String>,
        /// 출력할 probe install phase. fresh_start는 install_phase가 비어 있는 probe도 포함한다.
        #[arg(long = "install-phase")]
        install_phase: Option<String>,
    },
    /// emucap poll_events JSON을 runtime scene manifest probe와 매칭해 요약한다.
    SummarizeRuntimeProbeEvents {
        #[arg(long)]
        manifest: std::path::PathBuf,
        /// emucap poll_events JSON 파일. 여러 번 지정할 수 있다.
        #[arg(long = "event", required = true)]
        events: Vec<std::path::PathBuf>,
        /// 특정 scene id만 요약한다.
        #[arg(long)]
        scene: Option<String>,
    },
    /// Runtime probe event summary JSON이 scene proof로 쓸 수 있는지 검사한다.
    CheckRuntimeProbeSummary {
        #[arg(long)]
        summary: std::path::PathBuf,
        /// 특정 scene id만 검사한다. summary가 여러 scene이면 필수다.
        #[arg(long)]
        scene: Option<String>,
        /// scene 전체에서 요구할 최소 valid hit 수.
        #[arg(long, default_value_t = 1)]
        min_valid_hits: usize,
        /// 반드시 valid hit가 있어야 하는 probe id. 여러 번 지정할 수 있다.
        #[arg(long = "require-valid-probe")]
        required_valid_probes: Vec<String>,
        /// invalid hit가 하나라도 있으면 실패한다.
        #[arg(long)]
        reject_invalid_hits: bool,
        /// 어떤 manifest probe에도 매칭되지 않은 event가 하나라도 있으면 실패한다.
        #[arg(long)]
        reject_unmatched_events: bool,
        /// emucap dropped event가 있어도 통과를 허용한다.
        #[arg(long)]
        allow_dropped_events: bool,
    },
    /// PoC: 폰트 슬롯 한 개를 한글 '가'로 교체해 패치 ROM을 만든다(단계1 글리프 교체).
    PocPatch {
        #[arg(long)]
        rom: std::path::PathBuf,
        #[arg(long)]
        out: std::path::PathBuf,
        /// 교체할 폰트 글리프 인덱스(기본 0x3A = 'ん').
        #[arg(long, default_value_t = 0x3A)]
        index: usize,
    },
    /// TTF를 8×8 1bpp로 렌더해 ASCII로 출력(폰트·임계값 튜닝용).
    FontRender {
        #[arg(long)]
        ttf: std::path::PathBuf,
        #[arg(long)]
        text: String,
        #[arg(long, default_value_t = 8.0)]
        px: f32,
        #[arg(long, default_value_t = 128)]
        threshold: u8,
        #[arg(long, default_value_t = 0)]
        x_off: i32,
        #[arg(long, default_value_t = 0)]
        y_off: i32,
    },
    /// 2차 PoC: 프리픽스 디스패치 훅 + 한글 뱅크 + 스크립트 편집(인코딩 공간 검증).
    Poc2Patch {
        #[arg(long)]
        rom: std::path::PathBuf,
        #[arg(long)]
        out: std::path::PathBuf,
    },
    /// 2차 fixed PoC: prefix/base를 2 iteration으로 처리해 연속 텍스트 desync를 피한다.
    Poc2FixedPatch {
        #[arg(long)]
        rom: std::path::PathBuf,
        #[arg(long)]
        out: std::path::PathBuf,
    },
    /// 3차 PoC: 실제 폰트로 한글 2음절 단어를 인게임에 표시(TTF→화면 end-to-end).
    Poc3Patch {
        #[arg(long)]
        rom: std::path::PathBuf,
        #[arg(long)]
        out: std::path::PathBuf,
        #[arg(long)]
        ttf: std::path::PathBuf,
        #[arg(long, default_value = "마도")]
        word: String,
    },
    /// 3차 fixed PoC: 실제 폰트 2음절을 2-iteration prefix/base 모델로 표시한다.
    Poc3FixedPatch {
        #[arg(long)]
        rom: std::path::PathBuf,
        #[arg(long)]
        out: std::path::PathBuf,
        #[arg(long)]
        ttf: std::path::PathBuf,
        #[arg(long, default_value = "마도")]
        word: String,
    },
    /// 번역 JSON + 폰트 + 원본 → 패치 ROM 생성.
    Build {
        #[arg(long)]
        rom: std::path::PathBuf,
        #[arg(long)]
        translations: std::path::PathBuf,
        #[arg(long)]
        font: std::path::PathBuf,
        #[arg(long)]
        output: std::path::PathBuf,
        /// BPS patch output path. When set, build writes both patched ROM and source-verified BPS patch.
        #[arg(long)]
        bps_output: Option<std::path::PathBuf>,
        /// JP 원본이 아닌 소스로 빌드를 허용한다(기본은 CRC 불일치 시 실패).
        #[arg(long)]
        allow_noncanonical_source: bool,
        /// needs_human_review stage를 QA 후보로 빌드한다. complete 전용 배포 경로에는 영향 없음.
        #[arg(long)]
        preview_human_review: bool,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Info { rom } => rom::cmd_info(&rom),
        Command::PocPatch { rom, out, index } => rom::cmd_poc_patch(&rom, &out, index),
        Command::FontRender {
            ttf,
            text,
            px,
            threshold,
            x_off,
            y_off,
        } => font::cmd_font_render(&ttf, &text, px, threshold, x_off, y_off),
        Command::Poc2Patch { rom, out } => rom::cmd_poc2_patch(&rom, &out),
        Command::Poc2FixedPatch { rom, out } => rom::cmd_poc2_fixed_patch(&rom, &out),
        Command::Poc3Patch {
            rom,
            out,
            ttf,
            word,
        } => rom::cmd_poc3_patch(&rom, &out, &ttf, &word),
        Command::Poc3FixedPatch {
            rom,
            out,
            ttf,
            word,
        } => rom::cmd_poc3_fixed_patch(&rom, &out, &ttf, &word),
        Command::ScanPrefix { rom, script_dir } => {
            scan::cmd_scan_prefix(&rom, script_dir.as_deref())
        }
        Command::ScanFreespace { rom, min_run } => scan::cmd_scan_freespace(&rom, min_run),
        Command::ExtractText { rom, output } => script::cmd_extract_text(&rom, &output),
        Command::RoundtripText {
            rom,
            input,
            out_dir,
        } => script::cmd_roundtrip_text(&rom, &input, &out_dir),
        Command::InitTranslations {
            input,
            output,
            force,
        } => script::cmd_init_translations(&input, &output, force),
        Command::CheckGlyphs { translations } => translation::cmd_check_glyphs(&translations),
        Command::CheckTerms { terms, raw } => translation::cmd_check_terms(&terms, &raw),
        Command::CheckStage {
            stage,
            raw,
            terms,
            defer_glyph_cap,
        } => translation::cmd_check_stage(&stage, &raw, &terms, defer_glyph_cap),
        Command::CheckSurfaces {
            catalog,
            raw,
            require_release_ready,
        } => translation::cmd_check_surfaces(&catalog, &raw, require_release_ready),
        Command::CheckMoneySources { rom, raw } => translation::cmd_check_money_sources(&rom, &raw),
        Command::CheckUiGraphics { rom } => ui_graphics::cmd_check_ui_graphics(&rom),
        Command::DumpUiGraphic {
            rom,
            name,
            assemble,
        } => ui_graphics::cmd_dump_ui_graphic(&rom, &name, assemble.as_deref()),
        Command::PreviewUiRetype {
            rom,
            name,
            labels,
            font,
        } => ui_graphics::cmd_preview_ui_retype(&rom, &name, &labels, &font),
        Command::BuildUi {
            rom,
            font,
            output,
            bps_output,
            allow_noncanonical_source,
        } => ui_graphics::cmd_build_ui(
            &rom,
            &font,
            &output,
            bps_output.as_deref(),
            allow_noncanonical_source,
        ),
        Command::ScanScriptCallers { rom, context } => {
            script::cmd_scan_script_callers(&rom, context)
        }
        Command::CheckRuntime {
            rom,
            screenshot,
            frames,
            retroarch,
            core,
            replay,
            expected_crc32,
        } => runtime::cmd_check_runtime(
            &rom,
            &screenshot,
            frames,
            retroarch.as_deref(),
            core.as_deref(),
            replay.as_deref(),
            expected_crc32.as_deref(),
        ),
        Command::CheckRuntimeScenes {
            manifest,
            retroarch,
            core,
            require_all_active,
        } => runtime::cmd_check_runtime_scenes(
            &manifest,
            retroarch.as_deref(),
            core.as_deref(),
            require_all_active,
        ),
        Command::ExportRuntimeProbePlan {
            manifest,
            scene,
            install_phase,
        } => runtime::cmd_export_runtime_probe_plan(
            &manifest,
            scene.as_deref(),
            install_phase.as_deref(),
        ),
        Command::SummarizeRuntimeProbeEvents {
            manifest,
            events,
            scene,
        } => runtime::cmd_summarize_runtime_probe_events(&manifest, &events, scene.as_deref()),
        Command::CheckRuntimeProbeSummary {
            summary,
            scene,
            min_valid_hits,
            required_valid_probes,
            reject_invalid_hits,
            reject_unmatched_events,
            allow_dropped_events,
        } => runtime::cmd_check_runtime_probe_summary(
            &summary,
            scene.as_deref(),
            min_valid_hits,
            &required_valid_probes,
            reject_invalid_hits,
            reject_unmatched_events,
            allow_dropped_events,
        ),
        Command::Build {
            rom,
            translations,
            font,
            output,
            bps_output,
            allow_noncanonical_source,
            preview_human_review,
        } => rom::cmd_build(
            &rom,
            &translations,
            &font,
            &output,
            bps_output.as_deref(),
            allow_noncanonical_source,
            preview_human_review,
        ),
    }
}

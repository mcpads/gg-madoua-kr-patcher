use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

const DEFAULT_RETROARCH: &str = "/Applications/RetroArch.app/Contents/MacOS/RetroArch";
const DEFAULT_CORE_SUFFIX: &str =
    "Library/Application Support/RetroArch/cores/genesis_plus_gx_libretro.dylib";

pub fn cmd_check_runtime(
    rom_path: &Path,
    screenshot_path: &Path,
    frames: u32,
    retroarch_path: Option<&Path>,
    core_path: Option<&Path>,
    replay_path: Option<&Path>,
    expected_crc32: Option<&str>,
) -> Result<()> {
    anyhow::ensure!(frames > 0, "--frames must be > 0");
    anyhow::ensure!(
        rom_path.is_file(),
        "ROM 파일을 찾을 수 없음: {}",
        rom_path.display()
    );

    let retroarch = retroarch_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_RETROARCH));
    anyhow::ensure!(
        retroarch.is_file(),
        "RetroArch 실행 파일을 찾을 수 없음: {}",
        retroarch.display()
    );

    let core = match core_path {
        Some(path) => path.to_path_buf(),
        None => default_core_path()?,
    };
    anyhow::ensure!(
        core.is_file(),
        "RetroArch genesis_plus_gx core를 찾을 수 없음: {}",
        core.display()
    );

    if let Some(replay) = replay_path {
        anyhow::ensure!(
            replay.is_file(),
            "replay 파일을 찾을 수 없음: {}",
            replay.display()
        );
    }

    if let Some(parent) = screenshot_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("screenshot 디렉토리 생성 실패: {}", parent.display()))?;
    }
    if screenshot_path.exists() {
        std::fs::remove_file(screenshot_path)
            .with_context(|| format!("기존 screenshot 삭제 실패: {}", screenshot_path.display()))?;
    }

    let mut command = ProcessCommand::new(&retroarch);
    command.arg("-L").arg(&core);
    if let Some(replay) = replay_path {
        command.arg("-P").arg(replay);
    }
    command
        .arg(rom_path)
        .arg(format!("--max-frames={frames}"))
        .arg("--max-frames-ss")
        .arg(format!(
            "--max-frames-ss-path={}",
            screenshot_path.display()
        ));

    let output = command
        .output()
        .with_context(|| format!("RetroArch 실행 실패: {}", retroarch.display()))?;
    anyhow::ensure!(
        output.status.success(),
        "RetroArch exited with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let screenshot = std::fs::read(screenshot_path)
        .with_context(|| format!("screenshot 읽기 실패: {}", screenshot_path.display()))?;
    anyhow::ensure!(
        !screenshot.is_empty(),
        "screenshot이 비어 있음: {}",
        screenshot_path.display()
    );
    let crc32 = crc32fast::hash(&screenshot);
    if let Some(expected) = expected_crc32 {
        let expected = parse_crc32(expected)?;
        anyhow::ensure!(
            crc32 == expected,
            "screenshot CRC32 mismatch: expected {expected:08X}, got {crc32:08X}"
        );
    }

    println!("runtime gate 완료: {}", rom_path.display());
    println!("frames: {frames}");
    println!("screenshot: {}", screenshot_path.display());
    println!("screenshot bytes: {}", screenshot.len());
    println!("screenshot CRC32: {crc32:08X}");
    Ok(())
}

pub fn cmd_check_runtime_scenes(
    manifest_path: &Path,
    retroarch_path: Option<&Path>,
    core_path: Option<&Path>,
    require_all_active: bool,
) -> Result<()> {
    let manifest = load_runtime_scene_manifest(manifest_path)?;
    let mut active = 0usize;
    let mut pending = 0usize;

    for scene in &manifest.scenes {
        validate_runtime_scene(scene)?;
        validate_runtime_scene_artifacts(scene)?;
    }
    if require_all_active {
        ensure_all_runtime_scenes_active(&manifest)?;
    }

    for scene in &manifest.scenes {
        match scene.status.as_str() {
            "active" => {
                active += 1;
                let rom = scene
                    .rom
                    .as_deref()
                    .with_context(|| format!("{}: active scene requires rom", scene.id))?;
                let screenshot = scene
                    .screenshot
                    .as_deref()
                    .with_context(|| format!("{}: active scene requires screenshot", scene.id))?;
                println!("runtime scene: {}", scene.id);
                cmd_check_runtime(
                    rom,
                    screenshot,
                    scene.frames.unwrap_or(980),
                    retroarch_path,
                    core_path,
                    scene.replay.as_deref(),
                    scene.expected_crc32.as_deref(),
                )?;
            }
            "pending" => {
                pending += 1;
                println!("runtime scene pending: {} ({})", scene.id, scene.notes);
                if let Some(route) = &scene.candidate_route {
                    println!(
                        "  candidate route: {} from {} ({} actions)",
                        route.tool,
                        route.start,
                        route.actions.len()
                    );
                }
                for probe in &scene.probe_requirements {
                    println!(
                        "  probe requirement: {} {} {}",
                        probe.kind,
                        probe.id,
                        probe.address.as_deref().unwrap_or("-")
                    );
                }
            }
            _ => unreachable!("validate_runtime_scene checked status"),
        }
    }

    println!(
        "runtime scene manifest 검사 완료: {}",
        manifest_path.display()
    );
    println!("active scenes: {active}");
    println!("pending scenes: {pending}");
    Ok(())
}

pub fn cmd_export_runtime_probe_plan(
    manifest_path: &Path,
    scene_id: Option<&str>,
    install_phase: Option<&str>,
) -> Result<()> {
    let manifest = load_runtime_scene_manifest(manifest_path)?;
    for scene in &manifest.scenes {
        validate_runtime_scene(scene)?;
        validate_runtime_scene_artifacts(scene)?;
    }
    let plan = build_runtime_probe_plan(&manifest, manifest_path, scene_id, install_phase)?;
    println!("{}", serde_json::to_string_pretty(&plan)?);
    Ok(())
}

pub fn cmd_summarize_runtime_probe_events(
    manifest_path: &Path,
    event_paths: &[PathBuf],
    scene_id: Option<&str>,
) -> Result<()> {
    let summary = build_probe_event_report(manifest_path, event_paths, scene_id)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

pub fn cmd_check_runtime_probe_summary(
    summary_path: &Path,
    scene_id: Option<&str>,
    min_valid_hits: usize,
    required_valid_probes: &[String],
    reject_invalid_hits: bool,
    reject_unmatched_events: bool,
    allow_dropped_events: bool,
) -> Result<()> {
    let report = load_probe_event_report(summary_path)?;
    let result = check_probe_event_report(
        &report,
        scene_id,
        min_valid_hits,
        required_valid_probes,
        reject_invalid_hits,
        reject_unmatched_events,
        allow_dropped_events,
    )?;
    println!(
        "runtime probe summary gate 완료: {}",
        summary_path.display()
    );
    println!("scenes checked: {}", result.scenes_checked);
    println!("valid hits: {}", result.valid_hit_count);
    println!("invalid hits: {}", result.invalid_hit_count);
    println!("unmatched events: {}", result.unmatched_event_count);
    if !required_valid_probes.is_empty() {
        println!(
            "required valid probes: {}",
            required_valid_probes.join(", ")
        );
    }
    Ok(())
}

fn build_probe_event_report(
    manifest_path: &Path,
    event_paths: &[PathBuf],
    scene_id: Option<&str>,
) -> Result<ProbeEventReport> {
    anyhow::ensure!(
        !event_paths.is_empty(),
        "event 파일을 하나 이상 지정해야 함"
    );
    let manifest = load_runtime_scene_manifest(manifest_path)?;
    for scene in &manifest.scenes {
        validate_runtime_scene(scene)?;
    }

    let selected_scenes: Vec<&RuntimeScene> = manifest
        .scenes
        .iter()
        .filter(|scene| scene_id.is_none_or(|scene_id| scene.id == scene_id))
        .collect();
    anyhow::ensure!(
        !selected_scenes.is_empty(),
        "{}",
        match scene_id {
            Some(scene_id) => format!("runtime scene not found: {scene_id}"),
            None => "runtime scene manifest has no scenes".to_string(),
        }
    );

    let mut event_sets = Vec::new();
    let mut total_events = 0usize;
    let mut dropped = 0u64;
    for path in event_paths {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("event JSON 읽기 실패: {}", path.display()))?;
        let events: EmucapPollEvents = serde_json::from_str(&text)
            .with_context(|| format!("event JSON 파싱 실패: {}", path.display()))?;
        dropped += events.dropped.unwrap_or_default();
        total_events += events.events.len();
        event_sets.push((path, events));
    }

    let mut matched = 0usize;
    let mut unmatched = Vec::new();
    let mut scene_summaries = Vec::new();
    for scene in &selected_scenes {
        let mut probe_summaries = Vec::new();
        for probe in &scene.probe_requirements {
            if emucap_breakpoint_request(probe).is_none() {
                continue;
            }
            let mut hits = Vec::new();
            for (path, event_set) in &event_sets {
                for event in &event_set.events {
                    if probe_matches_event(probe, event)? {
                        let validity = evaluate_probe_hit_validity(probe, event)?;
                        hits.push(ProbeEventHit {
                            path: path.display().to_string(),
                            frame: event.frame,
                            breakpoint_id: event.breakpoint_id,
                            address: event.address,
                            value: event.value,
                            pc: event.pc,
                            valid_when_matched: validity.matched,
                            valid_when_detail: validity.detail,
                            snapshot: event.snapshot.clone(),
                        });
                    }
                }
            }
            matched += hits.len();
            let valid_hit_count = hits
                .iter()
                .filter(|hit| hit.valid_when_matched != Some(false))
                .count();
            let invalid_hit_count = hits
                .iter()
                .filter(|hit| hit.valid_when_matched == Some(false))
                .count();
            probe_summaries.push(ProbeEventSummary {
                probe_id: probe.id.clone(),
                kind: probe.kind.clone(),
                address: probe.address.clone(),
                end: probe.end.clone(),
                hit_count: hits.len(),
                valid_hit_count,
                invalid_hit_count,
                hits,
            });
        }
        let probe_count = probe_summaries.len();
        let probes_with_hits = probe_summaries
            .iter()
            .filter(|probe| probe.hit_count > 0)
            .count();
        let probes_with_valid_hits = probe_summaries
            .iter()
            .filter(|probe| probe.valid_hit_count > 0)
            .count();
        let hit_count = probe_summaries.iter().map(|probe| probe.hit_count).sum();
        let valid_hit_count = probe_summaries
            .iter()
            .map(|probe| probe.valid_hit_count)
            .sum();
        let invalid_hit_count = probe_summaries
            .iter()
            .map(|probe| probe.invalid_hit_count)
            .sum();
        scene_summaries.push(SceneEventSummary {
            scene_id: scene.id.clone(),
            probe_count,
            probes_with_hits,
            probes_with_valid_hits,
            hit_count,
            valid_hit_count,
            invalid_hit_count,
            probes: probe_summaries,
        });
    }

    for (path, event_set) in &event_sets {
        for event in &event_set.events {
            let mut event_matched = false;
            for scene in &selected_scenes {
                for probe in &scene.probe_requirements {
                    if probe_matches_event(probe, event)? {
                        event_matched = true;
                        break;
                    }
                }
                if event_matched {
                    break;
                }
            }
            if !event_matched {
                unmatched.push(UnmatchedProbeEvent {
                    path: path.display().to_string(),
                    kind: event.kind.clone(),
                    address: event.address,
                    pc: event.pc,
                    frame: event.frame,
                    breakpoint_id: event.breakpoint_id,
                });
            }
        }
    }

    Ok(ProbeEventReport {
        format: "madoua-runtime-probe-events-summary-v1".to_string(),
        manifest: manifest_path.display().to_string(),
        scene: scene_id.map(str::to_string),
        event_files: event_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        total_events,
        dropped_events: dropped,
        matched_events: matched,
        unmatched_events: unmatched,
        scenes: scene_summaries,
    })
}

fn load_probe_event_report(path: &Path) -> Result<ProbeEventReport> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("probe summary JSON 읽기 실패: {}", path.display()))?;
    let report: ProbeEventReport = serde_json::from_str(&text)
        .with_context(|| format!("probe summary JSON 파싱 실패: {}", path.display()))?;
    anyhow::ensure!(
        report.format == "madoua-runtime-probe-events-summary-v1",
        "지원하지 않는 probe summary format: {}",
        report.format
    );
    anyhow::ensure!(
        !report.scenes.is_empty(),
        "probe summary scenes가 비어 있음: {}",
        path.display()
    );
    Ok(report)
}

#[derive(Debug)]
struct ProbeSummaryGateResult {
    scenes_checked: usize,
    valid_hit_count: usize,
    invalid_hit_count: usize,
    unmatched_event_count: usize,
}

fn check_probe_event_report(
    report: &ProbeEventReport,
    scene_id: Option<&str>,
    min_valid_hits: usize,
    required_valid_probes: &[String],
    reject_invalid_hits: bool,
    reject_unmatched_events: bool,
    allow_dropped_events: bool,
) -> Result<ProbeSummaryGateResult> {
    if !allow_dropped_events {
        anyhow::ensure!(
            report.dropped_events == 0,
            "probe summary has dropped events: {}",
            report.dropped_events
        );
    }
    if scene_id.is_none() && report.scenes.len() != 1 {
        anyhow::bail!(
            "probe summary has {} scenes; pass --scene to choose one",
            report.scenes.len()
        );
    }

    let selected_scenes: Vec<&SceneEventSummary> = report
        .scenes
        .iter()
        .filter(|scene| scene_id.is_none_or(|scene_id| scene.scene_id == scene_id))
        .collect();
    anyhow::ensure!(
        !selected_scenes.is_empty(),
        "{}",
        match scene_id {
            Some(scene_id) => format!("probe summary scene not found: {scene_id}"),
            None => "probe summary has no scenes".to_string(),
        }
    );

    let valid_hit_count = selected_scenes
        .iter()
        .map(|scene| scene.valid_hit_count)
        .sum();
    let invalid_hit_count: usize = selected_scenes
        .iter()
        .map(|scene| scene.invalid_hit_count)
        .sum();
    anyhow::ensure!(
        valid_hit_count >= min_valid_hits,
        "runtime probe summary gate failed: valid_hit_count {valid_hit_count} < required {min_valid_hits}"
    );
    if reject_invalid_hits {
        anyhow::ensure!(
            invalid_hit_count == 0,
            "runtime probe summary gate failed: invalid_hit_count {invalid_hit_count} > 0"
        );
    }
    let unmatched_event_count = report.unmatched_events.len();
    if reject_unmatched_events {
        anyhow::ensure!(
            unmatched_event_count == 0,
            "runtime probe summary gate failed: unmatched_event_count {unmatched_event_count} > 0"
        );
    }

    for required_probe in required_valid_probes {
        let valid_for_probe: usize = selected_scenes
            .iter()
            .flat_map(|scene| &scene.probes)
            .filter(|probe| probe.probe_id == *required_probe)
            .map(|probe| probe.valid_hit_count)
            .sum();
        anyhow::ensure!(
            valid_for_probe > 0,
            "runtime probe summary gate failed: required probe {required_probe} has no valid hits"
        );
    }

    Ok(ProbeSummaryGateResult {
        scenes_checked: selected_scenes.len(),
        valid_hit_count,
        invalid_hit_count,
        unmatched_event_count,
    })
}

fn ensure_all_runtime_scenes_active(manifest: &RuntimeSceneManifest) -> Result<()> {
    let pending: Vec<&str> = manifest
        .scenes
        .iter()
        .filter(|scene| scene.status == "pending")
        .map(|scene| scene.id.as_str())
        .collect();
    anyhow::ensure!(
        pending.is_empty(),
        "runtime release readiness failed: pending scenes: {}",
        pending.join(", ")
    );
    Ok(())
}

fn default_core_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .context("HOME 환경변수가 없어 RetroArch core 기본 경로를 만들 수 없음")?;
    Ok(PathBuf::from(home).join(DEFAULT_CORE_SUFFIX))
}

fn parse_crc32(raw: &str) -> Result<u32> {
    let trimmed = raw.trim().trim_start_matches('$').trim_start_matches("0x");
    anyhow::ensure!(trimmed.len() == 8, "CRC32는 8자리 hex여야 함: {raw}");
    u32::from_str_radix(trimmed, 16).with_context(|| format!("CRC32 파싱 실패: {raw}"))
}

#[derive(Debug, Deserialize)]
struct RuntimeSceneManifest {
    format: String,
    scenes: Vec<RuntimeScene>,
}

#[derive(Debug, Deserialize)]
struct RuntimeScene {
    id: String,
    status: String,
    #[serde(default)]
    rom: Option<PathBuf>,
    #[serde(default)]
    screenshot: Option<PathBuf>,
    #[serde(default)]
    frames: Option<u32>,
    #[serde(default)]
    replay: Option<PathBuf>,
    #[serde(default)]
    expected_crc32: Option<String>,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    candidate_route: Option<CandidateRoute>,
    #[serde(default)]
    probe_requirements: Vec<ProbeRequirement>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CandidateRoute {
    tool: String,
    start: String,
    #[serde(default)]
    actions: Vec<CandidateRouteAction>,
    #[serde(default)]
    evidence_screenshot: String,
    #[serde(default)]
    evidence_crc32: String,
    #[serde(default)]
    expected_text_ref: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct CandidateRouteAction {
    op: String,
    #[serde(default)]
    frames: Option<u32>,
    #[serde(default)]
    buttons: Vec<String>,
    #[serde(default)]
    count: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ProbeRequirement {
    id: String,
    kind: String,
    #[serde(default)]
    memory_type: Option<String>,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    end: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    value_len: Option<u8>,
    #[serde(default)]
    value_mask: Option<String>,
    #[serde(default)]
    pc_min: Option<String>,
    #[serde(default)]
    pc_max: Option<String>,
    #[serde(default)]
    install_phase: Option<String>,
    #[serde(default)]
    bank_snapshot: Option<String>,
    #[serde(default)]
    valid_when: Option<String>,
    #[serde(default)]
    notes: String,
}

#[derive(Debug, Serialize)]
struct RuntimeProbePlan<'a> {
    format: &'static str,
    manifest: String,
    selection: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    install_phase: Option<String>,
    fresh_start_required: bool,
    notes: &'static str,
    scenes: Vec<RuntimeProbeScenePlan<'a>>,
}

#[derive(Debug, Serialize)]
struct RuntimeProbeScenePlan<'a> {
    id: &'a str,
    status: &'a str,
    notes: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidate_route: Option<&'a CandidateRoute>,
    probes: Vec<RuntimeProbeStep<'a>>,
}

#[derive(Debug, Serialize)]
struct RuntimeProbeStep<'a> {
    probe: &'a ProbeRequirement,
    #[serde(skip_serializing_if = "Option::is_none")]
    emucap_set_breakpoint: Option<EmucapBreakpointRequest<'a>>,
}

#[derive(Debug, Serialize)]
struct EmucapBreakpointRequest<'a> {
    tool: &'static str,
    args: EmucapBreakpointArgs<'a>,
}

#[derive(Debug, Serialize)]
struct EmucapBreakpointArgs<'a> {
    kind: &'static str,
    memory_type: &'a str,
    start: &'a str,
    end: &'a str,
    pause_on_hit: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value_len: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value_mask: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pc_min: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pc_max: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot: Option<Vec<&'a str>>,
}

fn build_runtime_probe_plan<'a>(
    manifest: &'a RuntimeSceneManifest,
    manifest_path: &Path,
    scene_id: Option<&str>,
    install_phase: Option<&str>,
) -> Result<RuntimeProbePlan<'a>> {
    if let Some(install_phase) = install_phase {
        anyhow::ensure!(
            supported_install_phase(install_phase),
            "unsupported install_phase filter {install_phase}"
        );
    }

    let scenes: Vec<RuntimeProbeScenePlan<'a>> = manifest
        .scenes
        .iter()
        .filter(|scene| match scene_id {
            Some(scene_id) => scene.id == scene_id,
            None => scene.status == "pending",
        })
        .map(|scene| RuntimeProbeScenePlan {
            id: &scene.id,
            status: &scene.status,
            notes: &scene.notes,
            candidate_route: scene.candidate_route.as_ref(),
            probes: scene
                .probe_requirements
                .iter()
                .filter(|probe| probe_matches_install_phase(probe, install_phase))
                .map(|probe| RuntimeProbeStep {
                    probe,
                    emucap_set_breakpoint: emucap_breakpoint_request(probe),
                })
                .collect(),
        })
        .collect();

    anyhow::ensure!(
        !scenes.is_empty(),
        "{}",
        match scene_id {
            Some(scene_id) => format!("runtime scene not found: {scene_id}"),
            None => "runtime scene manifest has no pending scenes".to_string(),
        }
    );

    Ok(RuntimeProbePlan {
        format: "madoua-emucap-probe-plan-v1",
        manifest: manifest_path.display().to_string(),
        selection: scene_id.map(str::to_owned),
        install_phase: install_phase.map(str::to_owned),
        fresh_start_required: true,
        notes: "Start from a fresh boot or explicitly documented fresh route. In this Mesen/GG setup, install breakpoints after reset because reset clears breakpoints. Interpret banked CPU addresses only with the included mapper snapshot.",
        scenes,
    })
}

fn probe_matches_install_phase(probe: &ProbeRequirement, install_phase: Option<&str>) -> bool {
    match install_phase {
        None => true,
        Some("fresh_start") => {
            probe.install_phase.as_deref().unwrap_or("fresh_start") == "fresh_start"
        }
        Some(phase) => probe.install_phase.as_deref() == Some(phase),
    }
}

fn emucap_breakpoint_request(probe: &ProbeRequirement) -> Option<EmucapBreakpointRequest<'_>> {
    let kind = emucap_breakpoint_kind(probe)?;
    let memory_type = probe.memory_type.as_deref()?;
    let address = probe.address.as_deref()?;
    let end = probe.end.as_deref().unwrap_or(address);
    Some(EmucapBreakpointRequest {
        tool: "set_breakpoint",
        args: EmucapBreakpointArgs {
            kind,
            memory_type,
            start: address,
            end,
            pause_on_hit: true,
            value: probe.value.as_deref(),
            value_len: probe.value.as_ref().map(|_| probe.value_len.unwrap_or(1)),
            value_mask: probe.value_mask.as_deref(),
            pc_min: probe.pc_min.as_deref(),
            pc_max: probe.pc_max.as_deref(),
            snapshot: probe
                .bank_snapshot
                .as_deref()
                .map(|snapshot| vec![snapshot]),
        },
    })
}

fn emucap_breakpoint_kind(probe: &ProbeRequirement) -> Option<&'static str> {
    match probe.kind.as_str() {
        "logical_exec_bp" => Some("exec"),
        "logical_read_bp" => Some("read"),
        "vram_write_bp" => Some("write"),
        _ => None,
    }
}

fn probe_matches_event(probe: &ProbeRequirement, event: &EmucapEvent) -> Result<bool> {
    let Some(expected_kind) = emucap_breakpoint_kind(probe) else {
        return Ok(false);
    };
    if event.kind.as_deref() != Some(expected_kind) {
        return Ok(false);
    }
    let Some(event_address) = event.address else {
        return Ok(false);
    };
    let Some(start) = probe.address.as_deref() else {
        return Ok(false);
    };
    let start = parse_address(start)?;
    let end = match probe.end.as_deref() {
        Some(end) => parse_address(end)?,
        None => start,
    };
    if !(start..=end).contains(&event_address) {
        return Ok(false);
    }
    if probe.pc_min.is_some() || probe.pc_max.is_some() {
        let Some(event_pc) = event.pc else {
            return Ok(false);
        };
        let pc_min = probe
            .pc_min
            .as_deref()
            .map(parse_address)
            .transpose()?
            .unwrap_or(0);
        let pc_max = probe
            .pc_max
            .as_deref()
            .map(parse_address)
            .transpose()?
            .unwrap_or(u32::MAX);
        if !(pc_min..=pc_max).contains(&event_pc) {
            return Ok(false);
        }
    }
    if let Some(value) = probe.value.as_deref() {
        let expected = parse_address(value)?;
        let Some(actual) = event.value else {
            return Ok(false);
        };
        let mask = match probe.value_mask.as_deref() {
            Some(mask) => parse_address(mask)?,
            None => u32::MAX,
        };
        if (actual & mask) != (expected & mask) {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Debug)]
struct ProbeHitValidity {
    matched: Option<bool>,
    detail: Option<String>,
}

fn evaluate_probe_hit_validity(
    probe: &ProbeRequirement,
    event: &EmucapEvent,
) -> Result<ProbeHitValidity> {
    let Some(valid_when) = probe.valid_when.as_deref().map(str::trim) else {
        return Ok(ProbeHitValidity {
            matched: None,
            detail: None,
        });
    };
    if valid_when.is_empty() {
        return Ok(ProbeHitValidity {
            matched: None,
            detail: None,
        });
    }

    let mut checks = Vec::new();
    for clause in valid_when.split(';').map(str::trim) {
        if let Some(raw) = clause.strip_prefix("slot1=") {
            checks.push((1u8, parse_condition_value(raw)?));
        } else if let Some(raw) = clause.strip_prefix("slot2=") {
            checks.push((2u8, parse_condition_value(raw)?));
        }
    }
    if checks.is_empty() {
        return Ok(ProbeHitValidity {
            matched: None,
            detail: Some(format!(
                "valid_when has no machine-parsed bank condition: {valid_when}"
            )),
        });
    }

    let mut details = Vec::new();
    let mut all_matched = true;
    for (slot, expected) in checks {
        let Some(actual) = mapper_slot_snapshot_byte(event, slot)? else {
            all_matched = false;
            details.push(format!("slot{slot}=missing expected 0x{expected:02X}"));
            continue;
        };
        if actual == expected {
            details.push(format!("slot{slot}=0x{actual:02X} matched"));
        } else {
            all_matched = false;
            details.push(format!(
                "slot{slot}=0x{actual:02X} expected 0x{expected:02X}"
            ));
        }
    }

    Ok(ProbeHitValidity {
        matched: Some(all_matched),
        detail: Some(details.join("; ")),
    })
}

fn parse_condition_value(raw: &str) -> Result<u8> {
    let token = raw
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_end_matches(',');
    let value = parse_address(token)?;
    anyhow::ensure!(value <= 0xFF, "valid_when 값이 1바이트를 초과함: {raw}");
    Ok(value as u8)
}

fn mapper_slot_snapshot_byte(event: &EmucapEvent, slot: u8) -> Result<Option<u8>> {
    let address = match slot {
        1 => 0xFFFE,
        2 => 0xFFFF,
        _ => return Ok(None),
    };
    for snapshot in &event.snapshot {
        if snapshot.memory_type != "smsMemory" {
            continue;
        }
        let bytes = parse_compact_hex_bytes(&snapshot.hex)?;
        let start = snapshot.address;
        let end = start + bytes.len() as u32;
        if (start..end).contains(&address) {
            let index = (address - start) as usize;
            return Ok(bytes.get(index).copied());
        }
    }
    Ok(None)
}

fn parse_compact_hex_bytes(raw: &str) -> Result<Vec<u8>> {
    let hex = raw.trim();
    anyhow::ensure!(
        hex.len() % 2 == 0,
        "snapshot hex length must be even: {raw}"
    );
    (0..hex.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&hex[index..index + 2], 16)
                .with_context(|| format!("snapshot hex 파싱 실패: {raw}"))
        })
        .collect()
}

fn load_runtime_scene_manifest(path: &Path) -> Result<RuntimeSceneManifest> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("runtime scene manifest 읽기 실패: {}", path.display()))?;
    let manifest: RuntimeSceneManifest = serde_json::from_str(&text)
        .with_context(|| format!("runtime scene manifest JSON 파싱 실패: {}", path.display()))?;
    anyhow::ensure!(
        manifest.format == "madoua-runtime-scenes-v1",
        "지원하지 않는 runtime scene manifest format: {}",
        manifest.format
    );
    anyhow::ensure!(
        !manifest.scenes.is_empty(),
        "runtime scene manifest scenes가 비어 있음"
    );
    Ok(manifest)
}

fn validate_runtime_scene(scene: &RuntimeScene) -> Result<()> {
    anyhow::ensure!(!scene.id.trim().is_empty(), "빈 runtime scene id");
    anyhow::ensure!(
        matches!(scene.status.as_str(), "active" | "pending"),
        "{}: 알 수 없는 runtime scene status {}",
        scene.id,
        scene.status
    );
    anyhow::ensure!(
        !scene.notes.trim().is_empty(),
        "{}: runtime scene notes가 비어 있음",
        scene.id
    );

    if scene.status == "active" {
        anyhow::ensure!(
            scene.rom.is_some(),
            "{}: active scene requires rom",
            scene.id
        );
        anyhow::ensure!(
            scene.screenshot.is_some(),
            "{}: active scene requires screenshot",
            scene.id
        );
        if let Some(frames) = scene.frames {
            anyhow::ensure!(frames > 0, "{}: frames must be > 0", scene.id);
        }
        if let Some(expected) = &scene.expected_crc32 {
            parse_crc32(expected).with_context(|| format!("{} expected_crc32", scene.id))?;
        }
    }
    if let Some(route) = &scene.candidate_route {
        validate_candidate_route(&scene.id, route)?;
    }
    for probe in &scene.probe_requirements {
        validate_probe_requirement(&scene.id, probe)?;
    }
    if scene.status == "pending" {
        anyhow::ensure!(
            scene.candidate_route.is_some() || !scene.probe_requirements.is_empty(),
            "{}: pending scene requires candidate_route or probe_requirements",
            scene.id
        );
    }
    Ok(())
}

fn validate_candidate_route(scene_id: &str, route: &CandidateRoute) -> Result<()> {
    anyhow::ensure!(
        matches!(route.tool.as_str(), "emucap"),
        "{}: unsupported candidate_route tool {}",
        scene_id,
        route.tool
    );
    anyhow::ensure!(
        !route.start.trim().is_empty(),
        "{}: candidate_route start가 비어 있음",
        scene_id
    );
    anyhow::ensure!(
        !route.actions.is_empty(),
        "{}: candidate_route actions가 비어 있음",
        scene_id
    );
    anyhow::ensure!(
        !route.evidence_screenshot.trim().is_empty(),
        "{}: candidate_route evidence_screenshot가 비어 있음",
        scene_id
    );
    anyhow::ensure!(
        !route.evidence_crc32.trim().is_empty(),
        "{}: candidate_route evidence_crc32가 비어 있음",
        scene_id
    );
    parse_crc32(&route.evidence_crc32)
        .with_context(|| format!("{} candidate_route evidence_crc32", scene_id))?;
    anyhow::ensure!(
        !route.expected_text_ref.trim().is_empty(),
        "{}: candidate_route expected_text_ref가 비어 있음",
        scene_id
    );
    for (index, action) in route.actions.iter().enumerate() {
        match action.op.as_str() {
            "step" => {
                let frames = action.frames.with_context(|| {
                    format!("{}: route action {index} step requires frames", scene_id)
                })?;
                anyhow::ensure!(
                    frames > 0,
                    "{}: route action {index} frames must be > 0",
                    scene_id
                );
            }
            "tap" => {
                anyhow::ensure!(
                    !action.buttons.is_empty(),
                    "{}: route action {index} tap requires buttons",
                    scene_id
                );
                let count = action.count.unwrap_or(1);
                anyhow::ensure!(
                    count > 0,
                    "{}: route action {index} count must be > 0",
                    scene_id
                );
            }
            "press" => {
                anyhow::ensure!(
                    !action.buttons.is_empty(),
                    "{}: route action {index} press requires buttons",
                    scene_id
                );
                let frames = action.frames.with_context(|| {
                    format!("{}: route action {index} press requires frames", scene_id)
                })?;
                anyhow::ensure!(
                    frames > 0,
                    "{}: route action {index} frames must be > 0",
                    scene_id
                );
            }
            _ => anyhow::bail!(
                "{}: route action {index} unsupported op {}",
                scene_id,
                action.op
            ),
        }
    }
    anyhow::ensure!(
        route.evidence_screenshot.ends_with(".png"),
        "{}: candidate_route evidence_screenshot should be a PNG path",
        scene_id
    );
    Ok(())
}

fn validate_probe_requirement(scene_id: &str, probe: &ProbeRequirement) -> Result<()> {
    anyhow::ensure!(
        !probe.id.trim().is_empty(),
        "{}: probe requirement id가 비어 있음",
        scene_id
    );
    anyhow::ensure!(
        matches!(
            probe.kind.as_str(),
            "logical_exec_bp" | "logical_read_bp" | "vram_write_bp" | "route_replay"
        ),
        "{}: unsupported probe requirement kind {}",
        scene_id,
        probe.kind
    );
    anyhow::ensure!(
        !probe.notes.trim().is_empty(),
        "{}: probe requirement {} notes가 비어 있음",
        scene_id,
        probe.id
    );

    if matches!(
        probe.kind.as_str(),
        "logical_exec_bp" | "logical_read_bp" | "vram_write_bp"
    ) {
        let memory_type = probe
            .memory_type
            .as_deref()
            .with_context(|| format!("{}: probe {} requires memory_type", scene_id, probe.id))?;
        anyhow::ensure!(
            matches!(memory_type, "smsMemory" | "smsVideoRam" | "smsPrgRom"),
            "{}: probe {} unsupported memory_type {}",
            scene_id,
            probe.id,
            memory_type
        );
        let address = probe
            .address
            .as_deref()
            .with_context(|| format!("{}: probe {} requires address", scene_id, probe.id))?;
        let start = parse_address(address).with_context(|| {
            format!(
                "{}: probe {} address 파싱 실패: {}",
                scene_id, probe.id, address
            )
        })?;
        if let Some(end) = probe.end.as_deref() {
            let end_value = parse_address(end).with_context(|| {
                format!("{}: probe {} end 파싱 실패: {}", scene_id, probe.id, end)
            })?;
            anyhow::ensure!(
                end_value >= start,
                "{}: probe {} end must be >= address",
                scene_id,
                probe.id
            );
        }
    }
    if probe.value.is_some() || probe.value_len.is_some() || probe.value_mask.is_some() {
        anyhow::ensure!(
            !matches!(probe.kind.as_str(), "logical_exec_bp"),
            "{}: probe {} logical_exec_bp does not support value filters",
            scene_id,
            probe.id
        );
        let value_raw = probe.value.as_deref().with_context(|| {
            format!(
                "{}: probe {} value filter fields require value",
                scene_id, probe.id
            )
        })?;
        let value_len = probe.value_len.unwrap_or(1);
        anyhow::ensure!(
            (1..=4).contains(&value_len),
            "{}: probe {} value_len must be 1..=4",
            scene_id,
            probe.id
        );
        let value = parse_address(value_raw).with_context(|| {
            format!(
                "{}: probe {} value 파싱 실패: {}",
                scene_id, probe.id, value_raw
            )
        })?;
        ensure_value_fits_len(scene_id, probe, "value", value, value_len)?;
        if let Some(mask_raw) = probe.value_mask.as_deref() {
            let value_mask = parse_address(mask_raw).with_context(|| {
                format!(
                    "{}: probe {} value_mask 파싱 실패: {}",
                    scene_id, probe.id, mask_raw
                )
            })?;
            ensure_value_fits_len(scene_id, probe, "value_mask", value_mask, value_len)?;
        }
    }
    if probe.pc_min.is_some() || probe.pc_max.is_some() {
        anyhow::ensure!(
            matches!(
                probe.kind.as_str(),
                "logical_exec_bp" | "logical_read_bp" | "vram_write_bp"
            ),
            "{}: probe {} pc filters require a breakpoint kind",
            scene_id,
            probe.id
        );
        let pc_min_raw = probe.pc_min.as_deref().with_context(|| {
            format!(
                "{}: probe {} pc filter fields require pc_min",
                scene_id, probe.id
            )
        })?;
        let pc_max_raw = probe.pc_max.as_deref().with_context(|| {
            format!(
                "{}: probe {} pc filter fields require pc_max",
                scene_id, probe.id
            )
        })?;
        let pc_min = parse_address(pc_min_raw).with_context(|| {
            format!(
                "{}: probe {} pc_min 파싱 실패: {}",
                scene_id, probe.id, pc_min_raw
            )
        })?;
        let pc_max = parse_address(pc_max_raw).with_context(|| {
            format!(
                "{}: probe {} pc_max 파싱 실패: {}",
                scene_id, probe.id, pc_max_raw
            )
        })?;
        anyhow::ensure!(
            pc_max >= pc_min,
            "{}: probe {} pc_max must be >= pc_min",
            scene_id,
            probe.id
        );
    }
    if let Some(install_phase) = probe.install_phase.as_deref() {
        anyhow::ensure!(
            supported_install_phase(install_phase),
            "{}: probe {} unsupported install_phase {}",
            scene_id,
            probe.id,
            install_phase
        );
    }
    if let Some(snapshot) = probe.bank_snapshot.as_deref() {
        validate_bank_snapshot(scene_id, probe, snapshot)?;
        anyhow::ensure!(
            probe
                .valid_when
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
            "{}: probe {} bank_snapshot requires valid_when",
            scene_id,
            probe.id
        );
    }
    Ok(())
}

fn supported_install_phase(install_phase: &str) -> bool {
    matches!(
        install_phase,
        "fresh_start" | "after_scene_reached" | "manual_deferred"
    )
}

fn ensure_value_fits_len(
    scene_id: &str,
    probe: &ProbeRequirement,
    label: &str,
    value: u32,
    value_len: u8,
) -> Result<()> {
    let max_value = if value_len == 4 {
        u32::MAX
    } else {
        (1u32 << (value_len * 8)) - 1
    };
    anyhow::ensure!(
        value <= max_value,
        "{}: probe {} {} must fit {} byte(s)",
        scene_id,
        probe.id,
        label,
        value_len
    );
    Ok(())
}

fn validate_bank_snapshot(scene_id: &str, probe: &ProbeRequirement, snapshot: &str) -> Result<()> {
    let mut parts = snapshot.split(':');
    let memory_type = parts.next().unwrap_or_default();
    let address = parts.next().unwrap_or_default();
    let length = parts.next().unwrap_or_default();
    anyhow::ensure!(
        parts.next().is_none()
            && !memory_type.is_empty()
            && !address.is_empty()
            && !length.is_empty(),
        "{}: probe {} bank_snapshot must be memory_type:address:length",
        scene_id,
        probe.id
    );
    anyhow::ensure!(
        memory_type == "smsMemory",
        "{}: probe {} bank_snapshot memory_type must be smsMemory",
        scene_id,
        probe.id
    );
    parse_address(address).with_context(|| {
        format!(
            "{}: probe {} bank_snapshot address 파싱 실패: {}",
            scene_id, probe.id, address
        )
    })?;
    let length = parse_address(length).with_context(|| {
        format!(
            "{}: probe {} bank_snapshot length 파싱 실패: {}",
            scene_id, probe.id, length
        )
    })?;
    anyhow::ensure!(
        length > 0,
        "{}: probe {} bank_snapshot length must be > 0",
        scene_id,
        probe.id
    );
    Ok(())
}

fn parse_address(raw: &str) -> Result<u32> {
    let trimmed = raw.trim();
    anyhow::ensure!(!trimmed.is_empty(), "빈 숫자");
    let hex = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .or_else(|| trimmed.strip_prefix('$'));
    if let Some(hex) = hex {
        u32::from_str_radix(hex, 16).with_context(|| format!("hex 파싱 실패: {raw}"))
    } else {
        trimmed
            .parse::<u32>()
            .with_context(|| format!("decimal 파싱 실패: {raw}"))
    }
}

#[derive(Debug, Deserialize)]
struct EmucapPollEvents {
    #[serde(default)]
    dropped: Option<u64>,
    #[serde(default)]
    events: Vec<EmucapEvent>,
}

#[derive(Debug, Deserialize)]
struct EmucapEvent {
    #[serde(default)]
    address: Option<u32>,
    #[serde(default)]
    breakpoint_id: Option<u64>,
    #[serde(default)]
    frame: Option<u64>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    value: Option<u32>,
    #[serde(default)]
    pc: Option<u32>,
    #[serde(default)]
    snapshot: Vec<EmucapEventSnapshot>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct EmucapEventSnapshot {
    address: u32,
    hex: String,
    memory_type: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProbeEventReport {
    format: String,
    manifest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    scene: Option<String>,
    event_files: Vec<String>,
    total_events: usize,
    dropped_events: u64,
    matched_events: usize,
    unmatched_events: Vec<UnmatchedProbeEvent>,
    scenes: Vec<SceneEventSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SceneEventSummary {
    scene_id: String,
    probe_count: usize,
    probes_with_hits: usize,
    probes_with_valid_hits: usize,
    hit_count: usize,
    valid_hit_count: usize,
    invalid_hit_count: usize,
    probes: Vec<ProbeEventSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProbeEventSummary {
    probe_id: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end: Option<String>,
    hit_count: usize,
    valid_hit_count: usize,
    invalid_hit_count: usize,
    hits: Vec<ProbeEventHit>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProbeEventHit {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    breakpoint_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pc: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    valid_when_matched: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    valid_when_detail: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    snapshot: Vec<EmucapEventSnapshot>,
}

#[derive(Debug, Serialize, Deserialize)]
struct UnmatchedProbeEvent {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pc: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    breakpoint_id: Option<u64>,
}

fn validate_runtime_scene_artifacts(scene: &RuntimeScene) -> Result<()> {
    if let Some(route) = &scene.candidate_route {
        let path = Path::new(&route.evidence_screenshot);
        anyhow::ensure!(
            path.is_file(),
            "{}: candidate_route evidence_screenshot 파일을 찾을 수 없음: {}",
            scene.id,
            path.display()
        );
        let bytes = std::fs::read(path).with_context(|| {
            format!(
                "{}: candidate_route evidence_screenshot 읽기 실패: {}",
                scene.id,
                path.display()
            )
        })?;
        anyhow::ensure!(
            bytes.starts_with(b"\x89PNG\r\n\x1A\n"),
            "{}: candidate_route evidence_screenshot is not a PNG: {}",
            scene.id,
            path.display()
        );
        let crc32 = crc32fast::hash(&bytes);
        let expected = parse_crc32(&route.evidence_crc32)?;
        anyhow::ensure!(
            crc32 == expected,
            "{}: candidate_route evidence_screenshot CRC32 mismatch: expected {expected:08X}, got {crc32:08X}: {}",
            scene.id,
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_crc32_accepts_common_hex_forms() {
        assert_eq!(parse_crc32("EDC0A75F").unwrap(), 0xEDC0_A75F);
        assert_eq!(parse_crc32("0xEDC0A75F").unwrap(), 0xEDC0_A75F);
        assert_eq!(parse_crc32("$EDC0A75F").unwrap(), 0xEDC0_A75F);
    }

    #[test]
    fn parse_crc32_rejects_wrong_width() {
        let err = parse_crc32("A75F").unwrap_err();
        assert!(err.to_string().contains("8자리"));
    }

    #[test]
    fn runtime_scene_manifest_accepts_active_and_pending_scenes() {
        let manifest: RuntimeSceneManifest = serde_json::from_str(
            r#"{
  "format": "madoua-runtime-scenes-v1",
  "scenes": [
    {
      "id": "boot_intro_complete",
      "status": "active",
      "rom": "out/madoua_build_workflow_complete.gg",
      "screenshot": "out/emu/build_workflow_complete_f980.png",
      "frames": 980,
      "expected_crc32": "BB5EE302",
      "notes": "boot gate"
    },
    {
      "id": "shop_money_fresh_scene",
      "status": "pending",
      "candidate_route": {
        "tool": "emucap",
        "start": "reset",
        "actions": [
          { "op": "step", "frames": 604 },
          { "op": "tap", "buttons": ["two"], "count": 45 }
        ],
        "evidence_screenshot": "out/emu/example.png",
        "evidence_crc32": "089856CA",
        "expected_text_ref": "cutscene/005"
      },
      "notes": "needs replay"
    }
  ]
}"#,
        )
        .unwrap();

        for scene in &manifest.scenes {
            validate_runtime_scene(scene).unwrap();
        }
    }

    #[test]
    fn runtime_scene_manifest_rejects_active_scene_without_rom() {
        let scene = RuntimeScene {
            id: "bad".to_string(),
            status: "active".to_string(),
            rom: None,
            screenshot: Some(PathBuf::from("out/s.png")),
            frames: Some(980),
            replay: None,
            expected_crc32: None,
            notes: "missing rom".to_string(),
            candidate_route: None,
            probe_requirements: Vec::new(),
        };

        let err = validate_runtime_scene(&scene).unwrap_err();
        assert!(err.to_string().contains("requires rom"));
    }

    #[test]
    fn runtime_scene_manifest_release_readiness_rejects_pending() {
        let manifest: RuntimeSceneManifest = serde_json::from_str(
            r#"{
  "format": "madoua-runtime-scenes-v1",
  "scenes": [
    {
      "id": "boot_intro_complete",
      "status": "active",
      "rom": "out/madoua_build_workflow_complete.gg",
      "screenshot": "out/emu/build_workflow_complete_f980.png",
      "frames": 980,
      "notes": "boot gate"
    },
    {
      "id": "shop_money_fresh_scene",
      "status": "pending",
      "probe_requirements": [
        {
          "id": "shop-money-byte-read",
          "kind": "logical_read_bp",
          "memory_type": "smsMemory",
          "address": "0x9BC6",
          "value": "0x0C",
          "bank_snapshot": "smsMemory:0xFFFE:2",
          "valid_when": "slot2=0x09",
          "notes": "shop money byte read in bank 9"
        }
      ],
      "notes": "needs replay"
    }
  ]
}"#,
        )
        .unwrap();

        for scene in &manifest.scenes {
            validate_runtime_scene(scene).unwrap();
        }
        let err = ensure_all_runtime_scenes_active(&manifest).unwrap_err();
        assert!(err.to_string().contains("shop_money_fresh_scene"));
    }

    #[test]
    fn runtime_scene_manifest_release_readiness_accepts_all_active() {
        let manifest: RuntimeSceneManifest = serde_json::from_str(
            r#"{
  "format": "madoua-runtime-scenes-v1",
  "scenes": [
    {
      "id": "boot_intro_complete",
      "status": "active",
      "rom": "out/madoua_build_workflow_complete.gg",
      "screenshot": "out/emu/build_workflow_complete_f980.png",
      "frames": 980,
      "notes": "boot gate"
    }
  ]
}"#,
        )
        .unwrap();

        for scene in &manifest.scenes {
            validate_runtime_scene(scene).unwrap();
        }
        ensure_all_runtime_scenes_active(&manifest).unwrap();
    }

    #[test]
    fn runtime_scene_manifest_rejects_bad_candidate_route() {
        let route = CandidateRoute {
            tool: "emucap".to_string(),
            start: "reset".to_string(),
            actions: vec![CandidateRouteAction {
                op: "tap".to_string(),
                frames: None,
                buttons: Vec::new(),
                count: Some(1),
            }],
            evidence_screenshot: "out/emu/example.png".to_string(),
            evidence_crc32: "089856CA".to_string(),
            expected_text_ref: "cutscene/005".to_string(),
        };

        let err = validate_candidate_route("bad_route", &route).unwrap_err();
        assert!(err.to_string().contains("requires buttons"));
    }

    #[test]
    fn runtime_scene_manifest_rejects_candidate_route_without_evidence() {
        let route = CandidateRoute {
            tool: "emucap".to_string(),
            start: "reset".to_string(),
            actions: vec![CandidateRouteAction {
                op: "step".to_string(),
                frames: Some(60),
                buttons: Vec::new(),
                count: None,
            }],
            evidence_screenshot: String::new(),
            evidence_crc32: "089856CA".to_string(),
            expected_text_ref: "cutscene/005".to_string(),
        };

        let err = validate_candidate_route("missing_evidence", &route).unwrap_err();
        assert!(err.to_string().contains("evidence_screenshot"));
    }

    #[test]
    fn runtime_scene_manifest_rejects_pending_without_route_or_probe() {
        let scene = RuntimeScene {
            id: "pending".to_string(),
            status: "pending".to_string(),
            rom: None,
            screenshot: None,
            frames: None,
            replay: None,
            expected_crc32: None,
            notes: "missing route and probe".to_string(),
            candidate_route: None,
            probe_requirements: Vec::new(),
        };

        let err = validate_runtime_scene(&scene).unwrap_err();
        assert!(
            err.to_string()
                .contains("candidate_route or probe_requirements")
        );
    }

    #[test]
    fn runtime_scene_manifest_accepts_logical_read_probe() {
        let probe = ProbeRequirement {
            id: "region-relocated-read".to_string(),
            kind: "logical_read_bp".to_string(),
            memory_type: Some("smsMemory".to_string()),
            address: Some("0x7E6C".to_string()),
            end: None,
            value: Some("0xA3".to_string()),
            value_len: None,
            value_mask: None,
            pc_min: None,
            pc_max: None,
            install_phase: None,
            bank_snapshot: Some("smsMemory:0xFFFE:2".to_string()),
            valid_when: Some("slot1=0x07".to_string()),
            notes: "region relocated first byte".to_string(),
        };

        validate_probe_requirement("region_text_fresh_scene", &probe).unwrap();
    }

    #[test]
    fn runtime_scene_manifest_accepts_logical_exec_probe() {
        let probe = ProbeRequirement {
            id: "region-table-dispatch-a041-exec".to_string(),
            kind: "logical_exec_bp".to_string(),
            memory_type: Some("smsMemory".to_string()),
            address: Some("0xA041".to_string()),
            end: None,
            value: None,
            value_len: None,
            value_mask: None,
            pc_min: None,
            pc_max: None,
            install_phase: None,
            bank_snapshot: Some("smsMemory:0xFFFE:2".to_string()),
            valid_when: Some("slot2=0x06".to_string()),
            notes: "region table dispatch route-discovery anchor".to_string(),
        };

        validate_probe_requirement("region_text_fresh_scene", &probe).unwrap();
    }

    #[test]
    fn runtime_scene_manifest_rejects_value_filter_on_logical_exec_probe() {
        let probe = ProbeRequirement {
            id: "bad-exec-filter".to_string(),
            kind: "logical_exec_bp".to_string(),
            memory_type: Some("smsMemory".to_string()),
            address: Some("0xA041".to_string()),
            end: None,
            value: Some("0xCD".to_string()),
            value_len: None,
            value_mask: None,
            pc_min: None,
            pc_max: None,
            install_phase: None,
            bank_snapshot: Some("smsMemory:0xFFFE:2".to_string()),
            valid_when: Some("slot2=0x06".to_string()),
            notes: "exec probes should not carry data value filters".to_string(),
        };

        let err = validate_probe_requirement("region_text_fresh_scene", &probe).unwrap_err();
        assert!(err.to_string().contains("does not support value filters"));
    }

    #[test]
    fn runtime_scene_manifest_accepts_range_probe() {
        let probe = ProbeRequirement {
            id: "shop-money-main-font-glyph-read".to_string(),
            kind: "logical_read_bp".to_string(),
            memory_type: Some("smsMemory".to_string()),
            address: Some("0x9B55".to_string()),
            end: Some("0x9B5C".to_string()),
            value: None,
            value_len: None,
            value_mask: None,
            pc_min: None,
            pc_max: None,
            install_phase: None,
            bank_snapshot: Some("smsMemory:0xFFFE:2".to_string()),
            valid_when: Some("slot2=0x06".to_string()),
            notes: "fresh shop proof for the main-font money glyph source range".to_string(),
        };

        validate_probe_requirement("shop_money_fresh_scene", &probe).unwrap();
    }

    #[test]
    fn runtime_scene_manifest_accepts_pc_filtered_probe() {
        let probe = ProbeRequirement {
            id: "route-search-shop-money-range".to_string(),
            kind: "logical_read_bp".to_string(),
            memory_type: Some("smsMemory".to_string()),
            address: Some("0x9BC6".to_string()),
            end: Some("0x9CC0".to_string()),
            value: Some("0x0C".to_string()),
            value_len: None,
            value_mask: None,
            pc_min: Some("0x8000".to_string()),
            pc_max: Some("0x8FFF".to_string()),
            install_phase: None,
            bank_snapshot: Some("smsMemory:0xFFFE:2".to_string()),
            valid_when: Some("slot2=0x09".to_string()),
            notes: "route-search range filtered away from font decode reads".to_string(),
        };

        validate_probe_requirement("shop_money_fresh_scene", &probe).unwrap();
    }

    #[test]
    fn runtime_scene_manifest_accepts_masked_vram_write_probe() {
        let probe = ProbeRequirement {
            id: "shop-money-vram-nametable-low-byte-write".to_string(),
            kind: "vram_write_bp".to_string(),
            memory_type: Some("smsVideoRam".to_string()),
            address: Some("0x3800".to_string()),
            end: Some("0x3EFF".to_string()),
            value: Some("0x0060".to_string()),
            value_len: Some(2),
            value_mask: Some("0x00FF".to_string()),
            pc_min: None,
            pc_max: None,
            install_phase: Some("after_scene_reached".to_string()),
            bank_snapshot: None,
            valid_when: None,
            notes: "deferred classifier for nametable writes of a candidate tile low byte"
                .to_string(),
        };

        validate_probe_requirement("shop_money_fresh_scene", &probe).unwrap();
    }

    #[test]
    fn runtime_scene_manifest_rejects_reversed_range_probe() {
        let probe = ProbeRequirement {
            id: "bad-range".to_string(),
            kind: "logical_read_bp".to_string(),
            memory_type: Some("smsMemory".to_string()),
            address: Some("0x9B5C".to_string()),
            end: Some("0x9B55".to_string()),
            value: None,
            value_len: None,
            value_mask: None,
            pc_min: None,
            pc_max: None,
            install_phase: None,
            bank_snapshot: None,
            valid_when: None,
            notes: "range end before start must fail".to_string(),
        };

        let err = validate_probe_requirement("shop_money_fresh_scene", &probe).unwrap_err();
        assert!(err.to_string().contains("end must be >= address"));
    }

    #[test]
    fn runtime_probe_plan_exports_emucap_breakpoint_args() {
        let manifest: RuntimeSceneManifest = serde_json::from_str(
            r#"{
  "format": "madoua-runtime-scenes-v1",
  "scenes": [
    {
      "id": "shop_money_fresh_scene",
      "status": "pending",
      "probe_requirements": [
        {
          "id": "shop-00-money-byte-read",
          "kind": "logical_read_bp",
          "memory_type": "smsMemory",
          "address": "0x9BC6",
          "value": "0x0C",
          "bank_snapshot": "smsMemory:0xFFFE:2",
          "valid_when": "slot2=0x09",
          "notes": "shop/00 money byte read is valid only when slot 2 maps bank 9."
        }
      ],
      "notes": "needs fresh shop proof"
    }
  ]
}"#,
        )
        .unwrap();

        for scene in &manifest.scenes {
            validate_runtime_scene(scene).unwrap();
        }
        let plan = build_runtime_probe_plan(
            &manifest,
            Path::new("assets/qa/runtime_scenes.json"),
            Some("shop_money_fresh_scene"),
            None,
        )
        .unwrap();
        let value = serde_json::to_value(&plan).unwrap();

        assert_eq!(value["format"], "madoua-emucap-probe-plan-v1");
        assert_eq!(value["fresh_start_required"], true);
        assert_eq!(
            value["scenes"][0]["probes"][0]["emucap_set_breakpoint"]["tool"],
            "set_breakpoint"
        );
        assert_eq!(
            value["scenes"][0]["probes"][0]["emucap_set_breakpoint"]["args"]["kind"],
            "read"
        );
        assert_eq!(
            value["scenes"][0]["probes"][0]["emucap_set_breakpoint"]["args"]["memory_type"],
            "smsMemory"
        );
        assert_eq!(
            value["scenes"][0]["probes"][0]["emucap_set_breakpoint"]["args"]["start"],
            "0x9BC6"
        );
        assert_eq!(
            value["scenes"][0]["probes"][0]["emucap_set_breakpoint"]["args"]["value"],
            "0x0C"
        );
        assert_eq!(
            value["scenes"][0]["probes"][0]["emucap_set_breakpoint"]["args"]["snapshot"][0],
            "smsMemory:0xFFFE:2"
        );
    }

    #[test]
    fn runtime_probe_plan_exports_exec_breakpoint_args() {
        let manifest: RuntimeSceneManifest = serde_json::from_str(
            r#"{
  "format": "madoua-runtime-scenes-v1",
  "scenes": [
    {
      "id": "region_text_fresh_scene",
      "status": "pending",
      "probe_requirements": [
        {
          "id": "region-table-dispatch-a041-exec",
          "kind": "logical_exec_bp",
          "memory_type": "smsMemory",
          "address": "0xA041",
          "bank_snapshot": "smsMemory:0xFFFE:2",
          "valid_when": "slot2=0x06",
          "notes": "route-discovery exec anchor"
        }
      ],
      "notes": "needs region route proof"
    }
  ]
}"#,
        )
        .unwrap();

        for scene in &manifest.scenes {
            validate_runtime_scene(scene).unwrap();
        }
        let plan = build_runtime_probe_plan(
            &manifest,
            Path::new("assets/qa/runtime_scenes.json"),
            Some("region_text_fresh_scene"),
            None,
        )
        .unwrap();
        let value = serde_json::to_value(&plan).unwrap();

        assert_eq!(
            value["scenes"][0]["probes"][0]["emucap_set_breakpoint"]["args"]["kind"],
            "exec"
        );
        assert_eq!(
            value["scenes"][0]["probes"][0]["emucap_set_breakpoint"]["args"]["start"],
            "0xA041"
        );
        assert!(
            value["scenes"][0]["probes"][0]["emucap_set_breakpoint"]["args"]
                .get("value")
                .is_none()
        );
        assert_eq!(
            value["scenes"][0]["probes"][0]["emucap_set_breakpoint"]["args"]["snapshot"][0],
            "smsMemory:0xFFFE:2"
        );
    }

    #[test]
    fn runtime_probe_plan_exports_range_breakpoint_args() {
        let manifest: RuntimeSceneManifest = serde_json::from_str(
            r#"{
  "format": "madoua-runtime-scenes-v1",
  "scenes": [
    {
      "id": "shop_money_fresh_scene",
      "status": "pending",
      "probe_requirements": [
        {
          "id": "shop-money-main-font-glyph-read",
          "kind": "logical_read_bp",
          "memory_type": "smsMemory",
          "address": "0x9B55",
          "end": "0x9B5C",
          "bank_snapshot": "smsMemory:0xFFFE:2",
          "valid_when": "slot2=0x06",
          "notes": "fresh shop proof for the main-font money glyph source range"
        }
      ],
      "notes": "needs fresh shop proof"
    }
  ]
}"#,
        )
        .unwrap();

        for scene in &manifest.scenes {
            validate_runtime_scene(scene).unwrap();
        }
        let plan = build_runtime_probe_plan(
            &manifest,
            Path::new("assets/qa/runtime_scenes.json"),
            Some("shop_money_fresh_scene"),
            None,
        )
        .unwrap();
        let value = serde_json::to_value(&plan).unwrap();

        assert_eq!(
            value["scenes"][0]["probes"][0]["emucap_set_breakpoint"]["args"]["start"],
            "0x9B55"
        );
        assert_eq!(
            value["scenes"][0]["probes"][0]["emucap_set_breakpoint"]["args"]["end"],
            "0x9B5C"
        );
    }

    #[test]
    fn runtime_probe_plan_exports_pc_filter_args() {
        let manifest: RuntimeSceneManifest = serde_json::from_str(
            r#"{
  "format": "madoua-runtime-scenes-v1",
  "scenes": [
    {
      "id": "shop_money_fresh_scene",
      "status": "pending",
      "probe_requirements": [
        {
          "id": "route-search-shop-money-range",
          "kind": "logical_read_bp",
          "memory_type": "smsMemory",
          "address": "0x9BC6",
          "end": "0x9CC0",
          "value": "0x0C",
          "pc_min": "0x8000",
          "pc_max": "0x8FFF",
          "bank_snapshot": "smsMemory:0xFFFE:2",
          "valid_when": "slot2=0x09",
          "notes": "route-search range filtered away from font decode reads"
        }
      ],
      "notes": "needs fresh shop proof"
    }
  ]
}"#,
        )
        .unwrap();

        for scene in &manifest.scenes {
            validate_runtime_scene(scene).unwrap();
        }
        let plan = build_runtime_probe_plan(
            &manifest,
            Path::new("assets/qa/runtime_scenes.json"),
            Some("shop_money_fresh_scene"),
            None,
        )
        .unwrap();
        let value = serde_json::to_value(&plan).unwrap();
        let args = &value["scenes"][0]["probes"][0]["emucap_set_breakpoint"]["args"];

        assert_eq!(args["pc_min"], "0x8000");
        assert_eq!(args["pc_max"], "0x8FFF");
    }

    #[test]
    fn runtime_probe_plan_exports_masked_vram_write_breakpoint_args() {
        let manifest: RuntimeSceneManifest = serde_json::from_str(
            r#"{
  "format": "madoua-runtime-scenes-v1",
  "scenes": [
    {
      "id": "shop_money_fresh_scene",
      "status": "pending",
      "probe_requirements": [
        {
          "id": "shop-money-vram-nametable-low-byte-write",
          "kind": "vram_write_bp",
          "memory_type": "smsVideoRam",
          "address": "0x3800",
          "end": "0x3EFF",
          "value": "0x0060",
          "value_len": 2,
          "value_mask": "0x00FF",
          "install_phase": "after_scene_reached",
          "notes": "deferred classifier for nametable writes"
        }
      ],
      "notes": "needs fresh shop proof"
    }
  ]
}"#,
        )
        .unwrap();

        for scene in &manifest.scenes {
            validate_runtime_scene(scene).unwrap();
        }
        let plan = build_runtime_probe_plan(
            &manifest,
            Path::new("assets/qa/runtime_scenes.json"),
            Some("shop_money_fresh_scene"),
            None,
        )
        .unwrap();
        let value = serde_json::to_value(&plan).unwrap();

        let args = &value["scenes"][0]["probes"][0]["emucap_set_breakpoint"]["args"];
        assert_eq!(args["kind"], "write");
        assert_eq!(args["memory_type"], "smsVideoRam");
        assert_eq!(args["start"], "0x3800");
        assert_eq!(args["end"], "0x3EFF");
        assert_eq!(args["value"], "0x0060");
        assert_eq!(args["value_len"], 2);
        assert_eq!(args["value_mask"], "0x00FF");
        assert_eq!(
            value["scenes"][0]["probes"][0]["probe"]["install_phase"],
            "after_scene_reached"
        );
    }

    #[test]
    fn runtime_probe_plan_filters_by_install_phase() {
        let manifest: RuntimeSceneManifest = serde_json::from_str(
            r#"{
  "format": "madoua-runtime-scenes-v1",
  "scenes": [
    {
      "id": "shop_money_fresh_scene",
      "status": "pending",
      "probe_requirements": [
        {
          "id": "shop-money-main-font-glyph-read",
          "kind": "logical_read_bp",
          "memory_type": "smsMemory",
          "address": "0x9B55",
          "end": "0x9B5C",
          "bank_snapshot": "smsMemory:0xFFFE:2",
          "valid_when": "slot2=0x06",
          "notes": "from-reset source read"
        },
        {
          "id": "shop-money-vram-nametable-low-byte-write",
          "kind": "vram_write_bp",
          "memory_type": "smsVideoRam",
          "address": "0x3800",
          "end": "0x3EFF",
          "value": "0x0060",
          "value_len": 2,
          "value_mask": "0x00FF",
          "install_phase": "after_scene_reached",
          "notes": "deferred classifier for nametable writes"
        }
      ],
      "notes": "needs fresh shop proof"
    }
  ]
}"#,
        )
        .unwrap();

        for scene in &manifest.scenes {
            validate_runtime_scene(scene).unwrap();
        }
        let fresh_plan = build_runtime_probe_plan(
            &manifest,
            Path::new("assets/qa/runtime_scenes.json"),
            Some("shop_money_fresh_scene"),
            Some("fresh_start"),
        )
        .unwrap();
        assert_eq!(fresh_plan.install_phase.as_deref(), Some("fresh_start"));
        assert_eq!(fresh_plan.scenes[0].probes.len(), 1);
        assert_eq!(
            fresh_plan.scenes[0].probes[0].probe.id,
            "shop-money-main-font-glyph-read"
        );

        let after_scene_plan = build_runtime_probe_plan(
            &manifest,
            Path::new("assets/qa/runtime_scenes.json"),
            Some("shop_money_fresh_scene"),
            Some("after_scene_reached"),
        )
        .unwrap();
        assert_eq!(
            after_scene_plan.install_phase.as_deref(),
            Some("after_scene_reached")
        );
        assert_eq!(after_scene_plan.scenes[0].probes.len(), 1);
        assert_eq!(
            after_scene_plan.scenes[0].probes[0].probe.id,
            "shop-money-vram-nametable-low-byte-write"
        );

        let err = build_runtime_probe_plan(
            &manifest,
            Path::new("assets/qa/runtime_scenes.json"),
            Some("shop_money_fresh_scene"),
            Some("wrong_phase"),
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("unsupported install_phase filter wrong_phase")
        );
    }

    #[test]
    fn runtime_probe_event_summary_matches_manifest_probe() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("runtime_scenes.json");
        let event_path = dir.path().join("events.json");
        std::fs::write(
            &manifest_path,
            r#"{
  "format": "madoua-runtime-scenes-v1",
  "scenes": [
    {
      "id": "cutscene_field_text_emucap_route",
      "status": "pending",
      "probe_requirements": [
        {
          "id": "cutscene-loop-a4fb-exec",
          "kind": "logical_exec_bp",
          "memory_type": "smsMemory",
          "address": "0xA4FB",
          "bank_snapshot": "smsMemory:0xFFFE:2",
          "valid_when": "slot2=0x06",
          "notes": "cutscene classifier"
        }
      ],
      "notes": "pending cutscene route"
    }
  ]
}"#,
        )
        .unwrap();
        std::fs::write(
            &event_path,
            r#"{
  "dropped": 0,
  "events": [
    {
      "address": 42235,
      "breakpoint_id": 5,
      "frame": 668,
      "kind": "exec",
      "snapshot": [
        { "address": 65534, "hex": "0006", "memory_type": "smsMemory" }
      ],
      "value": 205
    }
  ]
}"#,
        )
        .unwrap();

        let manifest = load_runtime_scene_manifest(&manifest_path).unwrap();
        let plan = build_runtime_probe_plan(
            &manifest,
            Path::new("runtime_scenes.json"),
            Some("cutscene_field_text_emucap_route"),
            None,
        )
        .unwrap();
        assert_eq!(plan.scenes[0].probes.len(), 1);

        let event_text = std::fs::read_to_string(&event_path).unwrap();
        let event_set: EmucapPollEvents = serde_json::from_str(&event_text).unwrap();
        assert!(probe_matches_event(plan.scenes[0].probes[0].probe, &event_set.events[0]).unwrap());

        let report = build_probe_event_report(
            &manifest_path,
            std::slice::from_ref(&event_path),
            Some("cutscene_field_text_emucap_route"),
        )
        .unwrap();
        assert_eq!(report.total_events, 1);
        assert_eq!(report.matched_events, 1);
        assert_eq!(report.scenes[0].probe_count, 1);
        assert_eq!(report.scenes[0].probes_with_hits, 1);
        assert_eq!(report.scenes[0].probes_with_valid_hits, 1);
        assert_eq!(report.scenes[0].hit_count, 1);
        assert_eq!(report.scenes[0].valid_hit_count, 1);
        assert_eq!(report.scenes[0].invalid_hit_count, 0);
        assert_eq!(report.scenes[0].probes[0].hit_count, 1);
        assert_eq!(report.scenes[0].probes[0].valid_hit_count, 1);
        assert_eq!(report.scenes[0].probes[0].invalid_hit_count, 0);
        assert_eq!(
            report.scenes[0].probes[0].hits[0].valid_when_matched,
            Some(true)
        );
    }

    #[test]
    fn runtime_probe_event_summary_marks_wrong_mapper_bank_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("runtime_scenes.json");
        let event_path = dir.path().join("events.json");
        std::fs::write(
            &manifest_path,
            r#"{
  "format": "madoua-runtime-scenes-v1",
  "scenes": [
    {
      "id": "shop_money_fresh_scene",
      "status": "pending",
      "probe_requirements": [
        {
          "id": "shop-money-main-font-glyph-read",
          "kind": "logical_read_bp",
          "memory_type": "smsMemory",
          "address": "0x9B55",
          "end": "0x9B5C",
          "bank_snapshot": "smsMemory:0xFFFE:2",
          "valid_when": "slot2=0x06",
          "notes": "main-font money candidate"
        }
      ],
      "notes": "pending shop route"
    }
  ]
}"#,
        )
        .unwrap();
        std::fs::write(
            &event_path,
            r#"{
  "dropped": 0,
  "events": [
    {
      "address": 39765,
      "breakpoint_id": 18,
      "frame": 11351,
      "kind": "read",
      "snapshot": [
        { "address": 65534, "hex": "000e", "memory_type": "smsMemory" }
      ],
      "value": 206
    }
  ]
}"#,
        )
        .unwrap();

        let report = build_probe_event_report(
            &manifest_path,
            std::slice::from_ref(&event_path),
            Some("shop_money_fresh_scene"),
        )
        .unwrap();
        let scene = &report.scenes[0];
        let probe = &scene.probes[0];
        assert_eq!(scene.hit_count, 1);
        assert_eq!(scene.valid_hit_count, 0);
        assert_eq!(scene.invalid_hit_count, 1);
        assert_eq!(scene.probes_with_hits, 1);
        assert_eq!(scene.probes_with_valid_hits, 0);
        assert_eq!(probe.hit_count, 1);
        assert_eq!(probe.valid_hit_count, 0);
        assert_eq!(probe.invalid_hit_count, 1);
        assert_eq!(probe.hits[0].valid_when_matched, Some(false));
        assert!(
            probe.hits[0]
                .valid_when_detail
                .as_deref()
                .unwrap()
                .contains("slot2=0x0E expected 0x06")
        );
    }

    #[test]
    fn runtime_probe_summary_gate_accepts_required_valid_probe() {
        let report = ProbeEventReport {
            format: "madoua-runtime-probe-events-summary-v1".to_string(),
            manifest: "assets/qa/runtime_scenes.json".to_string(),
            scene: Some("cutscene_field_text_emucap_route".to_string()),
            event_files: vec!["out/emu/cutscene_events.json".to_string()],
            total_events: 1,
            dropped_events: 0,
            matched_events: 1,
            unmatched_events: Vec::new(),
            scenes: vec![SceneEventSummary {
                scene_id: "cutscene_field_text_emucap_route".to_string(),
                probe_count: 1,
                probes_with_hits: 1,
                probes_with_valid_hits: 1,
                hit_count: 1,
                valid_hit_count: 1,
                invalid_hit_count: 0,
                probes: vec![ProbeEventSummary {
                    probe_id: "cutscene-loop-a4fb-exec".to_string(),
                    kind: "logical_exec_bp".to_string(),
                    address: Some("0xA4FB".to_string()),
                    end: None,
                    hit_count: 1,
                    valid_hit_count: 1,
                    invalid_hit_count: 0,
                    hits: Vec::new(),
                }],
            }],
        };
        let required = vec!["cutscene-loop-a4fb-exec".to_string()];

        let result = check_probe_event_report(
            &report,
            Some("cutscene_field_text_emucap_route"),
            1,
            &required,
            true,
            false,
            false,
        )
        .unwrap();

        assert_eq!(result.scenes_checked, 1);
        assert_eq!(result.valid_hit_count, 1);
        assert_eq!(result.invalid_hit_count, 0);
        assert_eq!(result.unmatched_event_count, 0);
    }

    #[test]
    fn runtime_probe_summary_gate_rejects_wrong_bank_raw_hits() {
        let report = ProbeEventReport {
            format: "madoua-runtime-probe-events-summary-v1".to_string(),
            manifest: "assets/qa/runtime_scenes.json".to_string(),
            scene: Some("shop_money_fresh_scene".to_string()),
            event_files: vec!["out/emu/shop_events.json".to_string()],
            total_events: 2,
            dropped_events: 0,
            matched_events: 2,
            unmatched_events: Vec::new(),
            scenes: vec![SceneEventSummary {
                scene_id: "shop_money_fresh_scene".to_string(),
                probe_count: 1,
                probes_with_hits: 1,
                probes_with_valid_hits: 0,
                hit_count: 2,
                valid_hit_count: 0,
                invalid_hit_count: 2,
                probes: vec![ProbeEventSummary {
                    probe_id: "shop-money-main-font-glyph-read".to_string(),
                    kind: "logical_read_bp".to_string(),
                    address: Some("0x9B55".to_string()),
                    end: Some("0x9B5C".to_string()),
                    hit_count: 2,
                    valid_hit_count: 0,
                    invalid_hit_count: 2,
                    hits: Vec::new(),
                }],
            }],
        };
        let required = vec!["shop-money-main-font-glyph-read".to_string()];

        let err = check_probe_event_report(
            &report,
            Some("shop_money_fresh_scene"),
            1,
            &required,
            false,
            false,
            false,
        )
        .unwrap_err();

        assert!(err.to_string().contains("valid_hit_count 0"));
    }

    #[test]
    fn runtime_probe_summary_gate_can_reject_invalid_hits() {
        let report = ProbeEventReport {
            format: "madoua-runtime-probe-events-summary-v1".to_string(),
            manifest: "assets/qa/runtime_scenes.json".to_string(),
            scene: Some("mixed_scene".to_string()),
            event_files: vec!["out/emu/mixed_events.json".to_string()],
            total_events: 2,
            dropped_events: 0,
            matched_events: 2,
            unmatched_events: Vec::new(),
            scenes: vec![SceneEventSummary {
                scene_id: "mixed_scene".to_string(),
                probe_count: 1,
                probes_with_hits: 1,
                probes_with_valid_hits: 1,
                hit_count: 2,
                valid_hit_count: 1,
                invalid_hit_count: 1,
                probes: vec![ProbeEventSummary {
                    probe_id: "mixed-probe".to_string(),
                    kind: "logical_read_bp".to_string(),
                    address: Some("0x9B55".to_string()),
                    end: Some("0x9B5C".to_string()),
                    hit_count: 2,
                    valid_hit_count: 1,
                    invalid_hit_count: 1,
                    hits: Vec::new(),
                }],
            }],
        };

        let err =
            check_probe_event_report(&report, Some("mixed_scene"), 1, &[], true, false, false)
                .unwrap_err();

        assert!(err.to_string().contains("invalid_hit_count 1"));
    }

    #[test]
    fn runtime_probe_summary_gate_can_reject_unmatched_events() {
        let report = ProbeEventReport {
            format: "madoua-runtime-probe-events-summary-v1".to_string(),
            manifest: "assets/qa/runtime_scenes.json".to_string(),
            scene: Some("mixed_scene".to_string()),
            event_files: vec!["out/emu/mixed_events.json".to_string()],
            total_events: 2,
            dropped_events: 0,
            matched_events: 1,
            unmatched_events: vec![UnmatchedProbeEvent {
                path: "out/emu/mixed_events.json".to_string(),
                kind: Some("read".to_string()),
                address: Some(0x9BD3),
                pc: Some(0x9A54),
                frame: Some(7877),
                breakpoint_id: Some(6),
            }],
            scenes: vec![SceneEventSummary {
                scene_id: "mixed_scene".to_string(),
                probe_count: 1,
                probes_with_hits: 1,
                probes_with_valid_hits: 1,
                hit_count: 1,
                valid_hit_count: 1,
                invalid_hit_count: 0,
                probes: vec![ProbeEventSummary {
                    probe_id: "mixed-probe".to_string(),
                    kind: "logical_read_bp".to_string(),
                    address: Some("0x9B55".to_string()),
                    end: Some("0x9B5C".to_string()),
                    hit_count: 1,
                    valid_hit_count: 1,
                    invalid_hit_count: 0,
                    hits: Vec::new(),
                }],
            }],
        };

        let result =
            check_probe_event_report(&report, Some("mixed_scene"), 1, &[], false, false, false)
                .unwrap();
        assert_eq!(result.unmatched_event_count, 1);

        let err =
            check_probe_event_report(&report, Some("mixed_scene"), 1, &[], false, true, false)
                .unwrap_err();

        assert!(err.to_string().contains("unmatched_event_count 1"));
    }

    #[test]
    fn runtime_probe_event_summary_matches_range_probe() {
        let probe = ProbeRequirement {
            id: "shop-money-main-font-glyph-read".to_string(),
            kind: "logical_read_bp".to_string(),
            memory_type: Some("smsMemory".to_string()),
            address: Some("0x9B55".to_string()),
            end: Some("0x9B5C".to_string()),
            value: None,
            value_len: None,
            value_mask: None,
            pc_min: None,
            pc_max: None,
            install_phase: None,
            bank_snapshot: Some("smsMemory:0xFFFE:2".to_string()),
            valid_when: Some("slot2=0x06".to_string()),
            notes: "main-font source range".to_string(),
        };
        let event: EmucapEvent = serde_json::from_str(
            r#"{
  "address": 39768,
  "breakpoint_id": 38,
  "frame": 1234,
  "kind": "read",
  "value": 124
}"#,
        )
        .unwrap();

        assert!(probe_matches_event(&probe, &event).unwrap());
    }

    #[test]
    fn runtime_probe_event_summary_applies_pc_filter() {
        let probe = ProbeRequirement {
            id: "route-search-shop-money-range".to_string(),
            kind: "logical_read_bp".to_string(),
            memory_type: Some("smsMemory".to_string()),
            address: Some("0x9BC6".to_string()),
            end: Some("0x9CC0".to_string()),
            value: Some("0x0C".to_string()),
            value_len: None,
            value_mask: None,
            pc_min: Some("0x8000".to_string()),
            pc_max: Some("0x8FFF".to_string()),
            install_phase: None,
            bank_snapshot: Some("smsMemory:0xFFFE:2".to_string()),
            valid_when: Some("slot2=0x09".to_string()),
            notes: "route-search range filtered away from font decode reads".to_string(),
        };
        let font_decode_hit: EmucapEvent = serde_json::from_str(
            r#"{
  "address": 39891,
  "breakpoint_id": 6,
  "frame": 7877,
  "kind": "read",
  "pc": 39508,
  "snapshot": [
    { "address": 65534, "hex": "0006", "memory_type": "smsMemory" }
  ],
  "value": 12
}"#,
        )
        .unwrap();
        let route_hit: EmucapEvent = serde_json::from_str(
            r#"{
  "address": 39891,
  "breakpoint_id": 7,
  "frame": 9000,
  "kind": "read",
  "pc": 33024,
  "snapshot": [
    { "address": 65534, "hex": "0009", "memory_type": "smsMemory" }
  ],
  "value": 12
}"#,
        )
        .unwrap();

        assert!(!probe_matches_event(&probe, &font_decode_hit).unwrap());
        assert!(probe_matches_event(&probe, &route_hit).unwrap());
    }

    #[test]
    fn runtime_probe_event_summary_matches_masked_write_probe() {
        let probe = ProbeRequirement {
            id: "shop-money-vram-nametable-low-byte-write".to_string(),
            kind: "vram_write_bp".to_string(),
            memory_type: Some("smsVideoRam".to_string()),
            address: Some("0x3800".to_string()),
            end: Some("0x3EFF".to_string()),
            value: Some("0x0060".to_string()),
            value_len: Some(2),
            value_mask: Some("0x00FF".to_string()),
            pc_min: None,
            pc_max: None,
            install_phase: Some("after_scene_reached".to_string()),
            bank_snapshot: None,
            valid_when: None,
            notes: "deferred classifier for nametable writes".to_string(),
        };
        let event: EmucapEvent = serde_json::from_str(
            r#"{
  "address": 14336,
  "breakpoint_id": 41,
  "frame": 4321,
  "kind": "write",
  "value": 54880
}"#,
        )
        .unwrap();

        assert!(probe_matches_event(&probe, &event).unwrap());
    }

    #[test]
    fn runtime_scene_manifest_accepts_candidate_route_press_action() {
        let route = CandidateRoute {
            tool: "emucap".to_string(),
            start: "title menu".to_string(),
            actions: vec![CandidateRouteAction {
                op: "press".to_string(),
                frames: Some(120),
                buttons: vec!["two".to_string()],
                count: None,
            }],
            evidence_screenshot: "out/emu/example.png".to_string(),
            evidence_crc32: "089856CA".to_string(),
            expected_text_ref: "cutscene/006".to_string(),
        };

        validate_candidate_route("press_route", &route).unwrap();
    }

    #[test]
    fn runtime_scene_manifest_rejects_missing_candidate_evidence_file() {
        let scene = RuntimeScene {
            id: "candidate".to_string(),
            status: "pending".to_string(),
            rom: None,
            screenshot: None,
            frames: None,
            replay: None,
            expected_crc32: None,
            notes: "candidate route".to_string(),
            candidate_route: Some(CandidateRoute {
                tool: "emucap".to_string(),
                start: "reset".to_string(),
                actions: vec![CandidateRouteAction {
                    op: "step".to_string(),
                    frames: Some(60),
                    buttons: Vec::new(),
                    count: None,
                }],
                evidence_screenshot: "out/emu/does-not-exist.png".to_string(),
                evidence_crc32: "089856CA".to_string(),
                expected_text_ref: "cutscene/005".to_string(),
            }),
            probe_requirements: Vec::new(),
        };

        validate_runtime_scene(&scene).unwrap();
        let err = validate_runtime_scene_artifacts(&scene).unwrap_err();
        assert!(err.to_string().contains("evidence_screenshot"));
    }

    #[test]
    fn runtime_scene_manifest_accepts_candidate_evidence_png() {
        let dir = tempfile::tempdir().unwrap();
        let screenshot = dir.path().join("candidate.png");
        std::fs::write(&screenshot, b"\x89PNG\r\n\x1A\nminimal").unwrap();
        let expected_crc32 = format!("{:08X}", crc32fast::hash(b"\x89PNG\r\n\x1A\nminimal"));
        let scene = RuntimeScene {
            id: "candidate".to_string(),
            status: "pending".to_string(),
            rom: None,
            screenshot: None,
            frames: None,
            replay: None,
            expected_crc32: None,
            notes: "candidate route".to_string(),
            candidate_route: Some(CandidateRoute {
                tool: "emucap".to_string(),
                start: "reset".to_string(),
                actions: vec![CandidateRouteAction {
                    op: "step".to_string(),
                    frames: Some(60),
                    buttons: Vec::new(),
                    count: None,
                }],
                evidence_screenshot: screenshot.display().to_string(),
                evidence_crc32: expected_crc32,
                expected_text_ref: "cutscene/005".to_string(),
            }),
            probe_requirements: Vec::new(),
        };

        validate_runtime_scene(&scene).unwrap();
        validate_runtime_scene_artifacts(&scene).unwrap();
    }
}

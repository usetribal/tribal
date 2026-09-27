use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CliPhase {
    pub name: &'static str,
    pub millis: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrepReport {
    pub total_millis: u128,
    pub phases: Vec<CliPhase>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RatatuiPhase {
    pub name: &'static str,
    pub millis: u128,
    pub micros: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RatatuiReport {
    pub total_millis: u128,
    pub phases: Vec<RatatuiPhase>,
    pub tty: bool,
}

pub fn phase_from_elapsed(
    name: &'static str,
    elapsed: std::time::Duration,
    detail: Option<String>,
) -> RatatuiPhase {
    let micros = elapsed.as_micros();
    RatatuiPhase {
        name,
        millis: micros / 1000,
        micros,
        detail,
    }
}

pub fn emit_prep(report: &PrepReport) {
    if !crate::env::profiling_enabled() {
        return;
    }
    if let Ok(json) = serde_json::to_string(report) {
        eprintln!("{json}");
    }
}

pub fn emit_ratatui(report: &RatatuiReport) {
    if !crate::env::profiling_enabled() {
        return;
    }
    let mut value = serde_json::to_value(report).unwrap_or_default();
    if let Some(obj) = value.as_object_mut() {
        obj.insert("kind".into(), "ratatui".into());
    }
    if let Ok(json) = serde_json::to_string(&value) {
        eprintln!("{json}");
    }
}

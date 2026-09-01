//! Battery info via `pmset -g batt` + `ioreg -rn AppleSmartBattery`.
//!
//! Shelling out keeps us free of heavy IOKit FFI for a value that only
//! needs to refresh every few seconds.
//!
//! Return `None` when no battery is present (desktop Macs) or when parsing
//! fails completely — the frontend hides the battery block in that case.

use crate::collector::snapshot::BatteryInfo;
use std::process::Command;

/// Read the current battery state. Best-effort: any missing field is left as
/// `None` rather than failing the whole struct.
pub fn collect() -> Option<BatteryInfo> {
    let pmset = run("pmset", &["-g", "batt"])?;
    // If the pmset output doesn't mention InternalBattery, this Mac has no battery.
    if !pmset.contains("InternalBattery") {
        return None;
    }

    let percent = parse_pmset_percent(&pmset)?;
    let (is_charging, is_plugged_in) = parse_pmset_state(&pmset);
    let time_remaining_sec = parse_pmset_time(&pmset);

    // ioreg carries the richer battery registry entry; missing fields = None.
    let ioreg = run("ioreg", &["-rn", "AppleSmartBattery"]).unwrap_or_default();
    let cycle_count = parse_ioreg_int(&ioreg, "CycleCount").map(|v| v as u32);
    let design = parse_ioreg_int(&ioreg, "DesignCapacity");
    let max_cap = parse_ioreg_int(&ioreg, "AppleRawMaxCapacity")
        .or_else(|| parse_ioreg_int(&ioreg, "MaxCapacity"));
    let health_percent = match (design, max_cap) {
        (Some(d), Some(m)) if d > 0 => Some((m as f32 / d as f32 * 100.0).clamp(0.0, 120.0)),
        _ => None,
    };

    // Instantaneous power: amperage (mA, signed) × voltage (mV).
    let voltage_mv = parse_ioreg_int(&ioreg, "Voltage");
    let amperage_ma = parse_ioreg_signed(&ioreg, "InstantAmperage")
        .or_else(|| parse_ioreg_signed(&ioreg, "Amperage"));
    let charge_rate_w = match (voltage_mv, amperage_ma) {
        (Some(v), Some(a)) => Some((v as f32 * a as f32) / 1_000_000.0),
        _ => None,
    };

    Some(BatteryInfo {
        percent,
        is_charging,
        is_plugged_in,
        time_remaining_sec,
        cycle_count,
        health_percent,
        charge_rate_w,
    })
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `100%;` → 100.0
fn parse_pmset_percent(text: &str) -> Option<f32> {
    for line in text.lines() {
        if let Some(pct_end) = line.find('%') {
            // Walk backwards from the '%' gathering digits.
            let prefix = &line[..pct_end];
            let digits: String = prefix
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            if let Ok(v) = digits.parse::<f32>() {
                return Some(v.clamp(0.0, 100.0));
            }
        }
    }
    None
}

/// Returns (is_charging, is_plugged_in).
fn parse_pmset_state(text: &str) -> (bool, bool) {
    let lower = text.to_ascii_lowercase();
    let is_plugged = lower.contains("ac power") || lower.contains("charging");
    let is_charging = lower.contains("charging") && !lower.contains("not charging");
    (is_charging, is_plugged)
}

/// Parse "X:YY remaining" → seconds. Returns None if "(no estimate)" present.
fn parse_pmset_time(text: &str) -> Option<u32> {
    if text.contains("(no estimate)") {
        return None;
    }
    for line in text.lines() {
        if let Some(idx) = line.find("remaining") {
            let prefix = &line[..idx];
            // Scan back for a "H:MM" token.
            if let Some(tok) = prefix.split_whitespace().rev().find(|t| t.contains(':')) {
                let mut parts = tok.split(':');
                let h: u32 = parts.next()?.parse().ok()?;
                let m: u32 = parts.next()?.parse().ok()?;
                return Some(h * 3600 + m * 60);
            }
        }
    }
    None
}

/// Find `"Key" = 123` in ioreg output. Returns unsigned integer value.
fn parse_ioreg_int(text: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{key}\"");
    for line in text.lines() {
        if let Some(pos) = line.find(&needle) {
            let tail = &line[pos + needle.len()..];
            if let Some(eq) = tail.find('=') {
                let val = tail[eq + 1..].trim();
                let digits: String = val
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '-')
                    .collect();
                if let Ok(v) = digits.parse::<i64>() {
                    if v >= 0 {
                        return Some(v as u64);
                    }
                }
            }
        }
    }
    None
}

/// Signed variant for Amperage (negative = discharging).
fn parse_ioreg_signed(text: &str, key: &str) -> Option<i64> {
    let needle = format!("\"{key}\"");
    for line in text.lines() {
        if let Some(pos) = line.find(&needle) {
            let tail = &line[pos + needle.len()..];
            if let Some(eq) = tail.find('=') {
                let val = tail[eq + 1..].trim();
                let digits: String = val
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '-')
                    .collect();
                if let Ok(v) = digits.parse::<i64>() {
                    return Some(v);
                }
            }
        }
    }
    None
}

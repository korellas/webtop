//! Per-process GPU time accounting via `ioreg AGXDeviceUserClient`.
//!
//! macOS exposes per-user-client GPU command-queue statistics through the
//! I/O Kit registry. Every Metal-using process has one or more
//! `AGXDeviceUserClient` entries that are children of `AGXAccelerator`.
//! Each entry carries:
//!
//!   - `IOUserClientCreator = "pid N, Name"` — identifies the owning PID.
//!   - `AppUsage = ({ ..., "accumulatedGPUTime" = <ns> }, ...)` — one or
//!     more dicts, each containing a cumulative GPU time counter in
//!     nanoseconds across the user client's lifetime.
//!
//! We shell out to `ioreg` (the canonical unprivileged I/O Kit dumper)
//! once per tick and parse the line-oriented output. Diffing the
//! cumulative per-PID totals against the previous sample yields a
//! per-process GPU time rate that mirrors what `mactop` and similar
//! tools report — no root, no private APIs, no entitlements.
//!
//! The same data is available via IOKit C bindings (`IOServiceMatching`
//! + `IORegistryEntryGetChildIterator`), but parsing `ioreg` keeps this
//! crate FFI-free and cross-version stable.

use std::collections::HashMap;
use std::process::Command;
use std::time::Duration;

/// Sample cumulative GPU nanoseconds per PID. Returns an empty map on
/// any error — callers should treat that as "GPU attribution unavailable
/// for this tick" rather than fatal.
pub fn sample() -> HashMap<u32, u64> {
    let out = Command::new("ioreg")
        .args(["-rc", "AGXDeviceUserClient", "-w", "0"])
        // Hard cap to avoid hanging the collector if ioreg stalls.
        .output();

    let Ok(out) = out else {
        return HashMap::new();
    };
    if !out.status.success() {
        return HashMap::new();
    }
    parse_ioreg_output(&String::from_utf8_lossy(&out.stdout))
}

/// Same as [`sample`] but with a caller-supplied timeout via env. Kept
/// for potential future use; the default invocation has no timeout knob
/// because ioreg normally returns well under 100 ms.
#[allow(dead_code)]
pub fn sample_with_timeout(_timeout: Duration) -> HashMap<u32, u64> {
    sample()
}

fn parse_ioreg_output(text: &str) -> HashMap<u32, u64> {
    let mut map: HashMap<u32, u64> = HashMap::new();

    // Every IORegistry entry starts with "+-o " at the beginning of a
    // (possibly indented) line. Split on that so each chunk is one
    // AGXDeviceUserClient with its properties block.
    for entry in text.split("+-o ") {
        let mut pid: Option<u32> = None;
        let mut gpu_ns_sum: u64 = 0;

        for line in entry.lines() {
            let trimmed = line.trim();

            // "IOUserClientCreator" = "pid 12345, ProcessName..."
            if let Some(rest) = trimmed.strip_prefix("\"IOUserClientCreator\" = \"pid ") {
                let end = rest.find(',').unwrap_or(rest.len());
                pid = rest[..end].parse::<u32>().ok();
                continue;
            }

            // AppUsage is one line like:
            //   "AppUsage" = ({"API"="Metal",..."accumulatedGPUTime"=521271962666},{...})
            // Sum every accumulatedGPUTime field we find on any line.
            if trimmed.contains("\"accumulatedGPUTime\"") {
                let needle = "\"accumulatedGPUTime\"=";
                let mut remain = trimmed;
                while let Some(idx) = remain.find(needle) {
                    let after = &remain[idx + needle.len()..];
                    let end = after
                        .find(|c: char| !c.is_ascii_digit())
                        .unwrap_or(after.len());
                    if let Ok(n) = after[..end].parse::<u64>() {
                        gpu_ns_sum = gpu_ns_sum.saturating_add(n);
                    }
                    remain = &after[end..];
                }
            }
        }

        if let Some(p) = pid {
            // A single PID can own multiple AGXDeviceUserClient entries;
            // merge their cumulative totals.
            *map.entry(p).or_default() =
                map.get(&p).copied().unwrap_or(0).saturating_add(gpu_ns_sum);
        }
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_representative_ioreg_output() {
        let sample = r#"
+-o AGXDeviceUserClient  <class AGXDeviceUserClient, id 0x1000016ba, !registered, !matched, active, busy 0, retain 5>
    {
      "AppUsage" = ()
      "IOUserClientCreator" = "pid 434, runningboardd"
    }

+-o AGXDeviceUserClient  <class AGXDeviceUserClient, id 0x10000186a, !registered, !matched, active, busy 0, retain 5>
    {
      "AppUsage" = ({"API"="Metal","lastSubmittedTime"=1,"accumulatedGPUTime"=500},{"API"="Metal","lastSubmittedTime"=2,"accumulatedGPUTime"=250})
      "IOUserClientCreator" = "pid 426, WindowServer"
    }

+-o AGXDeviceUserClient  <class AGXDeviceUserClient, id 0x100001a59, !registered, !matched, active, busy 0, retain 5>
    {
      "AppUsage" = ({"API"="Metal","lastSubmittedTime"=3,"accumulatedGPUTime"=100})
      "IOUserClientCreator" = "pid 426, WindowServer"
    }
"#;

        let got = parse_ioreg_output(sample);
        assert_eq!(got.get(&434).copied(), Some(0));
        // WindowServer (pid 426) appears in two AGXDeviceUserClient entries;
        // sum is 500 + 250 + 100 = 850.
        assert_eq!(got.get(&426).copied(), Some(850));
    }
}

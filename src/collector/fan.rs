//! Fan-speed sampling via Apple SMC.
//!
//! macmon's `Sampler::get_metrics()` already reports CPU/GPU temperature
//! and power, but it doesn't expose fan keys. Its `sources::SMC` binding
//! is `pub` though, so we open our own SMC connection and read the
//! well-known fan keys directly:
//!
//!   • `FNum` (UInt8) — total number of fans (0 on fanless Macs like
//!     the MacBook Air).
//!   • `F{i}Ac` — instantaneous RPM for fan `i`. Apple Silicon laptops
//!     report this as a 4-byte little-endian `flt ` (IEEE-754 single).
//!     Intel-era fixed-point types (`fpe2`, `fp1f`) are also handled so
//!     we degrade gracefully on older hardware.
//!
//! We sample every collector tick — a single `IOConnectCallStructMethod`
//! per fan key is sub-millisecond, far cheaper than the 1-second IOReport
//! window macmon already pays for.
//!
//! GPU die temperature was tried here previously and removed: on
//! M3 / M3 Ultra / M4, the `Tg*` SMC keys are idle-gated and only
//! populate under sustained GPU load, which produced confusing "GPU is
//! 0 °C" readings 90 %+ of the time. macmon's maintainer confirmed
//! this is hardware behaviour, not a software bug:
//! https://github.com/vladkens/macmon/issues/12. CPU die temp is a
//! reasonable proxy for system thermal state since CPU and GPU share
//! the same SoC die.

use macmon::sources::SMC;

/// Cap on the number of fans we'll probe. Apple has shipped at most 2
/// fans on a single Mac, so 8 is a generous ceiling that protects us from
/// a corrupted `FNum` byte sending us into a long enumeration loop.
const MAX_FANS_PROBE: u8 = 8;

pub struct FanReader {
    smc: SMC,
    /// Cached fan count from `FNum` — read once, not per tick. The kernel
    /// can't add or remove fans at runtime, so this is safe to memoise.
    count: u8,
    /// Whether `FNum` has been read yet. `false` on first call so we can
    /// distinguish "not asked" from "asked and got zero".
    initialized: bool,
}

impl FanReader {
    /// Open a connection to AppleSMC. Returns `None` when the user's
    /// machine has no SMC interface (extremely unusual — even desktop
    /// Macs expose it) or the IOKit handshake fails.
    pub fn new() -> Option<Self> {
        let smc = SMC::new().ok()?;
        Some(Self {
            smc,
            count: 0,
            initialized: false,
        })
    }

    /// Number of fans the SMC reports. 0 → fanless or unreadable.
    pub fn fan_count(&mut self) -> u8 {
        if !self.initialized {
            self.count = self.read_fan_count();
            self.initialized = true;
        }
        self.count
    }

    /// Highest current fan RPM across all fans. The summary card / chart
    /// only has space for one number, and the fastest spinning fan is
    /// the most operationally meaningful (it's the one keeping the
    /// hottest die from throttling). Returns 0.0 on fanless Macs or
    /// when every key fails to read.
    pub fn max_rpm(&mut self) -> f32 {
        let count = self.fan_count().min(MAX_FANS_PROBE);
        if count == 0 {
            return 0.0;
        }
        let mut max = 0.0f32;
        for i in 0..count {
            let key = format!("F{i}Ac");
            if let Some(rpm) = self.read_rpm(&key) {
                if rpm.is_finite() && rpm > max {
                    max = rpm;
                }
            }
        }
        max
    }

    fn read_fan_count(&mut self) -> u8 {
        match self.smc.read_val("FNum") {
            // FNum is a single-byte UInt8. Anything else is a malformed
            // response — treat as zero fans.
            Ok(v) if !v.data.is_empty() => v.data[0],
            _ => 0,
        }
    }

    /// Decode a fan-speed SMC value. The unit string (4-char FourCC) tells
    /// us how to read the bytes:
    ///
    ///   • `flt ` → 4-byte little-endian IEEE-754 single (Apple Silicon).
    ///   • `fpe2` → 16-bit unsigned big-endian fixed-point, 2 fractional
    ///     bits (Intel — divide raw by 4).
    ///   • `fp1f` → 16-bit signed big-endian fixed-point, 15 fractional
    ///     bits (rare; treat raw / 32768 as RPM ratio — virtually never
    ///     used for fan keys in practice but kept for completeness).
    ///   • `ui16` → 16-bit unsigned big-endian (some controllers).
    ///   • `ui8 ` → single-byte count (used by `FNum`, not fan RPM, but
    ///     handle it so we don't choke on a misconfigured probe).
    fn read_rpm(&mut self, key: &str) -> Option<f32> {
        let v = self.smc.read_val(key).ok()?;
        // The unit field is space-padded to 4 chars; trim so matching is
        // robust regardless of trailing whitespace.
        let unit = v.unit.trim_end();
        match unit {
            "flt" if v.data.len() >= 4 => {
                let arr: [u8; 4] = v.data[0..4].try_into().ok()?;
                Some(f32::from_le_bytes(arr))
            }
            "fpe2" if v.data.len() >= 2 => {
                let arr: [u8; 2] = v.data[0..2].try_into().ok()?;
                Some(u16::from_be_bytes(arr) as f32 / 4.0)
            }
            "fp1f" if v.data.len() >= 2 => {
                let arr: [u8; 2] = v.data[0..2].try_into().ok()?;
                Some(i16::from_be_bytes(arr) as f32 / 32768.0)
            }
            "ui16" if v.data.len() >= 2 => {
                let arr: [u8; 2] = v.data[0..2].try_into().ok()?;
                Some(u16::from_be_bytes(arr) as f32)
            }
            "ui8" if !v.data.is_empty() => Some(v.data[0] as f32),
            _ => None,
        }
    }
}

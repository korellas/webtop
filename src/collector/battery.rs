//! Battery information from macOS power-source and I/O Registry APIs.
//!
//! The common state comes from IOPowerSources. Cycle count, design capacity,
//! and raw charge values live on AppleSmartBattery, so those are read directly
//! from IOKit without launching `pmset` or `ioreg`.

use crate::collector::snapshot::BatteryInfo;
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType,
};
use objc2_io_kit::{
    kIOMainPortDefault, IOObjectRelease, IOPSCopyPowerSourcesInfo, IOPSCopyPowerSourcesList,
    IOPSGetPowerSourceDescription, IORegistryEntryCreateCFProperty, IOServiceGetMatchingService,
    IOServiceMatching,
};

pub fn collect() -> Option<BatteryInfo> {
    let info = IOPSCopyPowerSourcesInfo()?;
    let sources = unsafe { IOPSCopyPowerSourcesList(Some(&info))? };
    // SAFETY: IOPSCopyPowerSourcesList documents every array member as a
    // CFTypeRef power-source handle.
    let sources: CFRetained<CFArray<CFType>> = unsafe { CFRetained::cast_unchecked(sources) };

    for source in &*sources {
        let description =
            unsafe { IOPSGetPowerSourceDescription(Some(&info), Some(source.as_ref()))? };
        // SAFETY: IOPSGetPowerSourceDescription documents string keys and
        // arbitrary CoreFoundation values.
        let description: CFRetained<CFDictionary<CFString, CFType>> =
            unsafe { CFRetained::cast_unchecked(description) };

        if cf_string(&description, "Type").as_deref() != Some("InternalBattery") {
            continue;
        }

        let current = cf_i64(&description, "Current Capacity")?;
        let maximum = cf_i64(&description, "Max Capacity")?;
        if maximum <= 0 {
            return None;
        }

        let is_charging = cf_bool(&description, "Is Charging").unwrap_or(false);
        let is_plugged_in =
            cf_string(&description, "Power Source State").as_deref() == Some("AC Power");
        let remaining_minutes = if is_charging {
            cf_i64(&description, "Time to Full Charge")
        } else {
            cf_i64(&description, "Time to Empty")
        };
        let time_remaining_sec = remaining_minutes
            .filter(|minutes| *minutes >= 0)
            .and_then(|minutes| u32::try_from(minutes).ok())
            .map(|minutes| minutes.saturating_mul(60));

        let registry = battery_registry_values();
        let voltage_mv = cf_i64(&description, "Voltage").or(registry.voltage_mv);
        let amperage_ma = cf_i64(&description, "Current").or(registry.amperage_ma);
        let health_percent = match (registry.design_capacity, registry.max_capacity) {
            (Some(design), Some(maximum)) if design > 0 => {
                Some((maximum as f32 / design as f32 * 100.0).clamp(0.0, 120.0))
            }
            _ => None,
        };

        return Some(BatteryInfo {
            percent: (current as f32 / maximum as f32 * 100.0).clamp(0.0, 100.0),
            is_charging,
            is_plugged_in,
            time_remaining_sec,
            cycle_count: registry.cycle_count.and_then(|v| u32::try_from(v).ok()),
            health_percent,
            charge_rate_w: charge_rate_w(voltage_mv, amperage_ma),
        });
    }

    None
}

fn cf_value(dictionary: &CFDictionary<CFString, CFType>, key: &str) -> Option<CFRetained<CFType>> {
    dictionary.get(&CFString::from_str(key))
}

fn cf_i64(dictionary: &CFDictionary<CFString, CFType>, key: &str) -> Option<i64> {
    cf_value(dictionary, key)?
        .downcast_ref::<CFNumber>()?
        .as_i64()
}

fn cf_bool(dictionary: &CFDictionary<CFString, CFType>, key: &str) -> Option<bool> {
    Some(
        cf_value(dictionary, key)?
            .downcast_ref::<CFBoolean>()?
            .value(),
    )
}

fn cf_string(dictionary: &CFDictionary<CFString, CFType>, key: &str) -> Option<String> {
    Some(
        cf_value(dictionary, key)?
            .downcast_ref::<CFString>()?
            .to_string(),
    )
}

#[derive(Default)]
struct BatteryRegistryValues {
    cycle_count: Option<i64>,
    design_capacity: Option<i64>,
    max_capacity: Option<i64>,
    voltage_mv: Option<i64>,
    amperage_ma: Option<i64>,
}

fn battery_registry_values() -> BatteryRegistryValues {
    let class_name = b"AppleSmartBattery\0";
    let matching = unsafe { IOServiceMatching(class_name.as_ptr().cast()) };
    let Some(matching) = matching else {
        return BatteryRegistryValues::default();
    };
    // SAFETY: CFMutableDictionary is a CFDictionary subclass and the matching
    // dictionary is consumed by IOServiceGetMatchingService.
    let matching = unsafe { CFRetained::cast_unchecked(matching) };
    let service = unsafe { IOServiceGetMatchingService(kIOMainPortDefault, Some(matching)) };
    if service == 0 {
        return BatteryRegistryValues::default();
    }

    let values = BatteryRegistryValues {
        cycle_count: registry_i64(service, "CycleCount"),
        design_capacity: registry_i64(service, "DesignCapacity"),
        max_capacity: registry_i64(service, "AppleRawMaxCapacity")
            .or_else(|| registry_i64(service, "MaxCapacity")),
        voltage_mv: registry_i64(service, "Voltage"),
        amperage_ma: registry_i64(service, "InstantAmperage")
            .or_else(|| registry_i64(service, "Amperage")),
    };
    IOObjectRelease(service);
    values
}

fn registry_i64(service: u32, key: &str) -> Option<i64> {
    let key = CFString::from_str(key);
    let value = unsafe { IORegistryEntryCreateCFProperty(service, Some(&key), None, 0) }?;
    value.downcast_ref::<CFNumber>()?.as_i64()
}

fn charge_rate_w(voltage_mv: Option<i64>, amperage_ma: Option<i64>) -> Option<f32> {
    match (voltage_mv, amperage_ma) {
        (Some(voltage), Some(amperage)) => Some((voltage as f32 * amperage as f32) / 1_000_000.0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::charge_rate_w;

    #[test]
    fn native_voltage_and_signed_current_preserve_power_direction() {
        assert_eq!(charge_rate_w(Some(12_000), Some(1_500)), Some(18.0));
        assert_eq!(charge_rate_w(Some(12_000), Some(-1_500)), Some(-18.0));
        assert_eq!(charge_rate_w(None, Some(1_500)), None);
    }
}

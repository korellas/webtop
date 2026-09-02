//! Per-process GPU time accounting through the I/O Registry API.
//!
//! AGXDeviceUserClient exposes a creator PID plus cumulative GPU-time values.
//! Reading those properties directly avoids launching and parsing `ioreg`.

use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_io_kit::{
    kIOMainPortDefault, kIORegistryIterateRecursively, kIOServicePlane, IOIteratorNext,
    IOObjectRelease, IORegistryEntryCreateCFProperty, IORegistryEntryCreateIterator,
    IOServiceGetMatchingService, IOServiceMatching,
};
use std::collections::HashMap;

pub fn sample() -> HashMap<u32, u64> {
    let class_name = b"AGXAccelerator\0";
    let Some(matching) = (unsafe { IOServiceMatching(class_name.as_ptr().cast()) }) else {
        return HashMap::new();
    };
    // SAFETY: CFMutableDictionary is a CFDictionary subclass and IOKit
    // consumes this matching dictionary.
    let matching = unsafe { CFRetained::cast_unchecked(matching) };
    let accelerator = unsafe { IOServiceGetMatchingService(kIOMainPortDefault, Some(matching)) };
    if accelerator == 0 {
        return HashMap::new();
    }
    let mut iterator = 0;
    let status = unsafe {
        IORegistryEntryCreateIterator(
            accelerator,
            kIOServicePlane.as_ptr().cast_mut().cast(),
            kIORegistryIterateRecursively,
            &mut iterator,
        )
    };
    IOObjectRelease(accelerator);
    if status != 0 || iterator == 0 {
        return HashMap::new();
    }

    let mut totals = HashMap::new();
    loop {
        let service = IOIteratorNext(iterator);
        if service == 0 {
            break;
        }

        if let Some(pid) = creator_pid(service) {
            let gpu_ns = app_usage_gpu_time(service);
            totals
                .entry(pid)
                .and_modify(|total: &mut u64| *total = total.saturating_add(gpu_ns))
                .or_insert(gpu_ns);
        }
        IOObjectRelease(service);
    }
    IOObjectRelease(iterator);
    totals
}

fn creator_pid(service: u32) -> Option<u32> {
    let value = registry_property(service, "IOUserClientCreator")?;
    let creator = value.downcast_ref::<CFString>()?.to_string();
    parse_creator_pid(&creator)
}

fn parse_creator_pid(creator: &str) -> Option<u32> {
    creator
        .strip_prefix("pid ")?
        .split(',')
        .next()?
        .trim()
        .parse()
        .ok()
}

fn app_usage_gpu_time(service: u32) -> u64 {
    let Some(value) = registry_property(service, "AppUsage") else {
        return 0;
    };
    let Ok(entries) = value.downcast::<CFArray>() else {
        return 0;
    };
    // SAFETY: IOKit materializes AppUsage as an array of CF property-list
    // values; every item is inspected with a checked downcast below.
    let entries: CFRetained<CFArray<CFType>> = unsafe { CFRetained::cast_unchecked(entries) };

    entries
        .iter()
        .filter_map(|entry| entry.downcast::<CFDictionary>().ok())
        .map(|dictionary| {
            // SAFETY: AGX AppUsage dictionaries use CFString keys and
            // property-list values.
            let dictionary: CFRetained<CFDictionary<CFString, CFType>> =
                unsafe { CFRetained::cast_unchecked(dictionary) };
            dictionary
                .get(&CFString::from_str("accumulatedGPUTime"))
                .and_then(|value| value.downcast::<CFNumber>().ok())
                .and_then(|number| number.as_i64())
                .and_then(|number| u64::try_from(number).ok())
                .unwrap_or(0)
        })
        .fold(0u64, u64::saturating_add)
}

fn registry_property(service: u32, key: &str) -> Option<CFRetained<CFType>> {
    let key = CFString::from_str(key);
    unsafe { IORegistryEntryCreateCFProperty(service, Some(&key), None, 0) }
}

#[cfg(test)]
mod tests {
    use super::parse_creator_pid;

    #[test]
    fn native_creator_string_yields_pid() {
        assert_eq!(parse_creator_pid("pid 426, WindowServer"), Some(426));
        assert_eq!(parse_creator_pid("WindowServer"), None);
    }
}

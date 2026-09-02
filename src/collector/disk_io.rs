//! Host-wide disk I/O counters from IOBlockStorageDriver.
//!
//! These are cumulative kernel counters. Diffing them is both cheaper and
//! more complete than refreshing every process through sysinfo each tick.

use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_io_kit::{
    kIOMainPortDefault, IOIteratorNext, IOObjectRelease, IORegistryEntryCreateCFProperty,
    IOServiceGetMatchingServices, IOServiceMatching,
};

pub struct DiskIoSampler {
    previous_read: Option<u64>,
    previous_write: Option<u64>,
}

impl Default for DiskIoSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl DiskIoSampler {
    pub fn new() -> Self {
        let initial = read_totals();
        Self {
            previous_read: initial.map(|totals| totals.0),
            previous_write: initial.map(|totals| totals.1),
        }
    }

    pub fn sample_delta(&mut self) -> (u64, u64) {
        let Some((read, write)) = read_totals() else {
            return (0, 0);
        };
        let delta = (
            counter_delta(self.previous_read, read),
            counter_delta(self.previous_write, write),
        );
        self.previous_read = Some(read);
        self.previous_write = Some(write);
        delta
    }
}

fn read_totals() -> Option<(u64, u64)> {
    let class_name = b"IOBlockStorageDriver\0";
    let matching = unsafe { IOServiceMatching(class_name.as_ptr().cast()) }?;
    // SAFETY: CFMutableDictionary is a CFDictionary subclass and IOKit
    // consumes the matching dictionary.
    let matching = unsafe { CFRetained::cast_unchecked(matching) };
    let mut iterator = 0;
    let status =
        unsafe { IOServiceGetMatchingServices(kIOMainPortDefault, Some(matching), &mut iterator) };
    if status != 0 || iterator == 0 {
        return None;
    }

    let mut read = 0u64;
    let mut write = 0u64;
    loop {
        let service = IOIteratorNext(iterator);
        if service == 0 {
            break;
        }
        if let Some((service_read, service_write)) = service_totals(service) {
            read = read.saturating_add(service_read);
            write = write.saturating_add(service_write);
        }
        IOObjectRelease(service);
    }
    IOObjectRelease(iterator);
    Some((read, write))
}

fn service_totals(service: u32) -> Option<(u64, u64)> {
    let key = CFString::from_str("Statistics");
    let value = unsafe { IORegistryEntryCreateCFProperty(service, Some(&key), None, 0) }?;
    let statistics = value.downcast::<CFDictionary>().ok()?;
    // SAFETY: IOBlockStorageDriver publishes Statistics with CFString keys
    // and property-list values.
    let statistics: CFRetained<CFDictionary<CFString, CFType>> =
        unsafe { CFRetained::cast_unchecked(statistics) };
    Some((
        dictionary_u64(&statistics, "Bytes (Read)").unwrap_or(0),
        dictionary_u64(&statistics, "Bytes (Write)").unwrap_or(0),
    ))
}

fn dictionary_u64(dictionary: &CFDictionary<CFString, CFType>, key: &str) -> Option<u64> {
    dictionary
        .get(&CFString::from_str(key))?
        .downcast::<CFNumber>()
        .ok()?
        .as_i64()
        .and_then(|value| u64::try_from(value).ok())
}

fn counter_delta(previous: Option<u64>, current: u64) -> u64 {
    previous
        .map(|previous| current.saturating_sub(previous))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::counter_delta;

    #[test]
    fn cumulative_disk_counters_handle_first_sample_and_reset() {
        assert_eq!(counter_delta(None, 1_000), 0);
        assert_eq!(counter_delta(Some(1_000), 1_750), 750);
        assert_eq!(counter_delta(Some(1_750), 100), 0);
    }
}

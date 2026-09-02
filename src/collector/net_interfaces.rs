//! Network interface enumeration: name, IPs, MAC, link speed, per-iface traffic,
//! plus Wi-Fi specifics (SSID, RSSI, channel, tx rate) for wireless interfaces.

use objc2_core_foundation::{CFArray, CFRetained};
use objc2_core_wlan::{CWSecurity, CWWiFiClient};
use objc2_system_configuration::SCNetworkInterface;
use serde::Serialize;
use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sysinfo::Networks;

#[derive(Debug, Clone, Serialize, Default)]
pub struct WirelessInfo {
    pub ssid: Option<String>,
    pub bssid: Option<String>,
    pub channel: Option<u32>,
    /// "2.4 GHz" | "5 GHz" | "6 GHz"
    pub band: Option<String>,
    pub rssi_dbm: Option<i32>,
    pub noise_dbm: Option<i32>,
    /// Current negotiated transmit rate in Mbps.
    pub tx_rate_mbps: Option<u32>,
    pub security: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetInterfaceInfo {
    pub name: String,
    pub is_up: bool,
    pub mac: Option<String>,
    pub ipv4: Vec<String>,
    pub ipv6: Vec<String>,
    /// Link speed in bits/sec, if known.
    pub link_speed_bps: Option<u64>,
    pub rx_bytes_sec: u64,
    pub tx_bytes_sec: u64,
    pub mtu: Option<u32>,
    /// Coarse interface kind.
    pub kind: &'static str,
    /// Additional Wi-Fi details — only populated for wireless interfaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wireless: Option<WirelessInfo>,
}

/// Per-interface traffic delta sampler.
struct NetSampler {
    networks: Networks,
    prev: HashMap<String, (u64, u64)>,
    last_at: Option<Instant>,
}

impl NetSampler {
    fn new() -> Self {
        Self {
            networks: Networks::new_with_refreshed_list(),
            prev: HashMap::new(),
            last_at: None,
        }
    }

    fn sample(&mut self) -> HashMap<String, (u64, u64)> {
        self.networks.refresh(true);
        let now = Instant::now();
        let dt = self
            .last_at
            .map(|prev| now.duration_since(prev).as_secs_f64().max(0.1))
            .unwrap_or(1.0);

        let mut rates = HashMap::new();
        let mut current = HashMap::new();
        for (name, data) in &self.networks {
            let rx = data.total_received();
            let tx = data.total_transmitted();
            current.insert(name.clone(), (rx, tx));
            let (prx, ptx) = self.prev.get(name).copied().unwrap_or((rx, tx));
            let d_rx = rx.saturating_sub(prx);
            let d_tx = tx.saturating_sub(ptx);
            rates.insert(
                name.clone(),
                ((d_rx as f64 / dt) as u64, (d_tx as f64 / dt) as u64),
            );
        }
        self.prev = current;
        self.last_at = Some(now);
        rates
    }
}

static SAMPLER: std::sync::OnceLock<Mutex<NetSampler>> = std::sync::OnceLock::new();

fn sampler() -> &'static Mutex<NetSampler> {
    SAMPLER.get_or_init(|| Mutex::new(NetSampler::new()))
}

/// Wi-Fi association details change slowly. Cache them for 15s.
struct WifiCache {
    info: Option<WirelessInfo>,
    iface: Option<String>,
    at: Instant,
}

static WIFI_CACHE: std::sync::OnceLock<Mutex<Option<WifiCache>>> = std::sync::OnceLock::new();

fn wifi_cache() -> &'static Mutex<Option<WifiCache>> {
    WIFI_CACHE.get_or_init(|| Mutex::new(None))
}

/// Enumerate "meaningful" network interfaces — ones with an IPv4 address that
/// is up. Hides the 20+ internal `anpi*`, `awdl*`, `utun*`, etc. pseudo-ifaces.
pub fn list_interfaces() -> Vec<NetInterfaceInfo> {
    let addrs = if_addrs::get_if_addrs().unwrap_or_default();
    let mut grouped: HashMap<String, (Vec<String>, Vec<String>)> = HashMap::new();
    for a in addrs {
        // Skip internal "meta" IPs: 169.254.x.x (link-local), fe80:: kept only
        // as a fallback when there's no global v6.
        let bucket = grouped.entry(a.name.clone()).or_default();
        match a.addr {
            if_addrs::IfAddr::V4(v) => bucket.0.push(v.ip.to_string()),
            if_addrs::IfAddr::V6(v) => bucket.1.push(v.ip.to_string()),
        }
    }

    let native_metadata = native_interface_metadata();
    let ifconfig = Command::new("ifconfig").output().ok();
    let if_text = ifconfig
        .as_ref()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).into_owned())
            } else {
                None
            }
        })
        .unwrap_or_default();
    let ifconfig_map = parse_ifconfig(&if_text);

    let rates = sampler().lock().unwrap().sample();

    // Only keep interfaces that:
    //   • have at least one IPv4 address (not link-local)
    //   • are marked UP
    //   • aren't loopback
    let mut out: Vec<NetInterfaceInfo> = grouped
        .iter()
        .filter_map(|(name, (ipv4, ipv6))| {
            // Filter out link-local 169.254.x.x addresses; keep the rest.
            let ipv4_global: Vec<String> = ipv4
                .iter()
                .filter(|ip| !ip.starts_with("169.254."))
                .cloned()
                .collect();
            if ipv4_global.is_empty() {
                return None;
            }
            if name == "lo0" {
                return None;
            }
            let info = ifconfig_map.get(name).cloned().unwrap_or_default();
            let native = native_metadata.get(name);
            if !info.is_up {
                return None;
            }
            let (rx, tx) = rates.get(name).copied().unwrap_or((0, 0));
            Some(NetInterfaceInfo {
                kind: native
                    .and_then(|metadata| metadata.kind)
                    .unwrap_or_else(|| classify_name_fallback(name)),
                mac: native
                    .and_then(|metadata| metadata.mac.clone())
                    .or_else(|| info.mac.clone()),
                link_speed_bps: info.link_speed_bps,
                mtu: info.mtu,
                is_up: info.is_up,
                ipv4: ipv4_global,
                ipv6: ipv6.clone(),
                rx_bytes_sec: rx,
                tx_bytes_sec: tx,
                name: name.clone(),
                wireless: None,
            })
        })
        .collect();

    // Enrich wireless interfaces with Wi-Fi details (cached call).
    for iface in out.iter_mut() {
        if iface.kind == "wifi" {
            let info = cached_wifi_info(&iface.name);
            // Wi-Fi doesn't report a stable `media` line in ifconfig; expose the
            // currently-negotiated TX rate as the effective link speed so the UI
            // can show "324 Mbps" instead of "—".
            if iface.link_speed_bps.is_none() {
                if let Some(ref w) = info {
                    if let Some(mbps) = w.tx_rate_mbps {
                        iface.link_speed_bps = Some((mbps as u64) * 1_000_000);
                    }
                }
            }
            iface.wireless = info;
        }
    }

    // Ethernet / wifi first, then VPN / bridge / other; alphabetical within.
    out.sort_by(|a, b| {
        kind_rank(a.kind)
            .cmp(&kind_rank(b.kind))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

#[derive(Default)]
struct NativeInterfaceMetadata {
    mac: Option<String>,
    kind: Option<&'static str>,
}

fn native_interface_metadata() -> HashMap<String, NativeInterfaceMetadata> {
    let interfaces = SCNetworkInterface::all();
    // SAFETY: SCNetworkInterfaceCopyAll documents every array entry as an
    // SCNetworkInterface reference.
    let interfaces: CFRetained<CFArray<SCNetworkInterface>> =
        unsafe { CFRetained::cast_unchecked(interfaces) };
    interfaces
        .iter()
        .filter_map(|interface| {
            let name = interface.bsd_name()?.to_string();
            let interface_type = interface
                .interface_type()
                .map(|value| value.to_string())
                .unwrap_or_default();
            let display_name = interface
                .localized_display_name()
                .map(|value| value.to_string())
                .unwrap_or_default();
            Some((
                name,
                NativeInterfaceMetadata {
                    mac: interface
                        .hardware_address_string()
                        .map(|value| value.to_string()),
                    kind: classify_sc_interface(&interface_type, &display_name),
                },
            ))
        })
        .collect()
}

fn classify_sc_interface(interface_type: &str, display_name: &str) -> Option<&'static str> {
    let haystack = format!("{interface_type} {display_name}").to_ascii_lowercase();
    if haystack.contains("ieee80211") || haystack.contains("wi-fi") || haystack.contains("airport")
    {
        Some("wifi")
    } else if haystack.contains("ethernet") || haystack.contains("lan") {
        Some("ethernet")
    } else if haystack.contains("bridge") || haystack.contains("thunderbolt") {
        Some("bridge")
    } else {
        None
    }
}

fn kind_rank(kind: &str) -> u8 {
    match kind {
        "wifi" => 0,
        "ethernet" => 1,
        "vpn" => 2,
        "bridge" => 3,
        "p2p" => 4,
        _ => 5,
    }
}

fn cached_wifi_info(iface: &str) -> Option<WirelessInfo> {
    let mut guard = wifi_cache().lock().ok()?;
    if let Some(cache) = guard.as_ref() {
        if cache.iface.as_deref() == Some(iface) && cache.at.elapsed() < Duration::from_secs(15) {
            return cache.info.clone();
        }
    }
    let info = native_wifi_info(iface).or_else(|| fetch_wifi_info_fallback(iface));
    *guard = Some(WifiCache {
        info: info.clone(),
        iface: Some(iface.to_string()),
        at: Instant::now(),
    });
    info
}

fn native_wifi_info(iface: &str) -> Option<WirelessInfo> {
    // SAFETY: CoreWLAN owns the shared client and the binding retains every
    // returned Objective-C object for the duration of this function.
    let client = unsafe { CWWiFiClient::sharedWiFiClient() };
    let interfaces = unsafe { client.interfaces()? };
    let interface = interfaces.to_vec().into_iter().find(|candidate| {
        unsafe { candidate.interfaceName() }
            .map(|name| name.to_string() == iface)
            .unwrap_or(false)
    })?;

    let ssid = unsafe { interface.ssid() }
        .map(|v| v.to_string())
        .filter(|v| !v.is_empty() && !v.contains("redacted"));
    let bssid = unsafe { interface.bssid() }
        .map(|v| v.to_string())
        .filter(|v| !v.is_empty() && !v.contains("redacted"));
    let channel = unsafe { interface.wlanChannel() }
        .map(|v| unsafe { v.channelNumber() })
        .filter(|v| *v > 0)
        .map(|v| v as u32);
    let rssi_dbm = match unsafe { interface.rssiValue() } as i32 {
        0 => None,
        v => Some(v),
    };
    let noise_dbm = match unsafe { interface.noiseMeasurement() } as i32 {
        0 => None,
        v => Some(v),
    };
    let tx_rate = unsafe { interface.transmitRate() };
    let tx_rate_mbps = (tx_rate.is_finite() && tx_rate > 0.0).then(|| tx_rate.round() as u32);
    let security = security_label(unsafe { interface.security() });

    Some(WirelessInfo {
        ssid,
        bssid,
        channel,
        band: channel.map(channel_to_band),
        rssi_dbm,
        noise_dbm,
        tx_rate_mbps,
        security,
    })
}

fn security_label(security: CWSecurity) -> Option<String> {
    let label = if security == CWSecurity::None {
        "Open"
    } else if security == CWSecurity::WEP || security == CWSecurity::DynamicWEP {
        "WEP"
    } else if security == CWSecurity::WPAPersonal {
        "WPA Personal"
    } else if security == CWSecurity::WPAPersonalMixed {
        "WPA/WPA2 Personal"
    } else if security == CWSecurity::WPA2Personal || security == CWSecurity::Personal {
        "WPA2 Personal"
    } else if security == CWSecurity::WPA3Personal {
        "WPA3 Personal"
    } else if security == CWSecurity::WPA3Transition {
        "WPA2/WPA3 Personal"
    } else if security == CWSecurity::WPAEnterprise {
        "WPA Enterprise"
    } else if security == CWSecurity::WPAEnterpriseMixed {
        "WPA/WPA2 Enterprise"
    } else if security == CWSecurity::WPA2Enterprise || security == CWSecurity::Enterprise {
        "WPA2 Enterprise"
    } else if security == CWSecurity::WPA3Enterprise {
        "WPA3 Enterprise"
    } else if security == CWSecurity::OWE || security == CWSecurity::OWETransition {
        "Enhanced Open"
    } else {
        return None;
    };
    Some(label.into())
}

/// Query `system_profiler SPAirPortDataType -json` for the currently-associated
/// network on the given interface. Returns `None` if not on Wi-Fi.
fn fetch_wifi_info_fallback(iface: &str) -> Option<WirelessInfo> {
    let out = Command::new("system_profiler")
        .args(["SPAirPortDataType", "-json", "-detailLevel", "basic"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let root: serde_json::Value = serde_json::from_str(&text).ok()?;
    let airport = root.get("SPAirPortDataType")?.as_array()?.first()?;
    let interfaces = airport.get("spairport_airport_interfaces")?.as_array()?;
    let entry = interfaces.iter().find(|i| {
        i.get("_name")
            .and_then(|n| n.as_str())
            .map(|n| n == iface)
            .unwrap_or(false)
    })?;

    let current = entry.get("spairport_current_network_information");
    let current = current?;

    // macOS 14+ privacy: without Location Services permission, system_profiler
    // returns the literal string "<redacted>" for SSID/BSSID. Normalize to None
    // so the frontend can show an informative placeholder instead.
    let ssid = current
        .get("_name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.contains("redacted"))
        .map(|s| s.to_string());
    let bssid = current
        .get("spairport_network_bssid")
        .and_then(|v| v.as_str())
        .filter(|s| !s.contains("redacted"))
        .map(|s| s.to_string());
    let channel = current
        .get("spairport_network_channel")
        .and_then(|v| v.as_str())
        .and_then(|s| parse_channel(s));
    let band = channel.map(channel_to_band);
    let rssi_dbm = current
        .get("spairport_signal_noise")
        .and_then(|v| v.as_str())
        .and_then(|s| parse_rssi(s));
    let noise_dbm = current
        .get("spairport_signal_noise")
        .and_then(|v| v.as_str())
        .and_then(|s| parse_noise(s));
    let tx_rate_mbps = current
        .get("spairport_network_rate")
        .and_then(|v| v.as_f64())
        .or_else(|| {
            current
                .get("spairport_network_rate")
                .and_then(|v| v.as_u64())
                .map(|v| v as f64)
        })
        .map(|v| v as u32);
    let security = current
        .get("spairport_security_mode")
        .and_then(|v| v.as_str())
        .map(normalize_security);

    Some(WirelessInfo {
        ssid,
        bssid,
        channel,
        band,
        rssi_dbm,
        noise_dbm,
        tx_rate_mbps,
        security,
    })
}

fn parse_channel(raw: &str) -> Option<u32> {
    // Examples: "36 (5 GHz, 80 MHz)", "6", "149 (5 GHz, 160 MHz)"
    let first_tok: String = raw.chars().take_while(|c| c.is_ascii_digit()).collect();
    first_tok.parse().ok()
}

fn channel_to_band(ch: u32) -> String {
    if ch <= 14 {
        "2.4 GHz".into()
    } else if ch < 200 {
        "5 GHz".into()
    } else {
        "6 GHz".into()
    }
}

/// `"-54 / -90 dBm"` → -54 (signal).
fn parse_rssi(s: &str) -> Option<i32> {
    let first = s.split('/').next()?;
    let digits: String = first
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    digits.parse().ok()
}

/// `"-54 / -90 dBm"` → -90 (noise).
fn parse_noise(s: &str) -> Option<i32> {
    let mut parts = s.split('/');
    parts.next()?;
    let second = parts.next()?;
    let digits: String = second
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    digits.parse().ok()
}

fn normalize_security(s: &str) -> String {
    // Strip the "spairport_security_mode_" prefix if present.
    s.strip_prefix("spairport_security_mode_")
        .unwrap_or(s)
        .replace('_', " ")
}

#[derive(Debug, Clone, Default)]
struct IfcInfo {
    mac: Option<String>,
    link_speed_bps: Option<u64>,
    mtu: Option<u32>,
    is_up: bool,
}

/// Parse `ifconfig` output into a map from interface name to scraped fields.
fn parse_ifconfig(text: &str) -> HashMap<String, IfcInfo> {
    let mut map: HashMap<String, IfcInfo> = HashMap::new();
    let mut current: Option<String> = None;

    for line in text.lines() {
        if !line.starts_with(char::is_whitespace) && line.contains(':') {
            let name = line.split(':').next().unwrap_or("").trim().to_string();
            current = Some(name.clone());
            let info = map.entry(name).or_default();
            info.is_up = line.contains("UP,") || line.contains("<UP");
            if let Some(pos) = line.find("mtu ") {
                let digits: String = line[pos + 4..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                info.mtu = digits.parse().ok();
            }
        } else if let Some(ref name) = current {
            let info = map.entry(name.clone()).or_default();
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("ether ") {
                info.mac = Some(rest.trim().to_string());
            } else if l.starts_with("media:") {
                info.link_speed_bps = parse_media_speed(l);
            } else if l.starts_with("status:") {
                info.is_up = info.is_up || l.contains("active");
            }
        }
    }
    map
}

fn parse_media_speed(line: &str) -> Option<u64> {
    let lower = line.to_ascii_lowercase();
    if lower.contains("10gbase") || lower.contains("10000base") {
        Some(10_000_000_000)
    } else if lower.contains("2.5gbase") || lower.contains("2500base") {
        Some(2_500_000_000)
    } else if lower.contains("5gbase") {
        Some(5_000_000_000)
    } else if lower.contains("1000base") || lower.contains("1gbase") {
        Some(1_000_000_000)
    } else if lower.contains("100base") {
        Some(100_000_000)
    } else if lower.contains("10base") {
        Some(10_000_000)
    } else {
        None
    }
}

fn classify_name_fallback(name: &str) -> &'static str {
    if name == "lo0" || name.starts_with("lo") {
        "loopback"
    } else if name.starts_with("bridge") {
        "bridge"
    } else if name.starts_with("awdl") || name.starts_with("llw") || name.starts_with("ap") {
        "p2p"
    } else if name.starts_with("utun") || name.starts_with("ipsec") || name.starts_with("ppp") {
        "vpn"
    } else if name.starts_with("en") {
        "ethernet"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_sc_interface, security_label};
    use objc2_core_wlan::CWSecurity;

    #[test]
    fn native_wifi_security_has_stable_labels() {
        assert_eq!(security_label(CWSecurity::None), Some("Open".into()));
        assert_eq!(
            security_label(CWSecurity::WPA3Transition),
            Some("WPA2/WPA3 Personal".into())
        );
        assert_eq!(security_label(CWSecurity::Unknown), None);
    }

    #[test]
    fn system_configuration_types_map_to_ui_kinds() {
        assert_eq!(classify_sc_interface("IEEE80211", "Wi-Fi"), Some("wifi"));
        assert_eq!(
            classify_sc_interface("Ethernet", "USB 10/100/1000 LAN"),
            Some("ethernet")
        );
        assert_eq!(
            classify_sc_interface("Bridge", "Thunderbolt Bridge"),
            Some("bridge")
        );
        assert_eq!(classify_sc_interface("Unknown", "Unknown"), None);
    }
}

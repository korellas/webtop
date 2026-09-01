//! One-shot SMC key dumper. Lists every `Tg*` / `Tp*` / `Te*` / `Ts*` key
//! the live SMC exposes along with its current value and unit so we can
//! see which sensors actually populate on this Mac.
//!
//! Run with:  `cargo run --release --example dump_smc_keys`

use macmon::sources::SMC;

fn main() {
    let mut smc = SMC::new().expect("SMC open failed");
    let keys = smc.read_all_keys().expect("read_all_keys failed");

    let mut groups: Vec<(&'static str, Vec<String>)> = vec![
        ("Tp* (P-cluster CPU)", Vec::new()),
        ("Te* (E-cluster CPU)", Vec::new()),
        ("Ts* (Super CPU, M5+)", Vec::new()),
        ("Tg* (GPU)", Vec::new()),
        ("TG* (legacy GPU)", Vec::new()),
        ("TPD* (M4 CPU?)", Vec::new()),
        ("F* (Fans)", Vec::new()),
    ];

    for k in &keys {
        let bucket = if k.starts_with("Tp") {
            0
        } else if k.starts_with("Te") {
            1
        } else if k.starts_with("Ts") {
            2
        } else if k.starts_with("Tg") {
            3
        } else if k.starts_with("TG") {
            4
        } else if k.starts_with("TPD") {
            5
        } else if k.starts_with("F") {
            6
        } else {
            continue;
        };
        groups[bucket].1.push(k.clone());
    }

    for (label, names) in groups {
        println!("\n== {label} ({} keys) ==", names.len());
        for name in names {
            let info = match smc.read_key_info(&name) {
                Ok(i) => i,
                Err(_) => continue,
            };
            let unit = std::str::from_utf8(&info.data_type.to_be_bytes())
                .unwrap_or("???")
                .to_string();
            let val = match smc.read_val(&name) {
                Ok(v) => v,
                Err(e) => {
                    println!("  {name}  unit={unit:<5}  ERR={e}");
                    continue;
                }
            };
            // Best-effort decode for display.
            let decoded = match unit.trim_end() {
                "flt" if val.data.len() >= 4 => {
                    let arr: [u8; 4] = val.data[..4].try_into().unwrap();
                    format!("{:.2}", f32::from_le_bytes(arr))
                }
                "ui8" if !val.data.is_empty() => format!("{}", val.data[0]),
                "ui16" if val.data.len() >= 2 => {
                    let arr: [u8; 2] = val.data[..2].try_into().unwrap();
                    format!("{}", u16::from_be_bytes(arr))
                }
                "fpe2" if val.data.len() >= 2 => {
                    let arr: [u8; 2] = val.data[..2].try_into().unwrap();
                    format!("{:.2}", u16::from_be_bytes(arr) as f32 / 4.0)
                }
                _ => format!("raw={:?}", val.data),
            };
            println!(
                "  {name}  unit={unit:<5}  size={}  val={decoded}",
                info.data_size
            );
        }
    }
}

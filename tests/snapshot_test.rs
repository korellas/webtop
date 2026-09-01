use webtop::collector::snapshot::{ProcessInfo, SystemSnapshot};

#[test]
fn snapshot_serializes_to_json() {
    let snap = SystemSnapshot {
        timestamp: 1713200000000,
        cpu_total: 42.5,
        cpu_p_cores: 58.0,
        cpu_e_cores: 12.0,
        cpu_cores: vec![58.0, 12.0, 24.0, 35.0],
        gpu_usage: 18.0,
        mem_used: 24_300_000_000,
        mem_total: 76_000_000_000,
        mem_swap_used: 1_200_000_000,
        mem_breakdown: Default::default(),
        disk_read_bytes_sec: 120_000_000,
        disk_write_bytes_sec: 45_000_000,
        disk_used: 1_400_000_000_000,
        disk_total: 2_000_000_000_000,
        net_up_bytes_sec: 2_400_000,
        net_down_bytes_sec: 890_000,
        power_total_w: 38.0,
        power_cpu_w: 18.0,
        power_gpu_w: 12.0,
        power_other_w: 8.0,
        cpu_temp_c: 56.5,
        gpu_temp_c: 49.0,
        fan_rpm: 2400.0,
        energy_session_wh: 0.42,
        energy_prev_month_wh: 0.0,
        battery: None,
        processes: vec![ProcessInfo {
            pid: 1234,
            name: "chrome".into(),
            user: "alice".into(),
            cpu_percent: 12.3,
            gpu_percent: 5.5,
            mem_bytes: 2_100_000_000,
            cmd: "/Applications/Chrome.app/Contents/MacOS/Chrome --type=renderer".into(),
        }],
        // Live samples carry no band — a single reading is its own extreme.
        band: None,
    };
    let json = serde_json::to_string(&snap).unwrap();
    assert!(json.contains("\"cpu_total\":42.5"));
    assert!(json.contains("\"chrome\""));
    // `band: None` must be omitted rather than serialised as null: it rides
    // every WebSocket push, and the frontend distinguishes "no band" from a
    // present-but-empty one.
    assert!(
        !json.contains("\"band\""),
        "band should be skipped when None"
    );
}

#[test]
fn snapshot_default_is_zeroed() {
    let snap = SystemSnapshot::default();
    assert_eq!(snap.cpu_total, 0.0);
    assert!(snap.processes.is_empty());
}

//! Throwaway harness: compare the scanner's totals against `du`.
use std::time::Instant;
fn main() {
    let home = std::env::var("HOME").unwrap();
    let root = std::path::Path::new(&home);
    let t = Instant::now();
    let r = webtop::collector::folder_sizes::scan(root, None);
    println!(
        "scanned in {:?}, {} folders >= 10MB, truncated={}",
        t.elapsed(),
        r.folders.len(),
        r.truncated
    );
    let mut top: Vec<_> = r.folders.iter().filter(|f| f.parent == root).collect();
    top.sort_by_key(|f| std::cmp::Reverse(f.size_bytes));
    println!(
        "{:<14} {:>10} {:>10} {:>8}",
        "FOLDER", "SIZE", "FILES", "UNREAD"
    );
    for f in top.iter().take(7) {
        println!(
            "{:<14} {:>9.1}G {:>10} {:>8}",
            f.path.file_name().unwrap().to_string_lossy(),
            f.size_bytes as f64 / 1073741824.0,
            f.file_count,
            f.unreadable
        );
    }
    let root_entry = r.folders.iter().find(|f| f.path == root).unwrap();
    println!(
        "\nROOT total: {:.1}G, {} files, {} unreadable",
        root_entry.size_bytes as f64 / 1073741824.0,
        root_entry.file_count,
        root_entry.unreadable
    );
}

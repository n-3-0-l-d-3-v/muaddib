//! `docs/design/CONSTRAINTS.md`: no trusted wall clock anywhere in the
//! kernel. This test turns that from a convention into a check. It scans
//! every crate's `src/` for any use of a physical-time API and fails the
//! build if it finds one. Comments are skipped, so docs can still *talk*
//! about the clocks the kernel refuses to use.
//!
//! Scope: kernel source only. Benchmarks (ticket 006) measure the kernel
//! from the outside with a timer, which is fine: that's the harness
//! timing the kernel, not the kernel reading time.

use std::fs;
use std::path::{Path, PathBuf};

const FORBIDDEN: &[&str] = &[
    "std::time",
    "SystemTime",
    "Instant",
    "UNIX_EPOCH",
    "chrono",
    "time::OffsetDateTime",
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The code part of a line: everything before a `//` comment.
fn code_part(line: &str) -> &str {
    line.split("//").next().unwrap_or("")
}

#[test]
fn no_kernel_source_reads_physical_time() {
    let crates_dir = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    for krate in fs::read_dir(crates_dir).unwrap() {
        let src = krate.unwrap().path().join("src");
        if src.is_dir() {
            rust_files(&src, &mut files);
        }
    }
    assert!(
        files.len() >= 5,
        "scanned suspiciously few files ({}), is the path right?",
        files.len()
    );

    let mut violations = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).unwrap();
        for (n, line) in text.lines().enumerate() {
            let code = code_part(line);
            for needle in FORBIDDEN {
                if code.contains(needle) {
                    violations.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "physical-time APIs used in kernel source:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_scanner_would_catch_a_violation() {
    assert!(FORBIDDEN
        .iter()
        .any(|n| code_part("let t = std::time::Instant::now();").contains(n)));
    assert!(!FORBIDDEN
        .iter()
        .any(|n| code_part("// never call Instant::now()").contains(n)));
}

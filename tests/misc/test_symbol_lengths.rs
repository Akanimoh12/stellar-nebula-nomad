//! Regression test for issues #392 / #393: `symbol_short!` accepts at most
//! 9 characters. A longer literal is a compile error on current SDKs, so this
//! scan guards against re-introducing one (e.g. `tx_success`, `error_spike`,
//! `incompatible`, `batch_completed`, `rolled_back`) anywhere in the crate.
use std::fs;
use std::path::Path;

const MAX_SHORT_SYMBOL: usize = 9;

fn visit(dir: &Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            visit(&path, out);
        } else if path.extension().map_or(false, |e| e == "rs") {
            let text = fs::read_to_string(&path).unwrap();
            for (n, line) in text.lines().enumerate() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") {
                    continue; // doc comments / examples
                }
                let mut rest = line;
                while let Some(i) = rest.find("symbol_short!(\"") {
                    let after = &rest[i + "symbol_short!(\"".len()..];
                    if let Some(end) = after.find('"') {
                        if end > MAX_SHORT_SYMBOL {
                            out.push(format!(
                                "{}:{}: `{}` is {} chars (max {})",
                                path.display(), n + 1, &after[..end], end, MAX_SHORT_SYMBOL
                            ));
                        }
                        rest = &after[end..];
                    } else {
                        break;
                    }
                }
            }
        }
    }
}

#[test]
fn no_symbol_short_literal_exceeds_nine_chars() {
    let mut bad = Vec::new();
    visit(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut bad);
    assert!(bad.is_empty(), "symbol_short! literals too long:\n{}", bad.join("\n"));
}

#[test]
fn metrics_and_migration_symbols_are_short() {
    for s in ["tx_ok", "tx_fail", "err_spike", "unhealthy", "high_gas", "incomp", "batch_ok", "rollback"] {
        assert!(s.len() <= MAX_SHORT_SYMBOL, "{s}");
    }
}

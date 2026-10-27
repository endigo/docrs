//! Console and JSON reporting shared by the parity suites.

use std::path::Path;

use serde_json::Value;

/// `text` cut to `width` bytes, with an ellipsis when it was longer.
pub fn clipped(mut text: String, width: usize) -> String {
    if text.len() > width {
        text.truncate(width);
        text.push('…');
    }
    text
}

/// The first `shown` failures, one per line after `prefix`, clipped to
/// `width` bytes.
pub fn print_failures(prefix: &str, failures: &[Value], shown: usize, width: usize) {
    for failure in failures.iter().take(shown) {
        println!("  {prefix}{}", clipped(failure.to_string(), width));
    }
}

/// Write `value` pretty-printed to `path` when a report was asked for.
pub fn write_json(path: Option<&Path>, value: &Value) -> Result<(), String> {
    if let Some(path) = path {
        std::fs::write(
            path,
            serde_json::to_string_pretty(value).unwrap_or_default(),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

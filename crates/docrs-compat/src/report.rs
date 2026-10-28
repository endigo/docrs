//! Console and JSON reporting shared by the parity suites.

use std::path::Path;

use serde_json::Value;

/// `text` cut to at most `width` bytes on a character boundary, with an
/// ellipsis when it was longer.
pub fn clipped(mut text: String, width: usize) -> String {
    if text.len() > width {
        let cut = (0..=width)
            .rev()
            .find(|at| text.is_char_boundary(*at))
            .unwrap_or(0);
        text.truncate(cut);
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

#[cfg(test)]
mod tests {
    use super::clipped;

    #[test]
    fn clipping_stops_on_a_character_boundary() {
        assert_eq!(clipped("abc".into(), 3), "abc");
        assert_eq!(clipped("abcd".into(), 3), "abc…");
        assert_eq!(clipped("a—b".into(), 2), "a…");
        assert_eq!(clipped("—".into(), 0), "…");
    }
}

//! Small text helpers (no deps, host-tested in host-tests/).

use core::fmt::{Display, Write};

/// Writes `template` to `out`, replacing `{i}` with `params[i]` (unknown or out-of-range
/// placeholders are kept as-is). Never allocates; `out` decides what to do when full.
pub fn fill_placeholders<T: Display>(template: &str, params: &[T], out: &mut impl Write) {
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        _ = out.write_str(&rest[..open]);
        let after = &rest[open + 1..];
        let param = after.find('}').and_then(|close| {
            let idx: usize = after[..close].parse().ok()?;
            Some((params.get(idx)?, close))
        });

        match param {
            Some((param, close)) => {
                _ = write!(out, "{param}");
                rest = &after[close + 1..];
            }
            None => {
                _ = out.write_char('{');
                rest = after;
            }
        }
    }
    _ = out.write_str(rest);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(t: &str, p: &[&str]) -> String {
        let mut s = String::new();
        fill_placeholders(t, p, &mut s);
        s
    }

    #[test]
    fn placeholders() {
        assert_eq!(fill("of the competitor ({0})", &["12.34"]), "of the competitor (12.34)");
        assert_eq!(fill("{1} {0} {0}", &["a", "b"]), "b a a");
        assert_eq!(fill("{2} {x} { {", &["a"]), "{2} {x} { {");
        assert_eq!(fill("", &["a"]), "");
        assert_eq!(fill("ó{0}ł", &["ż"]), "óżł");
    }
}

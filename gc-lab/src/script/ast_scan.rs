//! Conservative syntactic checks over the AST's derived `Debug` text, used by the
//! stage-1 fast paths of BYTECODE_VM.md until the stage-2 scope-analysis pass exists.
//! A match anywhere (identifiers, variant names, string literals) answers "yes", so
//! a false positive only keeps the slow, spec-literal path.
use std::fmt::{self, Debug, Write};

/// Whether the `Debug` text of any item contains one of `needles`. Stops formatting
/// at the first match.
pub(super) fn mentions(items: &[&dyn Debug], needles: &[&str]) -> bool {
    let keep = needles.iter().map(|n| n.len()).max().unwrap_or(1) - 1;
    let mut sink = Sink {
        needles,
        tail: String::new(),
        keep,
    };
    items.iter().any(|item| write!(sink, "{item:?}").is_err())
}

struct Sink<'a> {
    needles: &'a [&'a str],
    /// The last `keep` bytes seen, so a needle split across writes is still found.
    tail: String,
    keep: usize,
}

impl Write for Sink<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.tail.push_str(s);
        if self.needles.iter().any(|n| self.tail.contains(n)) {
            return Err(fmt::Error);
        }
        if self.tail.len() > self.keep {
            let mut cut = self.tail.len() - self.keep;
            while !self.tail.is_char_boundary(cut) {
                cut -= 1;
            }
            self.tail.drain(..cut);
        }
        Ok(())
    }
}

/// Syntax that can create a closure or run code that captures the current scope.
pub(super) const CAPTURES: &[&str] = &[
    "Function", "Arrow", "Class", "Method", "Getter", "Setter", "\"eval\"",
];

#[cfg(test)]
mod tests {
    #[test]
    fn finds_needles_across_write_boundaries() {
        let items: Vec<String> = vec!["ab".into(), "cd".into()];
        assert!(super::mentions(&[&items], &["b\", \"c"]));
        assert!(!super::mentions(&[&items], &["eval"]));
    }
}

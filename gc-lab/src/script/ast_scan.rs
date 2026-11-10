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

/// AST variants (in `Debug` text) whose subtree is a function body: a name mentioned
/// anywhere inside one may be captured by a closure.
const CLOSURE_VARIANTS: &[&str] = &[
    "Arrow",
    "AsyncArrow",
    "Function",
    "AsyncFunction",
    "GeneratorFunction",
    "Class",
    "ClassInit",
    "Getter",
    "Setter",
];

/// The names of `names` mentioned (textually, quoted as in `Debug`) inside a closure
/// subtree of `items`: an arrow, function, class, accessor or class element. A false
/// positive only moves a binding from a register to its environment record.
pub(super) fn captured_names(items: &dyn Debug, names: &[String]) -> Vec<String> {
    let text = format!("{items:?}");
    let bytes = text.as_bytes();
    let mut regions: Vec<&str> = vec![];
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            i = skip_string(bytes, i);
            continue;
        }
        let starts_ident = (b.is_ascii_alphabetic() || b == b'_')
            && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_'));
        if !starts_ident {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
            j += 1;
        }
        let ident = &text[i..j];
        let open = if bytes.get(j) == Some(&b'(') {
            Some(j)
        } else if text[j..].starts_with(" {") {
            Some(j + 1)
        } else {
            None
        };
        match open {
            Some(open) if CLOSURE_VARIANTS.contains(&ident) => {
                let end = matching(bytes, open);
                regions.push(&text[open..end]);
                i = end;
            }
            _ => i = j,
        }
    }
    names
        .iter()
        .filter(|name| {
            let quoted = format!("{name:?}");
            regions.iter().any(|r| r.contains(quoted.as_str()))
        })
        .cloned()
        .collect()
}

/// The index after the string literal starting at `i` (a `"`).
fn skip_string(bytes: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    i
}

/// The index after the bracket that closes the one at `open`, skipping strings.
fn matching(bytes: &[u8], open: usize) -> usize {
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i = skip_string(bytes, i);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_needles_across_write_boundaries() {
        let items: Vec<String> = vec!["ab".into(), "cd".into()];
        assert!(super::mentions(&[&items], &["b\", \"c"]));
        assert!(!super::mentions(&[&items], &["eval"]));
    }
    #[test]
    fn captured_names_only_inside_closures() {
        let names: Vec<String> = ["a", "b", "c", "Function"].map(String::from).to_vec();
        let text = r#"[Expr(Name("a")), Expr(String("Function(\"b\")")), Expr(Arrow { params: [], body: [Return(Name("b"))] }), Let("Function", Null)]"#;
        struct Raw(&'static str);
        impl std::fmt::Debug for Raw {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.0)
            }
        }
        let raw: &'static str = Box::leak(text.to_string().into_boxed_str());
        assert_eq!(
            super::captured_names(&Raw(raw), &names),
            vec!["b".to_string()]
        );
    }
}

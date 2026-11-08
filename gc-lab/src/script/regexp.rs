//! Pure ECMAScript RegExp engine: pattern parser (ES2025 syntax with the Annex B
//! extensions outside Unicode mode), a compiler to a small bytecode and a
//! backtracking matcher over UTF-16 code units. No engine state is touched here.
use super::identifiers::{id_continue, id_start};
use super::regexp_tables::{FOLD, NAMES, PROPERTIES, RANGES, STRINGS};
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub(super) struct Flags {
    pub d: bool,
    pub g: bool,
    pub i: bool,
    pub m: bool,
    pub s: bool,
    pub u: bool,
    pub v: bool,
    pub y: bool,
}
impl Flags {
    pub(super) fn parse(text: &str) -> Option<Flags> {
        let mut f = Flags::default();
        for c in text.chars() {
            let slot = match c {
                'd' => &mut f.d,
                'g' => &mut f.g,
                'i' => &mut f.i,
                'm' => &mut f.m,
                's' => &mut f.s,
                'u' => &mut f.u,
                'v' => &mut f.v,
                'y' => &mut f.y,
                _ => return None,
            };
            if *slot {
                return None;
            }
            *slot = true;
        }
        (!(f.u && f.v)).then_some(f)
    }
    pub(super) fn unicode(&self) -> bool {
        self.u || self.v
    }
    /// The canonical `flags` string order.
    pub(super) fn text(&self) -> String {
        let mut s = String::new();
        for (on, c) in [
            (self.d, 'd'),
            (self.g, 'g'),
            (self.i, 'i'),
            (self.m, 'm'),
            (self.s, 's'),
            (self.u, 'u'),
            (self.v, 'v'),
            (self.y, 'y'),
        ] {
            if on {
                s.push(c);
            }
        }
        s
    }
}

const INF: u32 = u32::MAX;
const UNSET: usize = usize::MAX;
type Ranges = Vec<(u32, u32)>;
type R<T> = Result<T, String>;

// ---------------------------------------------------------------- sets
fn norm(mut r: Ranges) -> Ranges {
    r.sort_unstable();
    let mut out: Ranges = Vec::with_capacity(r.len());
    for (a, b) in r {
        match out.last_mut() {
            Some(last) if a <= last.1.saturating_add(1) => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}
fn complement(r: &[(u32, u32)]) -> Ranges {
    let mut out = Vec::with_capacity(r.len() + 1);
    let mut next = 0u32;
    for &(a, b) in r {
        if a > next {
            out.push((next, a - 1));
        }
        next = b + 1;
    }
    if next <= 0x10FFFF {
        out.push((next, 0x10FFFF));
    }
    out
}
fn intersect(a: &[(u32, u32)], b: &[(u32, u32)]) -> Ranges {
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < a.len() && j < b.len() {
        let lo = a[i].0.max(b[j].0);
        let hi = a[i].1.min(b[j].1);
        if lo <= hi {
            out.push((lo, hi));
        }
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}
fn contains(r: &[(u32, u32)], c: u32) -> bool {
    let i = r.partition_point(|&(_, hi)| hi < c);
    i < r.len() && r[i].0 <= c
}

/// A class value: code point ranges plus (v flag only) strings whose length is not one.
#[derive(Clone, Default, Debug)]
struct CSet {
    r: Ranges,
    s: Vec<Vec<u32>>,
}
impl CSet {
    fn ranges(r: Ranges) -> CSet {
        CSet {
            r: norm(r),
            s: vec![],
        }
    }
    fn union(mut self, o: CSet) -> CSet {
        self.r.extend(o.r);
        self.r = norm(self.r);
        for s in o.s {
            if !self.s.contains(&s) {
                self.s.push(s);
            }
        }
        self
    }
    fn intersect(self, o: CSet) -> CSet {
        CSet {
            r: intersect(&self.r, &o.r),
            s: self.s.into_iter().filter(|s| o.s.contains(s)).collect(),
        }
    }
    fn subtract(self, o: CSet) -> CSet {
        CSet {
            r: intersect(&self.r, &complement(&o.r)),
            s: self.s.into_iter().filter(|s| !o.s.contains(s)).collect(),
        }
    }
}

// ---------------------------------------------------------------- case folding
fn canon(c: u32, unicode: bool) -> u32 {
    if unicode {
        return match FOLD.binary_search_by_key(&c, |e| e.0) {
            Ok(i) => FOLD[i].1,
            Err(_) => c,
        };
    }
    if c > 0xFFFF {
        return c;
    }
    let Some(ch) = char::from_u32(c) else {
        return c;
    };
    let mut up = ch.to_uppercase();
    if up.len() != 1 {
        return c;
    }
    let u = up.next().unwrap() as u32;
    if u > 0xFFFF || (c >= 128 && u < 128) {
        c
    } else {
        u
    }
}
/// Case-equivalence classes with more than one member.
fn groups(unicode: bool) -> &'static [Vec<u32>] {
    static U: OnceLock<Vec<Vec<u32>>> = OnceLock::new();
    static N: OnceLock<Vec<Vec<u32>>> = OnceLock::new();
    let build = || {
        let mut map: HashMap<u32, Vec<u32>> = HashMap::new();
        if unicode {
            for &(a, b) in FOLD {
                map.entry(b).or_default().push(a);
            }
        } else {
            for c in 0..0x10000u32 {
                let k = canon(c, false);
                if k != c {
                    map.entry(k).or_default().push(c);
                }
            }
        }
        let mut out: Vec<Vec<u32>> = map
            .into_iter()
            .map(|(k, mut v)| {
                if canon(k, unicode) == k {
                    v.push(k);
                }
                v.sort_unstable();
                v
            })
            .filter(|v| v.len() > 1)
            .collect();
        out.sort();
        out
    };
    if unicode {
        U.get_or_init(build)
    } else {
        N.get_or_init(build)
    }
}
fn close(r: Ranges, unicode: bool) -> Ranges {
    let mut out = r.clone();
    for g in groups(unicode) {
        if g.iter().any(|&c| contains(&r, c)) {
            out.extend(g.iter().map(|&c| (c, c)));
        }
    }
    norm(out)
}

// ---------------------------------------------------------------- properties
fn lookup(key: &str) -> Option<Ranges> {
    let canon = NAMES
        .binary_search_by(|e| e.0.cmp(key))
        .ok()
        .map(|i| NAMES[i].1)?;
    let i = PROPERTIES.binary_search_by(|e| e.0.cmp(canon)).ok()?;
    let (_, start, n) = PROPERTIES[i];
    Some(RANGES[start..start + n].to_vec())
}
fn property(name: &str, value: Option<&str>) -> Option<Ranges> {
    match value {
        Some(v) => {
            let p = match name {
                "General_Category" | "gc" => "gc",
                "Script" | "sc" => "sc",
                "Script_Extensions" | "scx" => "scx",
                _ => return None,
            };
            lookup(&format!("{p}={v}"))
        }
        None => lookup(&format!("gc={name}")).or_else(|| lookup(&format!("bin={name}"))),
    }
}
fn string_property(name: &str) -> Option<CSet> {
    let list = STRINGS.iter().find(|e| e.0 == name)?.1;
    let mut set = CSet::default();
    for s in list {
        let cps: Vec<u32> = s.chars().map(|c| c as u32).collect();
        if cps.len() == 1 {
            set.r.push((cps[0], cps[0]));
        } else {
            set.s.push(cps);
        }
    }
    set.r = norm(set.r);
    Some(set)
}
const SPACE: &[(u32, u32)] = &[
    (9, 0xD),
    (0x20, 0x20),
    (0xA0, 0xA0),
    (0x1680, 0x1680),
    (0x2000, 0x200A),
    (0x2028, 0x2029),
    (0x202F, 0x202F),
    (0x205F, 0x205F),
    (0x3000, 0x3000),
    (0xFEFF, 0xFEFF),
];
fn is_line_terminator(c: u32) -> bool {
    matches!(c, 0xA | 0xD | 0x2028 | 0x2029)
}
fn is_word(c: u32, unicode_icase: bool) -> bool {
    matches!(c, 0x30..=0x39 | 0x41..=0x5A | 0x61..=0x7A | 0x5F)
        || (unicode_icase && (c == 0x17F || c == 0x212A))
}
fn high(c: u32) -> bool {
    (0xD800..0xDC00).contains(&c)
}
fn low(c: u32) -> bool {
    (0xDC00..0xE000).contains(&c)
}
fn pair(h: u32, l: u32) -> u32 {
    0x10000 + ((h - 0xD800) << 10) + (l - 0xDC00)
}

// ---------------------------------------------------------------- AST
#[derive(Debug)]
enum Node {
    Empty,
    Char(u32, bool),
    Any(bool),
    Set(Ranges, bool),
    Bol(bool),
    Eol(bool),
    Word(bool, bool),
    Group(Box<Node>, Option<usize>),
    Look(bool, bool, Box<Node>),
    BackRef(Vec<usize>, bool),
    NamedRef(String, bool),
    Alt(Vec<Node>),
    Seq(Vec<Node>),
    Repeat(Box<Node>, u32, u32, bool, usize, usize),
}
enum Atom {
    Char(u32),
    Set(Ranges),
}

fn c(ch: char) -> u32 {
    ch as u32
}
fn err<T>(m: &str) -> R<T> {
    Err(format!("Invalid regular expression: {m}"))
}

struct Parser {
    s: Vec<u32>,
    i: usize,
    u: bool,
    v: bool,
    named: bool,
    total: usize,
    ncap: usize,
    icase: bool,
    multi: bool,
    dotall: bool,
    names: Vec<(String, Vec<usize>)>,
    name_paths: Vec<(String, Vec<(usize, usize)>)>,
    path: Vec<(usize, usize)>,
    disj: usize,
}

fn prescan(s: &[u32], v: bool) -> (usize, bool) {
    let (mut i, mut n, mut named, mut depth) = (0, 0, false, 0usize);
    while i < s.len() {
        let ch = s[i];
        if ch == c('\\') {
            i += 1;
        } else if ch == c('[') {
            if depth == 0 || v {
                depth += 1;
            }
        } else if ch == c(']') {
            depth = depth.saturating_sub(1);
        } else if ch == c('(') && depth == 0 {
            if s.get(i + 1) != Some(&c('?')) {
                n += 1;
            } else if s.get(i + 2) == Some(&c('<'))
                && !matches!(s.get(i + 3), Some(&x) if x == c('=') || x == c('!'))
            {
                n += 1;
                named = true;
            }
        }
        i += 1;
    }
    (n, named)
}

impl Parser {
    fn peek(&self) -> Option<u32> {
        self.s.get(self.i).copied()
    }
    fn at(&self, k: usize) -> Option<u32> {
        self.s.get(self.i + k).copied()
    }
    fn is(&self, ch: char) -> bool {
        self.peek() == Some(c(ch))
    }
    fn eat(&mut self, ch: char) -> bool {
        let hit = self.is(ch);
        if hit {
            self.i += 1;
        }
        hit
    }
    fn starts(&self, a: char, b: char) -> bool {
        self.peek() == Some(c(a)) && self.at(1) == Some(c(b))
    }
    fn close_group(&mut self) -> R<()> {
        if self.eat(')') {
            Ok(())
        } else {
            err("Unterminated group")
        }
    }

    fn disjunction(&mut self) -> R<Node> {
        let id = self.disj;
        self.disj += 1;
        let mut alts = vec![];
        loop {
            self.path.push((id, alts.len()));
            let a = self.alternative();
            self.path.pop();
            alts.push(a?);
            if !self.eat('|') {
                break;
            }
        }
        Ok(if alts.len() == 1 {
            alts.pop().unwrap()
        } else {
            Node::Alt(alts)
        })
    }
    fn alternative(&mut self) -> R<Node> {
        let mut seq = vec![];
        while let Some(ch) = self.peek() {
            if ch == c('|') || ch == c(')') {
                break;
            }
            seq.push(self.term()?);
        }
        Ok(match seq.len() {
            0 => Node::Empty,
            1 => seq.pop().unwrap(),
            _ => Node::Seq(seq),
        })
    }
    fn term(&mut self) -> R<Node> {
        let first = self.ncap;
        if self.eat('^') {
            return self.unquantifiable(Node::Bol(self.multi));
        }
        if self.eat('$') {
            return self.unquantifiable(Node::Eol(self.multi));
        }
        if self.is('\\') && matches!(self.at(1), Some(x) if x == c('b') || x == c('B')) {
            let neg = self.at(1) == Some(c('B'));
            self.i += 2;
            return self.unquantifiable(Node::Word(neg, self.icase && self.u));
        }
        if self.starts('(', '?') {
            let k = self.at(2);
            let behind =
                k == Some(c('<')) && matches!(self.at(3), Some(x) if x == c('=') || x == c('!'));
            if behind || k == Some(c('=')) || k == Some(c('!')) {
                let negate = self.at(if behind { 3 } else { 2 }) == Some(c('!'));
                self.i += if behind { 4 } else { 3 };
                let body = self.disjunction()?;
                self.close_group()?;
                let node = Node::Look(behind, negate, Box::new(body));
                if behind || self.u {
                    return self.unquantifiable(node);
                }
                return self.quantified(node, first);
            }
        }
        let atom = self.atom()?;
        self.quantified(atom, first)
    }
    fn unquantifiable(&mut self, node: Node) -> R<Node> {
        let save = self.i;
        let q = match self.peek() {
            Some(x) if x == c('*') || x == c('+') || x == c('?') => true,
            Some(x) if x == c('{') => self.braces().is_some(),
            _ => false,
        };
        self.i = save;
        if q {
            err("Nothing to repeat")
        } else {
            Ok(node)
        }
    }
    fn digits(&mut self) -> Option<u64> {
        let start = self.i;
        let mut n: u64 = 0;
        while let Some(d) = self.peek().filter(|d| (c('0')..=c('9')).contains(d)) {
            n = n.saturating_mul(10).saturating_add((d - c('0')) as u64);
            self.i += 1;
        }
        (self.i > start).then_some(n)
    }
    /// `{n}`, `{n,}` or `{n,m}` at the cursor; restores the cursor when absent.
    fn braces(&mut self) -> Option<(u64, u64)> {
        let start = self.i;
        let r = (|| {
            if !self.eat('{') {
                return None;
            }
            let a = self.digits()?;
            let b = if self.eat(',') {
                if self.is('}') {
                    u64::MAX
                } else {
                    self.digits()?
                }
            } else {
                a
            };
            self.eat('}').then_some((a, b))
        })();
        if r.is_none() {
            self.i = start;
        }
        r
    }
    fn quantified(&mut self, atom: Node, first: usize) -> R<Node> {
        let (min, max) = match self.peek() {
            Some(x) if x == c('*') => (0, u64::MAX),
            Some(x) if x == c('+') => (1, u64::MAX),
            Some(x) if x == c('?') => (0, 1),
            Some(x) if x == c('{') => match self.braces() {
                Some((a, b)) => {
                    if a > b {
                        return err("numbers out of order in {} quantifier");
                    }
                    self.i -= 1;
                    (a, b)
                }
                None if self.u => return err("Incomplete quantifier"),
                None => return Ok(atom),
            },
            _ => return Ok(atom),
        };
        self.i += 1;
        let greedy = !self.eat('?');
        let clamp = |n: u64| n.min(INF as u64) as u32;
        Ok(Node::Repeat(
            Box::new(atom),
            clamp(min),
            clamp(max),
            greedy,
            first,
            self.ncap,
        ))
    }
    fn atom(&mut self) -> R<Node> {
        let ch = self.peek().unwrap();
        match char::from_u32(ch).unwrap_or('\0') {
            '.' => {
                self.i += 1;
                Ok(Node::Any(self.dotall))
            }
            '(' => self.group(),
            '[' => {
                self.i += 1;
                self.class()
            }
            '\\' => self.atom_escape(),
            '*' | '+' | '?' => err("Nothing to repeat"),
            '{' => {
                if self.u {
                    return err("Lone quantifier brackets");
                }
                if self.braces().is_some() {
                    return err("Nothing to repeat");
                }
                self.i += 1;
                Ok(Node::Char(ch, self.icase))
            }
            '}' | ']' if self.u => err("Lone quantifier brackets"),
            _ => {
                self.i += 1;
                Ok(Node::Char(ch, self.icase))
            }
        }
    }
    fn group(&mut self) -> R<Node> {
        self.i += 1;
        if !self.eat('?') {
            self.ncap += 1;
            let idx = self.ncap;
            let body = self.disjunction()?;
            self.close_group()?;
            return Ok(Node::Group(Box::new(body), Some(idx)));
        }
        if self.eat(':') {
            let body = self.disjunction()?;
            self.close_group()?;
            return Ok(Node::Group(Box::new(body), None));
        }
        if self.eat('<') {
            let name = self.group_name()?;
            self.ncap += 1;
            let idx = self.ncap;
            for (n, p) in &self.name_paths {
                if *n == name && !compatible(p, &self.path) {
                    return err("Duplicate capture group name");
                }
            }
            self.name_paths.push((name.clone(), self.path.clone()));
            match self.names.iter_mut().find(|e| e.0 == name) {
                Some(e) => e.1.push(idx),
                None => self.names.push((name, vec![idx])),
            }
            let body = self.disjunction()?;
            self.close_group()?;
            return Ok(Node::Group(Box::new(body), Some(idx)));
        }
        self.modifiers()
    }
    fn modifiers(&mut self) -> R<Node> {
        let (mut add, mut rem, mut minus) = (String::new(), String::new(), false);
        loop {
            let Some(ch) = self.peek().and_then(char::from_u32) else {
                return err("Invalid group");
            };
            self.i += 1;
            match ch {
                'i' | 'm' | 's' => {
                    if add.contains(ch) || rem.contains(ch) {
                        return err("Repeated flag in modifiers");
                    }
                    if minus { &mut rem } else { &mut add }.push(ch);
                }
                '-' if !minus => minus = true,
                ':' => break,
                _ => return err("Invalid group"),
            }
        }
        if minus && add.is_empty() && rem.is_empty() {
            return err("Invalid modifiers");
        }
        let saved = (self.icase, self.multi, self.dotall);
        for (set, on) in [(&add, true), (&rem, false)] {
            for ch in set.chars() {
                match ch {
                    'i' => self.icase = on,
                    'm' => self.multi = on,
                    _ => self.dotall = on,
                }
            }
        }
        let body = self.disjunction();
        (self.icase, self.multi, self.dotall) = saved;
        let body = body?;
        self.close_group()?;
        Ok(Node::Group(Box::new(body), None))
    }
    fn group_name(&mut self) -> R<String> {
        let mut name = String::new();
        loop {
            let ch = match self.peek() {
                None => return err("Invalid capture group name"),
                Some(x) if x == c('>') => {
                    self.i += 1;
                    break;
                }
                Some(x) if x == c('\\') => {
                    self.i += 1;
                    if !self.eat('u') {
                        return err("Invalid capture group name");
                    }
                    match self.unicode_escape(true) {
                        Some(v) => v,
                        None => return err("Invalid Unicode escape"),
                    }
                }
                Some(x) => {
                    self.i += 1;
                    match self.peek() {
                        Some(y) if !self.u && high(x) && low(y) => {
                            self.i += 1;
                            pair(x, y)
                        }
                        _ => x,
                    }
                }
            };
            let Some(ch) = char::from_u32(ch) else {
                return err("Invalid capture group name");
            };
            let ok = if name.is_empty() {
                id_start(ch)
            } else {
                id_continue(ch)
            };
            if !ok {
                return err("Invalid capture group name");
            }
            name.push(ch);
        }
        if name.is_empty() {
            return err("Invalid capture group name");
        }
        Ok(name)
    }
    fn hex(&mut self, n: usize) -> Option<u32> {
        let mut v = 0;
        for k in 0..n {
            let d = char::from_u32(self.at(k)?)?.to_digit(16)?;
            v = v * 16 + d;
        }
        self.i += n;
        Some(v)
    }
    /// After `\u`: `XXXX`, and with `full` also `{X...}` and escaped surrogate pairs.
    fn unicode_escape(&mut self, full: bool) -> Option<u32> {
        let start = self.i;
        if full && self.eat('{') {
            let mut v: u32 = 0;
            let mut any = false;
            while let Some(d) = self
                .peek()
                .and_then(char::from_u32)
                .and_then(|c| c.to_digit(16))
            {
                v = v.saturating_mul(16).saturating_add(d);
                any = true;
                self.i += 1;
            }
            if any && v <= 0x10FFFF && self.eat('}') {
                return Some(v);
            }
            self.i = start;
            return None;
        }
        let a = self.hex(4)?;
        if full && high(a) && self.starts('\\', 'u') {
            let save = self.i;
            self.i += 2;
            match self.hex(4) {
                Some(b) if low(b) => return Some(pair(a, b)),
                _ => self.i = save,
            }
        }
        Some(a)
    }
    fn atom_escape(&mut self) -> R<Node> {
        self.i += 1;
        let Some(ch) = self.peek() else {
            return err("\\ at end of pattern");
        };
        if (c('1')..=c('9')).contains(&ch) {
            let start = self.i;
            let n = self.digits().unwrap();
            if n <= self.total as u64 {
                return Ok(Node::BackRef(vec![n as usize], self.icase));
            }
            if self.u {
                return err("Invalid escape");
            }
            self.i = start;
        }
        if ch == c('k') && (self.u || self.named) {
            self.i += 1;
            if !self.eat('<') {
                return err("Invalid named reference");
            }
            let name = self.group_name()?;
            return Ok(Node::NamedRef(name, self.icase));
        }
        if let Some(set) = self.class_escape()? {
            return Ok(self.set_node(set, false));
        }
        let v = self.char_escape(false)?;
        Ok(Node::Char(v, self.icase))
    }
    /// `\d \D \s \S \w \W` and, in Unicode mode, `\p{..} \P{..}`; cursor after `\`.
    fn class_escape(&mut self) -> R<Option<CSet>> {
        let Some(ch) = self.peek().and_then(char::from_u32) else {
            return Ok(None);
        };
        let base: Ranges = match ch.to_ascii_lowercase() {
            'd' => vec![(0x30, 0x39)],
            's' => SPACE.to_vec(),
            'w' => {
                let mut r = vec![(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)];
                if self.u && self.icase {
                    r.extend([(0x17F, 0x17F), (0x212A, 0x212A)]);
                }
                r
            }
            'p' if self.u => {
                self.i += 1;
                return self.property_escape(ch == 'P').map(Some);
            }
            _ => return Ok(None),
        };
        self.i += 1;
        let set = self.leaf(CSet::ranges(base));
        Ok(Some(if ch.is_ascii_uppercase() {
            CSet::ranges(complement(&set.r))
        } else {
            set
        }))
    }
    fn property_escape(&mut self, negate: bool) -> R<CSet> {
        if !self.eat('{') {
            return err("Invalid property name");
        }
        let mut name = String::new();
        let mut value: Option<String> = None;
        loop {
            let Some(ch) = self.peek().and_then(char::from_u32) else {
                return err("Invalid property name");
            };
            self.i += 1;
            match ch {
                '}' => break,
                '=' if value.is_none() => value = Some(String::new()),
                c if c.is_ascii_alphanumeric() || c == '_' => match &mut value {
                    Some(v) => v.push(c),
                    None => name.push(c),
                },
                _ => return err("Invalid property name"),
            }
        }
        if name.is_empty() || value.as_deref() == Some("") {
            return err("Invalid property name");
        }
        if let Some(r) = property(&name, value.as_deref()) {
            let set = self.leaf(CSet::ranges(r));
            return Ok(if negate {
                CSet::ranges(complement(&set.r))
            } else {
                set
            });
        }
        if value.is_none() && self.v && !negate {
            if let Some(set) = string_property(&name) {
                return Ok(self.leaf(set));
            }
        }
        err("Invalid property name")
    }
    fn octal(&mut self) -> u32 {
        let first = self.peek().unwrap() - c('0');
        self.i += 1;
        let mut v = first;
        let limit = if first <= 3 { 2 } else { 1 };
        for _ in 0..limit {
            match self.peek() {
                Some(d) if (c('0')..=c('7')).contains(&d) => {
                    v = v * 8 + (d - c('0'));
                    self.i += 1;
                }
                _ => break,
            }
        }
        v
    }
    /// CharacterEscape (plus the Annex B forms); cursor after `\`.
    fn char_escape(&mut self, class: bool) -> R<u32> {
        let Some(ch) = self.peek() else {
            return err("\\ at end of pattern");
        };
        self.i += 1;
        let next_digit = self.peek().is_some_and(|d| (c('0')..=c('9')).contains(&d));
        Ok(match char::from_u32(ch).unwrap_or('\0') {
            'f' => 0xC,
            'n' => 0xA,
            'r' => 0xD,
            't' => 9,
            'v' => 0xB,
            'c' => match self.peek().and_then(char::from_u32) {
                Some(l) if l.is_ascii_alphabetic() => {
                    self.i += 1;
                    l as u32 % 32
                }
                Some(l) if class && !self.u && (l.is_ascii_digit() || l == '_') => {
                    self.i += 1;
                    l as u32 % 32
                }
                _ if self.u => return err("Invalid unicode escape"),
                _ => {
                    self.i -= 1;
                    c('\\')
                }
            },
            '0' if !next_digit => 0,
            '0'..='7' if !self.u => {
                self.i -= 1;
                self.octal()
            }
            'x' => match self.hex(2) {
                Some(v) => v,
                None if self.u => return err("Invalid escape"),
                None => ch,
            },
            'u' => match self.unicode_escape(self.u) {
                Some(v) => v,
                None if self.u => return err("Invalid Unicode escape"),
                None => ch,
            },
            '-' if self.u && class => ch,
            s if self.u => {
                if "^$\\.*+?()[]{}|/".contains(s) {
                    ch
                } else {
                    return err("Invalid escape");
                }
            }
            'k' if self.named => return err("Invalid named reference"),
            _ => ch,
        })
    }
    /// Case-closes a leaf set in v+i mode, where folding precedes complement.
    fn leaf(&self, mut set: CSet) -> CSet {
        if self.v && self.icase {
            set.r = close(set.r, true);
            for s in &mut set.s {
                for ch in s.iter_mut() {
                    *ch = canon(*ch, true);
                }
            }
        }
        set
    }
    fn set_node(&self, set: CSet, negate: bool) -> Node {
        let r = if self.icase && !self.v {
            close(set.r, self.u)
        } else {
            set.r
        };
        let mut strings = set.s;
        strings.sort_by(|a, b| b.len().cmp(&a.len()));
        let mut alts = vec![];
        let mut empty = false;
        for s in strings {
            if s.is_empty() {
                empty = true;
            } else {
                alts.push(Node::Seq(
                    s.into_iter().map(|ch| Node::Char(ch, self.icase)).collect(),
                ));
            }
        }
        if !r.is_empty() || negate || alts.is_empty() {
            alts.push(Node::Set(r, negate));
        }
        if empty {
            alts.push(Node::Empty);
        }
        if alts.len() == 1 {
            alts.pop().unwrap()
        } else {
            Node::Alt(alts)
        }
    }
    fn class(&mut self) -> R<Node> {
        let negate = self.eat('^');
        if self.v {
            let (set, may) = self.class_set_expression()?;
            if negate && may {
                return err("Negated character class may contain strings");
            }
            return Ok(self.set_node(set, negate));
        }
        let mut r: Ranges = vec![];
        let add = |r: &mut Ranges, a: Atom| match a {
            Atom::Char(x) => r.push((x, x)),
            Atom::Set(s) => r.extend(s),
        };
        loop {
            match self.peek() {
                None => return err("Unterminated character class"),
                Some(x) if x == c(']') => {
                    self.i += 1;
                    break;
                }
                _ => {}
            }
            let a = self.class_atom()?;
            if self.is('-') && self.at(1).is_some_and(|x| x != c(']')) {
                self.i += 1;
                let b = self.class_atom()?;
                match (a, b) {
                    (Atom::Char(x), Atom::Char(y)) => {
                        if x > y {
                            return err("Range out of order in character class");
                        }
                        r.push((x, y));
                    }
                    (a, b) => {
                        if self.u {
                            return err("Invalid character class");
                        }
                        add(&mut r, a);
                        r.push((c('-'), c('-')));
                        add(&mut r, b);
                    }
                }
            } else {
                add(&mut r, a);
            }
        }
        Ok(self.set_node(CSet::ranges(r), negate))
    }
    fn class_atom(&mut self) -> R<Atom> {
        let ch = self.peek().unwrap();
        self.i += 1;
        if ch != c('\\') {
            return Ok(Atom::Char(ch));
        }
        match self.peek() {
            None => return err("\\ at end of pattern"),
            Some(x) if x == c('b') => {
                self.i += 1;
                return Ok(Atom::Char(8));
            }
            Some(x) if x == c('-') && self.u => {
                self.i += 1;
                return Ok(Atom::Char(x));
            }
            _ => {}
        }
        if let Some(set) = self.class_escape()? {
            return Ok(Atom::Set(set.r));
        }
        Ok(Atom::Char(self.char_escape(true)?))
    }
    // ---- v-mode ClassSetExpression; consumes the closing `]`.
    fn class_set_expression(&mut self) -> R<(CSet, bool)> {
        if self.eat(']') {
            return Ok((CSet::default(), false));
        }
        let (mut acc, mut may, range) = self.class_set_item()?;
        if self.starts('&', '&') || self.starts('-', '-') {
            let and = self.starts('&', '&');
            if range {
                return err("Invalid set operation in character class");
            }
            while self.starts(if and { '&' } else { '-' }, if and { '&' } else { '-' }) {
                self.i += 2;
                if and && self.is('&') {
                    return err("Invalid character in character class");
                }
                let (op, m, range) = self.class_set_item()?;
                if range {
                    return err("Invalid set operation in character class");
                }
                if and {
                    acc = acc.intersect(op);
                    may = may && m;
                } else {
                    acc = acc.subtract(op);
                }
            }
            if !self.eat(']') {
                return err("Invalid set operation in character class");
            }
            return Ok((acc, may));
        }
        loop {
            if self.eat(']') {
                return Ok((acc, may));
            }
            if self.peek().is_none() {
                return err("Unterminated character class");
            }
            if self.starts('&', '&') || self.starts('-', '-') {
                return err("Invalid set operation in character class");
            }
            let (op, m, _) = self.class_set_item()?;
            acc = acc.union(op);
            may |= m;
        }
    }
    /// One ClassSetOperand or ClassSetRange: (set, may contain strings, is a range).
    fn class_set_item(&mut self) -> R<(CSet, bool, bool)> {
        match self.peek() {
            None => return err("Unterminated character class"),
            Some(x) if x == c('[') => {
                self.i += 1;
                let negate = self.eat('^');
                let (set, may) = self.class_set_expression()?;
                if negate {
                    if may {
                        return err("Negated character class may contain strings");
                    }
                    return Ok((self.leaf(CSet::ranges(complement(&set.r))), false, false));
                }
                return Ok((set, may, false));
            }
            Some(x) if x == c('\\') => {
                if self.at(1) == Some(c('q')) {
                    self.i += 2;
                    return self.class_strings().map(|(s, m)| (s, m, false));
                }
                self.i += 1;
                if let Some(set) = self.class_escape()? {
                    let may = !set.s.is_empty();
                    return Ok((set, may, false));
                }
                self.i -= 1;
            }
            _ => {}
        }
        let a = self.class_set_char()?;
        if self.is('-') && self.at(1) != Some(c('-')) {
            self.i += 1;
            let b = self.class_set_char()?;
            if a > b {
                return err("Range out of order in character class");
            }
            return Ok((self.leaf(CSet::ranges(vec![(a, b)])), false, true));
        }
        Ok((self.leaf(CSet::ranges(vec![(a, a)])), false, false))
    }
    fn class_set_char(&mut self) -> R<u32> {
        let Some(ch) = self.peek() else {
            return err("Unterminated character class");
        };
        let sc = char::from_u32(ch).unwrap_or('\0');
        if sc == '\\' {
            self.i += 1;
            match self.peek().and_then(char::from_u32) {
                Some(p) if "&-!#%,:;<=>@`~".contains(p) => {
                    self.i += 1;
                    return Ok(p as u32);
                }
                Some('b') => {
                    self.i += 1;
                    return Ok(8);
                }
                _ => return self.char_escape(true),
            }
        }
        if "()[]{}/-|".contains(sc) {
            return err("Invalid character in character class");
        }
        if "&!#$%*+,.:;<=>?@^`~".contains(sc) && self.at(1) == Some(ch) {
            return err("Invalid set operation in character class");
        }
        self.i += 1;
        Ok(ch)
    }
    /// `\q{a|bc}` after `\q`.
    fn class_strings(&mut self) -> R<(CSet, bool)> {
        if !self.eat('{') {
            return err("Invalid escape");
        }
        let mut strings: Vec<Vec<u32>> = vec![vec![]];
        loop {
            if self.eat('}') {
                break;
            }
            if self.eat('|') {
                strings.push(vec![]);
                continue;
            }
            let ch = self.class_set_char()?;
            strings.last_mut().unwrap().push(ch);
        }
        let mut set = CSet::default();
        let mut may = false;
        for s in strings {
            if s.len() == 1 {
                set.r.push((s[0], s[0]));
            } else {
                may = true;
                if !set.s.contains(&s) {
                    set.s.push(s);
                }
            }
        }
        set.r = norm(set.r);
        Ok((self.leaf(set), may))
    }
}

fn compatible(a: &[(usize, usize)], b: &[(usize, usize)]) -> bool {
    for (x, y) in a.iter().zip(b) {
        if x.0 != y.0 {
            return false;
        }
        if x.1 != y.1 {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------- bytecode
#[derive(Debug, Clone)]
enum M {
    Char(u32),
    CharI(u32),
    Any(bool),
    Set(usize, bool),
}
#[derive(Debug)]
enum Inst {
    One(M, bool),
    Bol(bool),
    Eol(bool),
    Word(bool, bool),
    Split(usize, usize),
    Jmp(usize),
    Save(usize),
    Clear(usize, usize),
    LoopInit(usize),
    Loop {
        r: usize,
        min: u32,
        max: u32,
        greedy: bool,
        exit: usize,
    },
    Mark(usize),
    LoopEnd {
        r: usize,
        min: u32,
        head: usize,
    },
    Star(M, u32, u32),
    BackRef(Vec<usize>, bool, bool),
    Look(bool, usize),
    LookEnd,
    Match,
}

#[derive(Debug)]
pub(super) struct Program {
    insts: Vec<Inst>,
    sets: Vec<Ranges>,
    regs: usize,
    unicode: bool,
    /// Capture count including group 0.
    pub groups: usize,
    /// Group names in order of first appearance, with their group indices.
    pub names: Vec<(String, Vec<usize>)>,
}

struct Compiler {
    insts: Vec<Inst>,
    sets: Vec<Ranges>,
    regs: usize,
    unicode: bool,
}
impl Compiler {
    fn push(&mut self, i: Inst) -> usize {
        self.insts.push(i);
        self.insts.len() - 1
    }
    fn matcher(&mut self, n: &Node) -> Option<M> {
        Some(match n {
            Node::Char(ch, false) => M::Char(*ch),
            Node::Char(ch, true) => M::CharI(canon(*ch, self.unicode)),
            Node::Any(d) => M::Any(*d),
            Node::Set(r, neg) => {
                self.sets.push(r.clone());
                M::Set(self.sets.len() - 1, *neg)
            }
            _ => return None,
        })
    }
    fn emit(&mut self, n: &Node, back: bool) {
        match n {
            Node::Empty => {}
            Node::Char(..) | Node::Any(_) | Node::Set(..) => {
                let m = self.matcher(n).unwrap();
                self.push(Inst::One(m, back));
            }
            Node::Bol(m) => {
                self.push(Inst::Bol(*m));
            }
            Node::Eol(m) => {
                self.push(Inst::Eol(*m));
            }
            Node::Word(neg, ui) => {
                self.push(Inst::Word(*neg, *ui));
            }
            Node::Group(body, None) => self.emit(body, back),
            Node::Group(body, Some(k)) => {
                let (a, b) = if back {
                    (2 * k + 1, 2 * k)
                } else {
                    (2 * k, 2 * k + 1)
                };
                self.push(Inst::Save(a));
                self.emit(body, back);
                self.push(Inst::Save(b));
            }
            Node::Look(behind, negate, body) => {
                let at = self.push(Inst::Look(*negate, 0));
                self.emit(body, *behind);
                self.push(Inst::LookEnd);
                let end = self.insts.len();
                self.insts[at] = Inst::Look(*negate, end);
            }
            Node::BackRef(g, i) => {
                self.push(Inst::BackRef(g.clone(), *i, back));
            }
            Node::NamedRef(..) => unreachable!("resolved before compiling"),
            Node::Seq(items) => {
                if back {
                    items.iter().rev().for_each(|x| self.emit(x, back));
                } else {
                    items.iter().for_each(|x| self.emit(x, back));
                }
            }
            Node::Alt(alts) => {
                let mut jumps = vec![];
                for (k, a) in alts.iter().enumerate() {
                    if k + 1 < alts.len() {
                        let s = self.push(Inst::Split(0, 0));
                        self.emit(a, back);
                        jumps.push(self.push(Inst::Jmp(0)));
                        self.insts[s] = Inst::Split(s + 1, self.insts.len());
                    } else {
                        self.emit(a, back);
                    }
                }
                let end = self.insts.len();
                for j in jumps {
                    self.insts[j] = Inst::Jmp(end);
                }
            }
            Node::Repeat(body, min, max, greedy, first, last) => {
                if *max == 0 {
                    return;
                }
                if *min == 1 && *max == 1 {
                    return self.emit(body, back);
                }
                if !back && *greedy {
                    if let Some(m) = self.matcher(body) {
                        self.push(Inst::Star(m, *min, *max));
                        return;
                    }
                }
                let r = self.regs;
                self.regs += 2;
                self.push(Inst::LoopInit(r));
                let head = self.push(Inst::Loop {
                    r,
                    min: *min,
                    max: *max,
                    greedy: *greedy,
                    exit: 0,
                });
                self.push(Inst::Mark(r));
                if last > first {
                    self.push(Inst::Clear(*first + 1, *last));
                }
                self.emit(body, back);
                self.push(Inst::LoopEnd { r, min: *min, head });
                let exit = self.insts.len();
                if let Inst::Loop { exit: e, .. } = &mut self.insts[head] {
                    *e = exit;
                }
            }
        }
    }
}

fn resolve(n: &mut Node, names: &[(String, Vec<usize>)]) -> R<()> {
    match n {
        Node::NamedRef(name, i) => match names.iter().find(|e| e.0 == *name) {
            Some(e) => *n = Node::BackRef(e.1.clone(), *i),
            None => return err("Invalid named capture referenced"),
        },
        Node::Group(b, _) | Node::Look(_, _, b) | Node::Repeat(b, ..) => resolve(b, names)?,
        Node::Alt(v) | Node::Seq(v) => {
            for x in v {
                resolve(x, names)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Parses and compiles `pattern`; the error is the SyntaxError message.
pub(super) fn compile(pattern: &str, flags: Flags) -> R<Program> {
    let unicode = flags.unicode();
    let s: Vec<u32> = if unicode {
        pattern.chars().map(|ch| ch as u32).collect()
    } else {
        pattern.encode_utf16().map(u32::from).collect()
    };
    let (total, named) = prescan(&s, flags.v);
    let mut p = Parser {
        s,
        i: 0,
        u: unicode,
        v: flags.v,
        named,
        total,
        ncap: 0,
        icase: flags.i,
        multi: flags.m,
        dotall: flags.s,
        names: vec![],
        name_paths: vec![],
        path: vec![],
        disj: 0,
    };
    let mut root = p.disjunction()?;
    if p.i < p.s.len() {
        return err("Unmatched ')'");
    }
    resolve(&mut root, &p.names)?;
    let mut c = Compiler {
        insts: vec![],
        sets: vec![],
        regs: 0,
        unicode,
    };
    c.emit(&Node::Group(Box::new(root), Some(0)), false);
    c.push(Inst::Match);
    Ok(Program {
        insts: c.insts,
        sets: c.sets,
        regs: c.regs,
        unicode,
        groups: p.ncap + 1,
        names: p.names,
    })
}

// ---------------------------------------------------------------- matcher
enum Frame {
    Alt(usize, usize),
    Cap(usize, usize),
    Reg(usize, usize),
    Star(usize, usize, usize),
    Look(bool, usize, usize),
}
/// Matching ran out of its backtracking budget.
#[derive(Debug)]
pub(super) struct Exhausted;
pub(super) type Captures = Vec<Option<(usize, usize)>>;

struct State {
    caps: Vec<usize>,
    regs: Vec<usize>,
    stack: Vec<Frame>,
}

impl Program {
    fn read(&self, inp: &[u16], pos: usize, back: bool) -> Option<(u32, usize)> {
        if back {
            if pos == 0 {
                return None;
            }
            let ch = inp[pos - 1] as u32;
            if self.unicode && low(ch) && pos >= 2 && high(inp[pos - 2] as u32) {
                return Some((pair(inp[pos - 2] as u32, ch), pos - 2));
            }
            Some((ch, pos - 1))
        } else {
            let ch = *inp.get(pos)? as u32;
            if self.unicode && high(ch) {
                if let Some(&l) = inp.get(pos + 1) {
                    if low(l as u32) {
                        return Some((pair(ch, l as u32), pos + 2));
                    }
                }
            }
            Some((ch, pos + 1))
        }
    }
    fn step(&self, inp: &[u16], pos: usize, m: &M, back: bool) -> Option<usize> {
        let (ch, next) = self.read(inp, pos, back)?;
        let ok = match m {
            M::Char(x) => ch == *x,
            M::CharI(x) => canon(ch, self.unicode) == *x,
            M::Any(dotall) => *dotall || !is_line_terminator(ch),
            M::Set(i, neg) => contains(&self.sets[*i], ch) != *neg,
        };
        ok.then_some(next)
    }
    fn backref(
        &self,
        inp: &[u16],
        pos: usize,
        a: usize,
        b: usize,
        icase: bool,
        back: bool,
    ) -> Option<usize> {
        let len = b - a;
        let (s, e) = if back {
            (pos.checked_sub(len)?, pos)
        } else {
            (pos, pos + len)
        };
        if e > inp.len() {
            return None;
        }
        let (x, y) = (&inp[a..b], &inp[s..e]);
        let same = if !icase {
            x == y
        } else {
            let (mut i, mut j) = (0, 0);
            let mut ok = true;
            while ok && i < x.len() {
                let (p, ni) = self.read(x, i, false).unwrap();
                match self.read(y, j, false) {
                    Some((q, nj)) if ni - i == nj - j => {
                        ok = canon(p, self.unicode) == canon(q, self.unicode);
                        i = ni;
                        j = nj;
                    }
                    _ => ok = false,
                }
            }
            ok
        };
        same.then_some(if back { s } else { e })
    }
    fn backtrack(&self, inp: &[u16], st: &mut State) -> Option<(usize, usize)> {
        loop {
            match st.stack.pop()? {
                Frame::Cap(slot, old) => st.caps[slot] = old,
                Frame::Reg(r, old) => st.regs[r] = old,
                Frame::Alt(pc, pos) => return Some((pc, pos)),
                Frame::Star(pc, pos, min_pos) => {
                    let mut np = pos - 1;
                    if self.unicode
                        && np > min_pos
                        && low(inp[np] as u32)
                        && high(inp[np - 1] as u32)
                    {
                        np -= 1;
                    }
                    if np > min_pos {
                        st.stack.push(Frame::Star(pc, np, min_pos));
                    }
                    return Some((pc, np));
                }
                Frame::Look(negate, end, pos) => {
                    if negate {
                        return Some((end, pos));
                    }
                }
            }
        }
    }
    fn run(
        &self,
        inp: &[u16],
        start: usize,
        st: &mut State,
        budget: &mut u64,
    ) -> Result<bool, Exhausted> {
        st.caps.fill(UNSET);
        st.stack.clear();
        let (mut pc, mut pos) = (0usize, start);
        loop {
            if *budget == 0 {
                return Err(Exhausted);
            }
            *budget -= 1;
            let ok = match &self.insts[pc] {
                Inst::One(m, back) => match self.step(inp, pos, m, *back) {
                    Some(p) => {
                        pos = p;
                        true
                    }
                    None => false,
                },
                Inst::Bol(multi) => pos == 0 || (*multi && is_line_terminator(inp[pos - 1] as u32)),
                Inst::Eol(multi) => {
                    pos == inp.len() || (*multi && is_line_terminator(inp[pos] as u32))
                }
                Inst::Word(neg, ui) => {
                    let a = pos > 0 && is_word(inp[pos - 1] as u32, *ui);
                    let b = pos < inp.len() && is_word(inp[pos] as u32, *ui);
                    (a != b) != *neg
                }
                Inst::Split(a, b) => {
                    st.stack.push(Frame::Alt(*b, pos));
                    pc = *a;
                    continue;
                }
                Inst::Jmp(a) => {
                    pc = *a;
                    continue;
                }
                Inst::Save(slot) => {
                    st.stack.push(Frame::Cap(*slot, st.caps[*slot]));
                    st.caps[*slot] = pos;
                    true
                }
                Inst::Clear(a, b) => {
                    for slot in 2 * a..2 * b + 2 {
                        if st.caps[slot] != UNSET {
                            st.stack.push(Frame::Cap(slot, st.caps[slot]));
                            st.caps[slot] = UNSET;
                        }
                    }
                    true
                }
                Inst::LoopInit(r) => {
                    st.stack.push(Frame::Reg(*r, st.regs[*r]));
                    st.regs[*r] = 0;
                    true
                }
                Inst::Loop {
                    r,
                    min,
                    max,
                    greedy,
                    exit,
                } => {
                    let count = st.regs[*r];
                    if *max != INF && count >= *max as usize {
                        pc = *exit;
                    } else if count < *min as usize {
                        pc += 1;
                    } else if *greedy {
                        st.stack.push(Frame::Alt(*exit, pos));
                        pc += 1;
                    } else {
                        st.stack.push(Frame::Alt(pc + 1, pos));
                        pc = *exit;
                    }
                    continue;
                }
                Inst::Mark(r) => {
                    st.stack.push(Frame::Reg(r + 1, st.regs[r + 1]));
                    st.regs[r + 1] = pos;
                    true
                }
                Inst::LoopEnd { r, min, head } => {
                    let count = st.regs[*r];
                    if count >= *min as usize && pos == st.regs[r + 1] {
                        false
                    } else {
                        st.stack.push(Frame::Reg(*r, count));
                        st.regs[*r] = count + 1;
                        pc = *head;
                        continue;
                    }
                }
                Inst::Star(m, min, max) => {
                    let (mut n, mut p) = (0u32, pos);
                    let mut min_pos = if *min == 0 { pos } else { UNSET };
                    while *max == INF || n < *max {
                        match self.step(inp, p, m, false) {
                            Some(q) => {
                                p = q;
                                n += 1;
                                if n == *min {
                                    min_pos = p;
                                }
                            }
                            None => break,
                        }
                        if *budget == 0 {
                            return Err(Exhausted);
                        }
                        *budget -= 1;
                    }
                    if n < *min {
                        false
                    } else {
                        if p > min_pos {
                            st.stack.push(Frame::Star(pc + 1, p, min_pos));
                        }
                        pos = p;
                        true
                    }
                }
                Inst::BackRef(groups, icase, back) => {
                    match groups
                        .iter()
                        .find(|&&g| st.caps[2 * g] != UNSET && st.caps[2 * g + 1] != UNSET)
                    {
                        None => true,
                        Some(&g) => {
                            match self.backref(
                                inp,
                                pos,
                                st.caps[2 * g],
                                st.caps[2 * g + 1],
                                *icase,
                                *back,
                            ) {
                                Some(p) => {
                                    pos = p;
                                    true
                                }
                                None => false,
                            }
                        }
                    }
                }
                Inst::Look(negate, end) => {
                    st.stack.push(Frame::Look(*negate, *end, pos));
                    true
                }
                Inst::LookEnd => {
                    let idx = st
                        .stack
                        .iter()
                        .rposition(|f| matches!(f, Frame::Look(..)))
                        .expect("lookaround frame");
                    let Frame::Look(negate, end, lpos) = st.stack[idx] else {
                        unreachable!()
                    };
                    if negate {
                        while st.stack.len() > idx {
                            match st.stack.pop().unwrap() {
                                Frame::Cap(slot, old) => st.caps[slot] = old,
                                Frame::Reg(r, old) => st.regs[r] = old,
                                _ => {}
                            }
                        }
                        false
                    } else {
                        let undo: Vec<Frame> = st
                            .stack
                            .drain(idx..)
                            .filter(|f| matches!(f, Frame::Cap(..) | Frame::Reg(..)))
                            .collect();
                        st.stack.extend(undo);
                        pos = lpos;
                        pc = end;
                        continue;
                    }
                }
                Inst::Match => return Ok(true),
            };
            if ok {
                pc += 1;
            } else {
                match self.backtrack(inp, st) {
                    Some((p, q)) => {
                        pc = p;
                        pos = q;
                    }
                    None => return Ok(false),
                }
            }
        }
    }
    /// Runs the matcher from `start` (only there when `sticky`), advancing by code
    /// point in Unicode mode. Returns the captures of the first match.
    pub(super) fn exec(
        &self,
        inp: &[u16],
        mut start: usize,
        sticky: bool,
    ) -> Result<Option<Captures>, Exhausted> {
        let mut budget: u64 = 50_000_000;
        let mut st = State {
            caps: vec![UNSET; 2 * self.groups],
            regs: vec![0; self.regs],
            stack: vec![],
        };
        loop {
            if start > inp.len() {
                return Ok(None);
            }
            if self.run(inp, start, &mut st, &mut budget)? {
                return Ok(Some(
                    st.caps
                        .chunks(2)
                        .map(|p| (p[0] != UNSET && p[1] != UNSET).then(|| (p[0], p[1])))
                        .collect(),
                ));
            }
            if sticky {
                return Ok(None);
            }
            start = advance(inp, start, self.unicode);
        }
    }
}
/// AdvanceStringIndex.
pub(super) fn advance(inp: &[u16], i: usize, unicode: bool) -> usize {
    if unicode && i + 1 < inp.len() && high(inp[i] as u32) && low(inp[i + 1] as u32) {
        i + 2
    } else {
        i + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn m(p: &str, f: &str, s: &str) -> Option<Vec<Option<String>>> {
        let prog = compile(p, Flags::parse(f).unwrap()).unwrap();
        let u: Vec<u16> = s.encode_utf16().collect();
        prog.exec(&u, 0, false).unwrap().map(|c| {
            c.into_iter()
                .map(|x| x.map(|(a, b)| String::from_utf16(&u[a..b]).unwrap()))
                .collect()
        })
    }
    fn first(p: &str, f: &str, s: &str) -> Option<String> {
        m(p, f, s).and_then(|v| v[0].clone())
    }
    #[test]
    fn basics() {
        assert_eq!(first("b+", "", "abbbc").as_deref(), Some("bbb"));
        assert_eq!(first("a|ab", "", "abc").as_deref(), Some("a"));
        assert_eq!(
            first("(a|ab)(c|bcd)(d*)", "", "abcd").as_deref(),
            Some("abcd")
        );
        assert_eq!(m("(z)((a+)?(b+)?(c))*", "", "zaacbbbcac").unwrap()[4], None);
        assert_eq!(
            first("(?=(a+))a*b\\1", "", "baaabac").as_deref(),
            Some("aba")
        );
        assert_eq!(
            first("(.*?)a(?!(a+)b\\2c)\\2(.*)", "", "baaabaac").as_deref(),
            Some("baaabaac")
        );
        assert_eq!(
            first("(?<=\\$)\\d+(\\.\\d*)?", "", "cost $10.53").as_deref(),
            Some("10.53")
        );
        assert_eq!(first("(?<=(\\d+)(\\d+))$", "", "1053").as_deref(), Some(""));
        assert_eq!(
            m("(?<=(\\d+)(\\d+))$", "", "1053").unwrap()[1].as_deref(),
            Some("1")
        );
        assert_eq!(first("[^]", "", "\n").as_deref(), Some("\n"));
        assert_eq!(first("ABC", "i", "xabc").as_deref(), Some("abc"));
        assert_eq!(first("\\u{1F600}", "u", "x😀").as_deref(), Some("😀"));
        assert_eq!(first("^.$", "u", "😀").as_deref(), Some("😀"));
        assert_eq!(first("^.$", "", "😀"), None);
        assert_eq!(first("^\\p{Lu}+$", "u", "ABÇ").as_deref(), Some("ABÇ"));
        assert_eq!(
            first("[\\p{L}--[a-z]]+", "v", "abCDe").as_deref(),
            Some("CD")
        );
        assert_eq!(first("^\\p{RGI_Emoji}$", "v", "👍🏽").as_deref(), Some("👍🏽"));
        assert_eq!(first("(?<a>x)|(?<a>y)", "", "y").as_deref(), Some("y"));
        assert_eq!(first("(?i:a)b", "", "Ab").as_deref(), Some("Ab"));
        assert_eq!(first("(?i:a)b", "", "AB"), None);
        assert_eq!(first("\\bfoo\\b", "", "a foo b").as_deref(), Some("foo"));
        assert_eq!(first("a{2,3}", "", "aaaa").as_deref(), Some("aaa"));
        assert_eq!(first("a{,3}", "", "a{,3}").as_deref(), Some("a{,3}"));
        assert_eq!(first("\\8", "", "8").as_deref(), Some("8"));
        assert_eq!(first("[\\d-z]+", "", "1-z").as_deref(), Some("1-z"));
        assert_eq!(first("\\w", "iu", "\u{17F}").as_deref(), Some("\u{17F}"));
        assert_eq!(first("(a*)*b", "", "aaaab").as_deref(), Some("aaaab"));
        assert_eq!(first("(?:a|())*?b", "", "aab").as_deref(), Some("aab"));
        assert_eq!(m("(a)|b", "", "b").unwrap()[1], None);
        assert_eq!(first("\\cJ", "", "\n").as_deref(), Some("\n"));
        assert_eq!(first("\\c1", "", "\\c1").as_deref(), Some("\\c1"));
    }
    #[test]
    fn errors() {
        for (p, f) in [
            ("a**", ""),
            ("(", ""),
            (")", ""),
            ("[b-a]", ""),
            ("a{2,1}", ""),
            ("\\p{Foo}", "u"),
            ("(?<a>x)(?<a>y)", ""),
            ("\\k<b>(?<a>x)", ""),
            ("{", "u"),
            ("(?<=a)*", ""),
            ("[^\\p{RGI_Emoji}]", "v"),
            ("[a&&&b]", "v"),
            ("(?ii:a)", ""),
            ("(?-:a)", ""),
            ("\\1", "u"),
            ("[\\d-a]", "u"),
        ] {
            assert!(compile(p, Flags::parse(f).unwrap()).is_err(), "{p} /{f}");
        }
        assert!(compile("\\k", Flags::default()).is_ok());
    }
}

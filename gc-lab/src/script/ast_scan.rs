//! Conservative syntactic checks over the AST, used by the fast paths of BYTECODE_VM.md
//! and OBJECT_MODEL.md until a full scope-analysis pass exists.
//!
//! `Walk` visits every node and every string of a subtree in source order. The
//! matches are exhaustive, with no `..` rest patterns, so a new AST field or variant
//! fails to compile until the walker covers it: a check never misses a subtree. The
//! queries answer "yes" on any doubt (a string literal equal to a local's name counts
//! as a mention), and a false positive only keeps the slow, spec-literal path.
//!
//! This replaces scans over the derived `Debug` text, which formatted the whole subtree
//! for every query. Those cost about 8% of a small script's instructions on the Servo
//! path, and the cost grew with the script's size.
use super::parser::{Class, ClassInit, ClassKey, ClassMember, Expr, MemberKind, Param};
use super::parser::{Property, Stmt};
use super::patterns::{Pattern, PatternKey};
use super::templates::Template;
use std::collections::HashSet;

/// A visited node. `Str` is any string the AST holds: identifier names, string
/// literals, property keys, operators, binding kinds, template parts and labels.
#[derive(Clone, Copy)]
pub(super) enum Node<'a> {
    Expr(&'a Expr),
    Stmt(&'a Stmt),
    Pattern(&'a Pattern),
    Property(&'a Property),
    Member(&'a MemberKind),
    Str(&'a str),
}

/// What the visitor wants next.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Flow {
    Continue,
    /// Do not descend into this node's children.
    Skip,
    Stop,
}

type Visitor<'v> = dyn FnMut(Node<'_>) -> Flow + 'v;

pub(super) trait Walk {
    /// Visits `self` and its subtree; true when the visitor stopped the walk.
    fn walk(&self, v: &mut Visitor<'_>) -> bool;
}

/// Visits `node`; returns `Some(stopped)` when its children are not to be walked.
fn enter(node: Node<'_>, v: &mut Visitor<'_>) -> Option<bool> {
    match v(node) {
        Flow::Continue => None,
        Flow::Skip => Some(false),
        Flow::Stop => Some(true),
    }
}

impl Walk for str {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        v(Node::Str(self)) == Flow::Stop
    }
}
impl Walk for String {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        self.as_str().walk(v)
    }
}
impl<T: Walk + ?Sized> Walk for &T {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        (**self).walk(v)
    }
}
impl<T: Walk + ?Sized> Walk for Box<T> {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        (**self).walk(v)
    }
}
impl<T: Walk> Walk for [T] {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        self.iter().any(|item| item.walk(v))
    }
}
impl<T: Walk> Walk for Vec<T> {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        self.as_slice().walk(v)
    }
}
impl<T: Walk> Walk for Option<T> {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        self.as_ref().is_some_and(|item| item.walk(v))
    }
}
impl<A: Walk, B: Walk> Walk for (A, B) {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        self.0.walk(v) || self.1.walk(v)
    }
}

impl Walk for Expr {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        if let Some(stopped) = enter(Node::Expr(self), v) {
            return stopped;
        }
        match self {
            Expr::Template(t) => t.walk(v),
            Expr::Name(s)
            | Expr::String(s)
            | Expr::Number(s)
            | Expr::BigInt(s)
            | Expr::PrivateName(s)
            | Expr::SuperMember(s) => s.walk(v),
            Expr::RegExp { pattern, flags } => pattern.walk(v) || flags.walk(v),
            Expr::Units(_)
            | Expr::Bool(_)
            | Expr::Null
            | Expr::Undefined
            | Expr::This
            | Expr::NewTarget
            | Expr::ImportMeta => false,
            Expr::Arrow { params, body } | Expr::AsyncArrow { params, body } => {
                params.walk(v) || body.walk(v)
            }
            Expr::Function {
                name,
                params,
                body,
                method: _,
            }
            | Expr::AsyncFunction {
                name,
                params,
                body,
                method: _,
            }
            | Expr::GeneratorFunction {
                name,
                params,
                body,
                method: _,
                asynchronous: _,
            } => name.walk(v) || params.walk(v) || body.walk(v),
            Expr::Yield(e, _) => e.walk(v),
            Expr::Await(e)
            | Expr::Spread(e)
            | Expr::SuperComputed(e)
            | Expr::OptionalChain(e)
            | Expr::Optional(e)
            | Expr::Update(e, _, _) => e.walk(v),
            Expr::Sequence(items) | Expr::SuperCall(items) => items.walk(v),
            Expr::Object(properties) => properties.walk(v),
            Expr::Array(items) => items.walk(v),
            Expr::Member(e, s) => e.walk(v) || s.walk(v),
            Expr::Computed(a, b) | Expr::Assign(a, b) => a.walk(v) || b.walk(v),
            Expr::Call(callee, args) | Expr::New(callee, args) => callee.walk(v) || args.walk(v),
            Expr::Unary(op, e) => op.walk(v) || e.walk(v),
            Expr::Binary(op, a, b) | Expr::Compound(op, a, b) => {
                op.walk(v) || a.walk(v) || b.walk(v)
            }
            Expr::Destructure(pattern, e) => pattern.walk(v) || e.walk(v),
            Expr::Conditional(a, b, c) => a.walk(v) || b.walk(v) || c.walk(v),
            Expr::Class(class) => class.walk(v),
            Expr::ClassInit(init) => init.walk(v),
            Expr::ImportCall(specifier, options, _) => specifier.walk(v) || options.walk(v),
        }
    }
}

impl Walk for Template {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        let Template {
            site: _,
            parts,
            values,
            tag,
        } = self;
        tag.walk(v) || parts.walk(v) || values.walk(v)
    }
}

impl Walk for Param {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        let Param {
            binding,
            default,
            rest: _,
        } = self;
        binding.walk(v) || default.walk(v)
    }
}

impl Walk for Pattern {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        if let Some(stopped) = enter(Node::Pattern(self), v) {
            return stopped;
        }
        match self {
            Pattern::Name(name) => name.walk(v),
            Pattern::Target(e) => e.walk(v),
            Pattern::Array(items, rest) => items.walk(v) || rest.walk(v),
            Pattern::Object(properties, rest) => properties.walk(v) || rest.walk(v),
            Pattern::Default(pattern, e) => pattern.walk(v) || e.walk(v),
        }
    }
}

impl Walk for PatternKey {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        match self {
            PatternKey::Static(s) => s.walk(v),
            PatternKey::Computed(e) => e.walk(v),
        }
    }
}

impl Walk for Property {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        if let Some(stopped) = enter(Node::Property(self), v) {
            return stopped;
        }
        match self {
            Property::CoverDefault(name, e) => name.walk(v) || e.walk(v),
            Property::Computed(key, value) => key.walk(v) || value.walk(v),
            Property::Spread(e) | Property::Value(e) | Property::Prototype(e) => e.walk(v),
            Property::Getter(body) => body.walk(v),
            Property::Setter(param, body) => param.walk(v) || body.walk(v),
        }
    }
}

impl Walk for Class {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        let Class {
            name,
            heritage,
            constructor,
            members,
        } = self;
        name.walk(v) || heritage.walk(v) || constructor.walk(v) || members.walk(v)
    }
}

impl Walk for ClassMember {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        let ClassMember {
            is_static: _,
            key,
            kind,
        } = self;
        key.walk(v) || kind.walk(v)
    }
}

impl Walk for ClassKey {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        match self {
            ClassKey::Named(s) | ClassKey::Private(s) => s.walk(v),
            ClassKey::Computed(e) => e.walk(v),
        }
    }
}

impl Walk for MemberKind {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        if let Some(stopped) = enter(Node::Member(self), v) {
            return stopped;
        }
        match self {
            MemberKind::Method(e) => e.walk(v),
            MemberKind::Getter(body) | MemberKind::Block(body) => body.walk(v),
            MemberKind::Setter(param, body) => param.walk(v) || body.walk(v),
            MemberKind::Field(init) => init.walk(v),
        }
    }
}

impl Walk for ClassInit {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        match self {
            ClassInit::Field(key, init) => key.walk(v) || init.walk(v),
            ClassInit::PrivateMethod(name) => name.walk(v),
        }
    }
}

impl Walk for Stmt {
    fn walk(&self, v: &mut Visitor<'_>) -> bool {
        if let Some(stopped) = enter(Node::Stmt(self), v) {
            return stopped;
        }
        match self {
            Stmt::Empty | Stmt::Sloppy | Stmt::Continue => false,
            Stmt::With(e, body) => e.walk(v) || body.walk(v),
            Stmt::Declarations(items) | Stmt::Block(items) => items.walk(v),
            Stmt::Bind {
                kind,
                pattern,
                init,
            } => kind.walk(v) || pattern.walk(v) || init.walk(v),
            Stmt::Switch(e, cases) => e.walk(v) || cases.walk(v),
            Stmt::While {
                test,
                body,
                post: _,
            } => test.walk(v) || body.walk(v),
            Stmt::For {
                init,
                test,
                update,
                body,
            } => init.walk(v) || test.walk(v) || update.walk(v) || body.walk(v),
            Stmt::Const(name, e) | Stmt::Let(name, e) => name.walk(v) || e.walk(v),
            Stmt::Var(name, init) => name.walk(v) || init.walk(v),
            Stmt::Function(name, params, body)
            | Stmt::GeneratorFunction(name, params, body, _)
            | Stmt::AsyncFunction(name, params, body) => {
                name.walk(v) || params.walk(v) || body.walk(v)
            }
            Stmt::Set(object, key, value) => object.walk(v) || key.walk(v) || value.walk(v),
            Stmt::Expr(e) | Stmt::Return(e) | Stmt::Throw(e) => e.walk(v),
            Stmt::If(test, then, otherwise) => test.walk(v) || then.walk(v) || otherwise.walk(v),
            Stmt::Break(label) => label.walk(v),
            Stmt::Labeled(label, body) => label.walk(v) || body.walk(v),
            Stmt::ForIn {
                kind,
                binding,
                iterable,
                body,
            }
            | Stmt::ForOf {
                kind,
                binding,
                iterable,
                body,
                asynchronous: _,
            } => kind.walk(v) || binding.walk(v) || iterable.walk(v) || body.walk(v),
            Stmt::Try {
                body,
                catch,
                finally,
            } => body.walk(v) || catch.walk(v) || finally.walk(v),
        }
    }
}

/// Whether any node of `items` satisfies `pred`.
pub(super) fn any(items: &[&dyn Walk], mut pred: impl FnMut(Node<'_>) -> bool) -> bool {
    let mut visit = |node: Node<'_>| {
        if pred(node) {
            Flow::Stop
        } else {
            Flow::Continue
        }
    };
    items.iter().any(|item| item.walk(&mut visit))
}

/// Whether any string in `items` satisfies `pred`.
pub(super) fn any_str(items: &[&dyn Walk], mut pred: impl FnMut(&str) -> bool) -> bool {
    any(items, |node| matches!(node, Node::Str(s) if pred(s)))
}

/// Whether any string in `items` is one of `names`.
pub(super) fn mentions_any(items: &[&dyn Walk], names: &HashSet<String>) -> bool {
    !names.is_empty() && any_str(items, |s| names.contains(s))
}

/// Whether any string in `items` is `name`.
pub(super) fn mentions(items: &[&dyn Walk], name: &str) -> bool {
    any_str(items, |s| s == name)
}

/// Whether `node` names `name` as an identifier reference or a binding.
pub(super) fn is_name(node: Node<'_>, name: &str) -> bool {
    matches!(node, Node::Expr(Expr::Name(n)) | Node::Pattern(Pattern::Name(n)) if n == name)
}

/// Whether `node` begins a function body: an arrow, function, class, accessor or class
/// element. A name mentioned anywhere inside one may be captured by a closure.
pub(super) fn is_closure(node: Node<'_>) -> bool {
    matches!(
        node,
        Node::Expr(
            Expr::Arrow { .. }
                | Expr::AsyncArrow { .. }
                | Expr::Function { .. }
                | Expr::AsyncFunction { .. }
                | Expr::GeneratorFunction { .. }
                | Expr::Class(_)
                | Expr::ClassInit(_)
        ) | Node::Stmt(Stmt::Function(..) | Stmt::GeneratorFunction(..) | Stmt::AsyncFunction(..))
            | Node::Property(Property::Getter(_) | Property::Setter(..))
            | Node::Member(_)
    )
}

/// Whether `items` hold syntax that can create a closure or run a direct eval, so
/// that the current scope's bindings may be captured.
pub(super) fn may_capture(items: &[&dyn Walk]) -> bool {
    any(items, |node| is_closure(node) || is_name(node, "eval"))
}

/// The names of `names` mentioned inside a closure subtree of `items` (see
/// `is_closure`). A false positive only moves a binding from a register to its
/// environment record.
pub(super) fn captured_names(items: &dyn Walk, names: &[String]) -> Vec<String> {
    if names.is_empty() {
        return vec![];
    }
    let wanted: HashSet<&str> = names.iter().map(String::as_str).collect();
    let mut found: HashSet<&str> = HashSet::new();
    let mut visit = |node: Node<'_>| {
        if !is_closure(node) {
            return Flow::Continue;
        }
        let mut inside = |inner: Node<'_>| {
            if let Node::Str(s) = inner {
                if let Some(name) = wanted.get(s) {
                    found.insert(name);
                }
            }
            Flow::Continue
        };
        match node {
            Node::Expr(e) => e.walk(&mut inside),
            Node::Stmt(s) => s.walk(&mut inside),
            Node::Property(p) => p.walk(&mut inside),
            Node::Member(m) => m.walk(&mut inside),
            Node::Pattern(_) | Node::Str(_) => false,
        };
        Flow::Skip
    };
    items.walk(&mut visit);
    names
        .iter()
        .filter(|name| found.contains(name.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::parser::{Expr, Param, Property, Stmt};
    use std::collections::HashSet;

    fn name(s: &str) -> Expr {
        Expr::Name(s.into())
    }

    #[test]
    fn mentions_sees_every_string() {
        let body = vec![Stmt::Expr(Expr::Call(
            Box::new(Expr::Member(Box::new(name("o")), "p".into())),
            vec![Expr::String("q".into())],
        ))];
        for s in ["o", "p", "q"] {
            assert!(super::mentions(&[&body], s));
        }
        assert!(!super::mentions(&[&body], "eval"));
        let set: HashSet<String> = ["x".to_string(), "q".to_string()].into();
        assert!(super::mentions_any(&[&body], &set));
    }

    #[test]
    fn captured_names_only_inside_closures() {
        let names: Vec<String> = ["a", "b", "c", "Function"].map(String::from).to_vec();
        let body = vec![
            Stmt::Expr(name("a")),
            Stmt::Expr(Expr::String("Function(\"b\")".into())),
            Stmt::Expr(Expr::Arrow {
                params: vec![],
                body: vec![Stmt::Return(name("b"))],
            }),
            Stmt::Let("Function".into(), Expr::Null),
            Stmt::Expr(Expr::Object(vec![(
                "k".into(),
                Property::Setter(Param::from("v".to_string()), vec![Stmt::Expr(name("c"))]),
            )])),
        ];
        assert_eq!(
            super::captured_names(&body, &names),
            vec!["b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn may_capture_finds_closures_and_direct_eval() {
        let plain = vec![Stmt::Expr(Expr::Binary(
            "+".into(),
            Box::new(name("Function")),
            Box::new(Expr::String("Arrow".into())),
        ))];
        assert!(!super::may_capture(&[&plain]));
        let eval = vec![Stmt::Expr(Expr::Call(Box::new(name("eval")), vec![]))];
        assert!(super::may_capture(&[&eval]));
        let decl = vec![Stmt::Function("f".into(), vec![], vec![])];
        assert!(super::may_capture(&[&decl]));
    }
}

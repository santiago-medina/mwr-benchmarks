//! Decorators and auto-accessors: ClassDefinitionEvaluation as changed by tc39/ecma262
//! PR #2417 «Add Class and Class Element Decorators and `accessor` Keyword». Classes that
//! use neither decorators nor `accessor` keep `eval_class` (classes.rs); the reasons and
//! the readings of defective PR steps are in DECORATORS.md.
use super::{
    objects::{Descriptor, Key, Kind, Property},
    parser::{Class, ClassInit, ClassKey, Expr, MemberKind, Stmt},
    promises::{Cell, Internal},
    vm::{Engine, FunctionKind, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;

/// The abstract closures of CreateDecoratorAccessObject, CreateAddInitializerFunction and
/// MakeAutoAccessorGetter / MakeAutoAccessorSetter, as builtin function actions.
#[derive(Clone)]
pub(super) enum Action {
    /// `context.addInitializer`: append to array `list` unless `state` (a
    /// `Cell::Called` object, true once decoration finished) says otherwise.
    AddInitializer {
        state: Id,
        list: Id,
    },
    /// `context.access.get` / `.set` / `.has` for a property key or Private Name.
    AccessGet(Value),
    AccessSet(Value),
    AccessHas(Value),
    /// The auto-accessor getter / setter over the storage Private Name.
    AutoGet(u64),
    AutoSet(u64),
}
impl Action {
    pub fn edges(&self) -> Vec<Id> {
        match self {
            Self::AddInitializer { state, list } => vec![*state, *list],
            Self::AccessGet(key) | Self::AccessSet(key) | Self::AccessHas(key) => {
                key.id().into_iter().collect()
            }
            Self::AutoGet(_) | Self::AutoSet(_) => vec![],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ElementKind {
    Field,
    Method,
    Getter,
    Setter,
    Accessor,
}
impl ElementKind {
    fn name(self) -> &'static str {
        match self {
            Self::Field => "field",
            Self::Method => "method",
            Self::Getter => "getter",
            Self::Setter => "setter",
            Self::Accessor => "accessor",
        }
    }
}

/// A ClassElementDefinition Record. Every Value in it is protected for the duration of
/// the class evaluation; instance field and accessor records are copied into the class
/// scope (`%e{n}`) for InitializeInstanceElements.
struct Element {
    kind: ElementKind,
    is_static: bool,
    key: Key,
    /// The PrivateIdentifier ("#x") when the key is a Private Name.
    private: Option<String>,
    value: Value,
    get: Value,
    set: Value,
    /// [[BackingStorageKey]] of an auto-accessor.
    storage: Option<u64>,
    /// [[Initializers]]: the element's own initializer first, then the decorator
    /// initializers (DECORATORS.md, reading 1); `own` counts the former.
    initializers: Vec<Value>,
    own: usize,
    /// [[ExtraInitializers]]: an array the `addInitializer` builtins append to.
    extra: Id,
    /// [[Decorators]]: (decorator, receiver), last-to-first as DecoratorListEvaluation
    /// prepends.
    decorators: Vec<(Value, Value)>,
}

enum Record<'a> {
    Element(Element),
    Block(&'a [Stmt]),
}

impl Engine<'_> {
    /// DecoratorListEvaluation: each Decorator in source order, prepended to the list.
    /// DecoratorEvaluation's [[Receiver]] is the base of a property reference (EvaluateCall
    /// `this`), undefined for a DecoratorCallExpression or any other expression.
    fn decorator_list(&mut self, list: &[Expr], env: Id) -> Result<Vec<(Value, Value)>> {
        let mut out = vec![];
        for decorator in list {
            let (receiver, function) = match decorator {
                Expr::Call(..) => (Value::Undefined, self.eval(decorator, env)?),
                _ => self.call_target(decorator, env)?,
            };
            self.protect(&receiver)?;
            self.protect(&function)?;
            out.insert(0, (function, receiver));
        }
        Ok(out)
    }
    fn private_symbol(&mut self, description: String) -> Result<u64> {
        let Value::Symbol(id) = self.new_symbol(Some(description))? else {
            return Err(ScriptError::Runtime("private name allocation"));
        };
        self.private_symbols.insert(id);
        Ok(id)
    }
    fn named_builtin(&mut self, action: Action, length: usize, name: &str) -> Result<Value> {
        let f = self.internal_function(Internal::Decorator(action), length)?;
        self.protect(&f)?;
        if let Value::Object(id) = f {
            self.define(
                id,
                "name",
                Descriptor {
                    value: Some(Value::String(name.into())),
                    ..Descriptor::default()
                },
            )?;
        }
        Ok(f)
    }
    fn plain_object(&mut self) -> Result<Id> {
        let id = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
        self.protect(&Value::Object(id))?;
        Ok(id)
    }
    fn list_values(&mut self, list: Id) -> Result<Vec<Value>> {
        let length = self.length_of(list)?;
        let mut values = vec![];
        for i in 0..length {
            let v = self.get_member(Value::Object(list), i.to_string())?;
            self.protect(&v)?;
            values.push(v);
        }
        Ok(values)
    }
    fn new_list(&mut self) -> Result<Id> {
        let id = self.new_array(0)?;
        self.protect(&Value::Object(id))?;
        Ok(id)
    }
    /// The value of a key in a context object's `name` and the access builtins: the
    /// string, the Symbol, or the Private Name's symbol.
    fn key_name_value(&self, key: &Key) -> Value {
        match key {
            Key::String(s) => Value::String(s.clone().into()),
            Key::Symbol(id) => Value::Symbol(*id),
        }
    }
    /// CreateDecoratorContextObject.
    fn decorator_context(
        &mut self,
        kind: Option<ElementKind>,
        key: &Key,
        private: Option<&str>,
        extra: Id,
        state: Id,
        is_static: Option<bool>,
    ) -> Result<Value> {
        let context = self.plain_object()?;
        let kind_name = kind.map_or("class", ElementKind::name);
        self.put_data(context, "kind", Value::String(kind_name.into()))?;
        if let Some(kind) = kind {
            let access = self.decorator_access(kind, key)?;
            self.put_data(context, "access", access)?;
        }
        if let Some(is_static) = is_static {
            self.put_data(context, "static", Value::Bool(is_static))?;
        }
        if kind.is_some() {
            self.put_data(context, "private", Value::Bool(private.is_some()))?;
        }
        let name = match private {
            Some(n) => Value::String(n.into()),
            None => self.key_name_value(key),
        };
        self.put_data(context, "name", name)?;
        let add = self.named_builtin(
            Action::AddInitializer { state, list: extra },
            1,
            "addInitializer",
        )?;
        self.put_data(context, "addInitializer", add)?;
        Ok(Value::Object(context))
    }
    /// CreateDecoratorAccessObject.
    fn decorator_access(&mut self, kind: ElementKind, key: &Key) -> Result<Value> {
        let access = self.plain_object()?;
        let key = self.key_name_value(key);
        use ElementKind::*;
        if matches!(kind, Field | Method | Accessor | Getter) {
            let get = self.named_builtin(Action::AccessGet(key.clone()), 1, "")?;
            self.put_data(access, "get", get)?;
        }
        if matches!(kind, Field | Accessor | Setter) {
            let set = self.named_builtin(Action::AccessSet(key.clone()), 2, "")?;
            self.put_data(access, "set", set)?;
        }
        let has = self.named_builtin(Action::AccessHas(key), 1, "has")?;
        self.put_data(access, "has", has)?;
        Ok(Value::Object(access))
    }
    pub(super) fn decorator_internal(
        &mut self,
        action: Action,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        match action {
            Action::AddInitializer { state, list } => {
                if matches!(self.data(state)?.cell, Some(Cell::Called(true))) {
                    return self.throw_error(
                        "TypeError",
                        "addInitializer called after decoration finished",
                    );
                }
                let initializer = arg(0);
                if !self.callable(&initializer) {
                    return self.throw_error("TypeError", "initializer is not a function");
                }
                let length = self.length_of(list)?;
                self.put_data(list, length.to_string(), initializer)?;
                Ok(Value::Undefined)
            }
            Action::AccessGet(key) | Action::AccessSet(key) | Action::AccessHas(key)
                if !matches!(arg(0), Value::Object(_)) =>
            {
                let _ = key;
                self.throw_error("TypeError", "decorator access target is not an object")
            }
            Action::AccessGet(key) => {
                let key = self.to_key(key)?;
                self.get_member(arg(0), key)
            }
            Action::AccessSet(key) => {
                let key = self.to_key(key)?;
                self.set_member(arg(0), key, arg(1))?;
                Ok(Value::Undefined)
            }
            Action::AccessHas(key) => {
                let key = self.to_key(key)?;
                let Value::Object(id) = arg(0) else {
                    unreachable!()
                };
                if self.is_private(&key) {
                    // PrivateElementFind.
                    let present = match self.objects.get(&id) {
                        Some(
                            super::vm::Object::Environment { .. } | super::vm::Object::Node { .. },
                        )
                        | None => false,
                        _ => self.data(id)?.own(&key).is_some(),
                    };
                    Ok(Value::Bool(present))
                } else {
                    Ok(Value::Bool(self.has(id, key)?))
                }
            }
            Action::AutoGet(storage) => self.private_get(receiver, storage),
            Action::AutoSet(storage) => {
                self.private_set(receiver, storage, arg(0))?;
                Ok(Value::Undefined)
            }
        }
    }
    /// CreateFieldInitializerFunction: a method whose body evaluates the Initializer with
    /// NamedEvaluation by the field's name and whose environment marks it as a field
    /// initializer (§19.2.1.2 `arguments` early error for direct eval).
    fn field_initializer(&mut self, env: Id, name: &str, init: &Expr) -> Result<Value> {
        let fenv = self.field_initializer_env(env)?;
        let body = vec![Stmt::Return(Expr::ClassInit(Box::new(ClassInit::Value(
            name.into(),
            init.clone(),
        ))))];
        let f = self.make_function(fenv, None, &[], &body, FunctionKind::Method, false)?;
        self.protect(&f)?;
        Ok(f)
    }
    /// ClassDefinitionEvaluation as amended by PR #2417, for a class with decorators or
    /// auto-accessors. The class decorators are evaluated first, in the outer environment.
    pub(super) fn eval_decorated_class(
        &mut self,
        class: &Class,
        env: Id,
        inferred: Option<&str>,
    ) -> Result<Value> {
        let class_decorators = self.decorator_list(&class.decorators, env)?;
        let (scope, proto, f, fid, static_scope) = self.class_prologue(class, env, inferred)?;
        // ClassElementEvaluation of each element, in order: its decorators, then its key,
        // then its definition record.
        let mut records: Vec<Record> = vec![];
        for member in &class.members {
            self.tick()?;
            if let MemberKind::Block(body) = &member.kind {
                records.push(Record::Block(body));
                continue;
            }
            let menv = if member.is_static {
                static_scope
            } else {
                scope
            };
            let decorators = self.decorator_list(&member.decorators, scope)?;
            let key = self.class_key(&member.key, scope)?;
            let private = match &member.key {
                ClassKey::Private(n) => Some(n.clone()),
                _ => None,
            };
            let display = self.key_display(&key);
            let extra = self.new_list()?;
            let mut element = Element {
                kind: ElementKind::Field,
                is_static: member.is_static,
                key: key.clone(),
                private,
                value: Value::Undefined,
                get: Value::Undefined,
                set: Value::Undefined,
                storage: None,
                initializers: vec![],
                own: 0,
                extra,
                decorators,
            };
            match &member.kind {
                MemberKind::Block(_) => unreachable!(),
                MemberKind::Method(m) => {
                    element.kind = ElementKind::Method;
                    element.value = self.method_function(menv, &display, m)?;
                    self.protect(&element.value)?;
                }
                MemberKind::Getter(body) => {
                    element.kind = ElementKind::Getter;
                    element.get = self.make_function(
                        menv,
                        Some(&format!("get {display}")),
                        &[],
                        body,
                        FunctionKind::Method,
                        false,
                    )?;
                    self.protect(&element.get)?;
                }
                MemberKind::Setter(param, body) => {
                    element.kind = ElementKind::Setter;
                    element.set = self.make_function(
                        menv,
                        Some(&format!("set {display}")),
                        std::slice::from_ref(param),
                        body,
                        FunctionKind::Method,
                        false,
                    )?;
                    self.protect(&element.set)?;
                }
                MemberKind::Field(init) | MemberKind::Accessor(init) => {
                    if let MemberKind::Accessor(_) = &member.kind {
                        element.kind = ElementKind::Accessor;
                        // FieldDefinition : `accessor` ClassElementName Initializer?
                        let readable = match &key {
                            Key::String(s) => s.clone(),
                            Key::Symbol(id) if self.private_symbols.contains(id) => {
                                self.symbol_description(*id).unwrap_or_default()
                            }
                            Key::Symbol(id) => format!(
                                "Symbol({})",
                                self.symbol_description(*id).unwrap_or_default()
                            ),
                        };
                        let storage =
                            self.private_symbol(format!("{readable} accessor storage"))?;
                        element.storage = Some(storage);
                        element.get = self.named_builtin(
                            Action::AutoGet(storage),
                            0,
                            &format!("get {display}"),
                        )?;
                        element.set = self.named_builtin(
                            Action::AutoSet(storage),
                            1,
                            &format!("set {display}"),
                        )?;
                    }
                    if let Some(init) = init {
                        let initializer = self.field_initializer(menv, &display, init)?;
                        element.initializers.push(initializer);
                        element.own = 1;
                    }
                }
            }
            records.push(Record::Element(element));
        }
        // Static non-field elements, then instance non-field elements, then static
        // fields, then instance fields (ApplyDecoratorsAndDefineMethod /
        // ApplyDecoratorsToElementDefinition).
        let static_method_extra = self.new_list()?;
        let instance_method_extra = self.new_list()?;
        for (is_static, fields) in [(true, false), (false, false), (true, true), (false, true)] {
            for record in records.iter_mut() {
                let Record::Element(e) = record else { continue };
                if e.is_static != is_static || (e.kind == ElementKind::Field) != fields {
                    continue;
                }
                let (home, method_extra) = if is_static {
                    (fid, static_method_extra)
                } else {
                    (proto, instance_method_extra)
                };
                let extra = match e.kind {
                    ElementKind::Method | ElementKind::Getter | ElementKind::Setter => method_extra,
                    _ => e.extra,
                };
                self.apply_element_decorators(e, extra)?;
                if !fields {
                    self.define_method_property(home, e)?;
                }
            }
        }
        // F.[[Elements]] and F.[[Initializers]]: the synthesized `%fields` function runs
        // InitializeInstanceElements (private methods, method extra initializers, then
        // each field or accessor).
        let mut instance: Vec<Stmt> = vec![];
        let mut installed: Vec<String> = vec![];
        let mut element_inits: Vec<Stmt> = vec![];
        for (index, record) in records.iter().enumerate() {
            let Record::Element(e) = record else { continue };
            if e.is_static {
                continue;
            }
            if let (Some(n), true) = (&e.private, e.kind != ElementKind::Field) {
                match e.kind {
                    ElementKind::Method => {
                        self.initialize(scope, &format!("%pm:{n}"), e.value.clone())?
                    }
                    ElementKind::Getter => {
                        self.initialize(scope, &format!("%pg:{n}"), e.get.clone())?
                    }
                    ElementKind::Setter => {
                        self.initialize(scope, &format!("%ps:{n}"), e.set.clone())?
                    }
                    _ => {
                        self.initialize(scope, &format!("%pg:{n}"), e.get.clone())?;
                        self.initialize(scope, &format!("%ps:{n}"), e.set.clone())?;
                    }
                }
                if !installed.contains(n) {
                    installed.push(n.clone());
                    instance.push(Stmt::Expr(Expr::ClassInit(Box::new(
                        ClassInit::PrivateMethod(n.clone()),
                    ))));
                }
            }
            if matches!(e.kind, ElementKind::Field | ElementKind::Accessor) {
                let record = self.element_record(e)?;
                self.initialize(scope, &format!("%e{index}"), record)?;
                element_inits.push(Stmt::Expr(Expr::ClassInit(Box::new(ClassInit::Element(
                    Some(index),
                )))));
            }
        }
        self.initialize(scope, "%mx", Value::Object(instance_method_extra))?;
        instance.push(Stmt::Expr(Expr::ClassInit(Box::new(ClassInit::Element(
            None,
        )))));
        instance.extend(element_inits);
        let fenv = self.field_initializer_env(scope)?;
        let fields = self.make_function(fenv, None, &[], &instance, FunctionKind::Method, false)?;
        self.initialize(scope, "%fields", fields)?;
        // InitializePrivateMethods(F, staticElements).
        let mut static_private: Vec<(Key, Property)> = vec![];
        for record in &records {
            let Record::Element(e) = record else { continue };
            if !e.is_static || e.private.is_none() || e.kind == ElementKind::Field {
                continue;
            }
            let property = match e.kind {
                ElementKind::Method => Property::data(e.value.clone(), false, false, false),
                _ => {
                    let (mut get, mut set) = (Value::Undefined, Value::Undefined);
                    if let Some((_, existing)) = static_private.iter().find(|(k, _)| *k == e.key) {
                        if let Kind::Accessor { get: g, set: s } = &existing.kind {
                            (get, set) = (g.clone(), s.clone());
                        }
                    }
                    if matches!(e.kind, ElementKind::Getter | ElementKind::Accessor) {
                        get = e.get.clone();
                    }
                    if matches!(e.kind, ElementKind::Setter | ElementKind::Accessor) {
                        set = e.set.clone();
                    }
                    Property {
                        kind: Kind::Accessor { get, set },
                        enumerable: false,
                        configurable: false,
                    }
                }
            };
            if let Some(slot) = static_private.iter_mut().find(|(k, _)| *k == e.key) {
                slot.1 = property;
            } else {
                static_private.push((e.key.clone(), property));
            }
        }
        for (key, property) in static_private {
            self.private_add(fid, key, property)?;
        }
        // ApplyDecoratorsToClassDefinition.
        let class_extra = self.new_list()?;
        let class_name = class.name.as_deref().or(inferred).unwrap_or("");
        let class_key = Key::from(class_name);
        let mut f = f;
        for (decorator, receiver) in class_decorators {
            let state = self.decoration_state()?;
            let context =
                self.decorator_context(None, &class_key, None, class_extra, state, None)?;
            let new = self.invoke_this(decorator, receiver, vec![f.clone(), context])?;
            self.protect(&new)?;
            self.finish_decoration(state)?;
            if self.callable(&new) {
                f = new;
            } else if new != Value::Undefined {
                return self.throw_error("TypeError", "class decorator must return a function");
            }
        }
        if let Some(n) = &class.name {
            self.initialize(scope, n, f.clone())?;
        }
        for initializer in self.list_values(static_method_extra)? {
            self.invoke_this(initializer, f.clone(), vec![])?;
        }
        for record in &records {
            match record {
                Record::Element(e) if e.is_static => {
                    if matches!(e.kind, ElementKind::Field | ElementKind::Accessor) {
                        let initializers = e.initializers.clone();
                        let extra = self.list_values(e.extra)?;
                        self.initialize_field_or_accessor(
                            f.clone(),
                            e.storage.map(Key::Symbol).unwrap_or_else(|| e.key.clone()),
                            initializers,
                            extra,
                        )?;
                    }
                }
                Record::Element(_) => {}
                Record::Block(body) => {
                    let block = self.make_function(
                        static_scope,
                        None,
                        &[],
                        body,
                        FunctionKind::Method,
                        false,
                    )?;
                    self.protect(&block)?;
                    self.invoke_this(block, f.clone(), vec![])?;
                }
            }
        }
        for initializer in self.list_values(class_extra)? {
            self.invoke_this(initializer, f.clone(), vec![])?;
        }
        Ok(f)
    }
    fn decoration_state(&mut self) -> Result<Id> {
        let state = self.new_object(None)?;
        self.protect(&Value::Object(state))?;
        let mut data = self.data(state)?.clone();
        data.cell = Some(Cell::Called(false));
        self.replace_data(state, data)?;
        Ok(state)
    }
    fn finish_decoration(&mut self, state: Id) -> Result<()> {
        let mut data = self.data(state)?.clone();
        data.cell = Some(Cell::Called(true));
        self.replace_data(state, data)
    }
    /// ApplyDecoratorsToElementDefinition.
    fn apply_element_decorators(&mut self, e: &mut Element, extra: Id) -> Result<()> {
        let decorators = std::mem::take(&mut e.decorators);
        for (decorator, receiver) in decorators {
            let state = self.decoration_state()?;
            let context = self.decorator_context(
                Some(e.kind),
                &e.key,
                e.private.as_deref(),
                extra,
                state,
                Some(e.is_static),
            )?;
            let value = match e.kind {
                ElementKind::Method => e.value.clone(),
                ElementKind::Getter => e.get.clone(),
                ElementKind::Setter => e.set.clone(),
                ElementKind::Field => Value::Undefined,
                ElementKind::Accessor => {
                    let object = self.plain_object()?;
                    self.put_data(object, "get", e.get.clone())?;
                    self.put_data(object, "set", e.set.clone())?;
                    Value::Object(object)
                }
            };
            let new = self.invoke_this(decorator, receiver, vec![value, context])?;
            self.protect(&new)?;
            self.finish_decoration(state)?;
            match e.kind {
                ElementKind::Field => {
                    if self.callable(&new) {
                        e.initializers.insert(e.own, new);
                    } else if new != Value::Undefined {
                        return self
                            .throw_error("TypeError", "field decorator must return a function");
                    }
                }
                ElementKind::Accessor => {
                    if let Value::Object(_) = new {
                        let get = self.get_member(new.clone(), "get")?;
                        self.protect(&get)?;
                        if self.callable(&get) {
                            e.get = get;
                        } else if get != Value::Undefined {
                            return self.throw_error("TypeError", "accessor get is not a function");
                        }
                        let set = self.get_member(new.clone(), "set")?;
                        self.protect(&set)?;
                        if self.callable(&set) {
                            e.set = set;
                        } else if set != Value::Undefined {
                            return self.throw_error("TypeError", "accessor set is not a function");
                        }
                        let init = self.get_member(new, "init")?;
                        self.protect(&init)?;
                        if self.callable(&init) {
                            e.initializers.insert(e.own, init);
                        } else if init != Value::Undefined {
                            return self
                                .throw_error("TypeError", "accessor init is not a function");
                        }
                    } else if new != Value::Undefined {
                        return self
                            .throw_error("TypeError", "accessor decorator must return an object");
                    }
                }
                _ => {
                    if self.callable(&new) {
                        match e.kind {
                            ElementKind::Getter => e.get = new,
                            ElementKind::Setter => e.set = new,
                            _ => e.value = new,
                        }
                    } else if new != Value::Undefined {
                        return self
                            .throw_error("TypeError", "method decorator must return a function");
                    }
                }
            }
        }
        Ok(())
    }
    /// DefineMethodProperty for public methods, getters, setters and auto-accessors;
    /// private ones are installed by InitializePrivateMethods. Class methods stay
    /// non-enumerable (DECORATORS.md, readings 3 and 4).
    fn define_method_property(&mut self, home: Id, e: &Element) -> Result<()> {
        if e.private.is_some() {
            return Ok(());
        }
        let mut d = Descriptor {
            enumerable: Some(false),
            configurable: Some(true),
            ..Descriptor::default()
        };
        match e.kind {
            ElementKind::Method => {
                d.value = Some(e.value.clone());
                d.writable = Some(true);
            }
            ElementKind::Getter => d.get = Some(e.get.clone()),
            ElementKind::Setter => d.set = Some(e.set.clone()),
            ElementKind::Accessor => {
                d.get = Some(e.get.clone());
                d.set = Some(e.set.clone());
            }
            ElementKind::Field => return Ok(()),
        }
        self.define_or_throw(home, e.key.clone(), d)
    }
    /// An instance field or accessor record for the class scope:
    /// [field name (key or storage), initializers, extra initializers].
    fn element_record(&mut self, e: &Element) -> Result<Value> {
        let name = match e.storage {
            Some(id) => Value::Symbol(id),
            None => self.key_name_value(&e.key),
        };
        let initializers = self.array_from_values(e.initializers.clone())?;
        self.array_from_values(vec![name, initializers, Value::Object(e.extra)])
    }
    /// `ClassInit::Element`: with `Some(n)`, InitializeFieldOrAccessor for the instance
    /// record `%e{n}`; with `None`, the instance method extra initializers.
    pub(super) fn decorated_element_init(
        &mut self,
        index: Option<usize>,
        env: Id,
        target: Id,
    ) -> Result<Value> {
        let receiver = Value::Object(target);
        let Some(index) = index else {
            let list = self.lookup(env, "%mx")?;
            if let Value::Object(list) = list {
                for initializer in self.list_values(list)? {
                    self.invoke_this(initializer, receiver.clone(), vec![])?;
                }
            }
            return Ok(Value::Undefined);
        };
        let record = self.lookup(env, &format!("%e{index}"))?;
        let Value::Object(record) = record else {
            return Err(ScriptError::Runtime("missing element record"));
        };
        let parts = self.list_values(record)?;
        let [name, Value::Object(initializers), Value::Object(extra)] = parts.as_slice() else {
            return Err(ScriptError::Runtime("invalid element record"));
        };
        let (initializers, extra) = (*initializers, *extra);
        let name = self.to_key(name.clone())?;
        let initializers = self.list_values(initializers)?;
        let extra = self.list_values(extra)?;
        self.initialize_field_or_accessor(receiver, name, initializers, extra)?;
        Ok(Value::Undefined)
    }
    /// InitializeFieldOrAccessor: chain the initializers, define the field (or the
    /// accessor's storage), then run the extra initializers.
    fn initialize_field_or_accessor(
        &mut self,
        receiver: Value,
        name: Key,
        initializers: Vec<Value>,
        extra: Vec<Value>,
    ) -> Result<()> {
        let Value::Object(target) = receiver else {
            return self.throw_error("TypeError", "class element receiver");
        };
        let mut value = Value::Undefined;
        for initializer in initializers {
            value = self.invoke_this(initializer, receiver.clone(), vec![value])?;
            self.protect(&value)?;
        }
        if self.is_private(&name) {
            self.private_add(target, name, Property::data(value, true, false, false))?;
        } else {
            self.define_or_throw(
                target,
                name,
                Descriptor {
                    value: Some(value),
                    writable: Some(true),
                    enumerable: Some(true),
                    configurable: Some(true),
                    ..Descriptor::default()
                },
            )?;
        }
        for initializer in extra {
            self.invoke_this(initializer, receiver.clone(), vec![])?;
        }
        Ok(())
    }
}

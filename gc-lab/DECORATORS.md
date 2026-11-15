# Decorators and auto-accessors

Source of truth: tc39/ecma262 PR #2417 «Add Class and Class Element Decorators and
`accessor` Keyword» (open, Stage 3 text; the proposal repo has no spec of its own any
more). Plain-text copy used while implementing: workspace `spec/decorators/pr2417.txt`.
Decorator metadata (`context.metadata`, `Symbol.metadata`) is a separate proposal and is
not implemented: the context object has no `metadata` property.

test262 coverage: 20 positive syntax cases under `language/{statements,expressions}/class/decorator`
(they do call the decorators), 4 `accessor` field cases under `class/elements`, and 7
staging cases (`staging/decorators`, auto-accessors and escaped `accessor`).

## Grammar (what the parser accepts)

- `@` is a punctuator. `Decorator : @ DecoratorMemberExpression | @ ( Expression ) |
  @ DecoratorMemberExpression Arguments`, with `DecoratorMemberExpression` an
  IdentifierReference followed by `.IdentifierName` / `.PrivateIdentifier` links.
  Anything else after `@` is a SyntaxError (e.g. `@a[0]`, `@a?.b`, `@new X`).
- `DecoratorList? class ...` for declarations and expressions, and in modules
  `DecoratorList? export Declaration`, `export DecoratorList? class`, and both forms of
  `export default`. Decorators both before and after `export` are a SyntaxError.
- Class elements: `DecoratorList?` before methods, getters, setters, fields and
  auto-accessors, static or not. Decorators on a static block, on the constructor, or
  on an empty element (`@d ;`) are a SyntaxError.
- `accessor [no LineTerminator here] ClassElementName Initializer?`. Only the unescaped
  name counts as the keyword (an escaped `\u0061ccessor x` is a SyntaxError, staging tests; it lexes as a plain Name, so the parser checks the source text);
  `accessor` followed by a line break, `(`, `=`, `;` or `}` is an ordinary element named
  `accessor`. A private auto-accessor `#x` clashes with every other `#x` element.
- Class decorators are parsed before the class body, so `@C.#x` there refers to the
  enclosing class's private names. Element decorators are parsed inside the body and
  see the class's own private names, which matches the ClassElement evaluation scope.

## Runtime

The tree interpreter's `eval_class` (classes.rs) defines each method as soon as it is
evaluated, interleaved with computed keys. The PR changes ClassDefinitionEvaluation for
every class: all elements are evaluated first, then defined in a fixed order. The two
orders differ only when a definition throws (e.g. `static ["prototype"]() {}` before a
throwing computed key). To keep every current pass, classes that use no decorator and no
`accessor` keep the existing path. Classes that use either go through
`eval_decorated_class` (decorators.rs), which follows the PR step by step:

1. Class decorators are evaluated (DecoratorListEvaluation) before ClassDefinitionEvaluation.
   The list is built by prepending, so decorators apply last to first.
2. Per element, in source order: its decorators, then its key (ClassElementEvaluation),
   then the ClassElementDefinition record (method closures, getter/setter closures,
   field initializer, or auto-accessor getter/setter with a fresh storage Private Name).
3. Static non-field elements (ApplyDecoratorsAndDefineMethod on F), then instance
   non-field elements on the prototype, then static fields, then instance fields
   (ApplyDecoratorsToElementDefinition).
4. Instance elements and method extra initializers go to the `%fields` function.
   Then InitializePrivateMethods(F, staticElements), ApplyDecoratorsToClassDefinition
   (which may replace F), the class binding, static method extra initializers, static
   fields, accessors and blocks in order, and finally the class extra initializers.

`InitializeInstanceElements` is still the synthesized `%fields` function. Decorated
elements use `ClassInit::Element(n)`, where `n` indexes a list of element records kept
in the class scope. That list holds keys, initializer lists and extra-initializer lists,
which are values and so cannot be represented in the AST.

The context object (CreateDecoratorContextObject) and its `access` object, the
`addInitializer` function and the auto-accessor getter and setter are builtin functions
(`Internal::Decorator`, promises.rs dispatch): length, name and `[[Prototype]]` match
CreateBuiltinFunction, and a getter's `this` is its receiver.

`DecoratorEvaluation` returns a Reference as `[[Receiver]]`. Per EvaluateCall, the
receiver is the base of a property reference (`@a.b`, `@a.#b`, `@(a.b)`) and undefined
otherwise. Reading the Reference as a value would pass the decorator itself as `this`.
`with`-object bases are not modelled: `@f` inside `with (o)` passes undefined.

## Readings where the PR text is defective

Each choice follows the proposal README, the TypeScript 5 emit (`__esDecorate` /
`__runInitializers`) and test262:

1. **Field initializer order.** The PR appends the field's own initializer to
   `[[Initializers]]` and then *prepends* decorator initializers, so the original
   initializer would run last and overwrite every decorator result. The README says a
   decorator initializer "receives the initial value of the field". We run the field's
   own initializer first, then the decorator initializers in the prepended order
   (outermost decorator first), as TypeScript does.
2. **InitializeInstanceElements** reads `Call(_O_, _initializer_)`, with the arguments
   swapped. We call the initializer with `this = O`.
3. **DefineMethodProperty(_home_, _e_, _isStatic_)** passes isStatic as `enumerable`.
   Class methods and accessors stay non-enumerable (ES2015+ and test262).
4. **Public auto-accessor.** The PR defines it enumerable at element evaluation and then
   defines it again. We define it once, in the define phase, non-enumerable like every
   other class accessor (the README desugaring uses `get x()`/`set x()`).

## Limits

- The resumable path (asynchronous.rs) lowers a class whose heritage or computed keys
  contain `await`/`yield`. That lowering pre-evaluates those keys and would move them
  ahead of decorator evaluation, so such a class with decorators or `accessor` is
  Unsupported. A decorated class whose keys do not suspend uses the normal path.
- Decorated classes always take the tree path. The VM already hands every `Expr::Class`
  to the tree.

## Results (checkpoint 47, 2026-11-15)

Full run t262-full54: 47485/47708 (99.5%), +27, lost 0. The decorator and class/elements smoke (3090 cases) has no failures. After the rebase onto the 2026-11-14 fixes, t262-full55 is 47585/47708 (99.7%), lost 0.

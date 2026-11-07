use gc_lab::script::parser::{parse, Expr, Stmt};

#[test]
fn parses_requested_program_without_recognizing_a_hardcoded_source_string() {
    let ast = parse(include_str!("../examples/button.js")).unwrap();
    assert_eq!(ast.len(), 3);
    assert!(matches!(&ast[0], Stmt::Const(name, Expr::Call(_, _)) if name == "button"));
    let alternate =
        parse("/* header */ const other = document.createElement('a'); other.click();").unwrap();
    assert_eq!(alternate.len(), 2);
}
#[test]
fn parses_parameters_returns_grouping_and_unicode_strings() {
    let ast = parse("const f = (x) => { return x; }; (f)('Sí\\n');").unwrap();
    assert!(matches!(&ast[0], Stmt::Const(_, Expr::Arrow { params, .. }) if params == &["x"]));
}
#[test]
fn rejects_unsupported_or_malformed_input_with_an_offset() {
    for source in [
        "let x = 'a';",
        "const x = 1;",
        "const x = () => {",
        "const x = (a,a)=>{};",
        "const x = 'oops",
        "/*oops",
        "const x = 'a'",
        "x = 'a';",
        "const x = '\\u1234';",
    ] {
        let error = parse(source).unwrap_err();
        assert!(error.offset <= source.len());
    }
}
#[test]
fn parser_enforces_source_token_and_recursion_limits() {
    assert!(parse(&" ".repeat(65_537)).is_err());
    assert!(parse(&format!("{}'x'{};", "(".repeat(100), ")".repeat(100))).is_err());
    assert!(parse(&"x;".repeat(5000)).is_err());
}

#[test]
fn rejects_early_errors_even_in_uncalled_functions_and_unsupported_asi() {
    for source in [
        "const class = 'x';",
        "const f = (x) => { const x = 'y'; };",
        "const f = () => { const x = 'a'; const x = 'b'; };",
        "return 'x';",
        "const f = ()\n=> {};",
        "const f = () => { return\n'x'; };",
    ] {
        assert!(parse(source).is_err(), "{source}");
    }
}

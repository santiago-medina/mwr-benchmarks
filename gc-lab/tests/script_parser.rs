use gc_lab::script::parser::{parse, Expr, Param, Stmt};

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
    assert!(
        matches!(&ast[0], Stmt::Const(_, Expr::Arrow { params, .. }) if params == &[Param::from("x".to_string())])
    );
}
#[test]
fn rejects_unsupported_or_malformed_input_with_an_offset() {
    for source in [
        "const x = () => {",
        "const x = (a,a)=>{};",
        "const x = 'oops",
        "/*oops",
    ] {
        let error = parse(source).unwrap_err();
        assert!(error.offset <= source.len());
    }
}
#[test]
fn parser_enforces_source_token_and_recursion_limits() {
    assert!(parse(&" ".repeat((8 << 20) + 1)).is_err());
    assert!(parse(&format!("{}'x'{};", "(".repeat(100), ")".repeat(100))).is_err());
    assert!(parse(&"x;".repeat(600_000)).is_err());
}

#[test]
fn rejects_early_errors_even_in_uncalled_functions_and_strict_grammar() {
    for source in [
        "const class = 'x';",
        "const f = (x) => { const x = 'y'; };",
        "const f = () => { const x = 'a'; const x = 'b'; };",
        "return 'x';",
        "const f = ()\n=> {};",
    ] {
        assert!(parse(source).is_err(), "{source}");
    }
}

#[test]
fn parses_objects_functions_numbers_and_asi() {
    for source in [
        "let x = 1\nx = 2",
        "const x = '\\u1234'",
        "function f(){return\n'x';}",
        "const x={get a(){return 1;},set a(v){this.x=v;},method(){}};",
        "try {throw 1;} catch(e){} finally{}",
    ] {
        parse(source).unwrap();
    }
    for source in [
        "throw\n1",
        "const x={__proto__:null,__proto__:null};",
        "delete x;",
        "const x={get a(v){}};",
    ] {
        assert!(parse(source).is_err(), "{source}");
    }
}

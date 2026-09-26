use paco_diag::Reporter;
use paco_mir::TypeLayouts;
use paco_span::SourceMap;
use paco_syntax::ast::Module;
use paco_syntax::{lex::lex, parse::parse_module};

fn parsed_module(source: &str) -> Module {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors(), "{}", reporter.emit_to_string(&sources));
    module
}

#[test]
fn struct_field_offsets_match_hand_computed_default_repr() {
    let module = parsed_module("struct Point { a: bool, b: i64, c: bool }");
    let layouts = TypeLayouts::from_module(&module);

    assert_eq!(layouts.struct_field("Point", &[], "b").1, 0);
    assert_eq!(layouts.struct_field("Point", &[], "a").1, 8);
    assert_eq!(layouts.struct_field("Point", &[], "c").1, 9);
    assert_eq!(layouts.struct_layout("Point", &[]).size, 16);
}

#[test]
fn nested_struct_layout_resolves_recursively() {
    let module = parsed_module("struct Inner { v: i64 } struct Outer { flag: bool, inner: Inner }");
    let layouts = TypeLayouts::from_module(&module);

    assert_eq!(layouts.struct_layout("Inner", &[]).size, 8);
    assert_eq!(layouts.struct_field("Outer", &[], "inner").1, 0);
    assert_eq!(layouts.struct_field("Outer", &[], "flag").1, 8);
    assert_eq!(
        layouts.struct_field("Outer", &[], "inner").0,
        paco_types::Type::Struct("Inner".to_string(), Vec::new())
    );
}

#[test]
fn enum_layout_reserves_a_discriminant_and_sizes_to_the_largest_variant() {
    let module = parsed_module("enum Shape { Circle(i64), Empty }");
    let layouts = TypeLayouts::from_module(&module);

    assert_eq!(layouts.enum_variant_field("Shape", &[], "Circle", 0).1, 8);
    assert_eq!(
        layouts.enum_variant_field("Shape", &[], "Circle", 0).0,
        paco_types::Type::Int(paco_types::IntWidth::I64)
    );
    assert_eq!(layouts.enum_layout("Shape", &[]).size, 16);
    assert_eq!(layouts.enum_layout("Shape", &[]).align, 8);
}

#[test]
fn generic_struct_monomorphizes_a_field_by_its_concrete_type_argument() {
    let module = parsed_module("struct Box { value: i64 }\nstruct Wrapper<T> { value: T }");
    let layouts = TypeLayouts::from_module(&module);

    let i64_ty = paco_types::Type::Int(paco_types::IntWidth::I64);
    let bool_ty = paco_types::Type::Bool;

    let as_i64 = layouts.struct_layout("Wrapper", std::slice::from_ref(&i64_ty));
    assert_eq!(as_i64.size, 8);
    assert_eq!(
        layouts.struct_field("Wrapper", std::slice::from_ref(&i64_ty), "value").0,
        i64_ty
    );

    let as_bool = layouts.struct_layout("Wrapper", std::slice::from_ref(&bool_ty));
    assert_eq!(as_bool.size, 1);
    assert_eq!(
        layouts.struct_field("Wrapper", std::slice::from_ref(&bool_ty), "value").0,
        bool_ty
    );
}

#[test]
fn generic_enum_monomorphizes_construct_and_match_round_trip() {
    let module = parsed_module("enum Option<T> { Some(T), None }");
    let layouts = TypeLayouts::from_module(&module);

    let i64_ty = paco_types::Type::Int(paco_types::IntWidth::I64);
    let (field_ty, offset) = layouts.enum_variant_field("Option", std::slice::from_ref(&i64_ty), "Some", 0);
    assert_eq!(field_ty, i64_ty);
    assert_eq!(offset, 8);
    assert_eq!(layouts.enum_variant_index("Option", std::slice::from_ref(&i64_ty), "Some"), 0);
    assert_eq!(layouts.enum_variant_index("Option", std::slice::from_ref(&i64_ty), "None"), 1);
}

#[test]
#[should_panic(expected = "recursive generic struct layout detected")]
fn a_self_referential_generic_struct_is_rejected_with_a_clear_error() {
    let module = parsed_module("struct Node<T> { child: Node<T> }");
    let layouts = TypeLayouts::from_module(&module);

    layouts.struct_layout("Node", &[paco_types::Type::Int(paco_types::IntWidth::I64)]);
}

#[test]
#[should_panic(expected = "exceeded the instantiation cap")]
fn excessive_monomorphization_is_a_reported_compile_time_error() {
    let module = parsed_module("struct Box<T> { value: T }");
    let layouts = TypeLayouts::from_module(&module).with_max_instantiations(2);

    for width in [
        paco_types::IntWidth::I8,
        paco_types::IntWidth::I16,
        paco_types::IntWidth::I32,
    ] {
        layouts.struct_layout("Box", &[paco_types::Type::Int(width)]);
    }
}

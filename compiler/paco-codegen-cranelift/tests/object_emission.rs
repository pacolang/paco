use cranelift_module::default_libcall_names;
use cranelift_object::{ObjectBuilder, ObjectModule};
use object::File as ObjectFile;
use paco_diag::Reporter;
use paco_mir::{Profile, TypeLayouts, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

#[test]
fn object_file_symbol_table_contains_every_declared_function() {
    let source = "fn add(a: i64, b: i64) -> i64 { a + b } fn main() -> i64 { add(1, 2) }";
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors(), "{}", reporter.emit_to_string(&sources));

    let typed = infer_module(&module, &mut reporter).expect("module should type-check");
    let drops =
        paco_borrow::analyze_module(&module, &mut reporter).expect("module should borrow-check");
    let registry = TypeRegistry::from_module(&module);
    let layouts = TypeLayouts::from_module(&module);

    let bodies: Vec<(String, paco_mir::Body)> = module
        .items
        .iter()
        .flat_map(|item| match item {
            Item::Fn(function) => {
                let (body, outlined) =
                    paco_mir::lower_function(function, &typed, &registry, &drops, Profile::Debug);
                let mut result = vec![(function.name.clone(), body)];
                result.extend(outlined);
                result
            }
            _ => Vec::new(),
        })
        .collect();

    let isa = paco_codegen_cranelift::host_isa().expect("host isa");
    let object_builder = ObjectBuilder::new(isa, "main", default_libcall_names()).unwrap();
    let mut module_out = ObjectModule::new(object_builder);
    paco_codegen_cranelift::declare_and_define(&mut module_out, &bodies, &[], &layouts).unwrap();

    let product = module_out.finish();
    let bytes = product.emit().unwrap();

    let object_file = ObjectFile::parse(bytes.as_slice()).expect("emitted bytes should parse as an object file");
    let symbol_names = crate::symbol_names(&object_file, |_| true);

    assert!(symbol_names.contains(&"add"), "{symbol_names:?}");
    assert!(symbol_names.contains(&"main"), "{symbol_names:?}");
}

use object::ObjectSymbol;
use paco_codegen_cranelift::CraneliftBackend;
use paco_diag::Reporter;
use paco_mir::{Backend, Profile, Target, TypeLayouts};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};

#[test]
fn compiled_objects_allocate_through_the_runtime_not_libc() {
    {
        let case = "slices, strings and closures";
        let source = "fn main() {\n    let mut cells: []i64 = slice_of_zeros<i64>(4);\n    cells[1] = 2;\n    let text = string_concat(&\"a\", &\"b\");\n    let offset = cells[1];\n    let add = |n: i64| n + offset;\n    print(add(1));\n    print(text)\n}\n";
        let mut sources = SourceMap::new();
        let file = sources.add_file("main.paco", source);
        let mut reporter = Reporter::new();
        let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
        let module = parse_module(&tokens, &mut reporter).unwrap();
        let typed = paco_types::infer_module(&module, &mut reporter).unwrap();
        let drops = paco_borrow::analyze_module(&module, &mut reporter).unwrap();
        let registry = paco_mir::TypeRegistry::from_module(&module);
        let layouts = TypeLayouts::from_module(&module);
        let mut backend = CraneliftBackend::new(Vec::new(), &layouts);
        for item in &module.items {
            if let Item::Fn(function) = item {
                let (body, outlined) = paco_mir::lower_function(function, &typed, &registry, &drops, Profile::Debug);
                backend.lower_body(&function.name, &body).unwrap();
                for (name, body) in outlined {
                    backend.lower_body(&name, &body).unwrap();
                }
            }
        }
        let bytes = backend.finish(&Target { triple: None, profile: Profile::Debug }).unwrap();
        let object = object::File::parse(&*bytes).unwrap();
        let undefined = crate::symbol_names(&object, |symbol| symbol.is_undefined());
        for libc in ["malloc", "calloc", "realloc", "free"] {
            assert!(!undefined.contains(&libc), "{case} imports `{libc}`: {undefined:?}");
        }
        assert!(undefined.contains(&"paco_free"), "{case}: {undefined:?}");
    }
}

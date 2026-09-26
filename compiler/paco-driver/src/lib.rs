//! Paco driver command-line interface.

use std::{
    collections::HashSet,
    fs,
    path::PathBuf,
};

pub mod cache;
pub mod git;
pub mod manifest;
pub mod pkg_cache;
mod lowering;

use clap::{Parser, Subcommand};
use paco_diag::{Diagnostic, Reporter, Severity};
use paco_span::{SourceMap, Span};
use paco_syntax::{
    ast::{Expr, FnDecl, Item, Module, Ty, UsePathKind},
    lex::{Token, TokenKind, lex},
    parse::{expr_span, parse_module},
};

#[derive(Debug, Parser)]
#[command(name = "paco")]
#[command(about = "Paco compiler driver")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Build {
        path: Option<PathBuf>,
        #[arg(long)]
        release: bool,
        /// `json` writes one diagnostic object per line, with applicable fixes.
        #[arg(long, value_enum)]
        format: Option<Format>,
        #[arg(long)]
        target: Option<String>,
        /// Overrides the backend `--release` implies.
        #[arg(long, value_enum)]
        backend: Option<BackendChoice>,
        /// `static` (musl) by default; `dynamic` (glibc) when the program has `extern` blocks.
        #[arg(long, value_enum)]
        link: Option<LinkChoice>,
        /// The target system's root for dynamic cross builds; defaults to `PACO_SYSROOT`.
        #[arg(long)]
        sysroot: Option<PathBuf>,
    },
    Check {
        file: PathBuf,
        #[arg(long, value_enum)]
        format: Option<Format>,
    },
    /// Builds the program like `paco build` (through the build cache) and
    /// runs it with the arguments after `--`.
    Run {
        file: Option<PathBuf>,
        #[arg(long, value_enum)]
        format: Option<Format>,
        #[arg(last = true)]
        args: Vec<String>,
    },
    Test {
        /// The project directory (or a file within it) to discover
        /// `_test.paco` files under; defaults to the current directory.
        path: Option<PathBuf>,
        /// Only run test functions whose name contains this substring.
        filter: Option<String>,
        /// Overrides `paco build`'s default backend (Cranelift) for the
        /// synthesized test binaries.
        #[arg(long, value_enum)]
        backend: Option<BackendChoice>,
    },
    Fmt {
        file: PathBuf,
        #[arg(long)]
        write: bool,
    },
    Doc,
    /// Removes what `paco build` wrote for `path`; `--cache` empties the
    /// build cache instead.
    Clean {
        path: Option<PathBuf>,
        #[arg(long)]
        cache: bool,
    },
    /// Prints what a diagnostic code means, its cause and its fix.
    Explain {
        code: String,
    },
    /// Prints the type of every `let` that has dimensions, and where each
    /// dimension name was bound.
    Shapes {
        file: PathBuf,
        #[arg(long, value_enum)]
        format: Option<Format>,
    },
    /// Reads `paco.mod` and fetches (or reuses an already-cached copy of)
    /// every dependency it declares, writing `paco.lock`.
    Get {
        /// The `paco.mod` file to read; defaults to `paco.mod` in the
        /// current directory.
        path: Option<PathBuf>,
    },
    /// `paco.mod`-related subcommands.
    Mod {
        #[command(subcommand)]
        command: ModCommands,
    },
    /// Rewrites `use stdlib::numerics`/`stdlib::math`/`stdlib::blas` (and the
    /// `numerics::`/`.add`/`.sub`/`.mul` call sites their move affects) to
    /// the `github.com/pacolang/numerics` library, and adds it to
    /// `paco.mod` (`extract-domain-libraries` task 4.3).
    Fix {
        /// The `paco.mod` file to read; defaults to `paco.mod` in the
        /// current directory.
        path: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ModCommands {
    /// Reports every `paco.mod` dependency no source file `use`s, and every
    /// domain-shaped `use` path with no matching `paco.mod` entry.
    Tidy {
        /// The `paco.mod` file to read; defaults to `paco.mod` in the
        /// current directory.
        path: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, clap::ValueEnum)]
pub enum Format {
    #[default]
    Human,
    Json,
}

thread_local! {
    static FORMAT: std::cell::Cell<Format> = const { std::cell::Cell::new(Format::Human) };
}

/// How diagnostics are written for the rest of this thread's work.
pub fn set_format(format: Format) {
    FORMAT.with(|slot| slot.set(format));
}

fn emit(reporter: &Reporter, sources: &SourceMap) -> String {
    match FORMAT.with(|slot| slot.get()) {
        Format::Human => reporter.emit_to_string(sources),
        Format::Json => reporter.emit_json(sources),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum BackendChoice {
    Cranelift,
    Llvm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum LinkChoice {
    Static,
    Dynamic,
}

pub struct BuildOptions {
    pub release: bool,
    pub target: Option<String>,
    pub backend: BackendChoice,
    pub link: Option<LinkChoice>,
    pub sysroot: Option<PathBuf>,
}

/// The link mode and complete triple for a build: an explicit `--link`
/// wins, then a `-musl`/`-gnu` suffix, then whether the program has
/// `extern` blocks. `<arch>-unknown-linux` (or no `--target`, meaning the
/// host's architecture) is completed with the mode's C library.
pub fn resolve_target(
    requested: Option<&str>,
    link: Option<LinkChoice>,
    has_extern: bool,
) -> (paco_link::LinkMode, String) {
    let host = paco_link::host_triple();
    let mode = match (link, requested) {
        (Some(LinkChoice::Static), _) => paco_link::LinkMode::Static,
        (Some(LinkChoice::Dynamic), _) => paco_link::LinkMode::Dynamic,
        (None, Some(triple)) if triple.ends_with("-musl") => paco_link::LinkMode::Static,
        (None, Some(triple)) if triple.ends_with("-gnu") => paco_link::LinkMode::Dynamic,
        _ if has_extern => paco_link::LinkMode::Dynamic,
        _ => paco_link::LinkMode::Static,
    };
    let Some(requested) = requested.or_else(|| host.contains("-linux-").then_some(host)) else {
        return (mode, host.to_string());
    };
    let base = requested.strip_suffix("-gnu").or_else(|| requested.strip_suffix("-musl")).unwrap_or(requested);
    if !base.ends_with("-unknown-linux") {
        return (mode, requested.to_string());
    }
    let env = match mode {
        paco_link::LinkMode::Static => "musl",
        paco_link::LinkMode::Dynamic => "gnu",
    };
    (mode, format!("{base}-{env}"))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverOutput {
    pub stdout: String,
    pub stderr: String,
}

pub fn run(cli: Cli) -> Result<DriverOutput, String> {
    let format = match &cli.command {
        Commands::Run { format, .. } | Commands::Build { format, .. } | Commands::Check { format, .. } | Commands::Shapes { format, .. } => {
            format.unwrap_or_default()
        }
        _ => Format::Human,
    };
    set_format(format);
    let result = run_command(cli);
    set_format(Format::Human);
    result
}

fn run_command(cli: Cli) -> Result<DriverOutput, String> {
    match cli.command {
        Commands::Run { file, args, .. } => {
            let (output, status) = run_program(&file.unwrap_or_else(|| PathBuf::from("main.paco")), &args)?;
            match status {
                Some(0) => Ok(output),
                status => Err(format!("{}program exited with {}", output.stderr, status.map_or("a signal".to_string(), |code| format!("status {code}")))),
            }
        }
        Commands::Build { path, release, target, backend, link, sysroot, .. } => {
            let backend = backend.unwrap_or(if release { BackendChoice::Llvm } else { BackendChoice::Cranelift });
            let sysroot = sysroot.or_else(|| std::env::var_os("PACO_SYSROOT").map(PathBuf::from));
            build_file(
                path.unwrap_or_else(|| PathBuf::from("main.paco")),
                BuildOptions { release, target, backend, link, sysroot },
            )
        }
        Commands::Check { file, .. } => check_file(file),
        Commands::Explain { code } => explain_in(REGISTRY, &code).map(|stdout| DriverOutput { stdout, stderr: String::new() }),
        Commands::Shapes { file, format } => shapes_file(file, format.unwrap_or_default()),
        Commands::Test { path, filter, backend } => test_command(path, filter, backend.unwrap_or(BackendChoice::Cranelift)),
        Commands::Fmt { file, write } => format_file(file, write),
        Commands::Doc => not_implemented("doc"),
        Commands::Clean { path, cache } => clean(path, cache),
        Commands::Get { path } => run_get(path),
        Commands::Mod { command: ModCommands::Tidy { path } } => run_mod_tidy(path),
        Commands::Fix { path } => run_fix(path),
    }
}

const REGISTRY: &str = include_str!("../../../docs/diagnostics/registry.toml");

/// `paco explain` against a registry's text: the code's message, what it
/// means, its cause, its fix and the rules it enforces.
pub fn explain_in(registry: &str, code: &str) -> Result<String, String> {
    let code = code.strip_prefix("PACO-").unwrap_or(code);
    let table: toml::Table = registry.parse().map_err(|error| format!("the diagnostics registry is malformed: {error}"))?;
    let Some(entry) = table.get(code).and_then(toml::Value::as_table) else {
        return Err(format!("unknown diagnostic code `PACO-{code}`"));
    };
    let field = |name: &str| entry.get(name).and_then(toml::Value::as_str).unwrap_or("").trim().to_string();
    if field("status") == "retired" {
        let successor = field("superseded_by");
        let successor = successor.strip_prefix("PACO-").unwrap_or(&successor);
        if successor.is_empty() {
            return Err(format!("PACO-{code} is retired: {}", field("explanation")));
        }
        return Err(format!("PACO-{code} is retired; its successor is PACO-{successor} (`paco explain {successor}`)"));
    }
    let mut out = format!("PACO-{code}: {}\n\n{}\n\nCause: {}\nFix: {}\n", field("message_template"), field("explanation"), field("cause"), field("fix"));
    for (label, name) in [("Spec", "spec_ref"), ("ADR", "adr_ref")] {
        let value = field(name);
        if !value.is_empty() {
            out.push_str(&format!("{label}: {value}\n"));
        }
    }
    Ok(out)
}

fn json_text(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `paco shapes`: the type of every `let` with dimensions in `file`, and
/// where each dimension name in it was bound.
fn shapes_file(file: PathBuf, format: Format) -> Result<DriverOutput, String> {
    let CheckedProgram { sources, module, mut reporter, discovered, prelude, .. } = check_program(file)?;
    if reporter.has_errors() {
        return Err(emit(&reporter, &sources));
    }
    let mut imports = visible_imports(&module, &discovered);
    imports.extend(prelude.as_ref().map(|prelude| (String::new(), prelude)));
    paco_types::record_shapes(true);
    let typed = paco_types::infer_module_with_imports(&module, &imports, &mut reporter);
    paco_types::record_shapes(false);
    let typed = typed.map_err(|_| emit(&reporter, &sources))?;
    let place = |span: Span| sources.location(span).map(|location| (location.file_name, location.start.line)).ok();
    let mut stdout = String::new();
    for row in typed.shapes() {
        let Some((file, line)) = place(row.span) else { continue };
        let mut names = Vec::new();
        collect_dimension_names(&row.ty, &mut names);
        let origins: Vec<(String, String, usize)> = names
            .iter()
            .filter_map(|name| {
                let (origin_file, origin_line) = place(typed.atom(name)?.origin)?;
                Some((paco_types::display_name(name), origin_file, origin_line))
            })
            .collect();
        match format {
            Format::Human => {
                stdout.push_str(&format!("{file}:{line} {}: {}\n", row.name, row.ty.name()));
                for (name, origin_file, origin_line) in &origins {
                    stdout.push_str(&format!("  {name} bound at {origin_file}:{origin_line}\n"));
                }
            }
            Format::Json => {
                let names: Vec<String> = origins
                    .iter()
                    .map(|(name, origin_file, origin_line)| {
                        format!("{{\"name\":{},\"file\":{},\"line\":{origin_line}}}", json_text(name), json_text(origin_file))
                    })
                    .collect();
                stdout.push_str(&format!(
                    "{{\"file\":{},\"line\":{line},\"name\":{},\"type\":{},\"names\":[{}]}}\n",
                    json_text(&file),
                    json_text(&row.name),
                    json_text(&row.ty.name()),
                    names.join(",")
                ));
            }
        }
    }
    Ok(DriverOutput { stdout, stderr: String::new() })
}

fn collect_dimension_names(ty: &paco_types::Type, out: &mut Vec<String>) {
    use paco_types::{Dim, Type};
    match ty {
        Type::Generic(name) if paco_types::is_atom(name) && !out.contains(name) => out.push(name.clone()),
        Type::Dim(Dim::Const(expr)) => {
            for name in expr.names() {
                if paco_types::is_atom(name) && !out.iter().any(|known| known == name) {
                    out.push(name.to_string());
                }
            }
        }
        Type::Struct(_, items) | Type::Enum(_, items) | Type::Tuple(items) | Type::Pack(items) => {
            items.iter().for_each(|item| collect_dimension_names(item, out))
        }
        Type::Borrow { ty, .. } | Type::Slice(ty) => collect_dimension_names(ty, out),
        _ => {}
    }
}

fn run_cache() -> cache::Cache {
    cache::Cache::open(cache::cache_dir(|name| std::env::var_os(name)), cache::Policy::default())
}

/// Builds and runs `file` with `args`, capturing what it prints: the
/// compile-time output, then the program's, and its exit status (`None`
/// when a signal ended it).
pub fn run_program(file: &std::path::Path, args: &[String]) -> Result<(DriverOutput, Option<i32>), String> {
    let cache = run_cache();
    let entry = build_cached(file, &cache, &compiler_identity())?;
    let output = std::process::Command::new(&entry.binary)
        .args(args)
        .output()
        .map_err(|error| format!("failed to run `{}`: {error}", entry.binary.display()))?;
    let stdout = entry.stdout + &String::from_utf8_lossy(&output.stdout);
    let stderr = entry.stderr + &String::from_utf8_lossy(&output.stderr);
    Ok((DriverOutput { stdout, stderr }, output.status.code()))
}

/// `paco run` from the command line: builds `file`, then becomes the
/// program (on Unix) so its output, signals and exit status are its own.
/// Returns the exit status when it could not do that.
pub fn exec_program(file: &std::path::Path, args: &[String]) -> i32 {
    let cache = run_cache();
    match build_cached(file, &cache, &compiler_identity()) {
        Ok(entry) => exec_entry(&cache, entry, args),
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

/// `exec_program` when `file` is already built and nothing it was built
/// from changed; `None` without running anything otherwise.
pub fn exec_cached(file: &std::path::Path, args: &[String]) -> Option<i32> {
    let cache = run_cache();
    let entry = cache.lookup(file, &run_key(&compiler_identity()))?;
    Some(exec_entry(&cache, entry, args))
}

fn exec_entry(cache: &cache::Cache, entry: cache::Entry, args: &[String]) -> i32 {
    use std::io::Write;
    print!("{}", entry.stdout);
    eprint!("{}", entry.stderr);
    let _ = std::io::stdout().flush();
    let mut command = std::process::Command::new(&entry.binary);
    command.args(args);
    #[cfg(not(unix))]
    let _ = cache;
    #[cfg(unix)]
    if !cache.is_temporary() {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        eprintln!("failed to run `{}`: {error}", entry.binary.display());
        return 1;
    }
    match command.status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("failed to run `{}`: {error}", entry.binary.display());
            1
        }
    }
}

/// One module's own `(TypedModule, TypeRegistry, DropPlan)` triple,
/// together with what `build_file`'s lowering passes need to know about
/// it: which AST it came from (for `qualified_module_functions` and, for
/// `generic-function-codegen`'s worklist, `TypeRegistry::find_method_decl`
/// — pointer-identity-matched against `typed`, so only the module that
/// actually owns a given `FnDecl` can successfully re-lower its body),
/// its own qualifier prefix, and whether every one of its own functions
/// should be lowered (the entry module) or only its `pub` ones (every
/// other module, imported/prelude).
struct ModuleContext<'a> {
    module: &'a Module,
    qualifier: String,
    typed: paco_types::TypedModule<'a>,
    registry: paco_mir::TypeRegistry<'a>,
    drops: paco_borrow::DropPlan<'a>,
}

fn lower_prelude_module(
    prelude_module: &Module,
) -> Option<(paco_types::TypedModule<'_>, paco_borrow::DropPlan<'_>, paco_mir::TypeRegistry<'_>)> {
    let mut prelude_reporter = Reporter::new();
    let prelude_typed = paco_types::infer_module_with_imports(prelude_module, &[], &mut prelude_reporter).ok()?;
    let prelude_drops = paco_borrow::analyze_typed_module(prelude_module, &[], Some(&prelude_typed), &mut prelude_reporter).ok()?;
    let prelude_registry = paco_mir::TypeRegistry::from_module_with_imports(prelude_module, &[]);
    Some((prelude_typed, prelude_drops, prelude_registry))
}

/// A function/method is "generic" (needs `generic-function-codegen`'s
/// per-instantiation worklist, not eager lowering) if any of its own
/// param types or its return type, per the module's own `TypedModule`,
/// still contains an unresolved `Type::Generic` anywhere — including
/// through `self`/`&self`'s own type, so every method of a generic
/// struct/enum counts, even one whose own body never touches the type
/// parameter (e.g. `Vec::len`) — simpler and safe, at the cost of
/// compiling such a method once per instantiation instead of once ever.
fn function_is_generic(typed: &paco_types::TypedModule<'_>, function: &FnDecl) -> bool {
    if !paco_syntax::ast::generic_names(&function.generics).is_empty()
        || typed.self_type_of(function).is_some_and(type_contains_generic)
    {
        return true;
    }
    let return_is_generic = typed
        .type_of_fn_return(function)
        .is_some_and(type_contains_generic);
    return_is_generic
        || function
            .params
            .iter()
            .any(|param| typed.type_of_param(param).is_some_and(type_contains_generic))
}

fn type_contains_generic(ty: &paco_types::Type) -> bool {
    use paco_types::Type;
    match ty {
        Type::Generic(name) => !paco_types::is_existential(name) && !paco_types::is_atom(name),
        Type::Spread(_) => true,
        Type::Dim(paco_types::Dim::Const(expr)) => {
            let names = expr.names();
            expr.as_lit().is_none() && (names.is_empty() || !names.iter().all(|name| paco_types::is_existential(name) || paco_types::is_atom(name)))
        }
        Type::Struct(_, args) | Type::Enum(_, args) | Type::Pack(args) => args.iter().any(type_contains_generic),
        Type::Borrow { ty, .. } | Type::RawPointer { ty, .. } | Type::Slice(ty) => type_contains_generic(ty),
        Type::Tuple(items) => items.iter().any(type_contains_generic),
        _ => false,
    }
}

/// A struct/enum's own method, found by walking `module`'s own items
/// directly — deliberately bypassing `TypeRegistry`, whose `structs`/
/// `enums` maps also include imported (e.g. prelude) declarations, which
/// would let this match a module that merely imports the type instead of
/// the one that owns it. The worklist drain needs the *owning* module
/// specifically, since only its own `ModuleContext::typed` has cached
/// types for that method body's own expressions (keyed by AST pointer
/// identity).
fn module_owns_method<'a>(
    module: &'a Module,
    type_name: &str,
    method_name: &str,
) -> Option<(&'a FnDecl, Vec<String>)> {
    for item in &module.items {
        let (name, methods, generics) = match item {
            Item::Struct(decl) => (&decl.name, &decl.methods, &decl.generics),
            Item::Enum(decl) => (&decl.name, &decl.methods, &decl.generics),
            _ => continue,
        };
        if name == type_name
            && let Some(method) = methods.iter().find(|method| method.name == method_name)
        {
            return Some((method, paco_syntax::ast::generic_names(generics)));
        }
    }
    let generics = module
        .items
        .iter()
        .find_map(|item| match item {
            Item::Struct(decl) if decl.name == type_name => Some(paco_syntax::ast::generic_names(&decl.generics)),
            Item::Enum(decl) if decl.name == type_name => Some(paco_syntax::ast::generic_names(&decl.generics)),
            _ => None,
        })
        .unwrap_or_default();
    module.items.iter().find_map(|item| match item {
        Item::Methods(block) if simple_ty_name(&block.target) == type_name => block
            .methods
            .iter()
            .find(|method| method.name == method_name)
            .map(|method| {
                let generics = if matches!(block.target, Ty::Slice(..)) {
                    paco_syntax::ast::generic_names(&block.generics)
                } else {
                    generics.clone()
                };
                (method, generics)
            }),
        _ => None,
    })
}

fn qualified_module_functions<'a>(module: &'a Module, qualifier: &str) -> Vec<(String, &'a FnDecl)> {
    let prefix = |name: &str| if qualifier.is_empty() { name.to_string() } else { format!("{qualifier}::{name}") };
    let mut functions = Vec::new();
    for item in &module.items {
        match item {
            Item::Fn(function) => functions.push((prefix(&function.name), function)),
            Item::Struct(decl) => {
                for method in &decl.methods {
                    functions.push((prefix(&format!("{}::{}", decl.name, method.name)), method));
                }
            }
            Item::Enum(decl) => {
                for method in &decl.methods {
                    functions.push((prefix(&format!("{}::{}", decl.name, method.name)), method));
                }
            }
            Item::Methods(block) => {
                let type_name = simple_ty_name(&block.target);
                for method in &block.methods {
                    let name = format!("{type_name}::{}", method.name);
                    let name = if paco_mir::is_primitive_type_name(&type_name) { name } else { prefix(&name) };
                    functions.push((name, method));
                }
            }
            Item::Trait(_) | Item::Use(_) | Item::Const(_) | Item::Extern(_) => {}
        }
    }
    functions
}

fn extern_signatures(module: &Module) -> Vec<(String, Vec<paco_types::Type>, paco_types::Type)> {
    let mut signatures = Vec::new();
    for item in &module.items {
        let Item::Extern(block) = item else { continue };
        for function in &block.functions {
            let params = function.params.iter().map(|param| extern_scalar_type(&param.ty)).collect();
            let return_ty = function.return_ty.as_ref().map_or(paco_types::Type::Unit, extern_scalar_type);
            signatures.push((function.name.clone(), params, return_ty));
        }
    }
    signatures
}

fn extern_scalar_type(ty: &Ty) -> paco_types::Type {
    if let Ty::RawPointer { mutable, ty: pointee, .. } = ty {
        return paco_types::Type::RawPointer { mutable: *mutable, ty: Box::new(extern_scalar_type(pointee)) };
    }
    let Ty::Path(path, _) = ty else {
        panic!("extern function type `{ty:?}` is not supported by codegen yet");
    };
    match path.join("::").as_str() {
        "i8" => paco_types::Type::Int(paco_types::IntWidth::I8),
        "i16" => paco_types::Type::Int(paco_types::IntWidth::I16),
        "i32" => paco_types::Type::Int(paco_types::IntWidth::I32),
        "i64" => paco_types::Type::Int(paco_types::IntWidth::I64),
        "u8" | "byte" => paco_types::Type::Int(paco_types::IntWidth::U8),
        "u16" => paco_types::Type::Int(paco_types::IntWidth::U16),
        "u32" => paco_types::Type::Int(paco_types::IntWidth::U32),
        "u64" => paco_types::Type::Int(paco_types::IntWidth::U64),
        "float" | "f64" => paco_types::Type::Float(paco_types::FloatWidth::F64),
        "bool" => paco_types::Type::Bool,
        other => panic!("extern function type `{other}` is not supported by codegen yet"),
    }
}

struct DiscoveredFile {
    qualifier: String,
    module: Module,
    qualified_heads: Vec<(String, Span)>,
}

/// The `.paco` file a `use` path's segments name, every segment but the
/// last a subdirectory of `base_dir` and the last `{seg}.paco`
/// (`cross-file-modules`). Rooted at the entry file's directory for a
/// `Plain`-kind path; at `<cache_dir>/src/` for a `Domain`-kind path once
/// `resolve_domain_use_path` has located that dependency's cache directory
/// (`git-module-fetch`'s own generalization — the function itself already
/// took a root parameter, only its callers needed to vary it).
fn use_path_to_file(path: &[String], base_dir: &std::path::Path) -> PathBuf {
    let is_std = path.first().is_some_and(|segment| segment == "stdlib");
    let mut result = if is_std { stdlib_root() } else { base_dir.to_path_buf() };
    let segments = if is_std { &path[1..] } else { path };
    for segment in &segments[..segments.len() - 1] {
        result.push(segment);
    }
    result.push(format!("{}.paco", segments.last().expect("use path is never empty")));
    result
}

/// `stdlib::numerics`/`stdlib::math`/`stdlib::blas` have moved to
/// `github.com/pacolang/numerics`'s `tensor`/`math`/`blas` modules
/// (`extract-domain-libraries`); each pair is `(old std qualifier, new
/// module name in the library)`. `numerics` is the only one that renames
/// (to `tensor`) — `math` and `blas` keep their name in the new library.
const MOVED_STD_MODULES: &[(&str, &str)] = &[("numerics", "tensor"), ("math", "math"), ("blas", "blas")];

/// `NUMERICS_MODULE_PATH`'s domain-path segments (`git-module-fetch`'s
/// `longest_prefix_match` form), with `module` appended.
fn numerics_domain_segments(module: &str) -> Vec<String> {
    ["github", "com", "pacolang", "numerics", module].iter().map(|segment| segment.to_string()).collect()
}

const NUMERICS_MODULE_PATH: &str = "github.com/pacolang/numerics";

fn numerics_library_path(module: &str) -> String {
    format!("{NUMERICS_MODULE_PATH}/{module}")
}

/// `Some((old, new))` when `path` is exactly `stdlib::<old>` for one of
/// `MOVED_STD_MODULES`.
fn moved_std_module(path: &[String]) -> Option<(&'static str, &'static str)> {
    if path.len() != 2 || path[0] != "stdlib" {
        return None;
    }
    MOVED_STD_MODULES.iter().find(|(old, _)| path[1] == *old).copied()
}

/// Whether `paco.mod` declares `github.com/pacolang/numerics` (or, once
/// `blas`/`math` are added there, a shorter prefix covering `module`) —
/// the same longest-prefix-match `resolve_domain_use_path` itself uses.
fn declares_numerics_module(project: Option<&Result<ProjectManifest, String>>, module: &str) -> bool {
    project
        .and_then(|result| result.as_ref().ok())
        .is_some_and(|project| manifest::longest_prefix_match(&project.manifest.dependencies, &numerics_domain_segments(module)).is_some())
}

/// Whether `entry_dir` is `stdlib_root()` itself (or under it) — i.e.
/// this compile is checking/building a `stdlib` file directly (`paco check
/// stdlib/blas.paco`, `check_std::every_std_module_checks_cleanly`), not a
/// user program. `stdlib/math.paco` and `stdlib/blas.paco` still `use
/// stdlib::numerics`/`use stdlib::math` internally, unchanged, while they still
/// physically live in `stdlib/` (`extract-domain-libraries` tasks 3.1/3.3
/// haven't moved them out yet) — the alias table exists for *user* code
/// migrating away from `stdlib::numerics`, never for `stdlib`'s own
/// composition, so it does not apply here at all.
fn entry_is_within_stdlib(entry_dir: &std::path::Path) -> bool {
    let (Ok(entry_dir), Ok(std_root)) = (entry_dir.canonicalize(), stdlib_root().canonicalize()) else {
        return false;
    };
    entry_dir.starts_with(&std_root)
}

/// Resolves an old `use stdlib::numerics`/`stdlib::math`/`stdlib::blas` line during
/// the transition window (`extract-domain-libraries` design.md's "alias
/// table" and its migration-plan step 3, tasks.md task 4.4): with
/// `paco.mod` declaring `github.com/pacolang/numerics`, this resolves to
/// the library with a deprecation warning; with a `paco.mod` present that
/// does *not* declare it, this is a build error naming the replacement
/// line and `paco get` (the spec delta's "Old import without the
/// dependency" scenario, which is phrased in terms of an existing
/// `paco.mod`); with **no `paco.mod` at all**, task 4.4's own release
/// scenario ("a program with the old import and no manifest still builds
/// and warns") keeps resolving straight to the still-present `stdlib` file,
/// with the same deprecation warning — this is what step 3's "keeping
/// `stdlib/numerics.paco`... in place" is *for*: every program without a
/// manifest (the overwhelming majority of existing, not-yet-migrated
/// code, including this compiler's own pre-4.1 test suite) is otherwise
/// unaffected by this release, exactly like an unmoved `stdlib` module. The
/// bound qualifier stays the old name either way (`discover_one`'s caller
/// binds `use_path.last()`, still `numerics`/`math`/`blas`); only the
/// file this resolves to (and, for the `paco.mod`-present-but-undeclared
/// case, whether it resolves at all) changes.
fn resolve_moved_std_module(
    old_module: &str,
    new_module: &str,
    use_span: Span,
    ctx: &DiscoveryContext<'_>,
    sources: &SourceMap,
    reporter: &mut Reporter,
) -> Result<PathBuf, String> {
    if entry_is_within_stdlib(ctx.entry_dir) {
        return Ok(use_path_to_file(&["stdlib".to_string(), old_module.to_string()], ctx.entry_dir));
    }
    let library_line = format!("use {};", numerics_library_path(new_module));
    if ctx.project.is_some() && !declares_numerics_module(ctx.project, new_module) {
        reporter.push(Diagnostic::error(
            "PACO-E0902",
            use_span,
            format!(
                "`use stdlib::{old_module}` has moved to `{NUMERICS_MODULE_PATH}`; add `{library_line}` and run `paco get {NUMERICS_MODULE_PATH}`"
            ),
        ));
        return Err(emit(reporter, sources));
    }
    reporter.push(Diagnostic::new(
        "PACO-E0901",
        Severity::Warning,
        use_span,
        format!("`use stdlib::{old_module}` is deprecated; replace it with `{library_line}` (the `{new_module}::` qualifier)"),
    ));
    if ctx.project.is_none() {
        return Ok(use_path_to_file(&["stdlib".to_string(), old_module.to_string()], ctx.entry_dir));
    }
    resolve_domain_use_path(ctx.project, &numerics_domain_segments(new_module))
}

/// `PACO-E0306` ("type is not supported yet: {path}") whose unresolved
/// path's last segment is exactly `Tensor` — bare or through a `tensor::`/
/// `numerics::` path — means the caller most likely meant the extracted
/// `github.com/pacolang/numerics` library's `Tensor`, since `paco-types`
/// has no knowledge of `paco.mod` or git dependencies to say so itself
/// (`extract-domain-libraries` task 4.2). The compiler cannot tell this
/// apart from an unrelated user type that also happens to be named
/// `Tensor` and does not exist yet — that ambiguity is inherent to a hint
/// named after the library's exact type name, so this matches on the name
/// alone, same as design.md accepts.
fn apply_tensor_missing_library_hint(reporter: &mut Reporter, project: Option<&Result<ProjectManifest, String>>) {
    let declared = declares_numerics_module(project, "tensor");
    for diagnostic in reporter.diagnostics_mut() {
        if diagnostic.code() != "PACO-E0306" {
            continue;
        }
        let Some(name) = diagnostic.primary().message.strip_prefix("type is not supported yet: ") else { continue };
        if name.rsplit("::").next() != Some("Tensor") {
            continue;
        }
        let span = diagnostic.primary().span;
        let mut hint =
            Diagnostic::error("PACO-E0903", span, format!("cannot find `Tensor`; add `use {};`", numerics_library_path("tensor")));
        if !declared {
            hint = hint.with_note(format!("run `paco get {NUMERICS_MODULE_PATH}` to add the dependency"));
        }
        *diagnostic = hint;
    }
}

fn parse_file(
    path: &std::path::Path,
    sources: &mut SourceMap,
    reporter: &mut Reporter,
) -> Result<(Module, Vec<(String, Span)>), String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read `{}`: {error}", path.display()))?;
    parse_source(path, source, sources, reporter)
}

/// The first segment of every `a::b` path in a file, outside its `use` lines.
fn qualified_heads(tokens: &[Token], module: &Module) -> Vec<(String, Span)> {
    let use_spans: Vec<Span> = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Use(decl) => Some(decl.span),
            _ => None,
        })
        .collect();
    tokens
        .windows(2)
        .enumerate()
        .filter(|(index, pair)| {
            pair[0].kind == TokenKind::Identifier
                && pair[1].kind == TokenKind::ColonColon
                && (*index == 0 || tokens[index - 1].kind != TokenKind::ColonColon)
        })
        .map(|(_, pair)| &pair[0])
        .filter(|token| !use_spans.iter().any(|span| span.start() <= token.span.start() && token.span.end() <= span.end()))
        .map(|token| (token.lexeme.clone(), token.span))
        .collect()
}

/// A module reached only through another module's imports lends its types to
/// the signatures that mention them, but its name is not bound here.
fn report_unimported_modules(
    heads: &[(String, Span)],
    module: &Module,
    discovered: &[DiscoveredFile],
    reporter: &mut Reporter,
) {
    let bound: HashSet<&str> = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Use(decl) => decl.alias.as_deref().or(decl.path.last().map(String::as_str)),
            Item::Struct(decl) => Some(decl.name.as_str()),
            Item::Enum(decl) => Some(decl.name.as_str()),
            Item::Trait(decl) => Some(decl.name.as_str()),
            _ => None,
        })
        .chain(module.name.as_ref().map(|decl| decl.name.as_str()))
        .collect();
    for (head, span) in heads {
        if !bound.contains(head.as_str()) && discovered.iter().any(|file| &file.qualifier == head) {
            reporter.push(Diagnostic::error(
                "PACO-E1001",
                *span,
                format!("module `{head}` is not imported in this file; add a `use` declaration for it"),
            ));
        }
    }
}

fn parse_source(
    path: &std::path::Path,
    source: String,
    sources: &mut SourceMap,
    reporter: &mut Reporter,
) -> Result<(Module, Vec<(String, Span)>), String> {
    let file_id = sources.add_file(path.display().to_string(), source);
    let source_ref = sources.source(file_id).unwrap_or("");
    let tokens = lex(source_ref, file_id, reporter);
    if reporter.has_errors() {
        return Err(reporter.emit_to_string(sources));
    }
    let module = parse_module(&tokens, reporter).map_err(|_| reporter.emit_to_string(sources))?;
    if reporter.has_errors() {
        return Err(reporter.emit_to_string(sources));
    }
    let heads = qualified_heads(&tokens, &module);
    Ok((module, heads))
}

/// What every recursive step of `use`-discovery needs but never mutates:
/// the entry file's directory (a `Plain`-kind path's root) and the
/// project's `paco.mod`/`paco.lock`/cache (a `Domain`-kind path's, once
/// resolved) — bundled so `discover_one` stays under clippy's argument
/// count, not for any deeper reason.
struct DiscoveryContext<'a> {
    entry_dir: &'a std::path::Path,
    project: Option<&'a Result<ProjectManifest, String>>,
}

fn discover_used_files(
    entry_dir: &std::path::Path,
    entry_module: &Module,
    project: Option<&Result<ProjectManifest, String>>,
    sources: &mut SourceMap,
    reporter: &mut Reporter,
) -> Result<Vec<DiscoveredFile>, String> {
    let mut discovered: Vec<DiscoveredFile> = Vec::new();
    let mut resolved: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut on_stack: Vec<PathBuf> = Vec::new();
    let ctx = DiscoveryContext { entry_dir, project };

    for item in &entry_module.items {
        if let Item::Use(decl) = item {
            discover_one(decl, &ctx, &mut discovered, &mut resolved, &mut on_stack, sources, reporter)?;
        }
    }

    Ok(discovered)
}

fn discover_one(
    decl: &paco_syntax::ast::UseDecl,
    ctx: &DiscoveryContext<'_>,
    discovered: &mut Vec<DiscoveredFile>,
    resolved: &mut std::collections::HashSet<PathBuf>,
    on_stack: &mut Vec<PathBuf>,
    sources: &mut SourceMap,
    reporter: &mut Reporter,
) -> Result<(), String> {
    let use_path = &decl.path;
    if use_path.len() == 2 && use_path[0] == "stdlib" && use_path[1] == "core" {
        let core_dir = stdlib_root().join("core");
        if resolved.contains(&core_dir) {
            return Ok(());
        }
        let module = load_prelude_from(&stdlib_root(), sources)?.ok_or_else(|| {
            format!("cannot find module `stdlib::core`: expected a directory at `{}`", core_dir.display())
        })?;
        resolved.insert(core_dir);
        discovered.push(DiscoveredFile { qualifier: "core".to_string(), module, qualified_heads: Vec::new() });
        return Ok(());
    }

    let file_path = match decl.kind {
        UsePathKind::Plain => match moved_std_module(use_path) {
            Some((old, new)) => resolve_moved_std_module(old, new, decl.span, ctx, sources, reporter)?,
            None => use_path_to_file(use_path, ctx.entry_dir),
        },
        UsePathKind::Domain => resolve_domain_use_path(ctx.project, use_path)?,
    };
    if resolved.contains(&file_path) {
        return Ok(());
    }
    if on_stack.contains(&file_path) {
        return Err(format!("circular `use` involving `{}`", file_path.display()));
    }
    if !file_path.exists() {
        return Err(format!(
            "cannot find module `{}`: expected a file at `{}`",
            use_path.join("::"),
            file_path.display()
        ));
    }

    on_stack.push(file_path.clone());
    let (module, qualified_heads) = parse_file(&file_path, sources, reporter)?;
    for item in &module.items {
        if let Item::Use(decl) = item {
            discover_one(decl, ctx, discovered, resolved, on_stack, sources, reporter)?;
        }
    }
    on_stack.pop();

    resolved.insert(file_path);
    let qualifier = use_path.last().expect("use path is never empty").clone();
    discovered.push(DiscoveredFile { qualifier, module, qualified_heads });
    Ok(())
}

/// A project's `paco.mod`/`paco.lock`/package cache, loaded once per
/// compile so a `Domain`-kind `use` path can be resolved without
/// re-reading them per path (`git-module-fetch` task 6.1). `project_dir`
/// (the directory containing `paco.mod`) is what a `path`-kind
/// dependency's own relative path resolves against (task 8.4).
struct ProjectManifest {
    manifest: manifest::Manifest,
    lock: manifest::Lockfile,
    cache_root: PathBuf,
    project_dir: PathBuf,
}

/// `Some(Err(..))` only if `paco.mod` is present next to the entry file but
/// broken somehow; `None` if there simply is no `paco.mod` there (a
/// `Domain`-kind `use` path is then always an error, but a program with no
/// domain-shaped `use` paths at all is unaffected either way).
fn load_project_manifest(entry_dir: &std::path::Path) -> Option<Result<ProjectManifest, String>> {
    let manifest_path = entry_dir.join("paco.mod");
    if !manifest_path.is_file() {
        return None;
    }
    Some((|| {
        let manifest = manifest::Manifest::read(&manifest_path)?;
        let lock_path = entry_dir.join("paco.lock");
        let lock = if lock_path.is_file() { manifest::Lockfile::read(&lock_path)? } else { manifest::Lockfile::default() };
        let cache_root = pkg_cache::pkg_cache_root(|name| std::env::var_os(name))
            .ok_or_else(|| "cannot determine a package cache directory: set PACO_PKG_CACHE or HOME".to_string())?;
        Ok(ProjectManifest { manifest, lock, cache_root, project_dir: entry_dir.to_path_buf() })
    })())
}

/// The running compiler's version: `PACO_VERSION_OVERRIDE` (so a test can
/// exercise a version-range mismatch without bumping this crate's own
/// `Cargo.toml` version), else `CARGO_PKG_VERSION`.
fn running_version() -> (u64, u64, u64) {
    let text = std::env::var("PACO_VERSION_OVERRIDE").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
    let mut parts = text.split('.').map(|part| part.parse::<u64>().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

/// Checks `paco.mod`'s optional `paco` field — for the root project and
/// every already-fetched dependency's own `paco.mod`, if it has one —
/// against the running compiler's version, before any compilation happens
/// (spec: "`paco.mod` may declare the compiler versions a module
/// supports"). A dependency that has not been fetched yet is skipped here;
/// resolving an actual `use` of it already reports its own clear error.
fn check_compiler_version(project: Option<&Result<ProjectManifest, String>>) -> Result<(), String> {
    let Some(project) = project else { return Ok(()) };
    let project = project.as_ref().map_err(String::clone)?;
    let running = running_version();
    let show = |version: (u64, u64, u64)| format!("{}.{}.{}", version.0, version.1, version.2);
    if let Some(range) = &project.manifest.paco
        && !range.contains(running)
    {
        return Err(format!(
            "`{}` requires paco `{}`, but the running compiler is `{}`; run a compatible paco version",
            project.manifest.module,
            range.as_str(),
            show(running)
        ));
    }
    for (dependency_path, source) in &project.manifest.dependencies {
        // A `path` dependency's own `paco.mod` (if any) is out of scope
        // here — no cache directory exists for it to look up.
        let Some(key) = source.cache_key() else { continue };
        let dependency_manifest_path = pkg_cache::cache_dir_for(&project.cache_root, dependency_path, key).join("paco.mod");
        if !dependency_manifest_path.is_file() {
            continue;
        }
        let dependency_manifest = manifest::Manifest::read(&dependency_manifest_path)?;
        if let Some(range) = &dependency_manifest.paco
            && !range.contains(running)
        {
            return Err(format!(
                "`{dependency_path}` requires paco `{}`, but the running compiler is `{}`; run a compatible paco version",
                range.as_str(),
                show(running)
            ));
        }
    }
    Ok(())
}

/// Resolves a `Domain`-kind `use` path's segments to a local file, per
/// design.md's "longest-prefix match, then a rooted `use_path_to_file`"
/// decision: matches `path` against `paco.mod`'s declared dependencies by
/// longest segment-prefix, then — for a `path` dependency (task 8.4), the
/// unmatched remainder resolves directly under that local directory, no
/// cache or lock involved at all; for a tag/branch/rev dependency, first
/// verifies the matched dependency's cache directory is checked out at
/// exactly `paco.lock`'s recorded commit (a mismatch — stale cache, edited
/// lock file, moved tag — is a clear error naming the fix, not a silent
/// build against the wrong code) — then resolves the unmatched remainder
/// (or, if empty, the dependency's own last declared segment) via
/// `use_path_to_file` rooted at `<cache_dir>/src/` or `<local_dir>/src/`.
fn resolve_domain_use_path(project: Option<&Result<ProjectManifest, String>>, path: &[String]) -> Result<PathBuf, String> {
    let joined = path.join(".");
    let not_fetched = || {
        format!(
            "cannot resolve domain-shaped `use {joined}`: no matching entry in `paco.mod` (add one and run `paco get`), \
             or it has not been fetched yet (run `paco get`)"
        )
    };
    let project = match project {
        None => return Err(not_fetched()),
        Some(Err(error)) => return Err(format!("cannot resolve domain-shaped `use {joined}`: {error}")),
        Some(Ok(project)) => project,
    };
    let Some((dependency, remainder)) = manifest::longest_prefix_match(&project.manifest.dependencies, path) else {
        return Err(not_fetched());
    };
    let (dependency_path, source) = dependency;
    let last_declared_segment = || manifest::key_segments(dependency_path).pop().expect("dependency path is never empty");
    let effective_path = |remainder: Vec<String>| if remainder.is_empty() { vec![last_declared_segment()] } else { remainder };

    if let manifest::DependencySource::Path(local_path) = source {
        let local_dir = project.project_dir.join(local_path);
        return Ok(use_path_to_file(&effective_path(remainder), &local_dir.join("src")));
    }

    let key = source.cache_key().expect("every DependencySource besides Path has a cache key");
    let Some(locked) = project.lock.find(dependency_path).filter(|locked| locked.matches_source(source)) else {
        return Err(not_fetched());
    };
    let cache_dir = pkg_cache::cache_dir_for(&project.cache_root, dependency_path, key);
    if !cache_dir.is_dir() {
        return Err(not_fetched());
    }
    let actual_commit =
        git::resolve_ref(&cache_dir, "HEAD").map_err(|error| format!("cannot resolve domain-shaped `use {joined}`: {error}"))?;
    if actual_commit != locked.commit {
        return Err(format!(
            "cannot resolve domain-shaped `use {joined}`: the cached copy of `{dependency_path}` is at commit `{actual_commit}`, \
             but paco.lock expects `{}`; run `paco get`",
            locked.commit
        ));
    }
    Ok(use_path_to_file(&effective_path(remainder), &cache_dir.join("src")))
}

fn direct_imports<'a>(module: &Module, discovered: &'a [DiscoveredFile]) -> Vec<(String, &'a Module)> {
    let mut imports = Vec::new();
    for item in &module.items {
        let Item::Use(decl) = item else { continue };
        let bound_name = decl
            .alias
            .clone()
            .unwrap_or_else(|| decl.path.last().expect("use path is never empty").clone());
        let target_qualifier = decl.path.last().expect("use path is never empty");
        if let Some(found) = discovered.iter().find(|file| &file.qualifier == target_qualifier) {
            imports.push((bound_name, &found.module));
        }
    }
    imports
}

/// Direct imports plus every other module the program pulled in, whose types
/// can reach this module through the signatures it imports.
fn visible_imports<'a>(module: &Module, discovered: &'a [DiscoveredFile]) -> Vec<(String, &'a Module)> {
    let mut imports = direct_imports(module, discovered);
    for file in discovered {
        let already = imports.iter().any(|(_, imported)| std::ptr::eq(*imported, &file.module));
        if !already && file.qualifier != "core" && !std::ptr::eq(&file.module, module) {
            imports.push((file.qualifier.clone(), &file.module));
        }
    }
    imports
}

/// `PACO_STD`, then the distribution's `lib/paco/stdlib` next to the running
/// executable (`paco-link`'s `Toolchain::locate()`, the same lookup that
/// finds `lib/paco/<target>/`), then the standard library next to the
/// compiler's own sources (a development checkout).
fn stdlib_root() -> PathBuf {
    if let Some(path) = std::env::var_os("PACO_STD") {
        return PathBuf::from(path);
    }
    if let Some(distribution) = paco_link::toolchain::Toolchain::locate().distribution {
        let stdlib_dir = distribution.join("stdlib");
        if stdlib_dir.is_dir() {
            return stdlib_dir;
        }
    }
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib")
}

/// A file whose name ends in `_test.paco` is a test file (`src/foo_test.paco`,
/// `tests/foo_test.paco`) — the one place this suffix check lives; every other
/// task in this change calls this helper rather than inlining the suffix
/// check itself.
fn is_test_file(path: &std::path::Path) -> bool {
    path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with("_test.paco"))
}

/// `#[test]` only makes sense inside a `_test.paco` file (`is_test_file`);
/// tagging a function anywhere else is `PACO-E1002`. `DiscoveredFile` carries
/// no path of its own, so a function's file is recovered from its own span
/// the same way `shapes_file` recovers a span's file.
fn validate_test_attribute_placement(module: &Module, sources: &SourceMap, reporter: &mut Reporter) {
    for item in &module.items {
        let Item::Fn(function) = item else { continue };
        if !function.attrs.iter().any(|attr| attr.name == "test") {
            continue;
        }
        let Ok(location) = sources.location(function.span) else { continue };
        if is_test_file(std::path::Path::new(&location.file_name)) {
            continue;
        }
        reporter.push(Diagnostic::error(
            "PACO-E1002",
            function.span,
            format!(
                "function `{}` is tagged `#[test]` but `{}` is not a `_test.paco` file",
                function.name, location.file_name
            ),
        ));
    }
}

/// A `stdlib/core` file is part of the prelude, so it is checked as the merged
/// prelude module rather than alongside a second copy of itself.
fn is_prelude_file(file: &std::path::Path) -> bool {
    let (Ok(file), Ok(core_dir)) = (file.canonicalize(), stdlib_root().join("core").canonicalize()) else {
        return false;
    };
    file.parent() == Some(core_dir.as_path())
}

fn load_prelude(sources: &mut SourceMap) -> Result<Option<Module>, String> {
    load_prelude_from(&stdlib_root(), sources)
}

fn load_prelude_from(std_root: &std::path::Path, sources: &mut SourceMap) -> Result<Option<Module>, String> {
    let core_dir = std_root.join("core");
    if !core_dir.is_dir() {
        return Ok(None);
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(&core_dir)
        .map_err(|error| format!("failed to read `{}`: {error}", core_dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "paco"))
        .filter(|path| !is_test_file(path))
        .collect();
    entries.sort();

    let mut merged_items = Vec::new();
    let mut reporter = Reporter::new();
    for path in entries {
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("failed to read `{}`: {error}", path.display())),
        };
        let (module, _) = parse_source(&path, source, sources, &mut reporter)?;
        merged_items.extend(module.items);
    }

    Ok(Some(Module {
        name: None,
        items: merged_items,
        span: paco_span::Span::new_root(0, 0),
    }))
}

fn simple_ty_name(ty: &Ty) -> String {
    match ty {
        Ty::Path(path, _) | Ty::Generic { path, .. } if path.len() == 1 => path[0].clone(),
        Ty::Slice(..) => paco_types::SLICE_TYPE_NAME.to_string(),
        _ => panic!("methods block target must be a simple named type: {ty:?}"),
    }
}

fn build_file(file: PathBuf, options: BuildOptions) -> Result<DriverOutput, String> {
    let output = file.with_extension(std::env::consts::EXE_EXTENSION);
    let compilation = compile(&file, options, &output)?;
    Ok(DriverOutput { stdout: compilation.stdout, stderr: compilation.stderr })
}

/// The executable that compiles: `paco-compile`, next to the `paco`
/// launcher.
pub const COMPILER_EXE: &str = "paco-compile";

/// What identifies this compiler build in cache keys: its version and the
/// compiling executable's path, size and modification time.
pub fn compiler_identity() -> String {
    let exe = std::env::current_exe().ok().map(|exe| {
        let compiler = exe.with_file_name(COMPILER_EXE);
        if compiler.is_file() { compiler } else { exe }
    });
    let stamp = exe.as_ref().and_then(|exe| fs::metadata(exe).ok()).map(|metadata| {
        let modified = metadata.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
        format!("{}:{}", metadata.len(), modified.map_or(0, |time| time.as_nanos()))
    });
    format!("{} {} {}", env!("CARGO_PKG_VERSION"), exe.map(|exe| exe.display().to_string()).unwrap_or_default(), stamp.unwrap_or_default())
}

/// Everything besides source files that decides what `paco run` builds.
fn run_key(compiler: &str) -> cache::Key {
    let host = paco_link::host_triple();
    let toolchain = paco_link::toolchain::Toolchain::locate();
    let mut settings = vec![
        "debug cranelift".to_string(),
        host.to_string(),
        format!("sysroot={:?}", std::env::var_os("PACO_SYSROOT")),
        format!("sanitize={:?}", std::env::var_os("PACO_SANITIZE")),
        format!("system-alloc={:?}", std::env::var_os("PACO_SYSTEM_ALLOC")),
        format!("stdlib={}", stdlib_root().display()),
    ];
    for target in [host.replace("-gnu", "-musl"), host.replace("-musl", "-gnu")] {
        let archive = toolchain.component(&target, paco_link::toolchain::RUNTIME_ARCHIVE).ok();
        let stamp = archive.as_ref().and_then(|path| fs::metadata(path).ok()).map(|metadata| {
            (metadata.len(), metadata.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()))
        });
        settings.push(format!("{target}={archive:?}:{stamp:?}"));
    }
    cache::Key { compiler: compiler.to_string(), settings }
}

/// The binary `paco run` executes for `file`: from `cache` when nothing it
/// was built from changed, otherwise built (Cranelift, debug) and cached.
pub fn build_cached(file: &std::path::Path, cache: &cache::Cache, compiler: &str) -> Result<cache::Entry, String> {
    let key = run_key(compiler);
    if let Some(entry) = cache.lookup(file, &key) {
        return Ok(entry);
    }
    let scratch = cache.scratch()?;
    let output = scratch.join("program").with_extension(std::env::consts::EXE_EXTENSION);
    let options = BuildOptions {
        release: false,
        target: None,
        backend: BackendChoice::Cranelift,
        link: None,
        sysroot: std::env::var_os("PACO_SYSROOT").map(PathBuf::from),
    };
    let entry = compile(file, options, &output)
        .and_then(|compilation| cache.publish(file, &key, &output, compilation.inputs, &compilation.stdout, &compilation.stderr));
    let _ = fs::remove_dir_all(&scratch);
    entry
}

/// What a successful build printed at compile time and every source file
/// it read.
pub(crate) struct Compilation {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) inputs: Vec<cache::Input>,
}

thread_local! {
    static LINKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static OBJECTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many programs this thread has linked.
pub fn link_count() -> usize {
    LINKS.with(std::cell::Cell::get)
}

/// How many object files the last build on this thread linked: one, from
/// the build's single backend.
pub fn last_object_count() -> usize {
    OBJECTS.with(std::cell::Cell::get)
}

/// Builds `file` into the executable `output`; object files go next to it.
fn compile(file: &std::path::Path, options: BuildOptions, output: &std::path::Path) -> Result<Compilation, String> {
    let file = file.to_path_buf();
    let source = fs::read_to_string(&file)
        .map_err(|error| format!("failed to read `{}`: {error}", file.display()))?;
    let mut sources = SourceMap::new();
    let file_id = sources.add_file(file.display().to_string(), source);
    let source_ref = sources.source(file_id).unwrap_or("");
    let mut reporter = Reporter::new();

    let tokens = lex(source_ref, file_id, &mut reporter);
    if reporter.has_errors() {
        return Err(emit(&reporter, &sources));
    }
    let module =
        parse_module(&tokens, &mut reporter).map_err(|_| emit(&reporter, &sources))?;
    if reporter.has_errors() {
        return Err(emit(&reporter, &sources));
    }
    let heads = qualified_heads(&tokens, &module);

    compile_parsed_module(module, heads, &file, sources, reporter, None, options, output)
}

/// Runs the resolve → prelude → lower → codegen → link pipeline `compile`
/// runs, against an already-built `module` instead of one read fresh from
/// a single file on disk — `compile`'s own analogue of how
/// `check_program_at` splits into `check_parsed_module` (mirrored closely,
/// per `paco test`'s dispatch: this is the piece that lets a
/// driver-synthesized module, e.g. a colocated `_test.paco` file's items
/// merged into its production module via `merge_test_items` plus a
/// synthesized `main`, go all the way through codegen and linking, not
/// just the check pipeline). `file` still anchors error locations and,
/// absent `use_resolution_dir`, `use` resolution; `use_resolution_dir` is
/// `paco test`'s `tests/`-directory external-visibility mode, exactly as
/// for `check_program_at`.
#[allow(clippy::too_many_arguments)]
fn compile_parsed_module(
    module: Module,
    qualified_heads: Vec<(String, Span)>,
    file: &std::path::Path,
    mut sources: SourceMap,
    mut reporter: Reporter,
    use_resolution_dir: Option<PathBuf>,
    options: BuildOptions,
    output: &std::path::Path,
) -> Result<Compilation, String> {
    let BuildOptions { release, target: requested_target, backend, link, sysroot } = options;
    let profile = if release { paco_mir::Profile::Release } else { paco_mir::Profile::Debug };

    let entry_dir = use_resolution_dir.unwrap_or_else(|| file.parent().unwrap_or(std::path::Path::new(".")).to_path_buf());
    let project = load_project_manifest(&entry_dir);
    check_compiler_version(project.as_ref())?;
    let discovered = discover_used_files(&entry_dir, &module, project.as_ref(), &mut sources, &mut reporter)?;
    report_unimported_modules(&qualified_heads, &module, &discovered, &mut reporter);
    validate_test_attribute_placement(&module, &sources, &mut reporter);
    for discovered_file in &discovered {
        report_unimported_modules(&discovered_file.qualified_heads, &discovered_file.module, &discovered, &mut reporter);
        validate_test_attribute_placement(&discovered_file.module, &sources, &mut reporter);
    }
    if reporter.has_errors() {
        return Err(emit(&reporter, &sources));
    }
    let prelude = load_prelude(&mut sources)?;
    let mut imports = visible_imports(&module, &discovered);
    if let Some(prelude_module) = &prelude {
        imports.push((String::new(), prelude_module));
    }
    let mut comptime_output = lowering::ComptimeOutput::default();
    let expanded = expand_derives_with_output(&module, &imports, &mut reporter, &mut comptime_output);
    apply_tensor_missing_library_hint(&mut reporter, project.as_ref());
    let module = expanded.map_err(|_| emit(&reporter, &sources))?;

    if !module
        .items
        .iter()
        .any(|item| matches!(item, Item::Fn(function) if function.name == "main"))
    {
        reporter.push(Diagnostic::error(
            "PACO-E0001",
            module.span,
            "main function was not found",
        ));
        return Err(emit(&reporter, &sources));
    }

    let first_extern = std::iter::once(&module)
        .chain(discovered.iter().map(|discovered_file| &discovered_file.module))
        .flat_map(|module| &module.items)
        .find_map(|item| match item {
            Item::Extern(block) => Some(block.span),
            _ => None,
        });
    let sanitized = std::env::var_os("PACO_SANITIZE").is_some_and(|name| !name.is_empty());
    let link = if sanitized { Some(LinkChoice::Dynamic) } else { link };
    let (link_mode, triple) = resolve_target(requested_target.as_deref(), link, first_extern.is_some());
    if let (paco_link::LinkMode::Static, Some(span)) = (link_mode, first_extern) {
        reporter.push(Diagnostic::error(
            "PACO-E0804",
            span,
            format!(
                "`extern` blocks need dynamic linking against the target's C libraries, but `{triple}` is a static build; \
                 use `--link dynamic` or a `-gnu` target"
            ),
        ));
        return Err(emit(&reporter, &sources));
    }

    let mut layout_imports = imports.clone();
    layout_imports.extend(discovered.iter().map(|file| (file.qualifier.clone(), &file.module)));
    let layouts = paco_mir::TypeLayouts::from_module_with_imports(&module, &layout_imports);
    let mut externs = extern_signatures(&module);
    for discovered_file in &discovered {
        externs.extend(extern_signatures(&discovered_file.module));
    }
    let mut module_contexts = module_contexts(&module, &imports, &discovered, prelude.as_ref(), &mut reporter)
        .map_err(|_| emit(&reporter, &sources))?;
    label_origins(&mut module_contexts, &sources);
    let mut modules: Vec<&Module> = vec![&module];
    modules.extend(discovered.iter().map(|file| &file.module));
    modules.extend(prelude.as_ref());
    let lowering::Lowered { bodies, differentiated } =
        lowering::lower_evaluating_comptime(&module_contexts, &layouts, &externs, &modules, &sources, profile, &mut reporter, &mut comptime_output)
            .map_err(|_| emit(&reporter, &sources))?;
    if differentiated {
        externs.extend(paco_mir::autodiff::runtime_externs());
    }

    let target = paco_mir::Target { triple: Some(triple.clone()), profile };
    let object = match backend {
        BackendChoice::Cranelift => {
            let backend = paco_codegen_cranelift::CraneliftBackend::new(externs, &layouts).with_sources(&sources);
            emit_object(backend, &bodies, &target)
        }
        BackendChoice::Llvm => llvm_backend(externs, &layouts, &sources).and_then(|backend| emit_object(backend, &bodies, &target)),
    };
    let objects = vec![(output.with_extension("o"), object)];
    OBJECTS.with(|count| count.set(objects.len()));
    let mut object_paths = Vec::new();
    for (path, object) in objects {
        let bytes = object.map_err(|error| format!("codegen error: {error}"))?;
        fs::write(&path, &bytes).map_err(|error| format!("failed to write object file: {error}"))?;
        object_paths.push(path);
    }

    let extra_libs: Vec<String> = discovered
        .iter()
        .filter(|discovered_file| discovered_file.module.items.iter().any(|item| matches!(item, Item::Extern(_))))
        .map(|discovered_file| discovered_file.qualifier.clone())
        .collect();
    let link_result = paco_link::link_program(&paco_link::LinkRequest {
        objects: &object_paths,
        output,
        mode: link_mode,
        extra_libs: &extra_libs,
        target: &triple,
        sysroot: sysroot.as_deref(),
        debug: profile == paco_mir::Profile::Debug,
    })
    .map_err(|error| if error.starts_with("error[") { error } else { format!("link error: {error}") });
    for path in &object_paths {
        let _ = fs::remove_file(path);
    }
    link_result?;
    LINKS.with(|links| links.set(links.get() + 1));

    let mut inputs: Vec<cache::Input> = vec![cache::Input::directory(stdlib_root().join("core"))];
    for index in 0.. {
        let id = paco_span::FileId::new(index);
        let (Some(name), Some(text)) = (sources.file_name(id), sources.source(id)) else { break };
        let path = fs::canonicalize(name).unwrap_or_else(|_| PathBuf::from(name));
        if !inputs.iter().any(|input| input.path == path) {
            inputs.push(cache::Input::file(path, text.as_bytes()));
        }
    }
    // A successful build still runs `emit()` when there are warnings (e.g.
    // the moved-`stdlib`-module deprecation, task 4.1) — otherwise nothing
    // would ever surface a warning that did not also fail the build.
    let stderr = if reporter.diagnostics().is_empty() { comptime_output.stderr } else { emit(&reporter, &sources) };
    Ok(Compilation { stdout: comptime_output.stdout, stderr, inputs })
}

/// Gives every dimension name the `file:line` a run-time `DimError` names.
fn label_origins(contexts: &mut [ModuleContext<'_>], sources: &SourceMap) {
    for context in contexts {
        context.typed.label_origins(|span| {
            sources.location(span).map(|location| format!("{}:{}", location.file_name, location.start.line)).unwrap_or_default()
        });
    }
}

/// Type- and borrow-checks the entry module (already checked for
/// diagnostics), every discovered module and the prelude, for lowering.
fn module_contexts<'a>(
    module: &'a Module,
    imports: &[(String, &'a Module)],
    discovered: &'a [DiscoveredFile],
    prelude: Option<&'a Module>,
    reporter: &mut Reporter,
) -> Result<Vec<ModuleContext<'a>>, ()> {
    let typed = paco_types::infer_module_with_imports(module, imports, reporter).map_err(|_| ())?;
    let drops = paco_borrow::analyze_typed_module(module, imports, Some(&typed), reporter).map_err(|_| ())?;
    let registry = paco_mir::TypeRegistry::from_module_with_imports(module, imports);
    let mut contexts = vec![ModuleContext { module, qualifier: String::new(), typed, registry, drops }];
    for discovered_file in discovered {
        let mut file_imports = visible_imports(&discovered_file.module, discovered);
        if discovered_file.qualifier != "core"
            && let Some(prelude_module) = prelude
        {
            file_imports.push((String::new(), prelude_module));
        }
        let typed = paco_types::infer_module_with_imports(&discovered_file.module, &file_imports, reporter).map_err(|_| ())?;
        let drops = paco_borrow::analyze_typed_module(&discovered_file.module, &file_imports, Some(&typed), reporter).map_err(|_| ())?;
        let registry = paco_mir::TypeRegistry::from_module_with_imports(&discovered_file.module, &file_imports)
            .with_own_qualifier(&discovered_file.qualifier);
        contexts.push(ModuleContext {
            module: &discovered_file.module,
            qualifier: discovered_file.qualifier.clone(),
            typed,
            registry,
            drops,
        });
    }
    if !discovered.iter().any(|discovered_file| discovered_file.qualifier == "core")
        && let Some(prelude_module) = prelude
        && let Some((typed, drops, registry)) = lower_prelude_module(prelude_module)
    {
        contexts.push(ModuleContext { module: prelude_module, qualifier: String::new(), typed, registry, drops });
    }
    Ok(contexts)
}

fn emit_object(
    mut backend: impl paco_mir::Backend,
    bodies: &[(String, paco_mir::Body)],
    target: &paco_mir::Target,
) -> Result<paco_mir::ObjectFile, String> {
    for (name, body) in bodies {
        backend.lower_body(name, body)?;
    }
    backend.finish(target)
}

#[cfg(feature = "llvm")]
fn llvm_backend<'a>(
    externs: Vec<(String, Vec<paco_types::Type>, paco_types::Type)>,
    layouts: &'a paco_mir::TypeLayouts<'a>,
    sources: &'a SourceMap,
) -> Result<paco_codegen_llvm::LlvmBackend<'a>, String> {
    Ok(paco_codegen_llvm::LlvmBackend::new(externs, layouts).with_sources(sources))
}

#[cfg(not(feature = "llvm"))]
fn llvm_backend<'a>(
    _externs: Vec<(String, Vec<paco_types::Type>, paco_types::Type)>,
    _layouts: &'a paco_mir::TypeLayouts<'a>,
    _sources: &'a SourceMap,
) -> Result<paco_codegen_cranelift::CraneliftBackend<'a>, String> {
    Err("the LLVM backend (`paco build --release`) is not built in; rebuild paco with the `llvm` feature".to_string())
}

fn check_file(file: PathBuf) -> Result<DriverOutput, String> {
    let CheckedProgram { sources, module, mut reporter, discovered, prelude, mut output } = check_program(file)?;
    let mut modules: Vec<&Module> = vec![&module];
    modules.extend(discovered.iter().map(|file| &file.module));
    modules.extend(prelude.as_ref());
    if !reporter.has_errors() && (lowering::has_comptime(&modules) || modules.iter().any(|module| find_grad_call(module).is_some())) {
        let mut imports = visible_imports(&module, &discovered);
        imports.extend(prelude.as_ref().map(|prelude| (String::new(), prelude)));
        let mut layout_imports = imports.clone();
        layout_imports.extend(discovered.iter().map(|file| (file.qualifier.clone(), &file.module)));
        let layouts = paco_mir::TypeLayouts::from_module_with_imports(&module, &layout_imports);
        let externs: Vec<_> = std::iter::once(&module).chain(discovered.iter().map(|file| &file.module)).flat_map(extern_signatures).collect();
        let mut contexts = module_contexts(&module, &imports, &discovered, prelude.as_ref(), &mut reporter)
            .map_err(|_| emit(&reporter, &sources))?;
        label_origins(&mut contexts, &sources);
        lowering::lower_evaluating_comptime(&contexts, &layouts, &externs, &modules, &sources, paco_mir::Profile::Debug, &mut reporter, &mut output)
            .map_err(|_| emit(&reporter, &sources))?;
    }
    let stderr = if reporter.diagnostics().is_empty() {
        output.stderr
    } else {
        emit(&reporter, &sources)
    };
    Ok(DriverOutput { stdout: output.stdout, stderr })
}

fn format_file(file: PathBuf, write: bool) -> Result<DriverOutput, String> {
    let source = fs::read_to_string(&file)
        .map_err(|error| format!("failed to read `{}`: {error}", file.display()))?;
    let mut sources = SourceMap::new();
    let file_id = sources.add_file(file.display().to_string(), source);
    let source_ref = sources.source(file_id).unwrap_or("");
    let mut reporter = Reporter::new();

    let tokens = lex(source_ref, file_id, &mut reporter);
    if reporter.has_errors() {
        return Err(emit(&reporter, &sources));
    }

    let module =
        parse_module(&tokens, &mut reporter).map_err(|_| emit(&reporter, &sources))?;
    if reporter.has_errors() {
        return Err(emit(&reporter, &sources));
    }

    let formatted = paco_syntax::fmt::format_module(&module, Some(source_ref));

    if write {
        fs::write(&file, &formatted)
            .map_err(|error| format!("failed to write `{}`: {error}", file.display()))?;
        Ok(DriverOutput {
            stdout: String::new(),
            stderr: String::new(),
        })
    } else {
        Ok(DriverOutput {
            stdout: formatted,
            stderr: String::new(),
        })
    }
}

fn find_grad_call(module: &Module) -> Option<paco_span::Span> {
    use paco_syntax::ast::Visit;
    struct Finder<'a> {
        aliases: Vec<&'a str>,
        found: Option<paco_span::Span>,
    }
    impl Visit for Finder<'_> {
        fn visit_expr(&mut self, expr: &Expr) {
            if self.found.is_none()
                && let Expr::AssociatedCall { ty: Ty::Path(path, _), function, span, .. } = expr
                && function == "grad"
                && path.len() == 1
                && self.aliases.contains(&path[0].as_str())
            {
                self.found = Some(*span);
            }
            paco_syntax::ast::walk_expr(self, expr);
        }
    }
    let aliases = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Use(decl) if decl.path == ["stdlib", "autodiff"] => Some(decl.alias.as_deref().unwrap_or("autodiff")),
            _ => None,
        })
        .collect::<Vec<_>>();
    if aliases.is_empty() {
        return None;
    }
    let mut finder = Finder { aliases, found: None };
    finder.visit_module(module);
    finder.found
}

struct CheckedProgram {
    sources: SourceMap,
    module: paco_syntax::ast::Module,
    reporter: Reporter,
    discovered: Vec<DiscoveredFile>,
    prelude: Option<Module>,
    output: lowering::ComptimeOutput,
}

fn check_program(file: PathBuf) -> Result<CheckedProgram, String> {
    check_program_at(file, None)
}

/// Like `check_program`, but lets the caller override the directory a
/// `Plain`-kind `use` path resolves against, instead of `file`'s own
/// parent directory. `paco test`'s `tests/`-directory compilation mode
/// (unit-testing design.md's "two directories, two visibility levels"
/// Decision) passes the project root here, so a `tests/<name>_test.paco`
/// file's `use`s resolve the way an external consumer's would, while the
/// file itself still compiles as `module` — so it only gets the
/// `pub`-only access `Program::from_module_with_imports` already gives
/// every `imports` entry, never the same-module access `module` itself
/// gets. `src/<name>_test.paco`'s colocated mode does not need this: it
/// passes `None` and instead merges into the production module before
/// calling `check_parsed_module` (see `merge_test_items`).
fn check_program_at(file: PathBuf, use_resolution_dir: Option<PathBuf>) -> Result<CheckedProgram, String> {
    let source = fs::read_to_string(&file)
        .map_err(|error| format!("failed to read `{}`: {error}", file.display()))?;
    let mut sources = SourceMap::new();
    let file_id = sources.add_file(file.display().to_string(), source);
    let source = sources.source(file_id).unwrap_or("");
    let mut reporter = Reporter::new();

    let tokens = lex(source, file_id, &mut reporter);
    if reporter.has_errors() {
        return Err(emit(&reporter, &sources));
    }

    let module =
        parse_module(&tokens, &mut reporter).map_err(|_| emit(&reporter, &sources))?;
    if reporter.has_errors() {
        return Err(emit(&reporter, &sources));
    }
    let heads = qualified_heads(&tokens, &module);

    check_parsed_module(module, heads, &file, sources, reporter, use_resolution_dir)
}

/// Runs the resolve → prelude → type-check → borrow-check pipeline
/// `check_program_at` runs, against an already-built `module` instead of
/// one read fresh from a single file on disk. `file` still anchors error
/// locations and, absent `use_resolution_dir`, `use` resolution. This is
/// what lets a driver-constructed module — e.g. a colocated
/// `_test.paco` file's items merged into its production module via
/// `merge_test_items` — run through the exact same checks as
/// hand-written code, mirroring how `expand_derives_with_output` already
/// reassigns its own generated-item result into `module` before
/// typecheck.
fn check_parsed_module(
    module: Module,
    qualified_heads: Vec<(String, Span)>,
    file: &std::path::Path,
    mut sources: SourceMap,
    mut reporter: Reporter,
    use_resolution_dir: Option<PathBuf>,
) -> Result<CheckedProgram, String> {
    let entry_dir = use_resolution_dir.unwrap_or_else(|| file.parent().unwrap_or(std::path::Path::new(".")).to_path_buf());
    let project = load_project_manifest(&entry_dir);
    check_compiler_version(project.as_ref())?;
    let discovered = discover_used_files(&entry_dir, &module, project.as_ref(), &mut sources, &mut reporter)?;
    report_unimported_modules(&qualified_heads, &module, &discovered, &mut reporter);
    validate_test_attribute_placement(&module, &sources, &mut reporter);
    for discovered_file in &discovered {
        report_unimported_modules(&discovered_file.qualified_heads, &discovered_file.module, &discovered, &mut reporter);
        validate_test_attribute_placement(&discovered_file.module, &sources, &mut reporter);
    }
    if reporter.has_errors() {
        return Err(emit(&reporter, &sources));
    }
    let prelude = load_prelude(&mut sources)?;
    let (module, prelude) = if is_prelude_file(file) {
        (prelude.unwrap_or(module), None)
    } else {
        (module, prelude)
    };
    let mut imports = visible_imports(&module, &discovered);
    if let Some(prelude_module) = &prelude {
        imports.push((String::new(), prelude_module));
    }

    let mut output = lowering::ComptimeOutput::default();
    let expanded = expand_derives_with_output(&module, &imports, &mut reporter, &mut output);
    apply_tensor_missing_library_hint(&mut reporter, project.as_ref());
    let module = expanded.map_err(|_| emit(&reporter, &sources))?;
    for discovered_file in &discovered {
        let mut file_imports = visible_imports(&discovered_file.module, &discovered);
        if discovered_file.qualifier != "core"
            && let Some(prelude_module) = &prelude
        {
            file_imports.push((String::new(), prelude_module));
        }
        let typed = paco_types::infer_module_with_imports(&discovered_file.module, &file_imports, &mut reporter)
            .map_err(|_| emit(&reporter, &sources))?;
        paco_borrow::check_typed_module(&discovered_file.module, &file_imports, &typed, &mut reporter)
            .map_err(|_| emit(&reporter, &sources))?;
    }

    Ok(CheckedProgram {
        sources,
        module,
        reporter,
        discovered,
        prelude,
        output,
    })
}

/// `src/<name>_test.paco`'s colocated compilation mode (unit-testing
/// design.md's "two directories, two visibility levels" Decision): its
/// items join the production module it tests, so it compiles as part of
/// the *same* module — full, same-module access to private items — rather
/// than a separate module resolving `use`s under `pub`-only visibility
/// like `check_program_at`'s `tests/` mode does. `pub` (no internal
/// caller yet, matching `expand_derives`/`check_generated_item`'s own
/// precedent above): `paco test`'s future discovery/compile step
/// (tasks.md Section 5, not built yet) is the intended caller.
pub fn merge_test_items(mut module: Module, test_module: Module) -> Module {
    module.items.extend(test_module.items);
    module
}

fn resolve_key(qualifier: &str, name: &str) -> String {
    if qualifier.is_empty() { name.to_string() } else { format!("{qualifier}::{name}") }
}

fn resolve_names_from_imports(imports: &[(String, &Module)]) -> HashSet<String> {
    imports
        .iter()
        .flat_map(|(qualifier, imported)| {
            imported.items.iter().filter_map(move |item| match item {
                Item::Fn(function) if function.is_pub => Some(resolve_key(qualifier, &function.name)),
                Item::Struct(decl) if decl.is_pub => Some(resolve_key(qualifier, &decl.name)),
                Item::Enum(decl) if decl.is_pub => Some(resolve_key(qualifier, &decl.name)),
                _ => None,
            })
        })
        .collect()
}

/// `phase-9-comptime` Decision 3/Decision 7, task 5.5: re-entry for a
/// `Code`-producing comptime evaluation's own generated `Item` (e.g. a
/// `#[derive(..)]` expansion's `methods { .. }` block, task 6.2's own
/// future caller) — splices it into `module` and runs it through the same
/// resolve → type-check → borrow-check pipeline `check_program` runs for
/// hand-written code, so a mistake in generated code surfaces as an
/// ordinary diagnostic pointing at it, not a special-cased failure mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GeneratedItemError;

/// The resolve → type-check → borrow-check sequence `check_program` runs
/// for hand-written code, factored out so `check_generated_item` and
/// `expand_derives` can run it against a module they've already spliced
/// generated code into — the same checking, not a separate pass.
fn run_check_pipeline(
    module: &Module,
    imports: &[(String, &Module)],
    reporter: &mut Reporter,
) -> Result<(), GeneratedItemError> {
    let resolve_names = resolve_names_from_imports(imports);
    paco_resolve::resolve_module_with_imports(module, &resolve_names, reporter)
        .map_err(|_| GeneratedItemError)?;
    let typed = paco_types::infer_module_with_imports(module, imports, reporter).map_err(|_| GeneratedItemError)?;
    paco_borrow::check_typed_module(module, imports, &typed, reporter).map_err(|_| GeneratedItemError)?;
    Ok(())
}

pub fn check_generated_item(
    module: &Module,
    generated: Item,
    imports: &[(String, &Module)],
    reporter: &mut Reporter,
) -> Result<Module, GeneratedItemError> {
    let mut augmented = module.clone();
    augmented.items.push(generated);
    run_check_pipeline(&augmented, imports, reporter)?;
    Ok(augmented)
}

/// `phase-9-comptime` Decision 8, tasks 6.2/6.3: expands every
/// `#[derive(Trait, ...)]` found on a struct/enum in `module` — for each
/// trait named, looks up its `#[derivable(fn_name)]` registration
/// (`paco_hir::derivable_traits`), runs `fn_name(the annotated type)` at
/// compile time (`paco-comptime`, sandboxed like any other comptime
/// evaluation), and splices the resulting `Code`'s own item into the
/// module. Once every derive has been
/// collected, the whole expanded module runs through the Section 5
/// re-entry pipeline (`run_check_pipeline`, shared with
/// `check_generated_item`) exactly once — so generated code is
/// name-resolved and type-checked exactly like hand-written code, not a
/// separate trusted path, and a module with no derives at all still gets
/// checked normally. An unregistered trait name reports `PACO-E0353` and
/// is skipped rather than aborting the whole expansion (task 6.3).
pub fn expand_derives(
    module: &Module,
    imports: &[(String, &Module)],
    reporter: &mut Reporter,
) -> Result<Module, GeneratedItemError> {
    expand_derives_with_output(module, imports, reporter, &mut lowering::ComptimeOutput::default())
}

struct DeriveRequest {
    function: String,
    ty: paco_types::Type,
    span: Span,
}

fn expand_derives_with_output(
    module: &Module,
    imports: &[(String, &Module)],
    reporter: &mut Reporter,
    output: &mut lowering::ComptimeOutput,
) -> Result<Module, GeneratedItemError> {
    // `#[derivable(..)]` most often lives on a prelude trait declaration
    // (e.g. `Display`), not the user's own module, so every import's own
    // traits are scanned too, not just `module`'s.
    let mut derivable = paco_hir::derivable_traits(module);
    for (qualifier, imported) in imports {
        derivable.extend(paco_hir::derivable_traits(imported).into_iter().map(|(trait_name, fn_name)| {
            let fn_name = if qualifier.is_empty() { fn_name } else { format!("{qualifier}::{fn_name}") };
            (trait_name, fn_name)
        }));
    }
    let mut requests = Vec::new();
    for item in &module.items {
        let (attrs, ty) = match item {
            Item::Struct(decl) => (&decl.attrs, paco_types::Type::Struct(decl.name.clone(), Vec::new())),
            Item::Enum(decl) => (&decl.attrs, paco_types::Type::Enum(decl.name.clone(), Vec::new())),
            _ => continue,
        };
        for attr in attrs.iter().filter(|attr| attr.name == "derive") {
            for arg in &attr.args {
                let paco_syntax::ast::AttributeArg::Path(path, span) = arg else { continue };
                let [trait_name] = path.as_slice() else { continue };
                if trait_name == "Copy" {
                    continue;
                }
                match derivable.get(trait_name) {
                    Some(function) => requests.push(DeriveRequest { function: function.clone(), ty: ty.clone(), span: *span }),
                    None => reporter.push(Diagnostic::error(
                        "PACO-E0353",
                        *span,
                        format!("cannot derive `{trait_name}`: no `#[derivable(..)]` registration found"),
                    )),
                }
            }
        }
    }
    let mut expanded = module.clone();
    if !requests.is_empty() && !reporter.has_errors() {
        expanded.items.extend(derive_items(module, imports, &requests, reporter, output)?);
    }
    if reporter.has_errors() {
        return Err(GeneratedItemError);
    }
    run_check_pipeline(&expanded, imports, reporter)?;
    Ok(expanded)
}

/// Runs each derive function on its type at compile time and returns the
/// items they generate.
fn derive_items(
    module: &Module,
    imports: &[(String, &Module)],
    requests: &[DeriveRequest],
    reporter: &mut Reporter,
    output: &mut lowering::ComptimeOutput,
) -> Result<Vec<Item>, GeneratedItemError> {
    let roots = requests.iter().map(|request| request.function.clone()).collect();
    let slice = lowering::comptime_slice(module, &roots);
    paco_resolve::resolve_module_with_imports(&slice, &resolve_names_from_imports(imports), reporter)
        .map_err(|_| GeneratedItemError)?;
    let typed = paco_types::infer_module_with_imports(&slice, imports, reporter).map_err(|_| GeneratedItemError)?;
    let drops = paco_borrow::analyze_typed_module(&slice, imports, Some(&typed), reporter).map_err(|_| GeneratedItemError)?;
    let registry = paco_mir::TypeRegistry::from_module_with_imports(&slice, imports);
    let mut contexts = vec![ModuleContext { module: &slice, qualifier: String::new(), typed, registry, drops }];
    for (index, (qualifier, imported)) in imports.iter().enumerate() {
        let others: Vec<(String, &Module)> =
            imports.iter().enumerate().filter(|(other, _)| *other != index).map(|(_, import)| import.clone()).collect();
        let others: &[(String, &Module)] = if qualifier.is_empty() { &[] } else { &others };
        let mut scratch = Reporter::new();
        let Ok(typed) = paco_types::infer_module_with_imports(imported, others, &mut scratch) else { continue };
        let Ok(drops) = paco_borrow::analyze_typed_module(imported, others, Some(&typed), &mut scratch) else { continue };
        let registry = paco_mir::TypeRegistry::from_module_with_imports(imported, others).with_own_qualifier(qualifier);
        contexts.push(ModuleContext { module: imported, qualifier: qualifier.clone(), typed, registry, drops });
    }
    let layouts = paco_mir::TypeLayouts::from_module_with_imports(&slice, imports);
    let mut modules: Vec<&Module> = vec![&slice];
    modules.extend(imports.iter().map(|(_, imported)| *imported));
    let (structs, enums) = lowering::type_decls(&modules);
    let externs = modules.iter().flat_map(|module| extern_signatures(module)).map(|(name, ..)| name).collect();
    let program = paco_comptime::Program { layouts: &layouts, externs, structs, enums };
    let session = lowering::Session::new(&contexts, paco_mir::Profile::Debug);
    let mut provider = lowering::Provider::new(&session, Vec::new());
    let mut items = Vec::new();
    for request in requests {
        let fail = |reporter: &mut Reporter, message: String| {
            reporter.push(Diagnostic::error("PACO-E0350", request.span, format!("comptime evaluation failed: {message}")));
            GeneratedItemError
        };
        let argument = (paco_types::Type::TypeValue(Box::new(request.ty.clone())), paco_mir::ComptimeValue::Type(request.ty.clone()));
        let evaluation = paco_comptime::evaluate(&program, &mut provider, &request.function, &[argument], paco_comptime::Limits::default())
            .map_err(|error| fail(reporter, error.message))?;
        output.stdout.push_str(&evaluation.output);
        output.stderr.push_str(&evaluation.stderr);
        match evaluation.value {
            paco_mir::ComptimeValue::Code(body) => match *body {
                paco_syntax::ast::QuoteBody::Item(item) => items.push(item),
                paco_syntax::ast::QuoteBody::Expr(_) => {
                    return Err(fail(reporter, format!("`{}` produced an expression, not an item", request.function)));
                }
            },
            _ => return Err(fail(reporter, format!("`{}` did not produce a `Code` value", request.function))),
        }
    }
    Ok(items)
}

fn clean(path: Option<PathBuf>, cache: bool) -> Result<DriverOutput, String> {
    if cache {
        let Some(root) = cache::cache_dir(|name| std::env::var_os(name)) else {
            return Err("no build cache directory (set PACO_CACHE)".to_string());
        };
        if root.is_dir() {
            cache::Cache::open(Some(root), cache::Policy::default()).clear()?;
        }
    } else {
        let binary = path.unwrap_or_else(|| PathBuf::from("main.paco")).with_extension(std::env::consts::EXE_EXTENSION);
        for output in [binary.with_extension("o"), binary.with_extension("llvm.o"), cache::debug_file(&binary), binary] {
            match fs::remove_file(&output) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(format!("failed to remove `{}`: {error}", output.display()));
                }
                _ => {}
            }
        }
    }
    Ok(DriverOutput { stdout: String::new(), stderr: String::new() })
}

/// `paco get`: reads `paco.mod`, fetches (or reuses) every declared
/// dependency into the package cache, and writes `paco.lock` recording
/// each one's exact resolved commit (`git-module-fetch` tasks 4.1-4.4).
fn run_get(path: Option<PathBuf>) -> Result<DriverOutput, String> {
    let manifest_path = path.unwrap_or_else(|| PathBuf::from("paco.mod"));
    if !manifest_path.is_file() {
        return Err(format!(
            "no `paco.mod` found at `{}`; `paco get` needs a manifest declaring this module's dependencies",
            manifest_path.display()
        ));
    }
    let manifest = manifest::Manifest::read(&manifest_path)?;
    let project_dir = manifest_path.parent().filter(|dir| !dir.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    let lock_path = project_dir.join("paco.lock");
    let existing_lock = if lock_path.is_file() { manifest::Lockfile::read(&lock_path)? } else { manifest::Lockfile::default() };
    // `path` dependencies need no cache at all (task 8.4); only compute
    // (and require) a cache root when some other dependency needs one.
    let cache_root = if manifest.dependencies.iter().any(|(_, source)| source.cache_key().is_some()) {
        Some(
            pkg_cache::pkg_cache_root(|name| std::env::var_os(name))
                .ok_or_else(|| "cannot determine a package cache directory: set PACO_PKG_CACHE or HOME".to_string())?,
        )
    } else {
        None
    };

    let mut packages = Vec::with_capacity(manifest.dependencies.len());
    let mut stdout = String::new();
    for (dependency_path, source) in &manifest.dependencies {
        // `path` dependencies (task 8.4): no `paco get` fetch step at all,
        // and no `paco.lock` entry.
        let Some(key) = source.cache_key() else { continue };
        let cache_root = cache_root.as_ref().expect("a cache root was computed above whenever any dependency needs one");
        let existing = existing_lock.find(dependency_path).filter(|package| package.matches_source(source));
        let locked_commit = existing.map(|package| package.commit.clone());
        let url = pkg_cache::dependency_url(dependency_path);
        let ref_kind = if matches!(source, manifest::DependencySource::Rev(_)) { pkg_cache::RefKind::Commit } else { pkg_cache::RefKind::TagOrBranch };
        let fetched = pkg_cache::fetch_into_cache(cache_root, dependency_path, key, ref_kind, &url, locked_commit.as_deref())?;
        stdout.push_str(&format!("{dependency_path} {key} => {}\n", fetched.commit));
        let (tag, branch, rev) = match source {
            manifest::DependencySource::Tag(tag) => (Some(tag.clone()), None, None),
            manifest::DependencySource::Branch(branch) => (None, Some(branch.clone()), None),
            manifest::DependencySource::Rev(rev) => (None, None, Some(rev.clone())),
            manifest::DependencySource::Path(_) => unreachable!("path dependencies have no cache key and are skipped above"),
        };
        packages.push(manifest::LockedPackage { path: dependency_path.clone(), tag, branch, rev, commit: fetched.commit });
    }
    manifest::Lockfile { packages }.write(&lock_path)?;
    Ok(DriverOutput { stdout, stderr: String::new() })
}

/// `paco mod tidy`: cross-references `paco.mod`'s declared dependencies
/// against every `Domain`-kind `use` path actually present in the
/// program's `.paco` source files (`git-module-fetch` task 5.1).
fn run_mod_tidy(path: Option<PathBuf>) -> Result<DriverOutput, String> {
    let manifest_path = path.unwrap_or_else(|| PathBuf::from("paco.mod"));
    if !manifest_path.is_file() {
        return Err(format!(
            "no `paco.mod` found at `{}`; `paco mod tidy` needs a manifest to reconcile",
            manifest_path.display()
        ));
    }
    let manifest = manifest::Manifest::read(&manifest_path)?;
    let project_dir = manifest_path.parent().filter(|dir| !dir.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));

    let mut used_paths = Vec::new();
    collect_domain_use_paths(project_dir, &mut used_paths)?;

    let mut stdout = String::new();
    for (dependency_path, _source) in &manifest.dependencies {
        if !manifest::is_used(dependency_path, &used_paths) {
            stdout.push_str(&format!("unused dependency in paco.mod: {dependency_path}\n"));
        }
    }
    for used_path in &used_paths {
        if manifest::longest_prefix_match(&manifest.dependencies, used_path).is_none() {
            stdout.push_str(&format!("undeclared dependency: `use {}` has no matching paco.mod entry\n", used_path.join(".")));
        }
    }
    Ok(DriverOutput { stdout, stderr: String::new() })
}

/// Every `Domain`-kind `use` path in every `.paco` file under `dir`,
/// scanned recursively. Files that fail to lex/parse are skipped (`paco
/// mod tidy` reports usage, not correctness — `paco check`/`paco build`
/// already own diagnosing a broken file).
fn collect_domain_use_paths(dir: &std::path::Path, out: &mut Vec<Vec<String>>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|error| format!("failed to read `{}`: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("failed to read `{}`: {error}", dir.display()))?;
        let entry_path = entry.path();
        if entry_path.is_dir() {
            collect_domain_use_paths(&entry_path, out)?;
            continue;
        }
        if entry_path.extension().is_none_or(|ext| ext != "paco") {
            continue;
        }
        let Ok(source) = fs::read_to_string(&entry_path) else { continue };
        let mut sources = SourceMap::new();
        let mut reporter = Reporter::new();
        let file_id = sources.add_file(entry_path.display().to_string(), source);
        let tokens = lex(sources.source(file_id).unwrap_or(""), file_id, &mut reporter);
        if reporter.has_errors() {
            continue;
        }
        let Ok(module) = parse_module(&tokens, &mut reporter) else { continue };
        for item in &module.items {
            if let Item::Use(decl) = item
                && decl.kind == UsePathKind::Domain
            {
                out.push(decl.path.clone());
            }
        }
    }
    Ok(())
}

/// The `github.com/pacolang/numerics` version `paco fix` declares when it
/// adds the dependency: design.md's "at the latest tag compatible with
/// the running compiler" degrades to this fixed placeholder, since no
/// real `pacolang/numerics` tag has been cut yet (tasks.md task 3.5).
const NUMERICS_DEFAULT_VERSION: &str = "v0.1.0";

/// `paco fix` (`extract-domain-libraries` task 4.3): rewrites every moved
/// `use stdlib::numerics`/`stdlib::math`/`stdlib::blas` line under the package to
/// its `github.com/pacolang/numerics` replacement (keeping any `as`
/// alias); for an unaliased `stdlib::numerics`, also rewrites `numerics::`
/// path segments to `tensor::`; rewrites `.add`/`.sub`/`.mul` calls whose
/// receiver's type-checked static type is `Tensor` to
/// `.checked_add`/`.checked_sub`/`.checked_mul`; and adds the dependency
/// to `paco.mod` when some file needed it. Idempotent: nothing left to
/// rewrite (already-migrated files, an already-declared dependency)
/// changes nothing on a second run.
fn run_fix(path: Option<PathBuf>) -> Result<DriverOutput, String> {
    let manifest_path = path.unwrap_or_else(|| PathBuf::from("paco.mod"));
    if !manifest_path.is_file() {
        return Err(format!("no `paco.mod` found at `{}`; `paco fix` needs a manifest to migrate", manifest_path.display()));
    }
    let project_dir =
        manifest_path.parent().filter(|dir| !dir.as_os_str().is_empty()).unwrap_or(std::path::Path::new(".")).to_path_buf();

    let mut paco_files = Vec::new();
    collect_paco_files(&project_dir, &mut paco_files)?;

    let uses_moved_module = paco_files.iter().any(|file_path| file_uses_moved_std_module(file_path));

    let manifest_text = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("failed to read `{}`: {error}", manifest_path.display()))?;
    let manifest = manifest::Manifest::parse(&manifest_text)?;
    if uses_moved_module && manifest::longest_prefix_match(&manifest.dependencies, &numerics_domain_segments("tensor")).is_none() {
        let updated = add_dependency_line(&manifest_text, NUMERICS_MODULE_PATH, NUMERICS_DEFAULT_VERSION);
        fs::write(&manifest_path, &updated)
            .map_err(|error| format!("failed to write `{}`: {error}", manifest_path.display()))?;
    }

    // Reloaded so a dependency just added above is visible to type-checking below.
    let project = load_project_manifest(&project_dir);
    let mut stdout = String::new();
    for file_path in &paco_files {
        if fix_paco_file(file_path, &project_dir, project.as_ref())? {
            stdout.push_str(&format!("fixed {}\n", file_path.display()));
        }
    }
    Ok(DriverOutput { stdout, stderr: String::new() })
}

fn file_uses_moved_std_module(file_path: &std::path::Path) -> bool {
    let Ok(source) = fs::read_to_string(file_path) else { return false };
    let mut sources = SourceMap::new();
    let mut reporter = Reporter::new();
    let file_id = sources.add_file(file_path.display().to_string(), source);
    let tokens = lex(sources.source(file_id).unwrap_or(""), file_id, &mut reporter);
    if reporter.has_errors() {
        return false;
    }
    let Ok(module) = parse_module(&tokens, &mut reporter) else { return false };
    module.items.iter().any(|item| matches!(item, Item::Use(decl) if moved_std_module(&decl.path).is_some()))
}

/// Every `.paco` file under `dir`, scanned recursively (`collect_domain_use_paths`'s own walk).
fn collect_paco_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|error| format!("failed to read `{}`: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("failed to read `{}`: {error}", dir.display()))?;
        let entry_path = entry.path();
        if entry_path.is_dir() {
            collect_paco_files(&entry_path, out)?;
            continue;
        }
        if entry_path.extension().is_some_and(|ext| ext == "paco") {
            out.push(entry_path);
        }
    }
    Ok(())
}

/// Inserts `"<dependency_path>" = "<version>"` into `manifest_text`'s
/// `[dependencies]` table — right after its header line when one already
/// exists (TOML keeps every key up to the next `[table]` header in the
/// same table, so this stays inside it regardless of what else is
/// there), or in a new table appended at the end otherwise.
fn add_dependency_line(manifest_text: &str, dependency_path: &str, version: &str) -> String {
    let entry_line = format!("\"{dependency_path}\" = \"{version}\"\n");
    let mut updated = manifest_text.to_string();
    if let Some(header_start) = manifest_text.find("[dependencies]") {
        let header_end = manifest_text[header_start..].find('\n').map_or(manifest_text.len(), |offset| header_start + offset + 1);
        updated.insert_str(header_end, &entry_line);
    } else {
        if !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str("\n[dependencies]\n");
        updated.push_str(&entry_line);
    }
    updated
}

/// Rewrites one file's moved `use` lines, the `numerics::` qualifier (when
/// applicable) and `.add`/`.sub`/`.mul` calls on a `Tensor` receiver;
/// returns whether it changed anything. The method-call rewrite is
/// resolved against the type-checked program, never the syntax tree alone
/// (spec: "so a generic or aliased receiver with an unrelated method of
/// the same name is left unrewritten") — best-effort: a file that fails
/// to parse or type-check (unrelated to this migration) is left with just
/// its `use`-line/qualifier rewrites, if any.
fn fix_paco_file(
    file_path: &std::path::Path,
    entry_dir: &std::path::Path,
    project: Option<&Result<ProjectManifest, String>>,
) -> Result<bool, String> {
    let source = fs::read_to_string(file_path).map_err(|error| format!("failed to read `{}`: {error}", file_path.display()))?;
    let mut sources = SourceMap::new();
    let mut reporter = Reporter::new();
    let file_id = sources.add_file(file_path.display().to_string(), source.clone());
    let tokens = lex(sources.source(file_id).unwrap_or(""), file_id, &mut reporter);
    if reporter.has_errors() {
        return Ok(false);
    }
    let Ok(module) = parse_module(&tokens, &mut reporter) else { return Ok(false) };

    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut rewrites_numerics_qualifier = false;
    for item in &module.items {
        let Item::Use(decl) = item else { continue };
        let Some((old, new)) = moved_std_module(&decl.path) else { continue };
        let alias_suffix = decl.alias.as_ref().map(|alias| format!(" as {alias}")).unwrap_or_default();
        edits.push((decl.span.start(), decl.span.end(), format!("use {}{alias_suffix}", numerics_library_path(new))));
        if old == "numerics" && decl.alias.is_none() {
            rewrites_numerics_qualifier = true;
        }
    }
    if rewrites_numerics_qualifier {
        for (head, span) in qualified_heads(&tokens, &module) {
            if head == "numerics" {
                edits.push((span.start(), span.end(), "tensor".to_string()));
            }
        }
    }

    let prelude = load_prelude(&mut sources).ok().flatten();
    if let Some(loaded_project) = project
        && let Ok(discovered) = discover_used_files(entry_dir, &module, Some(loaded_project), &mut sources, &mut Reporter::new())
    {
        let mut imports = visible_imports(&module, &discovered);
        if let Some(prelude_module) = &prelude {
            imports.push((String::new(), prelude_module));
        }
        let mut scratch = Reporter::new();
        // Name resolution first, exactly like `run_check_pipeline`'s own
        // "resolve → type-check" sequence — type inference depends on it
        // (e.g. to bind a `match` arm's pattern identifiers as locals).
        let resolve_names = resolve_names_from_imports(&imports);
        if paco_resolve::resolve_module_with_imports(&module, &resolve_names, &mut scratch).is_ok()
            && let Ok(typed) = paco_types::infer_module_with_imports(&module, &imports, &mut scratch)
        {
            collect_operator_rewrites(&module, &typed, &source, &mut edits);
        }
    }

    if edits.is_empty() {
        return Ok(false);
    }
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    let mut updated = source;
    for (start, end, replacement) in edits {
        updated.replace_range(start..end, &replacement);
    }
    fs::write(file_path, &updated).map_err(|error| format!("failed to write `{}`: {error}", file_path.display()))?;
    Ok(true)
}

/// Finds every `.add`/`.sub`/`.mul` call in `module` whose receiver's
/// resolved static type is `Tensor` (a struct key whose last `::`
/// segment is exactly `Tensor` — the same "match on the name alone"
/// reasoning as `apply_tensor_missing_library_hint`, but here backed by
/// `typed`'s real type information rather than an unresolved-name
/// message, so a generic or unrelated same-named method is never
/// mistaken for it) and appends its rewrite to `edits`.
fn collect_operator_rewrites(module: &Module, typed: &paco_types::TypedModule<'_>, source: &str, edits: &mut Vec<(usize, usize, String)>) {
    use paco_syntax::ast::Visit;
    struct Finder<'a> {
        typed: &'a paco_types::TypedModule<'a>,
        source: &'a str,
        edits: &'a mut Vec<(usize, usize, String)>,
    }
    impl Visit for Finder<'_> {
        fn visit_expr(&mut self, expr: &Expr) {
            if let Expr::MethodCall { receiver, method, span, .. } = expr {
                let new_method = match method.as_str() {
                    "add" => Some("checked_add"),
                    "sub" => Some("checked_sub"),
                    "mul" => Some("checked_mul"),
                    _ => None,
                };
                if new_method.is_some() {
                    eprintln!("DEBUG method={method} receiver_type={:?}", self.typed.type_of(receiver));
                }
                if let Some(new_method) = new_method
                    && receiver_is_tensor(self.typed, receiver)
                    && let Some((start, end)) = locate_method_name(self.source, expr_span(receiver).end(), span.end(), method)
                {
                    self.edits.push((start, end, new_method.to_string()));
                }
            }
            paco_syntax::ast::walk_expr(self, expr);
        }
    }
    let mut finder = Finder { typed, source, edits };
    finder.visit_module(module);
}

fn receiver_is_tensor(typed: &paco_types::TypedModule<'_>, receiver: &Expr) -> bool {
    matches!(typed.type_of(receiver), Some(paco_types::Type::Struct(key, _)) if key.rsplit("::").next() == Some("Tensor"))
}

/// The exact byte range of the method name after a `.` within
/// `source[receiver_end..call_end]` — `MethodCall`'s own span covers the
/// whole call, not just the method name, so this locates it by scanning
/// every `.`, skipping any whitespace after it (a call can be written
/// `a\n    .add(&b)`, or with stray spaces) and comparing the identifier
/// that follows, rather than assuming a fixed offset. An identifier that
/// merely starts with `method` (`add_all`) never matches, since the whole
/// run of identifier characters is compared, not a fixed-length slice.
fn locate_method_name(source: &str, receiver_end: usize, call_end: usize, method: &str) -> Option<(usize, usize)> {
    let haystack = source.get(receiver_end..call_end)?;
    let bytes = haystack.as_bytes();
    let mut search_from = 0;
    while let Some(relative) = haystack[search_from..].find('.') {
        let dot_at = search_from + relative;
        let mut name_start = dot_at + 1;
        while bytes.get(name_start).is_some_and(u8::is_ascii_whitespace) {
            name_start += 1;
        }
        let mut name_end = name_start;
        while bytes.get(name_end).is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_') {
            name_end += 1;
        }
        if &haystack[name_start..name_end] == method {
            return Some((receiver_end + name_start, receiver_end + name_end));
        }
        search_from = dot_at + 1;
    }
    None
}

/// A discovered `_test.paco` file's own `#[test]`-marked test case, once
/// its compiled binary has run: `message` is `None` for a pass, and the
/// panic message (`TaskPanic.message`, already isolated per spawned task
/// by the runtime) for a failure.
struct TestCaseReport {
    name: String,
    message: Option<String>,
}

/// `paco test [path] [filter]`: discovers every `_test.paco` file under
/// the project rooted at `path` (or the current directory by default),
/// compiles and runs each one's `#[test]`-marked functions (name-filtered
/// by `filter` when given) and reports one aggregate pass/fail summary.
///
/// Architecture (unit-testing design.md's own recommendation, followed as
/// given): one small compiled binary per discovered `_test.paco` file, not
/// one unified binary for the whole project — reuses
/// `check_program_at`/`check_parsed_module`/`merge_test_items` (Section 4)
/// and `compile_parsed_module` (this task's own analogous split of
/// `compile`) exactly as built, with no second multi-module-union
/// mechanism invented here.
///
/// Disclosed limitation (investigated, not silently assumed away): every
/// spawned test task in one binary shares that binary's real stdout, and
/// grepping `stdlib/`/the runtime found no per-task output-capture primitive
/// to isolate it. So an arbitrary `print()` call inside a `#[test]`
/// function's own body is never captured or shown by `paco test`, whether
/// the test passes or fails — only its pass/fail result and, on failure,
/// its panic message (which genuinely IS isolated per task already, via
/// `JoinHandle::join`'s `TaskPanic`, no invention needed) are. This
/// satisfies spec.md's "a passing test's output SHALL NOT be shown"
/// exactly, and satisfies "a failing test's panic message ... SHALL be
/// shown" exactly, but not the "and any output it produced" half of that
/// same sentence for incidental `print()`s — a real, correctly-scoped gap,
/// not a workaround claiming full compliance.
fn test_command(path: Option<PathBuf>, filter: Option<String>, backend: BackendChoice) -> Result<DriverOutput, String> {
    let project_dir = test_project_dir(path);
    let mut test_files = Vec::new();
    collect_test_files(&project_dir, &mut test_files)?;
    test_files.sort();

    let cache = run_cache();
    let mut cases: Vec<TestCaseReport> = Vec::new();
    for test_file in &test_files {
        let scratch = cache.scratch()?;
        let output = scratch.join("paco_test").with_extension(std::env::consts::EXE_EXTENSION);
        let outcome = run_one_test_file(test_file, &project_dir, filter.as_deref(), backend, &output);
        let _ = fs::remove_dir_all(&scratch);
        cases.extend(outcome?);
    }

    report_test_cases(cases)
}

/// `path`'s project directory: `path` itself when it is already a
/// directory, else its parent — so both `paco test` (current directory)
/// and `paco test some/file.paco` (that file's project) resolve the same
/// way every other command resolves a project root (`load_project_manifest`
/// looks at exactly this directory, no upward search).
fn test_project_dir(path: Option<PathBuf>) -> PathBuf {
    let path = path.unwrap_or_else(|| PathBuf::from("."));
    if path.is_dir() { path } else { path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf() }
}

/// Every `_test.paco` file under `dir`, walked recursively (task 5.1: the
/// whole project tree rooted at the project directory — `tests/` is
/// walked too, right here, and distinguished from a colocated file only
/// later, by `is_integration_test_file`, per design.md's two-tier
/// visibility Decision). Never reaches `~/.paco/pkg` (the fetched-
/// dependency cache): this walk never leaves `dir`, and no project's own
/// directory is that cache.
fn collect_test_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|error| format!("failed to read `{}`: {error}", dir.display()))?;
    for entry in entries {
        let entry_path = entry.map_err(|error| format!("failed to read `{}`: {error}", dir.display()))?.path();
        if entry_path.is_dir() {
            collect_test_files(&entry_path, out)?;
            continue;
        }
        if is_test_file(&entry_path) {
            out.push(entry_path);
        }
    }
    Ok(())
}

/// Whether `test_file` is a `tests/<name>_test.paco` file (design.md's
/// external-visibility mode) rather than a colocated `src/`-style one —
/// decided by its path relative to `project_dir`'s first component, not
/// its own file name, since `is_test_file` alone cannot tell the two
/// modes apart.
fn is_integration_test_file(test_file: &std::path::Path, project_dir: &std::path::Path) -> bool {
    test_file
        .strip_prefix(project_dir)
        .ok()
        .and_then(|relative| relative.components().next())
        .is_some_and(|component| component.as_os_str() == "tests")
}

/// `foo_test.paco`'s colocated production sibling, `foo.paco`, in the same
/// directory — may or may not exist (a `_test.paco` file with no
/// production sibling is still a valid, if unusual, test file).
fn sibling_production_file(test_file: &std::path::Path) -> PathBuf {
    let name = test_file.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let base = name.strip_suffix("_test.paco").unwrap_or(name);
    test_file.with_file_name(format!("{base}.paco"))
}

/// Every `#[test]`-tagged function's own name in `module`'s own items —
/// task 5.2's collection step. `validate_test_attribute_placement`
/// (Section 2) already guarantees every `#[test]` this can see is inside a
/// real test file, so no re-check is needed here.
fn test_function_names(module: &Module) -> Vec<String> {
    module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(function) if function.attrs.iter().any(|attr| attr.name == "test") => Some(function.name.clone()),
            _ => None,
        })
        .collect()
}

/// The synthesized `fn main` a discovered test file's compiled binary
/// runs: `spawn`s every one of `names` (a zero-argument call each, the
/// only shape `#[test]` supports), then `.join()`s each in turn, printing
/// one `PACO_TEST <name> PASS`/`PACO_TEST <name> FAIL <message>` line per
/// result — `paco test`'s own reporting protocol, parsed back out by
/// `execute_test_binary`. Every task is spawned before any is joined, so
/// they run concurrently (task 5.2's "parallel by default"); a panicking
/// task's isolation from its siblings is the runtime's own already-built
/// per-task panic boundary (design.md's "no new scheduler" Decision), not
/// anything this function adds. Returns `i64` so the process's own exit
/// code (task 5.3's "non-zero exit if any test fails") needs no separate
/// primitive, per the dispatch's own confirmed `fn main() -> i64` finding.
fn synthesize_test_main(names: &[String]) -> String {
    if names.is_empty() {
        return "fn main() -> i64 { 0 }\n".to_string();
    }
    let mut source = String::from("fn main() -> i64 {\n    let mut failures: i64 = 0;\n");
    for (index, name) in names.iter().enumerate() {
        source.push_str(&format!("    let h{index} = spawn {name}();\n"));
    }
    for (index, name) in names.iter().enumerate() {
        source.push_str(&format!(
            "    match h{index}.join() {{\n        \
                 Result::Ok(_) => print(\"PACO_TEST {name} PASS\"),\n        \
                 Result::Err(e) => {{\n            \
                     print(string_concat(&\"PACO_TEST {name} FAIL \", &e.message));\n            \
                     failures = failures + 1;\n        \
                 }},\n    \
             }}\n"
        ));
    }
    source.push_str("    if failures > 0 { 1 } else { 0 }\n}\n");
    source
}

/// One discovered test file's whole `paco test` unit: parses it (and, in
/// colocated mode, its production sibling), merges/selects per
/// `is_integration_test_file`, appends `synthesize_test_main`'s `main` for
/// only the name-filtered subset of `#[test]` functions (task 5.4: an
/// unmatched function is still compiled, as part of the merged module,
/// just never spawned or reported), then runs the whole module through
/// `compile_parsed_module` — the exact same check → lower → codegen →
/// link pipeline `paco build` uses, through the same `backend` selection
/// (task 5.5). Returns the selected test names actually built into
/// `main`, for `execute_test_binary` to look for in the binary's output.
fn compile_test_binary(
    test_file: &std::path::Path,
    project_dir: &std::path::Path,
    filter: Option<&str>,
    backend: BackendChoice,
    output: &std::path::Path,
) -> Result<Vec<String>, String> {
    let mut sources = SourceMap::new();
    let mut reporter = Reporter::new();
    let (test_module, mut heads) = parse_file(test_file, &mut sources, &mut reporter)?;
    let selected: Vec<String> = test_function_names(&test_module)
        .into_iter()
        .filter(|name| filter.is_none_or(|substring| name.contains(substring)))
        .collect();

    let (mut module, use_resolution_dir) = if is_integration_test_file(test_file, project_dir) {
        (test_module, Some(project_dir.to_path_buf()))
    } else {
        let sibling = sibling_production_file(test_file);
        if sibling.is_file() {
            let (production_module, production_heads) = parse_file(&sibling, &mut sources, &mut reporter)?;
            heads.extend(production_heads);
            (merge_test_items(production_module, test_module), None)
        } else {
            (test_module, None)
        }
    };

    let (main_module, main_heads) =
        parse_source(std::path::Path::new("<paco test main>"), synthesize_test_main(&selected), &mut sources, &mut reporter)?;
    heads.extend(main_heads);
    module.items.extend(main_module.items);

    let options =
        BuildOptions { release: false, target: None, backend, link: None, sysroot: std::env::var_os("PACO_SYSROOT").map(PathBuf::from) };
    compile_parsed_module(module, heads, test_file, sources, reporter, use_resolution_dir, options, output)?;
    Ok(selected)
}

/// Runs `binary` and parses `synthesize_test_main`'s `PACO_TEST` lines out
/// of its stdout, one `TestCaseReport` per name in `expected` — in the
/// order `expected` gives, not stdout's (task order across concurrent
/// tasks is not guaranteed). A name `synthesize_test_main` was told to
/// spawn but whose line never appears (the binary crashed, or aborted,
/// before reporting it) is still reported, as a failure naming what
/// happened, rather than silently dropped.
fn execute_test_binary(binary: &std::path::Path, expected: &[String]) -> Result<Vec<TestCaseReport>, String> {
    let run = std::process::Command::new(binary).output().map_err(|error| format!("failed to run `{}`: {error}", binary.display()))?;
    let stdout = String::from_utf8_lossy(&run.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    let mut reports = Vec::with_capacity(expected.len());
    for name in expected {
        let pass_line = format!("PACO_TEST {name} PASS");
        let fail_prefix = format!("PACO_TEST {name} FAIL ");
        if lines.iter().any(|line| *line == pass_line) {
            reports.push(TestCaseReport { name: name.clone(), message: None });
        } else if let Some(line) = lines.iter().find(|line| line.starts_with(&fail_prefix)) {
            reports.push(TestCaseReport { name: name.clone(), message: Some(line[fail_prefix.len()..].to_string()) });
        } else {
            let status = run.status.code().map_or("a signal".to_string(), |code| format!("exit status {code}"));
            let stderr = String::from_utf8_lossy(&run.stderr);
            reports.push(TestCaseReport {
                name: name.clone(),
                message: Some(format!("the test binary ended ({status}) before reporting this test; stderr: {}", stderr.trim())),
            });
        }
    }
    Ok(reports)
}

/// One discovered test file: compiles its binary (still compiling every
/// `#[test]` function, filtered out or not), then, only when at least one
/// survived filtering, runs it and collects its reports, each name
/// qualified by the test file's path relative to the project (so two
/// files' same-named test cases stay distinguishable in the final
/// summary).
fn run_one_test_file(
    test_file: &std::path::Path,
    project_dir: &std::path::Path,
    filter: Option<&str>,
    backend: BackendChoice,
    output: &std::path::Path,
) -> Result<Vec<TestCaseReport>, String> {
    let selected = compile_test_binary(test_file, project_dir, filter, backend, output)?;
    if selected.is_empty() {
        return Ok(Vec::new());
    }
    let relative = test_file.strip_prefix(project_dir).unwrap_or(test_file);
    let mut reports = execute_test_binary(output, &selected)?;
    for report in &mut reports {
        report.name = format!("{}::{}", relative.display(), report.name);
    }
    Ok(reports)
}

/// Task 5.3's reporting: a summary line per case, a failing case's panic
/// message shown (a passing case's is not — there is none to show, by
/// construction), overall counts, and a non-zero process exit when any
/// case failed — via `Err`, the same convention `Commands::Run` already
/// uses for a nonzero program exit (`main.rs` exits 1 on any `Err`).
fn report_test_cases(cases: Vec<TestCaseReport>) -> Result<DriverOutput, String> {
    let total = cases.len();
    let passed = cases.iter().filter(|case| case.message.is_none()).count();
    let failed = total - passed;

    let mut summary = format!("running {total} test{}\n", if total == 1 { "" } else { "s" });
    for case in &cases {
        summary.push_str(&format!("test {} ... {}\n", case.name, if case.message.is_none() { "ok" } else { "FAILED" }));
    }
    if failed > 0 {
        summary.push_str("\nfailures:\n");
        for case in cases.iter().filter(|case| case.message.is_some()) {
            summary.push_str(&format!("\n---- {} ----\n{}\n", case.name, case.message.as_deref().unwrap_or_default()));
        }
        summary.push('\n');
    }
    summary.push_str(&format!("test result: {}. {passed} passed; {failed} failed\n", if failed == 0 { "ok" } else { "FAILED" }));

    if failed > 0 { Err(summary) } else { Ok(DriverOutput { stdout: summary, stderr: String::new() }) }
}

fn not_implemented(name: &str) -> Result<DriverOutput, String> {
    Err(format!("subcommand `{name}` is not implemented"))
}

#[cfg(test)]
mod domain_resolution_tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let suffix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        dir.push(format!("paco_domain_resolve_{name}_{}_{suffix}", std::process::id()));
        dir
    }

    fn git(dir: &std::path::Path, args: &[&str]) {
        let output = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().expect("git should be installed");
        assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    /// A fake `<cache_root>/example.com/team/json@v1.0.0/` cache directory
    /// (a real, committed git repo, no clone needed) with
    /// `src/<last-declared-segment>.paco`.
    fn fake_cache_dir(root: &std::path::Path, dependency_path: &str, tag: &str) -> (PathBuf, String) {
        let dir = pkg_cache::cache_dir_for(root, dependency_path, tag);
        fs::create_dir_all(dir.join("src")).unwrap();
        let file_name = manifest::key_segments(dependency_path).pop().unwrap();
        fs::write(dir.join("src").join(format!("{file_name}.paco")), "pub fn value() -> i64 { 1 }\n").unwrap();
        git(&dir, &["init", "--quiet"]);
        git(&dir, &["config", "user.email", "test@example.com"]);
        git(&dir, &["config", "user.name", "Test"]);
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "--quiet", "-m", "init"]);
        let commit = git::resolve_ref(&dir, "HEAD").unwrap();
        (dir, commit)
    }

    fn path(segments: &[&str]) -> Vec<String> {
        segments.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_paco_mod_is_a_clear_error() {
        let error = resolve_domain_use_path(None, &path(&["example", "com", "team", "json"])).unwrap_err();
        assert!(error.contains("paco.mod"), "{error}");
    }

    #[test]
    fn a_broken_project_manifest_propagates_its_error() {
        let broken: Result<ProjectManifest, String> = Err("paco.mod is missing the required `module` field".to_string());
        let error = resolve_domain_use_path(Some(&broken), &path(&["example", "com", "team", "json"])).unwrap_err();
        assert!(error.contains("module"), "{error}");
    }

    fn project(dependencies: Vec<(String, manifest::DependencySource)>, packages: Vec<manifest::LockedPackage>, cache_root: PathBuf) -> Result<ProjectManifest, String> {
        Ok(ProjectManifest {
            manifest: manifest::Manifest { module: "m".to_string(), dependencies, paco: None },
            lock: manifest::Lockfile { packages },
            cache_root: cache_root.clone(),
            project_dir: cache_root,
        })
    }

    #[test]
    fn no_matching_dependency_names_paco_get_or_paco_mod() {
        let project = project(vec![], vec![], temp_dir("no_dep"));
        let error = resolve_domain_use_path(Some(&project), &path(&["example", "com", "team", "json"])).unwrap_err();
        assert!(error.contains("paco get"), "{error}");
        assert!(error.contains("paco.mod"), "{error}");
    }

    #[test]
    fn a_declared_but_never_fetched_dependency_says_run_paco_get() {
        let project = project(
            vec![("example.com/team/json".to_string(), manifest::DependencySource::Tag("v1.0.0".to_string()))],
            vec![],
            temp_dir("never_fetched"),
        );
        let error = resolve_domain_use_path(Some(&project), &path(&["example", "com", "team", "json"])).unwrap_err();
        assert!(error.contains("paco get"), "{error}");
    }

    #[test]
    fn a_cache_directory_at_the_wrong_commit_is_a_clear_mismatch_error() {
        let root = temp_dir("mismatch");
        let (_, actual_commit) = fake_cache_dir(&root, "example.com/team/json", "v1.0.0");
        let project = project(
            vec![("example.com/team/json".to_string(), manifest::DependencySource::Tag("v1.0.0".to_string()))],
            vec![manifest::LockedPackage {
                path: "example.com/team/json".to_string(),
                tag: Some("v1.0.0".to_string()),
                commit: "0000000000000000000000000000000000000".to_string(),
                ..Default::default()
            }],
            root.clone(),
        );

        let error = resolve_domain_use_path(Some(&project), &path(&["example", "com", "team", "json"])).unwrap_err();

        assert!(error.contains("paco get"), "{error}");
        assert!(error.contains(&actual_commit), "{error}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_exact_dependency_match_with_empty_remainder_resolves_to_the_last_declared_segment() {
        let root = temp_dir("exact");
        let (_, commit) = fake_cache_dir(&root, "example.com/team/json", "v1.0.0");
        let project = project(
            vec![("example.com/team/json".to_string(), manifest::DependencySource::Tag("v1.0.0".to_string()))],
            vec![manifest::LockedPackage { path: "example.com/team/json".to_string(), tag: Some("v1.0.0".to_string()), commit, ..Default::default() }],
            root.clone(),
        );

        let resolved = resolve_domain_use_path(Some(&project), &path(&["example", "com", "team", "json"])).unwrap();

        assert_eq!(resolved, pkg_cache::cache_dir_for(&root, "example.com/team/json", "v1.0.0").join("src").join("json.paco"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_multi_module_dependency_resolves_its_remainder_under_the_cache_src_dir() {
        let root = temp_dir("multi");
        let (dir, commit) = fake_cache_dir(&root, "github.com/pacolang/numerics", "v1.0.0");
        fs::write(dir.join("src").join("blas.paco"), "pub fn value() -> i64 { 2 }\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "--quiet", "-m", "add blas"]);
        let commit = git::resolve_ref(&dir, "HEAD").unwrap_or(commit);
        let project = project(
            vec![("github.com/pacolang/numerics".to_string(), manifest::DependencySource::Tag("v1.0.0".to_string()))],
            vec![manifest::LockedPackage { path: "github.com/pacolang/numerics".to_string(), tag: Some("v1.0.0".to_string()), commit, ..Default::default() }],
            root.clone(),
        );

        let resolved = resolve_domain_use_path(Some(&project), &path(&["github", "com", "pacolang", "numerics", "blas"])).unwrap();

        assert_eq!(resolved, pkg_cache::cache_dir_for(&root, "github.com/pacolang/numerics", "v1.0.0").join("src").join("blas.paco"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_branch_dependency_resolves_via_its_branch_name_cache_key() {
        let root = temp_dir("branch");
        let (_, commit) = fake_cache_dir(&root, "example.com/team/dev", "main");
        let project = project(
            vec![("example.com/team/dev".to_string(), manifest::DependencySource::Branch("main".to_string()))],
            vec![manifest::LockedPackage { path: "example.com/team/dev".to_string(), branch: Some("main".to_string()), commit, ..Default::default() }],
            root.clone(),
        );

        let resolved = resolve_domain_use_path(Some(&project), &path(&["example", "com", "team", "dev"])).unwrap();

        assert_eq!(resolved, pkg_cache::cache_dir_for(&root, "example.com/team/dev", "main").join("src").join("dev.paco"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_rev_dependency_resolves_via_its_commit_cache_key() {
        let root = temp_dir("rev");
        let (_, commit) = fake_cache_dir(&root, "example.com/team/pinned", "a1b2c3d");
        let project = project(
            vec![("example.com/team/pinned".to_string(), manifest::DependencySource::Rev("a1b2c3d".to_string()))],
            vec![manifest::LockedPackage {
                path: "example.com/team/pinned".to_string(),
                rev: Some("a1b2c3d".to_string()),
                commit,
                ..Default::default()
            }],
            root.clone(),
        );

        let resolved = resolve_domain_use_path(Some(&project), &path(&["example", "com", "team", "pinned"])).unwrap();

        assert_eq!(resolved, pkg_cache::cache_dir_for(&root, "example.com/team/pinned", "a1b2c3d").join("src").join("pinned.paco"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_path_dependency_resolves_relative_to_the_project_dir_with_no_lock_entry_needed() {
        let root = temp_dir("path_dep");
        let local_dir = root.join("local");
        fs::create_dir_all(local_dir.join("src")).unwrap();
        fs::write(local_dir.join("src").join("local.paco"), "pub fn value() -> i64 { 1 }\n").unwrap();
        let project = project(
            vec![("example.com/team/local".to_string(), manifest::DependencySource::Path("local".to_string()))],
            vec![],
            root.clone(),
        );

        let resolved = resolve_domain_use_path(Some(&project), &path(&["example", "com", "team", "local"])).unwrap();

        assert_eq!(resolved, local_dir.join("src").join("local.paco"));
        let _ = fs::remove_dir_all(&root);
    }

    // --- `extract-domain-libraries` task 4.1: the alias table ---

    #[test]
    fn moved_std_module_matches_only_the_three_moved_modules() {
        assert_eq!(moved_std_module(&path(&["stdlib", "numerics"])), Some(("numerics", "tensor")));
        assert_eq!(moved_std_module(&path(&["stdlib", "math"])), Some(("math", "math")));
        assert_eq!(moved_std_module(&path(&["stdlib", "blas"])), Some(("blas", "blas")));
    }

    #[test]
    fn moved_std_module_leaves_unmoved_std_modules_and_non_std_paths_alone() {
        assert_eq!(moved_std_module(&path(&["stdlib", "string"])), None);
        assert_eq!(moved_std_module(&path(&["stdlib", "autodiff"])), None);
        assert_eq!(moved_std_module(&path(&["stdlib", "core"])), None);
        assert_eq!(moved_std_module(&path(&["numerics"])), None);
        assert_eq!(moved_std_module(&path(&["example", "com", "numerics"])), None);
    }

    // --- `extract-domain-libraries` task 4.2: the missing-library hint ---

    fn e0306(sources: &mut SourceMap, message: &str) -> Diagnostic {
        let file = sources.add_file("main.paco", "fn main() {}\n".to_string());
        Diagnostic::error("PACO-E0306", Span::new(file, 0, 1), format!("type is not supported yet: {message}"))
    }

    #[test]
    fn tensor_hint_replaces_e0306_for_bare_tensor_and_notes_paco_get_when_undeclared() {
        let mut sources = SourceMap::new();
        let mut reporter = Reporter::new();
        reporter.push(e0306(&mut sources, "Tensor"));

        apply_tensor_missing_library_hint(&mut reporter, None);

        let diagnostic = &reporter.diagnostics()[0];
        assert_eq!(diagnostic.code(), "PACO-E0903");
        assert!(diagnostic.primary().message.contains("use github.com/pacolang/numerics/tensor;"), "{}", diagnostic.primary().message);
        assert!(diagnostic.notes().iter().any(|note| note.contains("paco get github.com/pacolang/numerics")), "{:?}", diagnostic.notes());
    }

    #[test]
    fn tensor_hint_also_matches_a_qualified_tensor_path() {
        let mut sources = SourceMap::new();
        let mut reporter = Reporter::new();
        reporter.push(e0306(&mut sources, "tensor::Tensor"));
        reporter.push(e0306(&mut sources, "numerics::Tensor"));

        apply_tensor_missing_library_hint(&mut reporter, None);

        assert!(reporter.diagnostics().iter().all(|diagnostic| diagnostic.code() == "PACO-E0903"));
    }

    #[test]
    fn tensor_hint_omits_the_paco_get_note_when_the_dependency_is_already_declared() {
        let mut sources = SourceMap::new();
        let mut reporter = Reporter::new();
        reporter.push(e0306(&mut sources, "Tensor"));
        let project = project(
            vec![("github.com/pacolang/numerics".to_string(), manifest::DependencySource::Tag("v0.1.0".to_string()))],
            vec![],
            temp_dir("tensor_hint_declared"),
        );

        apply_tensor_missing_library_hint(&mut reporter, Some(&project));

        let diagnostic = &reporter.diagnostics()[0];
        assert_eq!(diagnostic.code(), "PACO-E0903");
        assert!(diagnostic.notes().is_empty(), "{:?}", diagnostic.notes());
    }

    #[test]
    fn tensor_hint_leaves_an_unrelated_e0306_diagnostic_alone() {
        let mut sources = SourceMap::new();
        let mut reporter = Reporter::new();
        reporter.push(e0306(&mut sources, "Frobnicator"));

        apply_tensor_missing_library_hint(&mut reporter, None);

        assert_eq!(reporter.diagnostics()[0].code(), "PACO-E0306");
    }

    // --- `extract-domain-libraries` task 4.3: `paco fix` ---

    #[test]
    fn add_dependency_line_inserts_right_after_an_existing_dependencies_header() {
        let manifest = "module = \"m\"\n\n[dependencies]\n\"example.com/team/json\" = \"v1.0.0\"\n";
        let updated = add_dependency_line(manifest, "github.com/pacolang/numerics", "v0.1.0");
        assert!(updated.contains("\"github.com/pacolang/numerics\" = \"v0.1.0\"\n"), "{updated}");
        let parsed = manifest::Manifest::parse(&updated).unwrap();
        assert_eq!(parsed.dependencies.len(), 2);
    }

    #[test]
    fn add_dependency_line_appends_a_new_table_when_none_exists() {
        let manifest = "module = \"m\"\n";
        let updated = add_dependency_line(manifest, "github.com/pacolang/numerics", "v0.1.0");
        let parsed = manifest::Manifest::parse(&updated).unwrap();
        assert_eq!(parsed.dependencies, vec![("github.com/pacolang/numerics".to_string(), manifest::DependencySource::Tag("v0.1.0".to_string()))]);
    }

    #[test]
    fn locate_method_name_finds_a_multiline_oddly_spaced_call() {
        let source = "fn f() {\n    a\n        .   add  (&b)\n}\n";
        let receiver_end = source.find(" .   add").unwrap();
        let call_end = source[receiver_end..].find(')').unwrap() + receiver_end + 1;

        let (start, end) = locate_method_name(source, receiver_end, call_end, "add").unwrap();

        assert_eq!(&source[start..end], "add");
    }

    #[test]
    fn locate_method_name_does_not_match_a_longer_identifier_that_starts_with_the_method_name() {
        let source = "a.add_all(&b)";
        assert_eq!(locate_method_name(source, 1, source.len(), "add"), None);
    }
}

#[cfg(test)]
mod prelude_tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let suffix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        dir.push(format!("paco_prelude_{name}_{}_{suffix}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn is_test_file_recognizes_only_the_underscore_test_suffix() {
        assert!(is_test_file(std::path::Path::new("foo_test.paco")));
        assert!(!is_test_file(std::path::Path::new("foo.paco")));
        assert!(!is_test_file(std::path::Path::new("footest.paco")));
    }

    #[test]
    fn no_std_core_directory_seeds_nothing() {
        let root = temp_dir("missing");
        let result = load_prelude_from(&root, &mut SourceMap::new()).unwrap();
        assert!(result.is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn std_core_files_are_parsed_and_merged() {
        let root = temp_dir("merge");
        let core_dir = root.join("core");
        fs::create_dir_all(&core_dir).unwrap();
        fs::write(core_dir.join("a.paco"), "pub fn one() -> i64 { 1 }\n").unwrap();
        fs::write(core_dir.join("b.paco"), "pub fn two() -> i64 { 2 }\n").unwrap();

        let module = load_prelude_from(&root, &mut SourceMap::new()).unwrap().expect("expected a merged module");
        let names: Vec<&str> = module
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Fn(function) => Some(function.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"one"));
        assert!(names.contains(&"two"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn std_core_test_files_are_skipped() {
        let root = temp_dir("skip_test_file");
        let core_dir = root.join("core");
        fs::create_dir_all(&core_dir).unwrap();
        fs::write(core_dir.join("a.paco"), "pub fn one() -> i64 { 1 }\n").unwrap();
        fs::write(core_dir.join("a_test.paco"), "pub fn hidden() -> i64 { 2 }\n").unwrap();

        let module = load_prelude_from(&root, &mut SourceMap::new()).unwrap().expect("expected a merged module");
        let names: Vec<&str> = module
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Fn(function) => Some(function.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["one"]);
        assert!(!names.contains(&"hidden"));

        let _ = fs::remove_dir_all(&root);
    }
}

/// `unit-testing` tasks 4.1/4.2: the two `_test.paco` visibility modes,
/// driven directly (not through `paco test` discovery, which is Section 5
/// and not built yet).
#[cfg(test)]
mod test_file_visibility_tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let suffix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        dir.push(format!("paco_test_visibility_{name}_{}_{suffix}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_project_with_private_fn(dir: &std::path::Path) {
        fs::write(dir.join("paco.mod"), "module = \"example.com/myproject\"\n").unwrap();
        fs::write(dir.join("amount.paco"), "fn private_helper() -> i64 { 42 }\n").unwrap();
    }

    /// spec.md's "A tests/-located test file cannot see private items"
    /// scenario.
    #[test]
    fn tests_dir_file_gets_the_same_visibility_error_an_external_consumer_would() {
        let dir = temp_dir("tests_dir_private");
        write_project_with_private_fn(&dir);
        fs::create_dir_all(dir.join("tests")).unwrap();
        let test_file = dir.join("tests").join("amount_test.paco");
        fs::write(&test_file, "use amount;\n\nfn call_helper() -> i64 {\n    amount::private_helper()\n}\n").unwrap();

        let error = match check_program_at(test_file, Some(dir.clone())) {
            Ok(_) => panic!("expected a visibility error, got success"),
            Err(error) => error,
        };
        assert!(error.contains("PACO-E0333"), "{error}");
        assert!(error.contains("private_helper"), "{error}");

        let _ = fs::remove_dir_all(&dir);
    }

    /// spec.md's "A src/-located test file sees private items" scenario,
    /// via `merge_test_items` + `check_parsed_module`.
    #[test]
    fn colocated_file_keeps_full_same_module_access_via_merge() {
        let dir = temp_dir("colocated_private");
        let production_file = dir.join("amount.paco");
        let mut sources = SourceMap::new();
        let mut reporter = Reporter::new();
        let (production_module, heads) = parse_source(
            &production_file,
            "fn private_helper() -> i64 { 42 }\n".to_string(),
            &mut sources,
            &mut reporter,
        )
        .unwrap();
        let (test_module, _) = parse_source(
            &dir.join("amount_test.paco"),
            "fn call_helper() -> i64 {\n    private_helper()\n}\n".to_string(),
            &mut sources,
            &mut reporter,
        )
        .unwrap();
        let merged = merge_test_items(production_module, test_module);

        match check_parsed_module(merged, heads, &production_file, sources, reporter, None) {
            Ok(checked) => assert!(!checked.reporter.has_errors(), "colocated merge reported an error"),
            Err(error) => panic!("colocated test file should see the private function: {error}"),
        }

        let _ = fs::remove_dir_all(&dir);
    }

    /// Regression: the same private function, called the same way, is
    /// accepted from a colocated (merged) test file and rejected from a
    /// `tests/`-located one — the two modes stay distinguished by
    /// location, not accidentally merged into one behavior.
    #[test]
    fn colocated_and_tests_dir_visibility_are_distinguished_by_location() {
        let dir = temp_dir("side_by_side");
        write_project_with_private_fn(&dir);
        let production_file = dir.join("amount.paco");

        let mut sources = SourceMap::new();
        let mut reporter = Reporter::new();
        let (production_module, heads) = parse_source(
            &production_file,
            fs::read_to_string(&production_file).unwrap(),
            &mut sources,
            &mut reporter,
        )
        .unwrap();
        let (test_module, _) = parse_source(
            &dir.join("amount_test.paco"),
            "fn call_helper() -> i64 {\n    private_helper()\n}\n".to_string(),
            &mut sources,
            &mut reporter,
        )
        .unwrap();
        let merged = merge_test_items(production_module, test_module);
        match check_parsed_module(merged, heads, &production_file, sources, reporter, None) {
            Ok(checked) => assert!(!checked.reporter.has_errors(), "colocated merge reported an error"),
            Err(error) => panic!("colocated test file should see the private function: {error}"),
        }

        fs::create_dir_all(dir.join("tests")).unwrap();
        let tests_dir_file = dir.join("tests").join("amount_test.paco");
        fs::write(&tests_dir_file, "use amount;\n\nfn call_helper() -> i64 {\n    amount::private_helper()\n}\n").unwrap();
        let tests_dir_error = match check_program_at(tests_dir_file, Some(dir.clone())) {
            Ok(_) => panic!("expected a visibility error, got success"),
            Err(error) => error,
        };
        assert!(tests_dir_error.contains("PACO-E0333"), "{tests_dir_error}");

        let _ = fs::remove_dir_all(&dir);
    }
}

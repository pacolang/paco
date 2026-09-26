//! Cranelift dev codegen backend: lowers `paco-mir::Body` to Cranelift IR.

mod debug;
mod drop;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    AbiParam, AtomicRmwOp, FuncRef, GlobalValue, InstBuilder, MemFlagsData, Signature, SourceLoc, StackSlotData,
    StackSlotKind, TrapCode, UserFuncName, Value, types,
};
use cranelift_codegen::isa::TargetIsa;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch, Variable};
use cranelift_module::{DataDescription, FuncId, Linkage, Module};

use drop::Glue;
use paco_mir::glue::{CELL_COUNT_OFFSET, CELL_LOCK_OFFSET, CELL_VALUE_OFFSET, SLICE_DATA_OFFSET, SLICE_LEN_OFFSET};

use paco_mir::{
    BasicBlockId, BinOp, Body, MathOp, Operand, Place, Profile, Rvalue, Statement, Terminator, TypeLayouts,
    UnOp,
};
use paco_types::{FloatWidth, IntWidth, Type};

pub fn host_isa() -> Result<Arc<dyn TargetIsa>, String> {
    target_isa(None)
}

/// The ISA for `triple`, or for the host when `triple` is `None`.
pub fn target_isa(triple: Option<&str>) -> Result<Arc<dyn TargetIsa>, String> {
    target_isa_with(triple, false)
}

fn target_isa_with(triple: Option<&str>, frame_pointers: bool) -> Result<Arc<dyn TargetIsa>, String> {
    let mut flag_builder = settings::builder();
    flag_builder
        .set("is_pic", "true")
        .map_err(|error| error.to_string())?;
    if frame_pointers {
        flag_builder.set("preserve_frame_pointers", "true").map_err(|error| error.to_string())?;
    }
    let isa_builder = match triple {
        None => cranelift_native::builder().map_err(|error| format!("unsupported host: {error}"))?,
        Some(name) => {
            let triple: target_lexicon::Triple =
                name.parse().map_err(|error| format!("unsupported target triple `{name}`: {error}"))?;
            cranelift_codegen::isa::lookup(triple).map_err(|error| format!("unsupported target triple `{name}`: {error}"))?
        }
    };
    isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(|error| error.to_string())
}

/// Buffers bodies, then declares and defines them all in one object file.
pub struct CraneliftBackend<'a> {
    bodies: Vec<(String, Body)>,
    imports: Vec<(String, Body)>,
    externs: Vec<(String, Vec<Type>, Type)>,
    layouts: &'a TypeLayouts<'a>,
    sources: Option<&'a paco_span::SourceMap>,
}

impl<'a> CraneliftBackend<'a> {
    pub fn new(externs: Vec<(String, Vec<Type>, Type)>, layouts: &'a TypeLayouts<'a>) -> Self {
        Self { bodies: Vec::new(), imports: Vec::new(), externs, layouts, sources: None }
    }

    /// Resolves panic locations against `sources`.
    pub fn with_sources(mut self, sources: &'a paco_span::SourceMap) -> Self {
        self.sources = Some(sources);
        self
    }
}

impl paco_mir::Backend for CraneliftBackend<'_> {
    fn lower_body(&mut self, name: &str, body: &Body) -> Result<(), String> {
        self.bodies.push((name.to_string(), body.clone()));
        Ok(())
    }

    fn declare_body(&mut self, name: &str, body: &Body) -> Result<(), String> {
        self.imports.push((name.to_string(), body.clone()));
        Ok(())
    }

    fn finish(self, target: &paco_mir::Target) -> Result<paco_mir::ObjectFile, String> {
        let debug = target.profile == Profile::Debug;
        let isa = target_isa_with(target.triple.as_deref(), debug)?;
        let builder = cranelift_object::ObjectBuilder::new(isa, "paco", cranelift_module::default_libcall_names())
            .map_err(|error| error.to_string())?;
        let mut module = cranelift_object::ObjectModule::new(builder);
        let locator = self.sources.map(paco_mir::SourceLocator::new);
        let mut panic_data = PanicData { locator: locator.as_ref(), debug, ..PanicData::default() };
        define_all(&mut module, &self.bodies, &self.imports, &self.externs, self.layouts, &mut panic_data)?;
        let mut product = module.finish();
        debug::emit(&mut product, &panic_data.functions, &panic_data.locations)?;
        let macho = product.object.format() == cranelift_object::object::BinaryFormat::MachO;
        let mut bytes = product.emit().map_err(|error| error.to_string())?;
        if macho && !panic_data.functions.is_empty() {
            debug::place_macho_debug_addresses(&mut bytes)?;
        }
        Ok(bytes)
    }
}

/// Declares `runtime/paco-runtime-ffi`'s entry points as external
/// (`Linkage::Import`) functions — resolved at link time against
/// `libpaco_runtime_ffi.a` (`paco-link`'s job, not this crate's), not
/// defined here. Signatures must match `paco-runtime-ffi/src/lib.rs`'s
/// real `extern "C"` functions exactly: every pointer and `usize` param is
/// `I64` (pointer width on this dev backend's only target, x86_64 — the
/// same convention `clif_type`/`place_address` already use throughout this
/// file), matching the C ABI these functions were written against.
#[derive(Clone, Copy)]
enum FfiReturn {
    None,
    Status,
    Pointer,
}

fn declare_ffi_functions<M: Module>(module: &mut M) -> HashMap<String, FuncId> {
    let mut func_ids = HashMap::new();
    let mut declare = |module: &mut M, name: &str, params: usize, returns: FfiReturn| {
        let mut sig = module.make_signature();
        for _ in 0..params {
            sig.params.push(AbiParam::new(types::I64));
        }
        match returns {
            FfiReturn::None => {}
            FfiReturn::Status => sig.returns.push(AbiParam::new(types::I32)),
            FfiReturn::Pointer => sig.returns.push(AbiParam::new(types::I64)),
        }
        let func_id = module
            .declare_function(name, Linkage::Import, &sig)
            .unwrap_or_else(|error| panic!("failed to declare `{name}`: {error}"));
        func_ids.insert(name.to_string(), func_id);
    };
    // (capacity, sender_out, receiver_out) -> ()
    declare(module, "paco_rt_channel", 3, FfiReturn::None);
    // (sender, value, value_len) -> i32
    declare(module, "paco_rt_send", 3, FfiReturn::Status);
    // (receiver, value_out, value_len) -> i32
    declare(module, "paco_rt_recv", 3, FfiReturn::Status);
    // (sender) -> ()
    declare(module, "paco_rt_sender_close", 1, FfiReturn::None);
    // (receiver) -> ()
    declare(module, "paco_rt_receiver_close", 1, FfiReturn::None);
    // (handle, result_out, result_len, message_out) -> i32
    declare(module, "paco_rt_join", 4, FfiReturn::Status);
    // (thunk, captures, captures_len, result_len) -> *mut JoinHandleOpaque
    declare(module, "paco_rt_spawn", 4, FfiReturn::Pointer);
    declare(module, "paco_rt_spawn_blocking", 4, FfiReturn::Pointer);
    declare(module, "paco_calloc", 2, FfiReturn::Pointer);
    // (receiver) -> i32 (bool: 0/1)
    declare(module, "paco_rt_receiver_is_ready", 1, FfiReturn::Status);
    // (thunk, captures, captures_len) -> *mut GeneratorOpaque
    declare(module, "paco_rt_generator_new", 3, FfiReturn::Pointer);
    // (elem, elem_len) -> i32 (1 when the generator is being dropped)
    declare(module, "paco_rt_generator_yield", 2, FfiReturn::Status);
    // (handle, elem_out, elem_len) -> i32
    declare(module, "paco_rt_generator_next", 3, FfiReturn::Status);
    func_ids
}

fn declare_extern_functions<M: Module>(
    module: &mut M,
    externs: &[(String, Vec<Type>, Type)],
) -> HashMap<String, FuncId> {
    let mut func_ids = HashMap::new();
    for (name, params, return_ty) in externs {
        let mut sig = module.make_signature();
        for param in params {
            if let Some(ty) = clif_type(param) {
                sig.params.push(AbiParam::new(ty));
            }
        }
        if let Some(ty) = clif_type(return_ty) {
            sig.returns.push(AbiParam::new(ty));
        }
        let func_id = module
            .declare_function(name, Linkage::Import, &sig)
            .unwrap_or_else(|error| panic!("failed to declare extern function `{name}`: {error}"));
        func_ids.insert(name.clone(), func_id);
    }
    func_ids
}

pub fn declare_and_define<M: Module>(
    module: &mut M,
    bodies: &[(String, Body)],
    externs: &[(String, Vec<Type>, Type)],
    layouts: &TypeLayouts<'_>,
) -> Result<HashMap<String, FuncId>, String> {
    declare_and_define_with_imports(module, bodies, &[], externs, layouts)
}

/// Like [`declare_and_define`], with `imports` declared but defined elsewhere.
pub fn declare_and_define_with_imports<M: Module>(
    module: &mut M,
    bodies: &[(String, Body)],
    imports: &[(String, Body)],
    externs: &[(String, Vec<Type>, Type)],
    layouts: &TypeLayouts<'_>,
) -> Result<HashMap<String, FuncId>, String> {
    define_all(module, bodies, imports, externs, layouts, &mut PanicData::default())
}

fn define_all<M: Module>(
    module: &mut M,
    bodies: &[(String, Body)],
    imports: &[(String, Body)],
    externs: &[(String, Vec<Type>, Type)],
    layouts: &TypeLayouts<'_>,
    panic_data: &mut PanicData<'_>,
) -> Result<HashMap<String, FuncId>, String> {
    let mut func_ids = declare_ffi_functions(module);
    func_ids.extend(declare_extern_functions(module, externs));
    if let Some((_, entry)) = bodies.iter().find(|(name, _)| name == paco_mir::ENTRY_SYMBOL) {
        let data_id = module
            .declare_data(paco_mir::ENTRY_RETURNS_VALUE_SYMBOL, Linkage::Export, false, false)
            .map_err(|error| error.to_string())?;
        let mut description = DataDescription::new();
        description.define(Box::new([(entry.return_ty != Type::Unit) as u8]));
        module.define_data(data_id, &description).map_err(|error| error.to_string())?;
    }
    for (name, body) in imports {
        let sig = make_signature(module, body, layouts);
        let func_id = module
            .declare_function(name, Linkage::Import, &sig)
            .map_err(|error| format!("failed to declare `{name}`: {error}"))?;
        func_ids.insert(name.clone(), func_id);
    }
    for (name, body) in bodies {
        let sig = make_signature(module, body, layouts);
        let func_id = module
            .declare_function(name, Linkage::Export, &sig)
            .map_err(|error| format!("failed to declare `{name}`: {error}"))?;
        func_ids.insert(name.clone(), func_id);
    }

    let params: HashMap<String, Vec<Type>> = bodies
        .iter()
        .chain(imports)
        .map(|(name, body)| (name.clone(), body.locals[..body.param_count].iter().map(|local| local.ty.clone()).collect()))
        .collect();
    let all_bodies: Vec<(String, Body)> = bodies.iter().chain(imports).cloned().collect();
    let user_drops: HashMap<Type, FuncId> =
        paco_mir::glue::user_drop_fns(&all_bodies).into_iter().map(|(ty, name)| (ty, func_ids[&name])).collect();
    let glue = drop::build_glue(module, bodies, layouts, &user_drops)?;

    let mut ctx = module.make_context();
    let mut fb_ctx = FunctionBuilderContext::new();
    for (name, body) in bodies {
        let func_id = func_ids[name];
        ctx.func.signature = make_signature(module, body, layouts);
        ctx.func.name = UserFuncName::user(0, func_id.as_u32());
        compile_function_with(module, &mut ctx, &mut fb_ctx, body, &func_ids, layouts, &glue, &params, panic_data, Some(func_id));
        module
            .define_function(func_id, &mut ctx)
            .map_err(|error| format!("failed to define `{name}`: {error}"))?;
        if panic_data.debug
            && let Some(code) = ctx.compiled_code()
        {
            let rows = code
                .buffer
                .get_srclocs_sorted()
                .iter()
                .filter(|range| !range.loc.is_default())
                .map(|range| (range.start, range.loc.bits()))
                .collect();
            panic_data.functions.push(debug::DebugFunction { id: func_id, size: code.buffer.total_size(), rows });
        }
        module.clear_context(&mut ctx);
    }

    Ok(func_ids)
}

fn clif_type(ty: &Type) -> Option<types::Type> {
    match ty {
        Type::Int(IntWidth::I8 | IntWidth::U8) => Some(types::I8),
        Type::Int(IntWidth::I16 | IntWidth::U16) => Some(types::I16),
        Type::Int(IntWidth::I32 | IntWidth::U32) => Some(types::I32),
        Type::Int(IntWidth::I64 | IntWidth::U64) => Some(types::I64),
        Type::Char => Some(types::I32),
        Type::Float(width) => Some(float_storage_type(*width)),
        Type::Bool => Some(types::I8),
        Type::Unit | Type::Never => None,
        Type::Struct(_, _) | Type::Enum(_, _) | Type::Tuple(_) => Some(types::I64),
        // A 16-byte [data_ptr, len] descriptor (see `SLICE_DATA_OFFSET`/
        // `SLICE_LEN_OFFSET`) can't fit in one Cranelift SSA value — like a
        // struct/enum, a `[]T`-typed local's `Variable` holds the
        // descriptor's own address, not the descriptor itself.
        Type::Slice(_) | Type::String => Some(types::I64),
        // A borrow is always an address, regardless of what it points at —
        // including `&[]T`/`&mut []T`: rather than a genuinely two-register
        // fat pointer, this is a plain pointer *to* the pointee's own
        // descriptor/storage (the same convention `place_address`'s
        // `Borrow`-unwrapping already assumes throughout — `struct_name_of`/
        // `enum_name_of`/`slice_elem_of` all strip exactly one `Borrow`
        // layer to find the underlying type, never treat a borrow's own
        // bytes as wider than a pointer).
        Type::Borrow { .. } | Type::RawPointer { .. } | Type::Fn(..) => Some(types::I64),
        other => panic!("codegen for this type is not implemented yet: {other:?}"),
    }
}

fn float_storage_type(width: FloatWidth) -> types::Type {
    match width {
        FloatWidth::F64 => types::F64,
        FloatWidth::F32 => types::F32,
        FloatWidth::F16 | FloatWidth::BF16 => types::I16,
        FloatWidth::F8E4M3 | FloatWidth::F8E5M2 => types::I8,
    }
}

fn small_float_code(width: FloatWidth) -> Option<i64> {
    match width {
        FloatWidth::F16 => Some(0),
        FloatWidth::BF16 => Some(1),
        FloatWidth::F8E4M3 => Some(2),
        FloatWidth::F8E5M2 => Some(3),
        FloatWidth::F64 | FloatWidth::F32 => None,
    }
}

fn float_format_code(width: FloatWidth) -> i64 {
    small_float_code(width).unwrap_or(if width == FloatWidth::F32 { 4 } else { 5 })
}

fn call_runtime<M: Module>(
    module: &mut M,
    builder: &mut FunctionBuilder,
    name: &str,
    params: &[types::Type],
    ret: types::Type,
    args: &[Value],
) -> Value {
    let mut sig = module.make_signature();
    sig.params.extend(params.iter().map(|ty| AbiParam::new(*ty)));
    sig.returns.push(AbiParam::new(ret));
    let func_id = module
        .declare_function(name, Linkage::Import, &sig)
        .unwrap_or_else(|error| panic!("failed to declare `{name}`: {error}"));
    let func_ref = module.declare_func_in_func(func_id, builder.func);
    let call = builder.ins().call(func_ref, args);
    builder.inst_results(call)[0]
}

fn float_to_f64<M: Module>(module: &mut M, builder: &mut FunctionBuilder, value: Value, width: FloatWidth) -> Value {
    match width {
        FloatWidth::F64 => value,
        FloatWidth::F32 => builder.ins().fpromote(types::F64, value),
        small => {
            let bits = builder.ins().uextend(types::I32, value);
            let code = builder.ins().iconst(types::I32, small_float_code(small).expect("small format"));
            call_runtime(module, builder, "paco_float_to_f64", &[types::I32, types::I32], types::F64, &[bits, code])
        }
    }
}

fn f64_to_float<M: Module>(module: &mut M, builder: &mut FunctionBuilder, value: Value, width: FloatWidth) -> Value {
    match width {
        FloatWidth::F64 => value,
        FloatWidth::F32 => builder.ins().fdemote(types::F32, value),
        small => {
            let code = builder.ins().iconst(types::I32, small_float_code(small).expect("small format"));
            let bits =
                call_runtime(module, builder, "paco_float_from_f64", &[types::F64, types::I32], types::I32, &[value, code]);
            builder.ins().ireduce(float_storage_type(small), bits)
        }
    }
}

fn compile_math<M: Module>(module: &mut M, builder: &mut FunctionBuilder, op: MathOp, values: &[Value], width: FloatWidth) -> Value {
    if small_float_code(width).is_some() {
        let wide: Vec<Value> = values.iter().map(|value| float_to_f64(module, builder, *value, width)).collect();
        let result = compile_math(module, builder, op, &wide, FloatWidth::F64);
        return f64_to_float(module, builder, result, width);
    }
    match op {
        MathOp::Sqrt => builder.ins().sqrt(values[0]),
        MathOp::Abs => builder.ins().fabs(values[0]),
        MathOp::Min => builder.ins().fmin(values[0], values[1]),
        MathOp::Max => builder.ins().fmax(values[0], values[1]),
        _ => {
            let symbol = op.runtime_symbol().expect("a runtime math function");
            let wide: Vec<Value> = values.iter().map(|value| float_to_f64(module, builder, *value, width)).collect();
            let params = vec![types::F64; wide.len()];
            let result = call_runtime(module, builder, symbol, &params, types::F64, &wide);
            f64_to_float(module, builder, result, width)
        }
    }
}

fn make_signature<M: Module>(module: &M, body: &Body, layouts: &TypeLayouts<'_>) -> Signature {
    let mut sig = module.make_signature();
    if is_real_aggregate(&body.return_ty, layouts) {
        sig.params.push(AbiParam::new(types::I64));
    }
    for local in &body.locals[..body.param_count] {
        if let Some(ty) = clif_type(&local.ty) {
            sig.params.push(AbiParam::new(ty));
        }
    }
    if let Some(ty) = clif_type(&body.return_ty) {
        sig.returns.push(AbiParam::new(ty));
    }
    sig
}

/// Where a local's value actually lives. Most locals are plain Cranelift
/// `Variable`s (SSA registers with no memory address at all) — cheap, and
/// correct for everything that is never borrowed. A local that `&`/`&mut`
/// is ever taken of (`Rvalue::Ref`) needs a real address instead, so it is
/// promoted to a stack slot up front, before any codegen for the function
/// body runs (never mixed mid-function — the storage kind is fixed for the
/// local's whole lifetime, avoiding any question of which one is the
/// current source of truth after the first borrow). A struct/enum-typed
/// local is never promoted even if borrowed: its `Variable` already holds
/// the *address* of its own (already stack-allocated) storage, established
/// when it was built via `Rvalue::Aggregate`, so taking its address is
/// already just reading that `Variable` — the same reason `place_address`'s
/// `Place::Local` arm below returns the raw value, not a load, for it.
#[derive(Clone, Copy)]
enum LocalStorage {
    Unit,
    Register(Variable),
    Stack(cranelift_codegen::ir::StackSlot, types::Type),
    Aggregate(cranelift_codegen::ir::StackSlot, u64),
}

/// Bundles the read-only, per-function context every codegen helper needs,
/// so functions taking it don't each spell out `locals`/`body`/`layouts`.
struct Ctx<'a> {
    locals: &'a [LocalStorage],
    body: &'a Body,
    layouts: &'a TypeLayouts<'a>,
    strings: HashMap<String, GlobalValue>,
    /// The caller-supplied address to write this function's own return
    /// value into, when `body.return_ty` is a real aggregate — `None`
    /// otherwise. Set once at function entry from the leading hidden
    /// parameter `make_signature` adds in that case.
    sret_addr: Option<Value>,
    glue: &'a Glue,
    glue_refs: HashMap<Type, (FuncRef, FuncRef)>,
    /// Set while a local owns a value that still has to be dropped.
    flags: Vec<Option<Variable>>,
    params: &'a HashMap<String, Vec<Type>>,
    panics: PanicSites,
}

impl Ctx<'_> {
    fn needs_drop(&self, ty: &Type) -> bool {
        self.glue.needs_drop(ty)
    }

    fn set_flag(&self, builder: &mut FunctionBuilder, local: paco_mir::Local, owned: bool) {
        if let Some(flag) = self.flags[local.0 as usize] {
            let value = builder.ins().iconst(types::I8, i64::from(owned));
            builder.def_var(flag, value);
        }
    }
}

/// Every local ever appearing as `&local`/`&mut local`'s direct target
/// (`Rvalue::Ref { place: Place::Local(_), .. }`) across the whole body —
/// see [`LocalStorage`] for why this determines its storage kind.
fn borrowed_locals(body: &Body) -> HashSet<u32> {
    body.blocks
        .iter()
        .flat_map(|block| &block.statements)
        .filter_map(|statement| match statement {
            Statement::Assign(_, Rvalue::Ref { place: Place::Local(local), .. }) => Some(local.0),
            _ => None,
        })
        .collect()
}

/// Whether `ty` is a struct/enum whose `Variable` already holds its own
/// address by construction (built via `Rvalue::Aggregate`, which
/// stack-allocates and returns that address) — `false` for a prelude
/// opaque handle type (`Sender`/`Receiver`/`JoinHandle`/`Generator`, ADR
/// 0022), which has no `Item::Struct` declaration and is never built via
/// `Aggregate` at all, so it needs the same stack-slot promotion a scalar
/// does when borrowed. See [`LocalStorage`].
fn is_real_aggregate(ty: &Type, layouts: &TypeLayouts<'_>) -> bool {
    matches!(ty, Type::Struct(name, _) if layouts.has_struct(name))
        || matches!(ty, Type::Enum(..) | Type::Tuple(_))
        // A `[]T`-typed local's `Variable` also already holds its own
        // address (whatever produced it — `Rvalue::RawAlloc`-and-store
        // today, eventually `slice_of_zeros`'s own codegen — returns an
        // address, matching `Rvalue::Aggregate`'s convention for structs).
        || matches!(ty, Type::Slice(_) | Type::String)
}

pub fn compile_function<M: Module>(
    module: &mut M,
    ctx: &mut cranelift_codegen::Context,
    fb_ctx: &mut FunctionBuilderContext,
    body: &Body,
    func_ids: &HashMap<String, FuncId>,
    layouts: &TypeLayouts<'_>,
) {
    compile_function_with(
        module,
        ctx,
        fb_ctx,
        body,
        func_ids,
        layouts,
        &Glue::default(),
        &HashMap::new(),
        &mut PanicData::default(),
        None,
    );
}

const PANIC_MESSAGES: [&str; 9] = [
    "division by zero",
    "remainder by zero",
    "attempt to divide with overflow",
    "attempt to add with overflow",
    "attempt to subtract with overflow",
    "attempt to multiply with overflow",
    "attempt to negate with overflow",
    "attempt to shift left with overflow",
    "attempt to shift right with overflow",
];

/// Module-wide data behind panic reports: NUL-terminated file names and
/// messages, declared once.
#[derive(Default)]
struct PanicData<'a> {
    locator: Option<&'a paco_mir::SourceLocator<'a>>,
    /// Debug builds keep frame pointers, pass them to panic reports and
    /// emit line tables.
    debug: bool,
    files: HashMap<String, cranelift_module::DataId>,
    messages: HashMap<&'static str, cranelift_module::DataId>,
    locations: Vec<(String, u32, u32)>,
    location_ids: HashMap<(String, u32, u32), u32>,
    functions: Vec<debug::DebugFunction>,
}

impl PanicData<'_> {
    fn c_string<M: Module>(module: &mut M, text: &str) -> cranelift_module::DataId {
        let data_id = module
            .declare_anonymous_data(false, false)
            .unwrap_or_else(|error| panic!("failed to declare a string constant: {error}"));
        let mut description = DataDescription::new();
        description.define(text.bytes().chain([0]).collect());
        module.define_data(data_id, &description).unwrap_or_else(|error| panic!("failed to define a string constant: {error}"));
        data_id
    }

    /// The per-function view: every location `body` can panic at, and the
    /// runtime entries it calls.
    fn sites<M: Module>(
        &mut self,
        module: &mut M,
        builder: &mut FunctionBuilder,
        body: &Body,
        own: Option<FuncId>,
    ) -> PanicSites {
        let sig = |params: &[types::Type]| {
            let mut sig = module.make_signature();
            sig.params.extend(params.iter().map(|ty| AbiParam::new(*ty)));
            sig
        };
        let (i32_, i64_) = (types::I32, types::I64);
        let signatures = [
            ("paco_rt_panic", sig(&[i64_, i64_, i32_, i32_, i64_, i64_])),
            ("paco_rt_panic_str", sig(&[i64_, i64_, i32_, i32_, i64_, i64_])),
            ("paco_rt_panic_bounds", sig(&[i64_, i64_, i64_, i32_, i32_, i64_, i64_])),
        ];
        let [panic, panic_str, panic_bounds] = signatures.map(|(name, sig)| {
            let func_id = module
                .declare_function(name, Linkage::Import, &sig)
                .unwrap_or_else(|error| panic!("failed to declare `{name}`: {error}"));
            module.declare_func_in_func(func_id, builder.func)
        });
        let mut messages = HashMap::new();
        for message in PANIC_MESSAGES {
            let data_id = *self.messages.entry(message).or_insert_with(|| Self::c_string(module, message));
            messages.insert(message, module.declare_data_in_func(data_id, builder.func));
        }
        let mut files: HashMap<&str, GlobalValue> = HashMap::new();
        let debug = self.debug && body.profile == Profile::Debug;
        let mut locate = |span: paco_span::Span| -> Location {
            let Some((file, line, column)) = self.locator.and_then(|locator| locator.locate(span)) else {
                return (None, 0, 0, SourceLoc::default());
            };
            let global = *files.entry(file).or_insert_with(|| {
                let data_id = *self.files.entry(file.to_string()).or_insert_with(|| Self::c_string(module, file));
                module.declare_data_in_func(data_id, builder.func)
            });
            let srcloc = if debug {
                let key = (file.to_string(), line, column);
                let next = self.locations.len() as u32;
                let id = *self.location_ids.entry(key.clone()).or_insert(next);
                if id == next {
                    self.locations.push(key);
                }
                SourceLoc::new(id)
            } else {
                SourceLoc::default()
            };
            (Some(global), line, column, srcloc)
        };
        let start = locate(body.span);
        let blocks = body
            .blocks
            .iter()
            .enumerate()
            .map(|(block, data)| {
                let statements = (0..data.statements.len()).map(|index| locate(body.statement_span(block, index))).collect();
                (statements, locate(body.terminator_span(block)))
            })
            .collect();
        PanicSites {
            panic,
            panic_str,
            panic_bounds,
            messages,
            blocks,
            function: own.filter(|_| debug).map(|id| module.declare_func_in_func(id, builder.func)),
            frame_pointers: debug,
            here: std::cell::Cell::new(start),
        }
    }
}

type Location = (Option<GlobalValue>, u32, u32, SourceLoc);

struct PanicSites {
    panic: FuncRef,
    panic_str: FuncRef,
    panic_bounds: FuncRef,
    messages: HashMap<&'static str, GlobalValue>,
    blocks: Vec<(Vec<Location>, Location)>,
    function: Option<FuncRef>,
    frame_pointers: bool,
    here: std::cell::Cell<Location>,
}

impl PanicSites {
    /// Calls `entry` with `leading` and the current location; never returns.
    fn call(&self, builder: &mut FunctionBuilder, entry: FuncRef, leading: &[Value]) {
        let (file, line, column, _) = self.here.get();
        let file = match file {
            Some(global) => builder.ins().symbol_value(types::I64, global),
            None => builder.ins().iconst(types::I64, 0),
        };
        let line = builder.ins().iconst(types::I32, i64::from(line));
        let column = builder.ins().iconst(types::I32, i64::from(column));
        let frame =
            if self.frame_pointers { builder.ins().get_frame_pointer(types::I64) } else { builder.ins().iconst(types::I64, 0) };
        let function = match self.function {
            Some(own) => builder.ins().func_addr(types::I64, own),
            None => builder.ins().iconst(types::I64, 0),
        };
        let args: Vec<Value> = leading.iter().copied().chain([file, line, column, frame, function]).collect();
        builder.ins().call(entry, &args);
        builder.ins().trap(TrapCode::user(1).unwrap());
    }

    /// Panics with `message` when `condition` is non-zero.
    fn check(&self, builder: &mut FunctionBuilder, condition: Value, message: &'static str) {
        self.check_with(builder, condition, |sites, builder| {
            let text = builder.ins().symbol_value(types::I64, sites.messages[message]);
            sites.call(builder, sites.panic_str, &[text]);
        });
    }

    fn check_with(&self, builder: &mut FunctionBuilder, condition: Value, fail: impl FnOnce(&Self, &mut FunctionBuilder)) {
        let failed = builder.create_block();
        let passed = builder.create_block();
        builder.set_cold_block(failed);
        builder.ins().brif(condition, failed, &[], passed, &[]);
        builder.switch_to_block(failed);
        fail(self, builder);
        builder.switch_to_block(passed);
    }
}

#[allow(clippy::too_many_arguments)]
fn compile_function_with<M: Module>(
    module: &mut M,
    ctx: &mut cranelift_codegen::Context,
    fb_ctx: &mut FunctionBuilderContext,
    body: &Body,
    func_ids: &HashMap<String, FuncId>,
    layouts: &TypeLayouts<'_>,
    glue: &Glue,
    params: &HashMap<String, Vec<Type>>,
    panic_data: &mut PanicData<'_>,
    own: Option<FuncId>,
) {
    let mut builder = FunctionBuilder::new(&mut ctx.func, fb_ctx);
    let panics = panic_data.sites(module, &mut builder, body, own);
    builder.set_srcloc(panics.here.get().3);

    let clif_blocks: Vec<_> = body.blocks.iter().map(|_| builder.create_block()).collect();

    let borrowed = borrowed_locals(body);
    let locals: Vec<LocalStorage> = body
        .locals
        .iter()
        .enumerate()
        .map(|(i, local)| match clif_type(&local.ty) {
            None => LocalStorage::Unit,
            Some(_) if is_real_aggregate(&local.ty, layouts) => {
                let (size, align) = layout_size_align(&local.ty, layouts);
                let slot = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    size.max(1) as u32,
                    align.trailing_zeros() as u8,
                ));
                LocalStorage::Aggregate(slot, size)
            }
            Some(ty) if borrowed.contains(&(i as u32)) => {
                let size = ty.bytes();
                let slot = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    size,
                    size.trailing_zeros() as u8,
                ));
                LocalStorage::Stack(slot, ty)
            }
            Some(ty) => LocalStorage::Register(builder.declare_var(ty)),
        })
        .collect();
    let mut strings = HashMap::new();
    for text in body_strings(body) {
        if strings.contains_key(text) {
            continue;
        }
        let data_id = module
            .declare_anonymous_data(false, false)
            .unwrap_or_else(|error| panic!("failed to declare a string constant: {error}"));
        let mut description = DataDescription::new();
        description.define(text.as_bytes().to_vec().into_boxed_slice());
        module
            .define_data(data_id, &description)
            .unwrap_or_else(|error| panic!("failed to define a string constant: {error}"));
        strings.insert(text.to_string(), module.declare_data_in_func(data_id, builder.func));
    }
    let flags: Vec<Option<Variable>> = body
        .locals
        .iter()
        .map(|local| glue.needs_drop(&local.ty).then(|| builder.declare_var(types::I8)))
        .collect();
    let glue_refs = glue
        .all()
        .map(|(ty, (drop_id, clone_id))| {
            let drop_ref = module.declare_func_in_func(*drop_id, builder.func);
            let clone_ref = module.declare_func_in_func(*clone_id, builder.func);
            (ty.clone(), (drop_ref, clone_ref))
        })
        .collect();
    let mut cx = Ctx {
        locals: &locals,
        body,
        layouts,
        strings,
        sret_addr: None,
        glue,
        glue_refs,
        flags,
        params,
        panics,
    };

    for (index, clif_block) in clif_blocks.iter().enumerate() {
        builder.switch_to_block(*clif_block);
        if index == 0 {
            builder.append_block_params_for_function_params(*clif_block);
            let mut params: Vec<Value> = builder.block_params(*clif_block).to_vec();
            if is_real_aggregate(&body.return_ty, layouts) {
                cx.sret_addr = Some(params.remove(0));
            }
            for flag in cx.flags.iter().flatten() {
                let zero = builder.ins().iconst(types::I8, 0);
                builder.def_var(*flag, zero);
            }
            let mut param_locals = body.locals[..body.param_count]
                .iter()
                .enumerate()
                .filter(|(_, local)| clif_type(&local.ty).is_some())
                .map(|(i, _)| i);
            for value in params {
                let Some(i) = param_locals.next() else { break };
                match cx.locals[i] {
                    LocalStorage::Register(var) => builder.def_var(var, value),
                    LocalStorage::Stack(slot, _) => {
                        let addr = builder.ins().stack_addr(types::I64, slot, 0);
                        builder.ins().store(MemFlagsData::trusted(), value, addr, 0);
                    }
                    LocalStorage::Aggregate(slot, size) => {
                        let addr = builder.ins().stack_addr(types::I64, slot, 0);
                        copy_words(&mut builder, value, addr, size);
                    }
                    LocalStorage::Unit => {}
                }
                cx.set_flag(&mut builder, paco_mir::Local(i as u32), true);
            }
        }
        for (statement_index, statement) in body.blocks[index].statements.iter().enumerate() {
            let here = cx.panics.blocks[index].0[statement_index];
            cx.panics.here.set(here);
            builder.set_srcloc(here.3);
            compile_statement(module, &mut builder, statement, &cx, func_ids, body.profile);
        }
        let here = cx.panics.blocks[index].1;
        cx.panics.here.set(here);
        builder.set_srcloc(here.3);
        compile_terminator(
            module,
            &mut builder,
            &body.blocks[index].terminator,
            &clif_blocks,
            &cx,
            func_ids,
            &body.return_ty,
        );
    }

    builder.seal_all_blocks();
    builder.finalize(module.target_config());
}

fn compile_statement<M: Module>(
    module: &mut M,
    builder: &mut FunctionBuilder,
    statement: &Statement,
    cx: &Ctx,
    func_ids: &HashMap<String, FuncId>,
    profile: Profile,
) {
    match statement {
        Statement::Assign(place, rvalue) => {
            if place_is_unit(place, cx.body) {
                return;
            }
            let value = match rvalue {
                Rvalue::Use(operand) => compile_owned_as(builder, operand, &place_ty(place, cx), cx),
                _ => compile_rvalue(module, builder, rvalue, cx, func_ids, profile),
            };
            if matches!(rvalue, Rvalue::Load { .. }) {
                write_place_unowned(builder, place, value, cx);
            } else {
                write_place(builder, place, value, cx);
            }
        }
        Statement::Store { address, value, ty } => {
            if matches!(ty, Type::Unit) {
                return;
            }
            let addr = compile_operand(builder, address, cx);
            let mut val = compile_owned_operand(builder, value, cx);
            if is_real_aggregate(ty, cx.layouts) {
                let (size, _) = layout_size_align(ty, cx.layouts);
                let size_value = builder.ins().iconst(types::I64, size.max(1) as i64);
                let boxed = call_returning(module, builder, "paco_alloc", types::I64, &[size_value]);
                copy_words(builder, val, boxed, size);
                val = boxed;
            }
            builder.ins().store(MemFlagsData::trusted(), val, addr, 0);
        }
        Statement::FreeBox { address, ty } => {
            if is_real_aggregate(ty, cx.layouts) {
                let addr = compile_operand(builder, address, cx);
                let boxed = builder.ins().load(types::I64, MemFlagsData::trusted(), addr, 0);
                call_void(module, builder, "paco_free", &[boxed]);
            }
        }
        Statement::Drop(Place::Local(local)) => drop_local_if_owned(builder, *local, cx),
        Statement::Drop(_) | Statement::StorageDead(_) => {}
    }
}

fn place_is_unit(place: &Place, body: &Body) -> bool {
    matches!(place, Place::Local(local) if matches!(body.locals[local.0 as usize].ty, Type::Unit | Type::Never))
}

fn compile_rvalue<M: Module>(
    module: &mut M,
    builder: &mut FunctionBuilder,
    rvalue: &Rvalue,
    cx: &Ctx,
    func_ids: &HashMap<String, FuncId>,
    profile: Profile,
) -> Value {
    match rvalue {
        Rvalue::Use(operand) => compile_owned_operand(builder, operand, cx),
        Rvalue::UnaryOp(op, operand) => {
            let operand_ty = operand_ty(operand, cx);
            let value = compile_operand(builder, operand, cx);
            compile_unary(builder, *op, value, &operand_ty, cx, profile)
        }
        Rvalue::BinaryOp(op, left_operand, right_operand) => {
            let operand_ty = operand_ty(left_operand, cx);
            let left = compile_operand(builder, left_operand, cx);
            let right = compile_operand(builder, right_operand, cx);
            if matches!(strip_borrow(&operand_ty), Type::String) {
                return compile_string_binary(module, builder, *op, left, right);
            }
            if let Type::Float(width) = operand_ty
                && small_float_code(width).is_some()
            {
                let wide_left = float_to_f64(module, builder, left, width);
                let wide_right = float_to_f64(module, builder, right, width);
                let wide_ty = Type::Float(FloatWidth::F64);
                let result = compile_binary(builder, *op, wide_left, wide_right, &wide_ty, profile, cx);
                return if builder.func.dfg.value_type(result) == types::F64 {
                    f64_to_float(module, builder, result, width)
                } else {
                    result
                };
            }
            compile_binary(builder, *op, left, right, &operand_ty, profile, cx)
        }
        Rvalue::Cast { operand, target } => {
            let source_ty = operand_ty(operand, cx);
            let value = compile_operand(builder, operand, cx);
            compile_cast(module, builder, value, &source_ty, target)
        }
        Rvalue::Math(op, args) => {
            let Type::Float(width) = operand_ty(&args[0], cx) else { panic!("`{}` needs a float receiver", op.name()) };
            let values: Vec<Value> = args.iter().map(|arg| compile_operand(builder, arg, cx)).collect();
            compile_math(module, builder, *op, &values, width)
        }
        Rvalue::Aggregate { ty, variant, fields } => {
            compile_aggregate(builder, ty, variant.as_deref(), fields, cx)
        }
        Rvalue::SliceLen(place) => {
            let descriptor = read_place(builder, place, cx);
            builder.ins().load(types::I64, MemFlagsData::trusted(), descriptor, SLICE_LEN_OFFSET)
        }
        Rvalue::Discriminant(place) => {
            let (addr, _ty) = place_address(builder, place, cx);
            builder.ins().load(types::I64, MemFlagsData::trusted(), addr, 0)
        }
        Rvalue::Load { address, ty } => {
            let addr = compile_operand(builder, address, cx);
            let clif_ty = clif_type(ty).unwrap_or_else(|| panic!("cannot load a unit-typed value"));
            builder.ins().load(clif_ty, MemFlagsData::trusted(), addr, 0)
        }
        Rvalue::RawAlloc { size } => allocate_stack_slot(builder, *size, 8),
        Rvalue::Ref { place, .. } => place_address(builder, place, cx).0,
        Rvalue::Quote { .. } => panic!("`quote` is evaluated at compile time"),
        Rvalue::FuncAddr(name) => {
            let func_id = *func_ids
                .get(name)
                .unwrap_or_else(|| panic!("undeclared function `{name}`"));
            let func_ref = module.declare_func_in_func(func_id, builder.func);
            builder.ins().func_addr(types::I64, func_ref)
        }
    }
}

fn compile_aggregate(
    builder: &mut FunctionBuilder,
    ty: &Type,
    variant: Option<&str>,
    fields: &[Operand],
    cx: &Ctx,
) -> Value {
    // Field values are compiled lazily, per field, rather than eagerly up
    // front: a `Unit`-typed field (e.g. `Result::Ok(())`, `paco-mir`'s
    // FFI-result lowering for `Sender::send`) has no runtime representation
    // at all — `compile_operand` panics on `Constant::Unit` — and must be
    // skipped entirely, the same "elided, never materialized" convention
    // `place_is_unit`/`Statement::Assign` already apply to Unit locals.
    //
    // Each field's *type* for this skip check (and for `store_field`'s
    // width/kind) comes from `operand_ty(field, cx)` — the operand's own
    // actual type — not from `cx.layouts`' declaration lookup. These
    // normally agree, but can genuinely differ when `Result`/`Option` are
    // declared as a single non-generic, concretely-typed source enum (a
    // `concurrency-codegen`-era workaround still used by some tests, e.g.
    // `enum Result { Ok(i64), Err(SendError) }`): one shared declaration is
    // then reused at every call site, even though `Sender::send`'s
    // `Result<(), SendError>` and `Receiver::recv`'s `Result<T, RecvError>`
    // instantiate `Ok`'s payload differently. Only the *offset* still comes
    // from `cx.layouts` (positional, correct regardless of instantiation for
    // `Result`/`Option`'s single-field variants).
    match (ty, variant) {
        (Type::Struct(name, args), None) => {
            let layout = cx.layouts.struct_layout(name, args);
            let addr = allocate_stack_slot(builder, layout.size, layout.align);
            for ((declared, offset), field) in cx.layouts.struct_fields(name, args).into_iter().zip(fields) {
                let mut field_ty = operand_ty(field, cx);
                if matches!(field_ty, Type::Unit) {
                    continue;
                }
                if matches!(&field_ty, Type::Borrow { ty, .. } if **ty == declared) {
                    field_ty = declared.clone();
                }
                let value = compile_owned_as(builder, field, &declared, cx);
                store_field(builder, value, &field_ty, addr, offset as i32, cx.layouts);
            }
            mark_live(builder, ty, addr, cx);
            addr
        }
        (Type::Enum(name, args), Some(variant_name)) => {
            let layout = cx.layouts.enum_layout(name, args);
            let addr = allocate_stack_slot(builder, layout.size, layout.align);
            let discriminant = cx.layouts.enum_variant_index(name, args, variant_name);
            let tag = builder.ins().iconst(types::I64, discriminant as i64);
            builder.ins().store(MemFlagsData::trusted(), tag, addr, 0);
            for (index, field) in fields.iter().enumerate() {
                let (_, offset) = cx.layouts.enum_variant_field(name, args, variant_name, index);
                let field_ty = operand_ty(field, cx);
                if matches!(field_ty, Type::Unit) {
                    continue;
                }
                let value = compile_owned_operand(builder, field, cx);
                store_field(builder, value, &field_ty, addr, offset as i32, cx.layouts);
            }
            mark_live(builder, ty, addr, cx);
            addr
        }
        (Type::Tuple(items), None) => {
            let layout = cx.layouts.tuple_layout(items);
            let addr = allocate_stack_slot(builder, layout.size, layout.align);
            for (index, field) in fields.iter().enumerate() {
                let field_ty = operand_ty(field, cx);
                if matches!(field_ty, Type::Unit) {
                    continue;
                }
                let (_, offset) = cx.layouts.tuple_field(items, index);
                let value = compile_owned_operand(builder, field, cx);
                store_field(builder, value, &field_ty, addr, offset as i32, cx.layouts);
            }
            addr
        }
        _ => panic!("aggregate codegen is not implemented yet: {ty:?} / {variant:?}"),
    }
}

fn mark_live(builder: &mut FunctionBuilder, ty: &Type, addr: Value, cx: &Ctx) {
    if let Some(offset) = cx.layouts.live_offset(ty) {
        let live = builder.ins().iconst(types::I8, 1);
        builder.ins().store(MemFlagsData::trusted(), live, addr, offset as i32);
    }
}

fn store_field(
    builder: &mut FunctionBuilder,
    value: Value,
    field_ty: &Type,
    dest_addr: Value,
    offset: i32,
    layouts: &TypeLayouts<'_>,
) {
    match field_ty {
        Type::Struct(name, args) if layouts.has_struct(name) => {
            copy_bytes(builder, value, dest_addr, offset, layouts.struct_layout(name, args).size)
        }
        Type::Enum(name, args) => copy_bytes(builder, value, dest_addr, offset, layouts.enum_layout(name, args).size),
        Type::Tuple(items) => copy_bytes(builder, value, dest_addr, offset, layouts.tuple_layout(items).size),
        // Like a struct/enum, a `[]T`-typed operand's `Value` holds the
        // 16-byte `[data_ptr, len]` descriptor's own *address*, not the
        // descriptor inline — the scalar `store` below would write only
        // that address's 8 bytes into the field, leaving the descriptor's
        // `len` half as uninitialized stack garbage.
        Type::Slice(_) | Type::String => copy_bytes(builder, value, dest_addr, offset, 16),
        _ => {
            builder.ins().store(MemFlagsData::trusted(), value, dest_addr, offset);
        }
    }
}

fn copy_bytes(builder: &mut FunctionBuilder, src_addr: Value, dest_addr: Value, dest_offset: i32, size: u64) {
    let dest = if dest_offset == 0 {
        dest_addr
    } else {
        builder.ins().iadd_imm_s(dest_addr, i64::from(dest_offset))
    };
    for i in 0..size {
        let byte = builder.ins().load(types::I8, MemFlagsData::trusted(), src_addr, i as i32);
        builder.ins().store(MemFlagsData::trusted(), byte, dest, i as i32);
    }
}

fn copy_words(builder: &mut FunctionBuilder, src: Value, dest: Value, size: u64) {
    let words = size / 8;
    for i in 0..words {
        let word = builder.ins().load(types::I64, MemFlagsData::trusted(), src, (i * 8) as i32);
        builder.ins().store(MemFlagsData::trusted(), word, dest, (i * 8) as i32);
    }
    for i in words * 8..size {
        let byte = builder.ins().load(types::I8, MemFlagsData::trusted(), src, i as i32);
        builder.ins().store(MemFlagsData::trusted(), byte, dest, i as i32);
    }
}

fn layout_size_align(ty: &Type, layouts: &TypeLayouts<'_>) -> (u64, u64) {
    match ty {
        Type::Struct(name, args) => {
            let layout = layouts.struct_layout(name, args);
            (layout.size, layout.align)
        }
        Type::Enum(name, args) => {
            let layout = layouts.enum_layout(name, args);
            (layout.size, layout.align)
        }
        Type::Tuple(items) => {
            let layout = layouts.tuple_layout(items);
            (layout.size, layout.align)
        }
        Type::Slice(_) | Type::String => (16, 8),
        other => panic!("not a real aggregate: {other:?}"),
    }
}

/// `value` is an aggregate's address, or the scalar itself (a shared-cell
/// pointer); glue always takes an address, so scalars go through a spill.
fn value_address(builder: &mut FunctionBuilder, ty: &Type, value: Value, cx: &Ctx) -> Value {
    if is_real_aggregate(ty, cx.layouts) {
        return value;
    }
    let slot = allocate_stack_slot(builder, 8, 8);
    builder.ins().store(MemFlagsData::trusted(), value, slot, 0);
    slot
}

fn drop_value(builder: &mut FunctionBuilder, ty: &Type, value: Value, cx: &Ctx) {
    if let Some(&(drop_ref, _)) = cx.glue_refs.get(ty) {
        let addr = value_address(builder, ty, value, cx);
        builder.ins().call(drop_ref, &[addr]);
    }
}

/// A fresh, independently owned copy of the value `value` holds.
fn clone_value(builder: &mut FunctionBuilder, ty: &Type, value: Value, cx: &Ctx) -> Value {
    let Some(&(_, clone_ref)) = cx.glue_refs.get(ty) else {
        return value;
    };
    if is_real_aggregate(ty, cx.layouts) {
        let (size, align) = layout_size_align(ty, cx.layouts);
        let copy = allocate_stack_slot(builder, size, align);
        copy_words(builder, value, copy, size);
        builder.ins().call(clone_ref, &[copy]);
        copy
    } else {
        let addr = value_address(builder, ty, value, cx);
        builder.ins().call(clone_ref, &[addr]);
        value
    }
}

/// Evaluates `operand` where its value is consumed into a new owner: a
/// moved local gives up ownership, anything else is cloned.
fn compile_owned_operand(builder: &mut FunctionBuilder, operand: &Operand, cx: &Ctx) -> Value {
    let ty = operand_ty(operand, cx);
    let value = compile_operand(builder, operand, cx);
    if !cx.needs_drop(&ty) {
        return value;
    }
    match operand {
        Operand::Move(Place::Local(local)) => {
            cx.set_flag(builder, *local, false);
            value
        }
        Operand::Move(Place::VariantField { base, .. })
            if matches!(base.as_ref(), Place::Local(local)
                if cx.body.locals[local.0 as usize].name.as_deref() == Some(paco_mir::TRY_TEMP)) =>
        {
            let Place::Local(local) = base.as_ref() else { unreachable!() };
            cx.set_flag(builder, *local, false);
            value
        }
        _ => clone_value(builder, &ty, value, cx),
    }
}

/// Like [`compile_owned_operand`], but a borrow of `expected` is copied out
/// of its referent, which the type checker accepts as an implicit deref.
fn compile_owned_as(builder: &mut FunctionBuilder, operand: &Operand, expected: &Type, cx: &Ctx) -> Value {
    if let Type::Borrow { ty: inner, .. } = operand_ty(operand, cx)
        && *inner == *expected
    {
        let pointer = compile_operand(builder, operand, cx);
        let value = match clif_type(expected) {
            Some(clif_ty) if !is_real_aggregate(expected, cx.layouts) => {
                builder.ins().load(clif_ty, MemFlagsData::trusted(), pointer, 0)
            }
            _ => pointer,
        };
        return clone_value(builder, expected, value, cx);
    }
    compile_owned_operand(builder, operand, cx)
}

fn drop_local_if_owned(builder: &mut FunctionBuilder, local: paco_mir::Local, cx: &Ctx) {
    let Some(flag) = cx.flags[local.0 as usize] else { return };
    let owned = builder.use_var(flag);
    let drop_block = builder.create_block();
    let done = builder.create_block();
    builder.ins().brif(owned, drop_block, &[], done, &[]);
    builder.switch_to_block(drop_block);
    let ty = cx.body.locals[local.0 as usize].ty.clone();
    let value = read_place(builder, &Place::Local(local), cx);
    drop_value(builder, &ty, value, cx);
    cx.set_flag(builder, local, false);
    builder.ins().jump(done, &[]);
    builder.switch_to_block(done);
}

fn allocate_stack_slot(builder: &mut FunctionBuilder, size: u64, align: u64) -> Value {
    let align_shift = align.trailing_zeros();
    let slot = builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        size as u32,
        align_shift as u8,
    ));
    builder.ins().stack_addr(types::I64, slot, 0)
}

fn place_address(builder: &mut FunctionBuilder, place: &Place, cx: &Ctx) -> (Value, Type) {
    match place {
        Place::Local(local) => {
            let ty = cx.body.locals[local.0 as usize].ty.clone();
            let addr = match cx.locals[local.0 as usize] {
                // A struct/enum's `Variable` already holds its own address.
                LocalStorage::Register(var) => builder.use_var(var),
                LocalStorage::Stack(slot, _) | LocalStorage::Aggregate(slot, _) => {
                    builder.ins().stack_addr(types::I64, slot, 0)
                }
                LocalStorage::Unit => panic!("place has no value (unit-typed local {local:?})"),
            };
            (addr, ty)
        }
        Place::Field { base, field } => {
            let (base_addr, base_ty) = place_address(builder, base, cx);
            let (field_ty, offset) = field_of(&base_ty, field, cx);
            (builder.ins().iadd_imm_s(base_addr, offset as i64), field_ty)
        }
        Place::VariantField { base, variant, index } => {
            let (base_addr, base_ty) = place_address(builder, base, cx);
            let (enum_name, args) = enum_name_of(&base_ty);
            let (field_ty, offset) = cx.layouts.enum_variant_field(&enum_name, &args, variant, *index);
            (builder.ins().iadd_imm_s(base_addr, offset as i64), field_ty)
        }
        Place::Index { base, index } => {
            let (base_addr, base_ty) = place_address(builder, base, cx);
            let elem_ty = slice_elem_of(&base_ty);
            let addr = compile_slice_element_address(builder, base_addr, index, &elem_ty, cx);
            (addr, elem_ty)
        }
        Place::Deref { address, ty } => {
            let addr = compile_operand(builder, address, cx);
            (addr, ty.clone())
        }
    }
}

/// `base_addr` points at a 16-byte `[data_ptr, len]` slice descriptor
/// (`SLICE_DATA_OFFSET`/`SLICE_LEN_OFFSET`). Panics on an out-of-range index
/// in every profile; the unsigned comparison also catches a negative index.
fn compile_slice_element_address(
    builder: &mut FunctionBuilder,
    base_addr: Value,
    index: &Operand,
    elem_ty: &Type,
    cx: &Ctx,
) -> Value {
    let elem_size = element_size(elem_ty, cx);
    let data_ptr = builder.ins().load(types::I64, MemFlagsData::trusted(), base_addr, SLICE_DATA_OFFSET);
    let len = builder.ins().load(types::I64, MemFlagsData::trusted(), base_addr, SLICE_LEN_OFFSET);
    let index_value = compile_operand(builder, index, cx);
    let out_of_bounds = builder.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, index_value, len);
    cx.panics.check_with(builder, out_of_bounds, |sites, builder| sites.call(builder, sites.panic_bounds, &[index_value, len]));
    let byte_offset = builder.ins().imul_imm_s(index_value, elem_size as i64);
    builder.ins().iadd(data_ptr, byte_offset)
}

fn element_size(elem_ty: &Type, cx: &Ctx) -> u64 {
    match elem_ty {
        Type::Struct(name, args) if cx.layouts.has_struct(name) => cx.layouts.struct_layout(name, args).size,
        Type::Struct(..) => 8,
        Type::Enum(name, args) => cx.layouts.enum_layout(name, args).size,
        Type::Tuple(items) => cx.layouts.tuple_layout(items).size,
        _ => paco_mir::scalar_layout(elem_ty)
            .unwrap_or_else(|| panic!("cannot compute a slice element's size: {elem_ty:?}"))
            .size,
    }
}

fn aggregate_layout_size_align(ty: &Type, cx: &Ctx) -> (u64, u64) {
    match ty {
        Type::Struct(name, args) => {
            let layout = cx.layouts.struct_layout(name, args);
            (layout.size, layout.align)
        }
        Type::Enum(name, args) => {
            let layout = cx.layouts.enum_layout(name, args);
            (layout.size, layout.align)
        }
        Type::Tuple(items) => {
            let layout = cx.layouts.tuple_layout(items);
            (layout.size, layout.align)
        }
        Type::Slice(_) | Type::String => (16, 8),
        other => panic!("not a real aggregate: {other:?}"),
    }
}

fn place_ty(place: &Place, cx: &Ctx) -> Type {
    match place {
        Place::Local(local) => cx.body.locals[local.0 as usize].ty.clone(),
        Place::Field { base, field } => field_of(&place_ty(base, cx), field, cx).0,
        Place::VariantField { base, variant, index } => {
            let base_ty = place_ty(base, cx);
            let (enum_name, args) = enum_name_of(&base_ty);
            cx.layouts.enum_variant_field(&enum_name, &args, variant, *index).0
        }
        Place::Index { base, .. } => slice_elem_of(&place_ty(base, cx)),
        Place::Deref { ty, .. } => ty.clone(),
    }
}

fn operand_ty(operand: &Operand, cx: &Ctx) -> Type {
    match operand {
        Operand::Copy(place) | Operand::Move(place) => place_ty(place, cx),
        Operand::Constant(constant) => match constant {
            paco_mir::Constant::Int(_, width) => Type::Int(*width),
            paco_mir::Constant::Float(_, width) => Type::Float(*width),
            paco_mir::Constant::Bool(_) => Type::Bool,
            paco_mir::Constant::Char(_) => Type::Char,
            paco_mir::Constant::Str(_) => Type::String,
            paco_mir::Constant::Unit => Type::Unit,
            paco_mir::Constant::Type(_) => Type::TypeValue(Box::new(Type::Unknown)),
        },
    }
}

fn read_place(builder: &mut FunctionBuilder, place: &Place, cx: &Ctx) -> Value {
    match place {
        Place::Local(local) => match cx.locals[local.0 as usize] {
            LocalStorage::Register(var) => builder.use_var(var),
            LocalStorage::Stack(slot, clif_ty) => {
                let addr = builder.ins().stack_addr(types::I64, slot, 0);
                builder.ins().load(clif_ty, MemFlagsData::trusted(), addr, 0)
            }
            LocalStorage::Aggregate(slot, _) => builder.ins().stack_addr(types::I64, slot, 0),
            LocalStorage::Unit => panic!("operand has no value (unit-typed local {local:?})"),
        },
        Place::Field { .. } | Place::VariantField { .. } | Place::Index { .. } | Place::Deref { .. } => {
            let (addr, ty) = place_address(builder, place, cx);
            match ty {
                // A `[]T` field's own storage *is* its 16-byte descriptor
                // (inline, per `store_field`'s `Type::Slice` arm), matching
                // struct/enum fields — so reading it, like them, yields the
                // field's own address, not a scalar load through it.
                Type::Struct(ref name, _) if cx.layouts.has_struct(name) => addr,
                Type::Enum(_, _) | Type::Tuple(_) | Type::Slice(_) | Type::String => addr,
                _ => {
                    let clif_ty = clif_type(&ty)
                        .unwrap_or_else(|| panic!("cannot load a unit-typed field: {ty:?}"));
                    builder.ins().load(clif_ty, MemFlagsData::trusted(), addr, 0)
                }
            }
        }
    }
}

/// Stores an owned `value` into `place`, dropping whatever it held before.
fn write_place(builder: &mut FunctionBuilder, place: &Place, value: Value, cx: &Ctx) {
    match place {
        Place::Local(local) => {
            drop_local_if_owned(builder, *local, cx);
            write_place_unowned(builder, place, value, cx);
            cx.set_flag(builder, *local, true);
        }
        Place::Field { .. } | Place::VariantField { .. } | Place::Index { .. } | Place::Deref { .. } => {
            let (addr, ty) = place_address(builder, place, cx);
            if cx.needs_drop(&ty) {
                let old = if is_real_aggregate(&ty, cx.layouts) {
                    addr
                } else {
                    let clif_ty = clif_type(&ty).expect("owning types are never unit");
                    builder.ins().load(clif_ty, MemFlagsData::trusted(), addr, 0)
                };
                drop_value(builder, &ty, old, cx);
            }
            store_field(builder, value, &ty, addr, 0, cx.layouts);
        }
    }
}

/// Stores the bits of `value` into `place` without taking ownership.
fn write_place_unowned(builder: &mut FunctionBuilder, place: &Place, value: Value, cx: &Ctx) {
    match place {
        Place::Local(local) => match cx.locals[local.0 as usize] {
            LocalStorage::Register(var) => builder.def_var(var, value),
            LocalStorage::Stack(slot, _) => {
                let addr = builder.ins().stack_addr(types::I64, slot, 0);
                builder.ins().store(MemFlagsData::trusted(), value, addr, 0);
            }
            LocalStorage::Aggregate(slot, size) => {
                let addr = builder.ins().stack_addr(types::I64, slot, 0);
                copy_words(builder, value, addr, size);
            }
            LocalStorage::Unit => {}
        },
        Place::Field { .. } | Place::VariantField { .. } | Place::Index { .. } | Place::Deref { .. } => {
            let (addr, ty) = place_address(builder, place, cx);
            store_field(builder, value, &ty, addr, 0, cx.layouts);
        }
    }
}


fn slice_elem_of(ty: &Type) -> Type {
    match ty {
        Type::Slice(elem) => elem.as_ref().clone(),
        Type::Borrow { ty, .. } => slice_elem_of(ty),
        other => panic!("expected a slice type, found {other:?}"),
    }
}

fn field_of(base_ty: &Type, field: &str, cx: &Ctx) -> (Type, u64) {
    match base_ty {
        Type::Borrow { ty, .. } => field_of(ty, field, cx),
        Type::Tuple(items) => {
            cx.layouts.tuple_field(items, field.parse().unwrap_or_else(|_| panic!("tuple field `{field}` is not an index")))
        }
        _ => {
            let (struct_name, args) = struct_name_of(base_ty);
            cx.layouts.struct_field(&struct_name, &args, field)
        }
    }
}

fn struct_name_of(ty: &Type) -> (String, Vec<Type>) {
    match ty {
        Type::Struct(name, args) => (name.clone(), args.clone()),
        Type::Borrow { ty, .. } => struct_name_of(ty),
        other => panic!("expected a struct type, found {other:?}"),
    }
}

fn enum_name_of(ty: &Type) -> (String, Vec<Type>) {
    match ty {
        Type::Enum(name, args) => (name.clone(), args.clone()),
        Type::Borrow { ty, .. } => enum_name_of(ty),
        other => panic!("expected an enum type, found {other:?}"),
    }
}

fn compile_operand(builder: &mut FunctionBuilder, operand: &Operand, cx: &Ctx) -> Value {
    match operand {
        Operand::Constant(constant) => match constant {
            paco_mir::Constant::Int(value, width) => {
                let clif_ty = clif_type(&Type::Int(*width)).expect("integer types are never unit");
                builder.ins().iconst(clif_ty, *value)
            }
            paco_mir::Constant::Float(bits, width) => match width {
                FloatWidth::F64 => builder.ins().f64const(f64::from_bits(*bits)),
                FloatWidth::F32 => builder.ins().f32const(f64::from_bits(*bits) as f32),
                small => builder.ins().iconst(float_storage_type(*small), small.encode(f64::from_bits(*bits)) as i64),
            },
            paco_mir::Constant::Bool(value) => builder.ins().iconst(types::I8, i64::from(*value)),
            paco_mir::Constant::Char(value) => builder.ins().iconst(types::I32, i64::from(u32::from(*value))),
            paco_mir::Constant::Str(text) => {
                let data = builder.ins().symbol_value(types::I64, cx.strings[text]);
                let len = builder.ins().iconst(types::I64, text.len() as i64);
                let descriptor = allocate_stack_slot(builder, 16, 8);
                builder.ins().store(MemFlagsData::trusted(), data, descriptor, SLICE_DATA_OFFSET);
                builder.ins().store(MemFlagsData::trusted(), len, descriptor, SLICE_LEN_OFFSET);
                descriptor
            }
            paco_mir::Constant::Unit => panic!("a unit value has no runtime representation"),
            paco_mir::Constant::Type(_) => panic!("a `type` value has no runtime representation"),
        },
        Operand::Copy(place) | Operand::Move(place) => read_place(builder, place, cx),
    }
}

fn compile_unary(builder: &mut FunctionBuilder, op: UnOp, value: Value, operand_ty: &Type, cx: &Ctx, profile: Profile) -> Value {
    if let (UnOp::Neg, Type::Int(width), Profile::Debug) = (op, operand_ty, profile)
        && width.is_signed()
    {
        let ty = builder.func.dfg.value_type(value);
        let min = builder.ins().iconst(ty, width.range().0 as i64);
        let overflow = builder.ins().icmp(IntCC::Equal, value, min);
        cx.panics.check(builder, overflow, "attempt to negate with overflow");
    }
    match (op, operand_ty) {
        (UnOp::Not, _) => builder.ins().icmp_imm_s(IntCC::Equal, value, 0),
        (UnOp::Neg, Type::Float(FloatWidth::F64 | FloatWidth::F32)) => builder.ins().fneg(value),
        (UnOp::Neg, Type::Float(width)) => {
            let sign = 1i64 << (width.bytes() * 8 - 1);
            builder.ins().bxor_imm_u(value, sign)
        }
        (UnOp::Neg, _) => builder.ins().ineg(value),
        (UnOp::BitNot, _) => builder.ins().bnot(value),
    }
}

enum CheckedOp {
    Add,
    Sub,
    Mul,
}

fn compile_binary(
    builder: &mut FunctionBuilder,
    op: BinOp,
    left: Value,
    right: Value,
    operand_ty: &Type,
    profile: Profile,
    cx: &Ctx,
) -> Value {
    match operand_ty {
        Type::Float(_) => match op {
            BinOp::Add => builder.ins().fadd(left, right),
            BinOp::Sub => builder.ins().fsub(left, right),
            BinOp::Mul => builder.ins().fmul(left, right),
            BinOp::Div => builder.ins().fdiv(left, right),
            BinOp::Rem => panic!("float remainder codegen is not implemented yet"),
            BinOp::Eq => builder.ins().fcmp(FloatCC::Equal, left, right),
            BinOp::Ne => builder.ins().fcmp(FloatCC::NotEqual, left, right),
            BinOp::Lt => builder.ins().fcmp(FloatCC::LessThan, left, right),
            BinOp::Le => builder.ins().fcmp(FloatCC::LessThanOrEqual, left, right),
            BinOp::Gt => builder.ins().fcmp(FloatCC::GreaterThan, left, right),
            BinOp::Ge => builder.ins().fcmp(FloatCC::GreaterThanOrEqual, left, right),
            _ => panic!("operator `{op:?}` is not valid over float operands"),
        },
        Type::Bool => match op {
            BinOp::And => builder.ins().band(left, right),
            BinOp::Or => builder.ins().bor(left, right),
            BinOp::Eq => builder.ins().icmp(IntCC::Equal, left, right),
            BinOp::Ne => builder.ins().icmp(IntCC::NotEqual, left, right),
            _ => panic!("operator `{op:?}` is not valid over bool operands"),
        },
        // A `char` is an unsigned 4-byte Unicode scalar value: ordering
        // compares codepoints numerically, unsigned (never negative).
        Type::Char => match op {
            BinOp::Eq => builder.ins().icmp(IntCC::Equal, left, right),
            BinOp::Ne => builder.ins().icmp(IntCC::NotEqual, left, right),
            BinOp::Lt => builder.ins().icmp(IntCC::UnsignedLessThan, left, right),
            BinOp::Le => builder.ins().icmp(IntCC::UnsignedLessThanOrEqual, left, right),
            BinOp::Gt => builder.ins().icmp(IntCC::UnsignedGreaterThan, left, right),
            BinOp::Ge => builder.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, left, right),
            _ => panic!("operator `{op:?}` is not valid over char operands"),
        },
        Type::Int(width) => {
            let signed = width.is_signed();
            match op {
                BinOp::Add => checked_int_op(builder, profile, CheckedOp::Add, left, right, signed, cx),
                BinOp::Sub => checked_int_op(builder, profile, CheckedOp::Sub, left, right, signed, cx),
                BinOp::Mul => checked_int_op(builder, profile, CheckedOp::Mul, left, right, signed, cx),
                BinOp::Div => {
                    check_divisor(builder, right, "division by zero", cx);
                    if signed {
                        let ty = builder.func.dfg.value_type(left);
                        let minus_one = builder.ins().icmp_imm_s(IntCC::Equal, right, -1);
                        let min = builder.ins().iconst(ty, i64::MIN >> (64 - ty.bits()));
                        let is_min = builder.ins().icmp(IntCC::Equal, left, min);
                        let overflow = builder.ins().band(minus_one, is_min);
                        cx.panics.check(builder, overflow, "attempt to divide with overflow");
                        builder.ins().sdiv(left, right)
                    } else {
                        builder.ins().udiv(left, right)
                    }
                }
                BinOp::Rem => {
                    check_divisor(builder, right, "remainder by zero", cx);
                    if signed {
                        builder.ins().srem(left, right)
                    } else {
                        builder.ins().urem(left, right)
                    }
                }
                BinOp::Eq => builder.ins().icmp(IntCC::Equal, left, right),
                BinOp::Ne => builder.ins().icmp(IntCC::NotEqual, left, right),
                BinOp::Lt => builder.ins().icmp(signed_cc(IntCC::SignedLessThan, signed), left, right),
                BinOp::Le => {
                    builder.ins().icmp(signed_cc(IntCC::SignedLessThanOrEqual, signed), left, right)
                }
                BinOp::Gt => builder.ins().icmp(signed_cc(IntCC::SignedGreaterThan, signed), left, right),
                BinOp::Ge => {
                    builder.ins().icmp(signed_cc(IntCC::SignedGreaterThanOrEqual, signed), left, right)
                }
                BinOp::BitAnd => builder.ins().band(left, right),
                BinOp::BitOr => builder.ins().bor(left, right),
                BinOp::BitXor => builder.ins().bxor(left, right),
                BinOp::Shl | BinOp::Shr => {
                    let bits = i64::from(builder.func.dfg.value_type(left).bits());
                    if profile == Profile::Debug {
                        let overflow = builder.ins().icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, right, bits);
                        let message =
                            if op == BinOp::Shl { "attempt to shift left with overflow" } else { "attempt to shift right with overflow" };
                        cx.panics.check(builder, overflow, message);
                    }
                    match (op, signed) {
                        (BinOp::Shl, _) => builder.ins().ishl(left, right),
                        (_, true) => builder.ins().sshr(left, right),
                        (_, false) => builder.ins().ushr(left, right),
                    }
                }
                BinOp::WrappingAdd => builder.ins().iadd(left, right),
                BinOp::WrappingSub => builder.ins().isub(left, right),
                BinOp::WrappingMul => builder.ins().imul(left, right),
                BinOp::AddOverflows | BinOp::SubOverflows | BinOp::MulOverflows => {
                    let (_, overflow) = match (op, signed) {
                        (BinOp::AddOverflows, true) => builder.ins().sadd_overflow(left, right),
                        (BinOp::AddOverflows, false) => builder.ins().uadd_overflow(left, right),
                        (BinOp::SubOverflows, true) => builder.ins().ssub_overflow(left, right),
                        (BinOp::SubOverflows, false) => builder.ins().usub_overflow(left, right),
                        (_, true) => builder.ins().smul_overflow(left, right),
                        (_, false) => builder.ins().umul_overflow(left, right),
                    };
                    overflow
                }
                BinOp::And | BinOp::Or => panic!("logical operator over int operands is not valid"),
            }
        }
        other => panic!("codegen for binary operands of type {other:?} is not implemented yet"),
    }
}

/// Maps a signed `IntCC` comparison to its unsigned counterpart when the
/// operand width is unsigned; `IntCC::unsigned` performs exactly that swap.
fn signed_cc(cc: IntCC, signed: bool) -> IntCC {
    if signed { cc } else { cc.unsigned() }
}

/// Whether an `as`-castable numeric `Type` reads as signed for extension
/// and float-conversion purposes; `char` is an unsigned codepoint.
fn int_signedness(ty: &Type) -> bool {
    matches!(ty, Type::Int(width) if width.is_signed())
}

/// Implements an `as` cast's runtime conversion between numeric primitives.
/// Int-to-int narrows by truncation (`ireduce`) or widens by sign/zero
/// extension per the *source*'s signedness (spec.md: "a narrowing cast
/// truncates the value's bit pattern"). Float-to-int uses the saturating
/// conversions (clamped to the target's range, `NaN` becomes `0`) since
/// spec.md does not define trapping behavior for an out-of-range float
/// cast, and a trap here would be a surprising, undocumented panic.
fn compile_cast<M: Module>(
    module: &mut M,
    builder: &mut FunctionBuilder,
    value: Value,
    source_ty: &Type,
    target_ty: &Type,
) -> Value {
    let source_clif = clif_type(source_ty).expect("cast source is never unit-typed");
    let target_clif = clif_type(target_ty).expect("cast target is never unit-typed");

    if let (Type::Float(source), Type::Float(target)) = (source_ty, target_ty) {
        if source == target {
            return value;
        }
        let wide = float_to_f64(module, builder, value, *source);
        return f64_to_float(module, builder, wide, *target);
    }
    if let Type::Float(source) = source_ty {
        let wide = float_to_f64(module, builder, value, *source);
        return if int_signedness(target_ty) {
            builder.ins().fcvt_to_sint_sat(target_clif, wide)
        } else {
            builder.ins().fcvt_to_uint_sat(target_clif, wide)
        };
    }
    if let Type::Float(target) = target_ty {
        let wide = if int_signedness(source_ty) {
            builder.ins().fcvt_from_sint(types::F64, value)
        } else {
            builder.ins().fcvt_from_uint(types::F64, value)
        };
        return f64_to_float(module, builder, wide, *target);
    }
    if source_clif == target_clif {
        return value;
    }
    if target_clif.bits() < source_clif.bits() {
        builder.ins().ireduce(target_clif, value)
    } else if int_signedness(source_ty) {
        builder.ins().sextend(target_clif, value)
    } else {
        builder.ins().uextend(target_clif, value)
    }
}

fn check_divisor(builder: &mut FunctionBuilder, divisor: Value, message: &'static str, cx: &Ctx) {
    let is_zero = builder.ins().icmp_imm_s(IntCC::Equal, divisor, 0);
    cx.panics.check(builder, is_zero, message);
}

fn checked_int_op(
    builder: &mut FunctionBuilder,
    profile: Profile,
    op: CheckedOp,
    left: Value,
    right: Value,
    signed: bool,
    cx: &Ctx,
) -> Value {
    match profile {
        Profile::Release => match op {
            CheckedOp::Add => builder.ins().iadd(left, right),
            CheckedOp::Sub => builder.ins().isub(left, right),
            CheckedOp::Mul => builder.ins().imul(left, right),
        },
        Profile::Debug => {
            let message = match op {
                CheckedOp::Add => "attempt to add with overflow",
                CheckedOp::Sub => "attempt to subtract with overflow",
                CheckedOp::Mul => "attempt to multiply with overflow",
            };
            let (result, overflow) = match (op, signed) {
                (CheckedOp::Add, true) => builder.ins().sadd_overflow(left, right),
                (CheckedOp::Add, false) => builder.ins().uadd_overflow(left, right),
                (CheckedOp::Sub, true) => builder.ins().ssub_overflow(left, right),
                (CheckedOp::Sub, false) => builder.ins().usub_overflow(left, right),
                (CheckedOp::Mul, true) => builder.ins().smul_overflow(left, right),
                (CheckedOp::Mul, false) => builder.ins().umul_overflow(left, right),
            };
            cx.panics.check(builder, overflow, message);
            result
        }
    }
}

fn compile_terminator<M: Module>(
    module: &mut M,
    builder: &mut FunctionBuilder,
    terminator: &Terminator,
    clif_blocks: &[cranelift_codegen::ir::Block],
    cx: &Ctx,
    func_ids: &HashMap<String, FuncId>,
    return_ty: &Type,
) {
    match terminator {
        Terminator::Goto(target) => {
            builder.ins().jump(clif_blocks[block_index(*target)], &[]);
        }
        Terminator::SwitchInt {
            discriminant,
            targets,
            otherwise,
        } => {
            let value = compile_operand(builder, discriminant, cx);
            let mut switch = Switch::new();
            for (case, target) in targets {
                switch.set_entry(*case as u128, clif_blocks[block_index(*target)]);
            }
            switch.emit(builder, value, clif_blocks[block_index(*otherwise)]);
        }
        Terminator::Call {
            target,
            args,
            destination,
            resume,
        } => {
            if target.0 == paco_mir::PANIC_SYMBOL {
                let message = compile_operand(builder, &args[0], cx);
                cx.panics.call(builder, cx.panics.panic, &[message]);
                return;
            }
            if target.0 == "print" {
                compile_print_call(module, builder, args, cx);
            } else if compile_builtin_call(module, builder, &target.0, args, destination.as_ref(), cx) {
            } else if target.0 == "slice_of_zeros" {
                let place = destination.as_ref().expect("`slice_of_zeros` always returns a value");
                compile_slice_of_zeros_call(module, builder, args, place, cx);
            } else if target.0 == "slice_as_ptr" || target.0 == "slice_as_mut_ptr" {
                let place = destination.as_ref().expect("`slice_as_ptr`/`slice_as_mut_ptr` always return a value");
                compile_slice_as_ptr_call(builder, args, place, cx);
            } else {
                let func_id = *func_ids
                    .get(&target.0)
                    .unwrap_or_else(|| panic!("undeclared function `{}`", target.0));
                let func_ref = module.declare_func_in_func(func_id, builder.func);
                let mut arg_values: Vec<Value> = Vec::with_capacity(args.len() + 1);
                if let Some(place) = destination {
                    let result_ty = place_ty(place, cx);
                    if is_real_aggregate(&result_ty, cx.layouts) {
                        let (size, align) = aggregate_layout_size_align(&result_ty, cx);
                        arg_values.push(allocate_stack_slot(builder, size, align));
                    }
                }
                let callee_params = cx.params.get(&target.0);
                for (index, arg) in args.iter().enumerate() {
                    let param_ty = callee_params.and_then(|params| params.get(index));
                    let auto_ref = matches!(param_ty, Some(Type::Borrow { .. }))
                        && !matches!(operand_ty(arg, cx), Type::Borrow { .. });
                    let value = if let Some(param_ty) = param_ty.filter(|_| !auto_ref) {
                        compile_owned_as(builder, arg, param_ty, cx)
                    } else {
                        compile_operand(builder, arg, cx)
                    };
                    arg_values.push(value);
                }
                let call = builder.ins().call(func_ref, &arg_values);
                if let Some(place) = destination {
                    let result = builder.inst_results(call)[0];
                    write_place(builder, place, result, cx);
                }
            }
            builder.ins().jump(clif_blocks[block_index(*resume)], &[]);
        }
        Terminator::CallIndirect {
            callee,
            args,
            destination,
            resume,
        } => {
            let callee = compile_operand(builder, callee, cx);
            let mut sig = module.make_signature();
            let mut arg_values: Vec<Value> = Vec::with_capacity(args.len() + 1);
            let result_ty = destination.as_ref().map(|place| place_ty(place, cx));
            if let Some(result_ty) = &result_ty
                && is_real_aggregate(result_ty, cx.layouts)
            {
                let (size, align) = aggregate_layout_size_align(result_ty, cx);
                arg_values.push(allocate_stack_slot(builder, size, align));
            }
            for (index, arg) in args.iter().enumerate() {
                let value = if index == 0 {
                    compile_operand(builder, arg, cx)
                } else {
                    compile_owned_operand(builder, arg, cx)
                };
                arg_values.push(value);
            }
            for value in &arg_values {
                sig.params.push(AbiParam::new(builder.func.dfg.value_type(*value)));
            }
            if let Some(ty) = result_ty.as_ref().and_then(clif_type) {
                sig.returns.push(AbiParam::new(ty));
            }
            let sig_ref = builder.import_signature(sig);
            let call = builder.ins().call_indirect(sig_ref, callee, &arg_values);
            if let Some(place) = destination {
                let result = builder.inst_results(call)[0];
                write_place(builder, place, result, cx);
            }
            builder.ins().jump(clif_blocks[block_index(*resume)], &[]);
        }
        Terminator::Return(operand) => match (clif_type(return_ty), operand) {
            // Unreachable dead block after an early exit; MIR still stamps it with
            // a Unit-constant Return even when the real return type isn't Unit.
            (Some(_), Operand::Constant(paco_mir::Constant::Unit)) => {
                builder.ins().trap(TrapCode::user(1).unwrap());
            }
            (Some(_), operand) if is_real_aggregate(return_ty, cx.layouts) => {
                let value = compile_owned_as(builder, operand, return_ty, cx);
                let sret_addr = cx.sret_addr.expect("aggregate-returning function must have an sret address");
                let (size, align) = aggregate_layout_size_align(return_ty, cx);
                builder.emit_small_memory_copy(
                    module.target_config(),
                    sret_addr,
                    value,
                    size,
                    align as u8,
                    align as u8,
                    true,
                    MemFlagsData::trusted(),
                );
                drop_all_owned(builder, cx);
                builder.ins().return_(&[sret_addr]);
            }
            (Some(_), operand) => {
                let value = compile_owned_as(builder, operand, return_ty, cx);
                drop_all_owned(builder, cx);
                builder.ins().return_(&[value]);
            }
            (None, _) => {
                drop_all_owned(builder, cx);
                builder.ins().return_(&[]);
            }
        },
        Terminator::Unreachable => {
            builder.ins().trap(TrapCode::user(1).unwrap());
        }
    }
}

fn drop_all_owned(builder: &mut FunctionBuilder, cx: &Ctx) {
    for index in (0..cx.body.locals.len()).rev() {
        drop_local_if_owned(builder, paco_mir::Local(index as u32), cx);
    }
}

fn strip_borrow(ty: &Type) -> &Type {
    match ty {
        Type::Borrow { ty, .. } => strip_borrow(ty),
        other => other,
    }
}

fn call_void<M: Module>(module: &mut M, builder: &mut FunctionBuilder, name: &str, args: &[Value]) {
    let mut sig = module.make_signature();
    for value in args {
        sig.params.push(AbiParam::new(builder.func.dfg.value_type(*value)));
    }
    let func_id = module
        .declare_function(name, Linkage::Import, &sig)
        .unwrap_or_else(|error| panic!("failed to declare `{name}`: {error}"));
    let func_ref = module.declare_func_in_func(func_id, builder.func);
    builder.ins().call(func_ref, args);
}

fn call_returning<M: Module>(
    module: &mut M,
    builder: &mut FunctionBuilder,
    name: &str,
    ret: types::Type,
    args: &[Value],
) -> Value {
    let params: Vec<types::Type> = args.iter().map(|value| builder.func.dfg.value_type(*value)).collect();
    call_runtime(module, builder, name, &params, ret, args)
}

fn compile_print_call<M: Module>(module: &mut M, builder: &mut FunctionBuilder, args: &[Operand], cx: &Ctx) {
    let ty = operand_ty(&args[0], cx);
    let mut value = compile_operand(builder, &args[0], cx);
    let inner = strip_borrow(&ty).clone();
    if matches!(ty, Type::Borrow { .. }) && !is_real_aggregate(&inner, cx.layouts) {
        let clif_ty = clif_type(&inner).expect("printed values are never unit-typed");
        value = builder.ins().load(clif_ty, MemFlagsData::trusted(), value, 0);
    }
    let mut code = None;
    let name = match inner {
        Type::Float(width) => {
            value = float_to_f64(module, builder, value, width);
            code = Some(builder.ins().iconst(types::I32, float_format_code(width)));
            "paco_print_float"
        }
        Type::Bool => "paco_print_bool",
        Type::Char => "paco_print_char",
        Type::String => "paco_print_str",
        Type::Int(IntWidth::U64) => "paco_print_uint",
        Type::Int(width) => {
            if width.bytes() < 8 {
                value = if width.is_signed() {
                    builder.ins().sextend(types::I64, value)
                } else {
                    builder.ins().uextend(types::I64, value)
                };
            }
            "paco_print_int"
        }
        other => panic!("`print` codegen is not implemented for {other:?}"),
    };
    let args: Vec<Value> = std::iter::once(value).chain(code).collect();
    call_void(module, builder, name, &args);
}

fn compile_string_binary<M: Module>(
    module: &mut M,
    builder: &mut FunctionBuilder,
    op: BinOp,
    left: Value,
    right: Value,
) -> Value {
    match op {
        BinOp::Eq => call_returning(module, builder, "paco_string_eq", types::I8, &[left, right]),
        BinOp::Ne => {
            let equal = call_returning(module, builder, "paco_string_eq", types::I8, &[left, right]);
            builder.ins().icmp_imm_s(IntCC::Equal, equal, 0)
        }
        BinOp::Add => {
            let out = allocate_stack_slot(builder, 16, 8);
            call_void(module, builder, "paco_string_concat", &[left, right, out]);
            out
        }
        other => panic!("operator `{other:?}` is not supported over strings"),
    }
}

fn compile_builtin_call<M: Module>(
    module: &mut M,
    builder: &mut FunctionBuilder,
    name: &str,
    args: &[Operand],
    destination: Option<&Place>,
    cx: &Ctx,
) -> bool {
    if let Some(op) = name.strip_prefix("$cell::") {
        compile_cell_call(module, builder, op, args, destination, cx);
        return true;
    }
    if let Some(entry) = network_entry(name) {
        let first = if name == "tcp_listen" { compile_operand(builder, &args[0], cx) } else { cell_pointer(builder, &args[0], cx) };
        match name {
            "tcp_listen" | "TcpListener::accept" => {
                let handle = call_returning(module, builder, entry, types::I64, &[first]);
                write_place(builder, destination.expect("returns a handle"), handle, cx);
            }
            "TcpStream::read" => {
                let max_len = compile_operand(builder, &args[1], cx);
                let out = allocate_stack_slot(builder, 16, 8);
                call_void(module, builder, entry, &[first, max_len, out]);
                write_place(builder, destination.expect("returns a string"), out, cx);
            }
            _ => {
                let text = compile_operand(builder, &args[1], cx);
                call_void(module, builder, entry, &[first, text]);
            }
        }
        return true;
    }
    let runtime_name = match name {
        "string_len_bytes" | "string_next_char_boundary" | "string_char_at" | "string_byte_at"
        | "string_slice_utf8" | "fs_read_to_string" | "stderr_write" | "string_concat" | "int_to_string" | "uint_to_string"
        | "bool_to_string" | "float_to_string" | "char_to_string" | "arg_count" | "arg_at" | "string_to_bytes"
        | "string_from_bytes" | "bytes_write_string" | "string_hash" | "slice_sort" => format!("paco_{name}"),
        _ => return false,
    };
    let mut values: Vec<Value> = Vec::with_capacity(args.len() + 1);
    for arg in args {
        let value = compile_operand(builder, arg, cx);
        if let Type::Float(width) = operand_ty(arg, cx) {
            values.push(float_to_f64(module, builder, value, width));
            values.push(builder.ins().iconst(types::I32, float_format_code(width)));
        } else {
            values.push(value);
        }
    }
    match name {
        "string_len_bytes" => {
            let len = builder.ins().load(types::I64, MemFlagsData::trusted(), values[0], SLICE_LEN_OFFSET);
            write_place(builder, destination.expect("returns a value"), len, cx);
        }
        "string_next_char_boundary" | "arg_count" | "string_hash" => {
            let next = call_returning(module, builder, &runtime_name, types::I64, &values);
            write_place(builder, destination.expect("returns a value"), next, cx);
        }
        "bytes_write_string" => {
            let written = call_returning(module, builder, &runtime_name, types::I8, &values);
            write_place(builder, destination.expect("returns a value"), written, cx);
        }
        "string_char_at" | "string_byte_at" | "string_slice_utf8" | "fs_read_to_string" | "string_from_bytes" => {
            compile_option_builtin(module, builder, &runtime_name, values, destination.expect("returns a value"), cx);
        }
        "stderr_write" | "slice_sort" => call_void(module, builder, &runtime_name, &values),
        _ => {
            let out = allocate_stack_slot(builder, 16, 8);
            values.push(out);
            call_void(module, builder, &runtime_name, &values);
            write_place(builder, destination.expect("returns a value"), out, cx);
        }
    }
    true
}

fn network_entry(name: &str) -> Option<&'static str> {
    Some(match name {
        "tcp_listen" => "paco_rt_tcp_listen",
        "TcpListener::accept" => "paco_rt_tcp_accept",
        "TcpStream::read" => "paco_rt_tcp_read",
        "TcpStream::write" => "paco_rt_tcp_write",
        _ => return None,
    })
}

fn compile_option_builtin<M: Module>(
    module: &mut M,
    builder: &mut FunctionBuilder,
    runtime_name: &str,
    mut values: Vec<Value>,
    destination: &Place,
    cx: &Ctx,
) {
    let (enum_name, type_args) = enum_name_of(&place_ty(destination, cx));
    let layout = cx.layouts.enum_layout(&enum_name, &type_args);
    let addr = allocate_stack_slot(builder, layout.size, layout.align);
    let (_, payload_offset) = cx.layouts.enum_variant_field(&enum_name, &type_args, "Some", 0);
    values.push(builder.ins().iadd_imm_s(addr, payload_offset as i64));
    let found = call_returning(module, builder, runtime_name, types::I32, &values);
    let some = builder.ins().iconst(types::I64, cx.layouts.enum_variant_index(&enum_name, &type_args, "Some") as i64);
    let none = builder.ins().iconst(types::I64, cx.layouts.enum_variant_index(&enum_name, &type_args, "None") as i64);
    let tag = builder.ins().select(found, some, none);
    builder.ins().store(MemFlagsData::trusted(), tag, addr, 0);
    write_place(builder, destination, addr, cx);
}


fn value_size(ty: &Type, cx: &Ctx) -> u64 {
    match ty {
        Type::Unit => 0,
        Type::Struct(name, _) if !cx.layouts.has_struct(name) => 8,
        _ if is_real_aggregate(ty, cx.layouts) => aggregate_layout_size_align(ty, cx).0,
        _ => paco_mir::scalar_layout(ty).unwrap_or_else(|| panic!("no layout for {ty:?}")).size,
    }
}

fn cell_pointer(builder: &mut FunctionBuilder, operand: &Operand, cx: &Ctx) -> Value {
    let value = compile_operand(builder, operand, cx);
    if matches!(operand_ty(operand, cx), Type::Borrow { .. }) {
        builder.ins().load(types::I64, MemFlagsData::trusted(), value, 0)
    } else {
        value
    }
}

fn lock_cell(builder: &mut FunctionBuilder, cell: Value) {
    let lock = builder.ins().iadd_imm_s(cell, i64::from(CELL_LOCK_OFFSET));
    let spin = builder.create_block();
    let acquired = builder.create_block();
    builder.ins().jump(spin, &[]);
    builder.switch_to_block(spin);
    let unlocked = builder.ins().iconst(types::I64, 0);
    let locked = builder.ins().iconst(types::I64, 1);
    let previous = builder.ins().atomic_cas(MemFlagsData::trusted(), lock, unlocked, locked);
    builder.ins().brif(previous, spin, &[], acquired, &[]);
    builder.switch_to_block(acquired);
}

fn unlock_cell(builder: &mut FunctionBuilder, cell: Value) {
    let lock = builder.ins().iadd_imm_s(cell, i64::from(CELL_LOCK_OFFSET));
    let unlocked = builder.ins().iconst(types::I64, 0);
    builder.ins().atomic_store(MemFlagsData::trusted(), unlocked, lock);
}

fn compile_cell_call<M: Module>(
    module: &mut M,
    builder: &mut FunctionBuilder,
    op: &str,
    args: &[Operand],
    destination: Option<&Place>,
    cx: &Ctx,
) {
    if op == "new" {
        let ty = operand_ty(&args[0], cx);
        let count = builder.ins().iconst(types::I64, 1);
        let size = builder.ins().iconst(types::I64, i64::from(CELL_VALUE_OFFSET) + value_size(&ty, cx) as i64);
        let cell = call_runtime(module, builder, "paco_calloc", &[types::I64, types::I64], types::I64, &[count, size]);
        builder.ins().store(MemFlagsData::trusted(), count, cell, CELL_COUNT_OFFSET);
        if ty != Type::Unit {
            let value = compile_owned_operand(builder, &args[0], cx);
            store_field(builder, value, &ty, cell, CELL_VALUE_OFFSET, cx.layouts);
        }
        write_place(builder, destination.expect("`new` returns a value"), cell, cx);
        return;
    }
    let cell = cell_pointer(builder, &args[0], cx);
    match op {
        "get" => {
            lock_cell(builder, cell);
            if let Some(place) = destination {
                let ty = place_ty(place, cx);
                let value = if is_real_aggregate(&ty, cx.layouts) {
                    let (size, align) = aggregate_layout_size_align(&ty, cx);
                    let copy = allocate_stack_slot(builder, size, align);
                    let source = builder.ins().iadd_imm_s(cell, i64::from(CELL_VALUE_OFFSET));
                    copy_bytes(builder, source, copy, 0, size);
                    copy
                } else {
                    let clif_ty = clif_type(&ty).expect("non-unit destinations have a clif type");
                    builder.ins().load(clif_ty, MemFlagsData::trusted(), cell, CELL_VALUE_OFFSET)
                };
                let value = clone_value(builder, &ty, value, cx);
                unlock_cell(builder, cell);
                write_place(builder, place, value, cx);
            } else {
                unlock_cell(builder, cell);
            }
        }
        "set" => {
            let ty = operand_ty(&args[1], cx);
            if ty != Type::Unit {
                let value = compile_owned_operand(builder, &args[1], cx);
                lock_cell(builder, cell);
                let old = if cx.needs_drop(&ty) {
                    let size = value_size(&ty, cx);
                    let old = allocate_stack_slot(builder, size.max(8), 8);
                    let source = builder.ins().iadd_imm_s(cell, i64::from(CELL_VALUE_OFFSET));
                    copy_words(builder, source, old, size);
                    Some(old)
                } else {
                    None
                };
                store_field(builder, value, &ty, cell, CELL_VALUE_OFFSET, cx.layouts);
                unlock_cell(builder, cell);
                if let Some(old) = old
                    && let Some(&(drop_ref, _)) = cx.glue_refs.get(&ty)
                {
                    builder.ins().call(drop_ref, &[old]);
                }
            }
        }
        "clone" => {
            let one = builder.ins().iconst(types::I64, 1);
            builder.ins().atomic_rmw(types::I64, MemFlagsData::trusted(), AtomicRmwOp::Add, cell, one);
            write_place(builder, destination.expect("`clone` returns a value"), cell, cx);
        }
        "strong_count" => {
            let count = builder.ins().atomic_load(types::I64, MemFlagsData::trusted(), cell);
            write_place(builder, destination.expect("`strong_count` returns a value"), count, cx);
        }
        other => panic!("unknown shared-cell operation `{other}`"),
    }
}

fn body_strings(body: &Body) -> Vec<&str> {
    let mut out = Vec::new();
    for block in &body.blocks {
        for statement in &block.statements {
            match statement {
                Statement::Assign(place, rvalue) => {
                    place_strings(place, &mut out);
                    rvalue_strings(rvalue, &mut out);
                }
                Statement::Store { address, value, .. } => {
                    operand_strings(address, &mut out);
                    operand_strings(value, &mut out);
                }
                Statement::FreeBox { address, .. } => operand_strings(address, &mut out),
                Statement::Drop(_) | Statement::StorageDead(_) => {}
            }
        }
        match &block.terminator {
            Terminator::SwitchInt { discriminant, .. } | Terminator::Return(discriminant) => {
                operand_strings(discriminant, &mut out)
            }
            Terminator::Call { args, destination, .. } => {
                args.iter().for_each(|arg| operand_strings(arg, &mut out));
                if let Some(place) = destination {
                    place_strings(place, &mut out);
                }
            }
            Terminator::CallIndirect { callee, args, destination, .. } => {
                operand_strings(callee, &mut out);
                args.iter().for_each(|arg| operand_strings(arg, &mut out));
                if let Some(place) = destination {
                    place_strings(place, &mut out);
                }
            }
            Terminator::Goto(_) | Terminator::Unreachable => {}
        }
    }
    out
}

fn operand_strings<'a>(operand: &'a Operand, out: &mut Vec<&'a str>) {
    match operand {
        Operand::Constant(paco_mir::Constant::Str(text)) => out.push(text),
        Operand::Copy(place) | Operand::Move(place) => place_strings(place, out),
        Operand::Constant(_) => {}
    }
}

fn place_strings<'a>(place: &'a Place, out: &mut Vec<&'a str>) {
    match place {
        Place::Local(_) => {}
        Place::Field { base, .. } | Place::VariantField { base, .. } => place_strings(base, out),
        Place::Index { base, index } => {
            place_strings(base, out);
            operand_strings(index, out);
        }
        Place::Deref { address, .. } => operand_strings(address, out),
    }
}

fn rvalue_strings<'a>(rvalue: &'a Rvalue, out: &mut Vec<&'a str>) {
    match rvalue {
        Rvalue::Use(operand)
        | Rvalue::UnaryOp(_, operand)
        | Rvalue::Cast { operand, .. }
        | Rvalue::Load { address: operand, .. } => operand_strings(operand, out),
        Rvalue::BinaryOp(_, left, right) => {
            operand_strings(left, out);
            operand_strings(right, out);
        }
        Rvalue::Aggregate { fields, .. } | Rvalue::Math(_, fields) => fields.iter().for_each(|field| operand_strings(field, out)),
        Rvalue::Ref { place, .. } | Rvalue::Discriminant(place) | Rvalue::SliceLen(place) => {
            place_strings(place, out)
        }
        Rvalue::RawAlloc { .. } | Rvalue::FuncAddr(_) | Rvalue::Quote { .. } => {}
    }
}

/// `slice_of_zeros<T>(len)` (design.md's explicitly-placeholder `[]T`-
/// construction builtin — not a considered general construction API, see
/// its own doc comment in `paco-types`). `len` is a runtime value, so the
/// backing data buffer cannot be a stack slot (`create_sized_stack_slot`
/// needs a compile-time-known size) — `paco_calloc` is the simplest available
/// allocator (libc is already linked into every `paco build` output via
/// `paco-link`'s existing C toolchain dependency; "zeros" in the name maps
/// directly to `paco_calloc`'s zero-initialization, no separate memset needed).
/// The 16-byte `[data_ptr, len]` descriptor itself, unlike the data, *is*
/// a fixed size, so it's an ordinary stack slot — the same treatment any
/// owned aggregate's own storage already gets. Freeing the data buffer on
/// drop is not implemented, consistent with `Statement::Drop` being a
/// no-op for every type in this backend today, not a new leak specific to
/// slices.
fn compile_slice_of_zeros_call<M: Module>(
    module: &mut M,
    builder: &mut FunctionBuilder,
    args: &[Operand],
    destination: &Place,
    cx: &Ctx,
) {
    let elem_ty = slice_elem_of(&place_ty(destination, cx));
    let elem_size = element_size(&elem_ty, cx);
    let len = compile_operand(builder, &args[0], cx);
    let elem_size_value = builder.ins().iconst(types::I64, elem_size as i64);

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let func_id = module
        .declare_function("paco_calloc", Linkage::Import, &sig)
        .unwrap_or_else(|error| panic!("failed to declare `paco_calloc`: {error}"));
    let func_ref = module.declare_func_in_func(func_id, builder.func);
    let call = builder.ins().call(func_ref, &[len, elem_size_value]);
    let data_ptr = builder.inst_results(call)[0];

    let descriptor_addr = allocate_stack_slot(builder, 16, 8);
    builder.ins().store(MemFlagsData::trusted(), data_ptr, descriptor_addr, SLICE_DATA_OFFSET);
    builder.ins().store(MemFlagsData::trusted(), len, descriptor_addr, SLICE_LEN_OFFSET);

    write_place(builder, destination, descriptor_addr, cx);
}

fn compile_slice_as_ptr_call(
    builder: &mut FunctionBuilder,
    args: &[Operand],
    destination: &Place,
    cx: &Ctx,
) {
    let base_addr = compile_operand(builder, &args[0], cx);
    let data_ptr = builder.ins().load(types::I64, MemFlagsData::trusted(), base_addr, SLICE_DATA_OFFSET);
    write_place(builder, destination, data_ptr, cx);
}

fn block_index(id: BasicBlockId) -> usize {
    id.0 as usize
}

#[cfg(test)]
mod tests {
    use paco_diag::Reporter;
    use paco_span::SourceMap;
    use paco_syntax::ast::Item;
    use paco_syntax::{lex::lex, parse::parse_module};
    use paco_types::infer_module;

    use super::*;

    fn lower_main(source: &str) -> (Body, paco_syntax::ast::Module) {
        let mut sources = SourceMap::new();
        let file = sources.add_file("main.paco", source);
        let mut reporter = Reporter::new();
        let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
        let module = parse_module(&tokens, &mut reporter).unwrap();
        assert!(!reporter.has_errors(), "{}", reporter.emit_to_string(&sources));

        let typed = infer_module(&module, &mut reporter).expect("module should type-check");
        let drops = paco_borrow::analyze_module(&module, &mut reporter).expect("module should borrow-check");
        let registry = paco_mir::TypeRegistry::from_module(&module);
        let main = module
            .items
            .iter()
            .find_map(|item| match item {
                Item::Fn(function) if function.name == "main" => Some(function),
                _ => None,
            })
            .unwrap();

        (paco_mir::lower_function(main, &typed, &registry, &drops, paco_mir::Profile::Debug).0, module)
    }

    fn host_module() -> cranelift_object::ObjectModule {
        let isa = host_isa().unwrap();
        let builder = cranelift_object::ObjectBuilder::new(isa, "test", cranelift_module::default_libcall_names()).unwrap();
        cranelift_object::ObjectModule::new(builder)
    }

    #[test]
    fn an_aggregate_returning_function_gets_a_leading_sret_param() {
        let (body, module_ast) = lower_main("struct Point { x: i64, y: i64 } fn main() -> Point { Point { x: 1, y: 2 } }");
        let layouts = TypeLayouts::from_module(&module_ast);
        let sig = make_signature(&host_module(), &body, &layouts);
        assert_eq!(sig.params.len(), 1, "expected exactly one leading sret param: {sig:?}");
        assert_eq!(sig.params[0].value_type, types::I64);
        assert_eq!(sig.returns.len(), 1);
    }

    #[test]
    fn a_scalar_returning_function_gets_no_sret_param() {
        let (body, module_ast) = lower_main("fn main() -> i64 { 1 }");
        let layouts = TypeLayouts::from_module(&module_ast);
        let sig = make_signature(&host_module(), &body, &layouts);
        assert_eq!(sig.params.len(), 0, "expected no leading sret param: {sig:?}");
    }
}

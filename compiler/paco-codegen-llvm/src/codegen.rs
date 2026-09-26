use std::collections::{HashMap, HashSet};

use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::intrinsics::Intrinsic;
use inkwell::module::{Linkage, Module};
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target as LlvmTarget, TargetMachine, TargetTriple,
};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FunctionType};
use inkwell::values::{BasicMetadataValueEnum, BasicValue, BasicValueEnum, FunctionValue, IntValue, PointerValue};
use inkwell::{AddressSpace, AtomicOrdering, AtomicRMWBinOp, FloatPredicate, IntPredicate, OptimizationLevel};

use paco_mir::glue::{
    CELL_COUNT_OFFSET, CELL_LOCK_OFFSET, CELL_VALUE_OFFSET, SLICE_DATA_OFFSET, SLICE_LEN_OFFSET, element_size,
};
use paco_mir::{
    BasicBlockId, BinOp, Body, Constant, Local, MathOp, ObjectFile, Operand, Place, Profile, Rvalue, Statement, Target,
    Terminator, TypeLayouts, UnOp,
};
use paco_types::{FloatWidth, IntWidth, Type};

/// The runtime entry points every program may call, with their parameter
/// count and whether they return a status (`i32`), a pointer (`i64`) or
/// nothing. Matches `paco-runtime-ffi`'s `extern "C"` functions.
const FFI_FUNCTIONS: &[(&str, usize, FfiReturn)] = &[
    ("paco_rt_channel", 3, FfiReturn::None),
    ("paco_rt_send", 3, FfiReturn::Status),
    ("paco_rt_recv", 3, FfiReturn::Status),
    ("paco_rt_sender_close", 1, FfiReturn::None),
    ("paco_rt_receiver_close", 1, FfiReturn::None),
    ("paco_rt_join", 4, FfiReturn::Status),
    ("paco_rt_spawn", 4, FfiReturn::Pointer),
    ("paco_rt_spawn_blocking", 4, FfiReturn::Pointer),
    ("paco_calloc", 2, FfiReturn::Pointer),
    ("paco_rt_receiver_is_ready", 1, FfiReturn::Status),
    ("paco_rt_generator_new", 3, FfiReturn::Pointer),
    ("paco_rt_generator_yield", 2, FfiReturn::Status),
    ("paco_rt_generator_next", 3, FfiReturn::Status),
];

#[derive(Clone, Copy)]
enum FfiReturn {
    None,
    Status,
    Pointer,
}

/// Buffers bodies; `finish` lowers them all into one LLVM module.
pub struct LlvmBackend<'a> {
    bodies: Vec<(String, Body)>,
    imports: Vec<(String, Body)>,
    externs: Vec<(String, Vec<Type>, Type)>,
    layouts: &'a TypeLayouts<'a>,
    sources: Option<&'a paco_span::SourceMap>,
}

impl<'a> LlvmBackend<'a> {
    pub fn new(externs: Vec<(String, Vec<Type>, Type)>, layouts: &'a TypeLayouts<'a>) -> Self {
        Self { bodies: Vec::new(), imports: Vec::new(), externs, layouts, sources: None }
    }

    /// Resolves panic locations against `sources`.
    pub fn with_sources(mut self, sources: &'a paco_span::SourceMap) -> Self {
        self.sources = Some(sources);
        self
    }

    /// The optimized LLVM IR text `finish` would compile.
    pub fn ir(&self, target: &Target) -> Result<String, String> {
        let context = Context::create();
        let (module, _) = self.build(&context, target)?;
        Ok(module.print_to_string().to_string())
    }

    fn build<'ctx>(&self, context: &'ctx Context, target: &Target) -> Result<(Module<'ctx>, TargetMachine), String> {
        let machine = target_machine(target)?;
        let module = context.create_module("paco");
        module.set_triple(&machine.get_triple());
        module.set_data_layout(&machine.get_target_data().get_data_layout());
        let locator = self.sources.map(paco_mir::SourceLocator::new);
        let mut generator = Generator::new(context, &module, self.layouts, locator.as_ref(), target.profile == Profile::Debug);
        generator.declare(&self.bodies, &self.imports, &self.externs);
        crate::glue::build(&mut generator, &self.bodies, &self.imports);
        for (name, body) in &self.bodies {
            generator.define(name, body);
        }
        generator.finish_debug_info();
        if let Some((_, entry)) = self.bodies.iter().find(|(name, _)| name == paco_mir::ENTRY_SYMBOL) {
            let flag = module.add_global(context.i8_type(), None, paco_mir::ENTRY_RETURNS_VALUE_SYMBOL);
            flag.set_initializer(&context.i8_type().const_int((entry.return_ty != Type::Unit) as u64, false));
            flag.set_constant(true);
        }
        module.verify().map_err(|error| format!("invalid LLVM module: {}", error.to_string()))?;
        if target.profile == Profile::Release {
            module
                .run_passes("default<O3>", &machine, PassBuilderOptions::create())
                .map_err(|error| format!("LLVM optimization failed: {}", error.to_string()))?;
        }
        Ok((module, machine))
    }
}

impl paco_mir::Backend for LlvmBackend<'_> {
    fn lower_body(&mut self, name: &str, body: &Body) -> Result<(), String> {
        self.bodies.push((name.to_string(), body.clone()));
        Ok(())
    }

    fn declare_body(&mut self, name: &str, body: &Body) -> Result<(), String> {
        self.imports.push((name.to_string(), body.clone()));
        Ok(())
    }

    fn finish(self, target: &Target) -> Result<ObjectFile, String> {
        let context = Context::create();
        let (module, machine) = self.build(&context, target)?;
        let buffer = machine
            .write_to_memory_buffer(&module, FileType::Object)
            .map_err(|error| format!("LLVM object emission failed: {}", error.to_string()))?;
        Ok(buffer.as_slice().to_vec())
    }
}

const MACOS_DEPLOYMENT_TARGET: &str = "11.0";

fn target_machine(target: &Target) -> Result<TargetMachine, String> {
    let config = InitializationConfig::default();
    LlvmTarget::initialize_x86(&config);
    LlvmTarget::initialize_aarch64(&config);
    let triple = match &target.triple {
        Some(triple) => match triple.strip_suffix("-apple-darwin") {
            Some(arch) => TargetTriple::create(&format!("{arch}-apple-macosx{MACOS_DEPLOYMENT_TARGET}")),
            None => TargetTriple::create(triple),
        },
        None => TargetMachine::get_default_triple(),
    };
    let name = triple.as_str().to_string_lossy().into_owned();
    let llvm_target = LlvmTarget::from_triple(&triple)
        .map_err(|error| format!("unsupported target triple `{name}`: {}", error.to_string()))?;
    let level = match target.profile {
        Profile::Release => OptimizationLevel::Aggressive,
        Profile::Debug => OptimizationLevel::None,
    };
    llvm_target
        .create_target_machine(&triple, "generic", "", level, RelocMode::PIC, CodeModel::Default)
        .ok_or_else(|| format!("unsupported target triple `{name}`"))
}

#[derive(Clone, Copy)]
enum Storage<'ctx> {
    Unit,
    /// A scalar whose value is itself used as an address by places rooted
    /// at it (a borrow, or a struct/enum handle).
    Register(PointerValue<'ctx>, BasicTypeEnum<'ctx>),
    /// A scalar that is borrowed: places rooted at it address its slot.
    Stack(PointerValue<'ctx>, BasicTypeEnum<'ctx>),
    Aggregate(PointerValue<'ctx>, u64),
}

pub(crate) struct Generator<'ctx, 'm, 'a> {
    pub(crate) context: &'ctx Context,
    pub(crate) module: &'m Module<'ctx>,
    pub(crate) builder: Builder<'ctx>,
    pub(crate) layouts: &'a TypeLayouts<'a>,
    pub(crate) glue: HashMap<Type, (FunctionValue<'ctx>, FunctionValue<'ctx>)>,
    params: HashMap<String, Vec<Type>>,
    strings: HashMap<String, PointerValue<'ctx>>,
    locator: Option<&'a paco_mir::SourceLocator<'a>>,
    frame_pointers: bool,
    c_strings: HashMap<String, PointerValue<'ctx>>,
    locations: Vec<(Vec<Location<'ctx>>, Location<'ctx>)>,
    here: std::cell::Cell<Location<'ctx>>,
    debug_info: Option<DebugInfo<'ctx>>,
}

struct DebugInfo<'ctx> {
    builder: inkwell::debug_info::DebugInfoBuilder<'ctx>,
    unit: inkwell::debug_info::DICompileUnit<'ctx>,
    files: HashMap<String, inkwell::debug_info::DIFile<'ctx>>,
    scope: Option<inkwell::debug_info::DIScope<'ctx>>,
}

type Location<'ctx> = (Option<PointerValue<'ctx>>, u32, u32);

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

struct Frame<'ctx, 'b> {
    body: &'b Body,
    function: FunctionValue<'ctx>,
    entry: BasicBlock<'ctx>,
    blocks: Vec<BasicBlock<'ctx>>,
    locals: Vec<Storage<'ctx>>,
    flags: Vec<Option<PointerValue<'ctx>>>,
    sret: Option<PointerValue<'ctx>>,
}

impl<'ctx, 'm, 'a> Generator<'ctx, 'm, 'a> {
    fn new(
        context: &'ctx Context,
        module: &'m Module<'ctx>,
        layouts: &'a TypeLayouts<'a>,
        locator: Option<&'a paco_mir::SourceLocator<'a>>,
        frame_pointers: bool,
    ) -> Self {
        let mut generator = Self {
            context,
            module,
            builder: context.create_builder(),
            layouts,
            glue: HashMap::new(),
            params: HashMap::new(),
            strings: HashMap::new(),
            locator,
            frame_pointers,
            c_strings: HashMap::new(),
            locations: Vec::new(),
            here: std::cell::Cell::new((None, 0, 0)),
            debug_info: None,
        };
        if frame_pointers && locator.is_some() {
            let (builder, unit) = module.create_debug_info_builder(
                true,
                inkwell::debug_info::DWARFSourceLanguage::C,
                "paco",
                ".",
                "paco",
                false,
                "",
                0,
                "",
                inkwell::debug_info::DWARFEmissionKind::LineTablesOnly,
                0,
                false,
                false,
                "",
                "",
            );
            generator.debug_info = Some(DebugInfo { builder, unit, files: HashMap::new(), scope: None });
        }
        for message in PANIC_MESSAGES {
            generator.c_string(message);
        }
        let (ptr, i32_, i64_) = (generator.ptr().into(), context.i32_type().into(), generator.i64().into());
        for (name, params) in [
            ("paco_rt_panic", vec![ptr, ptr, i32_, i32_, ptr, ptr]),
            ("paco_rt_panic_str", vec![ptr, ptr, i32_, i32_, ptr, ptr]),
            ("paco_rt_panic_bounds", vec![i64_, i64_, ptr, i32_, i32_, ptr, ptr]),
        ] {
            let function = module.add_function(name, generator.fn_type(&params, None), Some(Linkage::External));
            for attribute in ["noreturn", "cold"] {
                let kind = inkwell::attributes::Attribute::get_named_enum_kind_id(attribute);
                function.add_attribute(inkwell::attributes::AttributeLoc::Function, context.create_enum_attribute(kind, 0));
            }
        }
        generator
    }

    fn c_string(&mut self, text: &str) -> PointerValue<'ctx> {
        if let Some(pointer) = self.c_strings.get(text) {
            return *pointer;
        }
        let bytes = self.context.const_string(text.as_bytes(), true);
        let global = self.module.add_global(bytes.get_type(), None, "");
        global.set_initializer(&bytes);
        global.set_constant(true);
        global.set_linkage(Linkage::Private);
        let pointer = global.as_pointer_value();
        self.c_strings.insert(text.to_string(), pointer);
        pointer
    }

    fn locate(&mut self, span: paco_span::Span) -> Location<'ctx> {
        match self.locator.and_then(|locator| locator.locate(span)) {
            Some((file, line, column)) => (Some(self.c_string(file)), line, column),
            None => (None, 0, 0),
        }
    }

    fn finish_debug_info(&mut self) {
        for function in self.module.get_functions() {
            if function.count_basic_blocks() > 0 && self.frame_pointers {
                let attribute = self.context.create_string_attribute("frame-pointer", "all");
                function.add_attribute(inkwell::attributes::AttributeLoc::Function, attribute);
            }
        }
        let Some(debug) = &self.debug_info else { return };
        debug.builder.finalize();
        let i32_ = self.context.i32_type();
        self.module.add_basic_value_flag("Debug Info Version", inkwell::module::FlagBehavior::Warning, i32_.const_int(3, false));
        self.module.add_basic_value_flag("Dwarf Version", inkwell::module::FlagBehavior::Warning, i32_.const_int(4, false));
    }

    /// Attaches a subprogram for `body` to `function` and sets the location
    /// instructions get until the first statement.
    fn begin_debug_scope(&mut self, name: &str, function: FunctionValue<'ctx>, body: &Body) {
        let Some(debug) = &mut self.debug_info else { return };
        let Some((file, line, column)) = self.locator.and_then(|locator| locator.locate(body.span)) else {
            debug.scope = None;
            return;
        };
        let unit_file = debug.unit.get_file();
        let file = *debug.files.entry(file.to_string()).or_insert_with(|| debug.builder.create_file(file, ""));
        let signature = debug.builder.create_subroutine_type(unit_file, None, &[], 0);
        use inkwell::debug_info::AsDIScope;
        let subprogram = debug.builder.create_function(
            debug.unit.as_debug_info_scope(),
            if name == paco_mir::ENTRY_SYMBOL { "main" } else { name },
            Some(name),
            file,
            line,
            signature,
            false,
            true,
            line,
            0,
            false,
        );
        function.set_subprogram(subprogram);
        debug.scope = Some(subprogram.as_debug_info_scope());
        self.set_debug_location(line, column);
    }

    fn set_debug_location(&self, line: u32, column: u32) {
        if let Some(DebugInfo { builder, scope: Some(scope), .. }) = &self.debug_info
            && line != 0
        {
            let location = builder.create_debug_location(self.context, line, column, *scope, None);
            self.builder.set_current_debug_location(location);
        }
    }

    /// Calls the runtime panic entry `name` with `leading` and the current
    /// location; never returns.
    fn panic_call(&self, name: &str, leading: &[BasicValueEnum<'ctx>]) {
        let (file, line, column) = self.here.get();
        let null = self.ptr().const_null();
        let frame = if self.frame_pointers {
            let intrinsic = Intrinsic::find("llvm.frameaddress")
                .expect("llvm.frameaddress")
                .get_declaration(self.module, &[self.ptr().into()])
                .expect("frameaddress declaration");
            self.call(intrinsic, &[self.context.i32_type().const_zero().into()]).expect("frame address")
        } else {
            null.into()
        };
        let function = match self.builder.get_insert_block().and_then(|block| block.get_parent()) {
            Some(function) if self.frame_pointers => function.as_global_value().as_pointer_value(),
            _ => null,
        };
        let i32_ = self.context.i32_type();
        let mut args = leading.to_vec();
        args.extend([
            file.unwrap_or(null).into(),
            i32_.const_int(u64::from(line), false).into(),
            i32_.const_int(u64::from(column), false).into(),
            frame,
            function.into(),
        ]);
        self.call(self.module.get_function(name).expect("panic entry declared"), &args);
        self.builder.build_unreachable().expect("unreachable");
    }

    /// Panics with `message` when `condition` holds, then continues in a
    /// fresh block.
    fn panic_if(&self, function: FunctionValue<'ctx>, condition: IntValue<'ctx>, message: &str) {
        let text = self.c_strings[message];
        self.fail_if(function, condition, |generator| generator.panic_call("paco_rt_panic_str", &[text.into()]));
    }

    fn fail_if(&self, function: FunctionValue<'ctx>, condition: IntValue<'ctx>, fail: impl FnOnce(&Self)) {
        let failed = self.block(function);
        let passed = self.block(function);
        self.branch_if(condition, failed, passed);
        self.builder.position_at_end(failed);
        fail(self);
        self.builder.position_at_end(passed);
    }

    pub(crate) fn i8(&self) -> inkwell::types::IntType<'ctx> {
        self.context.i8_type()
    }

    pub(crate) fn i64(&self) -> inkwell::types::IntType<'ctx> {
        self.context.i64_type()
    }

    pub(crate) fn ptr(&self) -> inkwell::types::PointerType<'ctx> {
        self.context.ptr_type(AddressSpace::default())
    }

    pub(crate) fn int(&self, value: i64) -> IntValue<'ctx> {
        self.i64().const_int(value as u64, true)
    }

    fn llvm_ty(&self, ty: &Type) -> Option<BasicTypeEnum<'ctx>> {
        let context = self.context;
        Some(match ty {
            Type::Int(IntWidth::I8 | IntWidth::U8) | Type::Bool => context.i8_type().into(),
            Type::Int(IntWidth::I16 | IntWidth::U16) => context.i16_type().into(),
            Type::Int(IntWidth::I32 | IntWidth::U32) | Type::Char => context.i32_type().into(),
            Type::Int(IntWidth::I64 | IntWidth::U64) => context.i64_type().into(),
            Type::Float(width) => self.float_storage(*width),
            Type::Unit | Type::Never => return None,
            Type::Struct(..)
            | Type::Enum(..)
            | Type::Tuple(_)
            | Type::Slice(_)
            | Type::String
            | Type::Borrow { .. }
            | Type::RawPointer { .. }
            | Type::Fn(..) => self.ptr().into(),
            other => panic!("codegen for this type is not implemented yet: {other:?}"),
        })
    }

    fn float_storage(&self, width: FloatWidth) -> BasicTypeEnum<'ctx> {
        match width {
            FloatWidth::F64 => self.context.f64_type().into(),
            FloatWidth::F32 => self.context.f32_type().into(),
            FloatWidth::F16 | FloatWidth::BF16 => self.context.i16_type().into(),
            FloatWidth::F8E4M3 | FloatWidth::F8E5M2 => self.context.i8_type().into(),
        }
    }

    pub(crate) fn is_real_aggregate(&self, ty: &Type) -> bool {
        matches!(ty, Type::Struct(name, _) if self.layouts.has_struct(name))
            || matches!(ty, Type::Enum(..) | Type::Tuple(_) | Type::Slice(_) | Type::String)
    }

    fn size_align(&self, ty: &Type) -> (u64, u64) {
        match ty {
            Type::Struct(name, args) => {
                let layout = self.layouts.struct_layout(name, args);
                (layout.size, layout.align)
            }
            Type::Enum(name, args) => {
                let layout = self.layouts.enum_layout(name, args);
                (layout.size, layout.align)
            }
            Type::Tuple(items) => {
                let layout = self.layouts.tuple_layout(items);
                (layout.size, layout.align)
            }
            Type::Slice(_) | Type::String => (16, 8),
            other => panic!("not a real aggregate: {other:?}"),
        }
    }

    fn fn_type(&self, params: &[BasicTypeEnum<'ctx>], ret: Option<BasicTypeEnum<'ctx>>) -> FunctionType<'ctx> {
        let params: Vec<BasicMetadataTypeEnum<'ctx>> = params.iter().map(|ty| (*ty).into()).collect();
        match ret {
            Some(ret) => ret.fn_type(&params, false),
            None => self.context.void_type().fn_type(&params, false),
        }
    }

    fn body_fn_type(&self, body: &Body) -> FunctionType<'ctx> {
        let mut params = Vec::new();
        if self.is_real_aggregate(&body.return_ty) {
            params.push(self.ptr().into());
        }
        params.extend(body.locals[..body.param_count].iter().filter_map(|local| self.llvm_ty(&local.ty)));
        self.fn_type(&params, self.llvm_ty(&body.return_ty))
    }

    fn declare(&mut self, bodies: &[(String, Body)], imports: &[(String, Body)], externs: &[(String, Vec<Type>, Type)]) {
        let i64_ty: BasicTypeEnum<'ctx> = self.i64().into();
        for (name, params, returns) in FFI_FUNCTIONS {
            let ret: Option<BasicTypeEnum<'ctx>> = match returns {
                FfiReturn::None => None,
                FfiReturn::Status => Some(self.context.i32_type().into()),
                FfiReturn::Pointer => Some(i64_ty),
            };
            let ty = self.fn_type(&vec![i64_ty; *params], ret);
            self.module.add_function(name, ty, Some(Linkage::External));
        }
        for (name, params, return_ty) in externs {
            let params: Vec<_> = params.iter().filter_map(|param| self.llvm_ty(param)).collect();
            let ty = self.fn_type(&params, self.llvm_ty(return_ty));
            self.module.add_function(name, ty, Some(Linkage::External));
        }
        for (name, body) in bodies.iter().chain(imports) {
            self.module.add_function(name, self.body_fn_type(body), Some(Linkage::External));
            self.params
                .insert(name.clone(), body.locals[..body.param_count].iter().map(|local| local.ty.clone()).collect());
        }
    }

    pub(crate) fn runtime_fn(&self, name: &str, params: &[BasicTypeEnum<'ctx>], ret: Option<BasicTypeEnum<'ctx>>) -> FunctionValue<'ctx> {
        self.module
            .get_function(name)
            .unwrap_or_else(|| self.module.add_function(name, self.fn_type(params, ret), Some(Linkage::External)))
    }

    pub(crate) fn call(&self, function: FunctionValue<'ctx>, args: &[BasicValueEnum<'ctx>]) -> Option<BasicValueEnum<'ctx>> {
        let params = function.get_type().get_param_types();
        let args: Vec<BasicMetadataValueEnum<'ctx>> = args
            .iter()
            .zip(params)
            .map(|(arg, param)| self.coerce(*arg, BasicTypeEnum::try_from(param).expect("basic parameter")).into())
            .collect();
        self.builder.build_call(function, &args, "").expect("call").try_as_basic_value().basic()
    }

    /// Calls a runtime helper whose parameter types are the arguments' own.
    pub(crate) fn call_runtime(
        &self,
        name: &str,
        args: &[BasicValueEnum<'ctx>],
        ret: Option<BasicTypeEnum<'ctx>>,
    ) -> Option<BasicValueEnum<'ctx>> {
        let params: Vec<_> = args.iter().map(|arg| arg.get_type()).collect();
        self.call(self.runtime_fn(name, &params, ret), args)
    }

    pub(crate) fn coerce(&self, value: BasicValueEnum<'ctx>, ty: BasicTypeEnum<'ctx>) -> BasicValueEnum<'ctx> {
        if value.get_type() == ty {
            return value;
        }
        let builder = &self.builder;
        match (value, ty) {
            (BasicValueEnum::PointerValue(pointer), BasicTypeEnum::IntType(int)) => {
                builder.build_ptr_to_int(pointer, int, "").expect("ptrtoint").into()
            }
            (BasicValueEnum::IntValue(int), BasicTypeEnum::PointerType(pointer)) => {
                let wide = builder.build_int_z_extend_or_bit_cast(int, self.i64(), "").expect("zext");
                builder.build_int_to_ptr(wide, pointer, "").expect("inttoptr").into()
            }
            (BasicValueEnum::IntValue(int), BasicTypeEnum::IntType(target)) => {
                builder.build_int_cast_sign_flag(int, target, false, "").expect("int cast").into()
            }
            (value, ty) => builder.build_bit_cast(value, ty, "").expect("bitcast"),
        }
    }

    pub(crate) fn as_ptr(&self, value: BasicValueEnum<'ctx>) -> PointerValue<'ctx> {
        self.coerce(value, self.ptr().into()).into_pointer_value()
    }

    pub(crate) fn offset(&self, base: PointerValue<'ctx>, offset: i64) -> PointerValue<'ctx> {
        if offset == 0 {
            return base;
        }
        self.offset_by(base, self.int(offset))
    }

    pub(crate) fn offset_by(&self, base: PointerValue<'ctx>, offset: IntValue<'ctx>) -> PointerValue<'ctx> {
        unsafe { self.builder.build_gep(self.i8(), base, &[offset], "").expect("gep") }
    }

    pub(crate) fn load(&self, ty: BasicTypeEnum<'ctx>, address: PointerValue<'ctx>, offset: i64) -> BasicValueEnum<'ctx> {
        let load = self.builder.build_load(ty, self.offset(address, offset), "").expect("load");
        load.as_instruction_value().expect("load instruction").set_alignment(1).expect("alignment");
        load
    }

    pub(crate) fn store(&self, address: PointerValue<'ctx>, offset: i64, value: BasicValueEnum<'ctx>) {
        let store = self.builder.build_store(self.offset(address, offset), value).expect("store");
        store.set_alignment(1).expect("alignment");
    }

    pub(crate) fn memcpy(&self, dest: PointerValue<'ctx>, src: PointerValue<'ctx>, size: u64) {
        if size > 0 {
            self.builder.build_memcpy(dest, 1, src, 1, self.int(size as i64)).expect("memcpy");
        }
    }

    pub(crate) fn block(&self, function: FunctionValue<'ctx>) -> BasicBlock<'ctx> {
        self.context.append_basic_block(function, "")
    }

    pub(crate) fn branch_if(&self, condition: IntValue<'ctx>, then: BasicBlock<'ctx>, otherwise: BasicBlock<'ctx>) {
        let zero = condition.get_type().const_zero();
        let truth = if condition.get_type().get_bit_width() == 1 {
            condition
        } else {
            self.builder.build_int_compare(IntPredicate::NE, condition, zero, "").expect("icmp")
        };
        self.builder.build_conditional_branch(truth, then, otherwise).expect("br");
    }

    fn bool_value(&self, bit: IntValue<'ctx>) -> BasicValueEnum<'ctx> {
        self.builder.build_int_z_extend(bit, self.i8(), "").expect("zext").into()
    }

    fn trap(&self) {
        let trap = Intrinsic::find("llvm.trap").expect("llvm.trap").get_declaration(self.module, &[]).expect("trap");
        self.builder.build_call(trap, &[], "").expect("trap");
        self.builder.build_unreachable().expect("unreachable");
    }


    pub(crate) fn entry_alloca(&self, entry: BasicBlock<'ctx>, size: u64, align: u64) -> PointerValue<'ctx> {
        let builder = self.context.create_builder();
        match entry.get_first_instruction() {
            Some(first) => builder.position_before(&first),
            None => builder.position_at_end(entry),
        }
        let slot = builder.build_alloca(self.i8().array_type(size.max(1) as u32), "").expect("alloca");
        slot.as_instruction().expect("alloca instruction").set_alignment(align.max(1) as u32).expect("alignment");
        slot
    }

    fn define(&mut self, name: &str, body: &Body) {
        let function = self.module.get_function(name).expect("declared");
        for text in body_strings(body) {
            self.string_constant(&text);
        }
        self.begin_debug_scope(name, function, body);
        self.locations = (0..body.blocks.len())
            .map(|block| {
                let statements =
                    (0..body.blocks[block].statements.len()).map(|index| self.locate(body.statement_span(block, index))).collect();
                (statements, self.locate(body.terminator_span(block)))
            })
            .collect();
        let blocks: Vec<_> = body.blocks.iter().map(|_| self.block(function)).collect();
        let entry = self.context.prepend_basic_block(blocks[0], "entry");
        self.builder.position_at_end(entry);
        let borrowed = borrowed_locals(body);
        let mut locals = Vec::with_capacity(body.locals.len());
        for (index, local) in body.locals.iter().enumerate() {
            locals.push(match self.llvm_ty(&local.ty) {
                None => Storage::Unit,
                Some(_) if self.is_real_aggregate(&local.ty) => {
                    let (size, align) = self.size_align(&local.ty);
                    Storage::Aggregate(self.entry_alloca(entry, size, align), size)
                }
                Some(ty) if borrowed.contains(&(index as u32)) => Storage::Stack(self.entry_alloca(entry, 8, 8), ty),
                Some(ty) => Storage::Register(self.entry_alloca(entry, 8, 8), ty),
            });
        }
        let flags = body
            .locals
            .iter()
            .map(|local| {
                self.glue.contains_key(&local.ty).then(|| {
                    let flag = self.entry_alloca(entry, 1, 1);
                    self.store(flag, 0, self.i8().const_zero().into());
                    flag
                })
            })
            .collect();
        let mut frame = Frame { body, function, entry, blocks, locals, flags, sret: None };

        let mut params = function.get_param_iter();
        if self.is_real_aggregate(&body.return_ty) {
            frame.sret = Some(params.next().expect("sret parameter").into_pointer_value());
        }
        let param_locals = (0..body.param_count).filter(|index| self.llvm_ty(&body.locals[*index].ty).is_some());
        for (index, value) in param_locals.zip(params) {
            self.write_unowned(&frame, &Place::Local(Local(index as u32)), value);
            self.set_flag(&frame, Local(index as u32), true);
        }
        self.builder.build_unconditional_branch(frame.blocks[0]).expect("br");

        for (index, block) in body.blocks.iter().enumerate() {
            self.builder.position_at_end(frame.blocks[index]);
            for (statement_index, statement) in block.statements.iter().enumerate() {
                let here = self.locations[index].0[statement_index];
                self.here.set(here);
                self.set_debug_location(here.1, here.2);
                self.statement(&frame, statement);
            }
            let here = self.locations[index].1;
            self.here.set(here);
            self.set_debug_location(here.1, here.2);
            self.terminator(&frame, &block.terminator);
        }
        frame.flags.clear();
        self.builder.unset_current_debug_location();
    }

    fn set_flag(&self, frame: &Frame<'ctx, '_>, local: Local, owned: bool) {
        if let Some(flag) = frame.flags[local.0 as usize] {
            self.store(flag, 0, self.i8().const_int(u64::from(owned), false).into());
        }
    }

    fn place_ty(&self, frame: &Frame<'ctx, '_>, place: &Place) -> Type {
        match place {
            Place::Local(local) => frame.body.locals[local.0 as usize].ty.clone(),
            Place::Field { base, field } => self.field_of(&self.place_ty(frame, base), field).0,
            Place::VariantField { base, variant, index } => {
                let (name, args) = enum_name_of(&self.place_ty(frame, base));
                self.layouts.enum_variant_field(&name, &args, variant, *index).0
            }
            Place::Index { base, .. } => slice_elem_of(&self.place_ty(frame, base)),
            Place::Deref { ty, .. } => ty.clone(),
        }
    }

    fn operand_ty(&self, frame: &Frame<'ctx, '_>, operand: &Operand) -> Type {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => self.place_ty(frame, place),
            Operand::Constant(constant) => match constant {
                Constant::Int(_, width) => Type::Int(*width),
                Constant::Float(_, width) => Type::Float(*width),
                Constant::Bool(_) => Type::Bool,
                Constant::Char(_) => Type::Char,
                Constant::Str(_) => Type::String,
                Constant::Unit => Type::Unit,
                Constant::Type(_) => Type::TypeValue(Box::new(Type::Unknown)),
            },
        }
    }

    fn field_of(&self, base_ty: &Type, field: &str) -> (Type, u64) {
        match base_ty {
            Type::Borrow { ty, .. } => self.field_of(ty, field),
            Type::Tuple(items) => self
                .layouts
                .tuple_field(items, field.parse().unwrap_or_else(|_| panic!("tuple field `{field}` is not an index"))),
            _ => {
                let (name, args) = struct_name_of(base_ty);
                self.layouts.struct_field(&name, &args, field)
            }
        }
    }

    fn place_address(&self, frame: &Frame<'ctx, '_>, place: &Place) -> (PointerValue<'ctx>, Type) {
        match place {
            Place::Local(local) => {
                let ty = frame.body.locals[local.0 as usize].ty.clone();
                let address = match frame.locals[local.0 as usize] {
                    Storage::Register(slot, ty) => self.as_ptr(self.load(ty, slot, 0)),
                    Storage::Stack(slot, _) | Storage::Aggregate(slot, _) => slot,
                    Storage::Unit => panic!("place has no value (unit-typed local {local:?})"),
                };
                (address, ty)
            }
            Place::Field { base, field } => {
                let (base_address, base_ty) = self.place_address(frame, base);
                let (field_ty, offset) = self.field_of(&base_ty, field);
                (self.offset(base_address, offset as i64), field_ty)
            }
            Place::VariantField { base, variant, index } => {
                let (base_address, base_ty) = self.place_address(frame, base);
                let (name, args) = enum_name_of(&base_ty);
                let (field_ty, offset) = self.layouts.enum_variant_field(&name, &args, variant, *index);
                (self.offset(base_address, offset as i64), field_ty)
            }
            Place::Index { base, index } => {
                let (base_address, base_ty) = self.place_address(frame, base);
                let elem_ty = slice_elem_of(&base_ty);
                (self.slice_element_address(frame, base_address, index, &elem_ty), elem_ty)
            }
            Place::Deref { address, ty } => (self.as_ptr(self.operand(frame, address)), ty.clone()),
        }
    }

    /// Bounds-checked in every profile.
    fn slice_element_address(
        &self,
        frame: &Frame<'ctx, '_>,
        base: PointerValue<'ctx>,
        index: &Operand,
        elem_ty: &Type,
    ) -> PointerValue<'ctx> {
        let size = element_size(elem_ty, self.layouts);
        let data = self.as_ptr(self.load(self.ptr().into(), base, i64::from(SLICE_DATA_OFFSET)));
        let len = self.load(self.i64().into(), base, i64::from(SLICE_LEN_OFFSET)).into_int_value();
        let index = self.coerce(self.operand(frame, index), self.i64().into()).into_int_value();
        let out_of_bounds = self.builder.build_int_compare(IntPredicate::UGE, index, len, "").expect("icmp");
        self.fail_if(frame.function, out_of_bounds, |generator| {
            generator.panic_call("paco_rt_panic_bounds", &[index.into(), len.into()]);
        });
        let byte_offset = self.builder.build_int_mul(index, self.int(size as i64), "").expect("mul");
        self.offset_by(data, byte_offset)
    }

    fn read_place(&self, frame: &Frame<'ctx, '_>, place: &Place) -> BasicValueEnum<'ctx> {
        match place {
            Place::Local(local) => match frame.locals[local.0 as usize] {
                Storage::Register(slot, ty) | Storage::Stack(slot, ty) => self.load(ty, slot, 0),
                Storage::Aggregate(slot, _) => slot.into(),
                Storage::Unit => panic!("operand has no value (unit-typed local {local:?})"),
            },
            _ => {
                let (address, ty) = self.place_address(frame, place);
                if self.is_real_aggregate(&ty) {
                    address.into()
                } else {
                    let llvm_ty = self.llvm_ty(&ty).unwrap_or_else(|| panic!("cannot load a unit-typed field: {ty:?}"));
                    self.load(llvm_ty, address, 0)
                }
            }
        }
    }

    fn write_place(&self, frame: &Frame<'ctx, '_>, place: &Place, value: BasicValueEnum<'ctx>) {
        match place {
            Place::Local(local) => {
                self.drop_local_if_owned(frame, *local);
                self.write_unowned(frame, place, value);
                self.set_flag(frame, *local, true);
            }
            _ => {
                let (address, ty) = self.place_address(frame, place);
                if self.glue.contains_key(&ty) {
                    let old = if self.is_real_aggregate(&ty) {
                        address.into()
                    } else {
                        self.load(self.llvm_ty(&ty).expect("owning types are never unit"), address, 0)
                    };
                    self.drop_value(frame, &ty, old);
                }
                self.store_field(value, &ty, address, 0);
            }
        }
    }

    fn write_unowned(&self, frame: &Frame<'ctx, '_>, place: &Place, value: BasicValueEnum<'ctx>) {
        match place {
            Place::Local(local) => match frame.locals[local.0 as usize] {
                Storage::Register(slot, ty) | Storage::Stack(slot, ty) => self.store(slot, 0, self.coerce(value, ty)),
                Storage::Aggregate(slot, size) => self.memcpy(slot, self.as_ptr(value), size),
                Storage::Unit => {}
            },
            _ => {
                let (address, ty) = self.place_address(frame, place);
                self.store_field(value, &ty, address, 0);
            }
        }
    }

    pub(crate) fn store_field(&self, value: BasicValueEnum<'ctx>, ty: &Type, dest: PointerValue<'ctx>, offset: i64) {
        if self.is_real_aggregate(ty) {
            let (size, _) = self.size_align(ty);
            self.memcpy(self.offset(dest, offset), self.as_ptr(value), size);
        } else {
            self.store(dest, offset, value);
        }
    }

    fn value_address(&self, frame: &Frame<'ctx, '_>, ty: &Type, value: BasicValueEnum<'ctx>) -> PointerValue<'ctx> {
        if self.is_real_aggregate(ty) {
            return self.as_ptr(value);
        }
        let slot = self.entry_alloca(frame.entry, 8, 8);
        self.store(slot, 0, value);
        slot
    }

    fn drop_value(&self, frame: &Frame<'ctx, '_>, ty: &Type, value: BasicValueEnum<'ctx>) {
        if let Some(&(drop, _)) = self.glue.get(ty) {
            let address = self.value_address(frame, ty, value);
            self.call(drop, &[address.into()]);
        }
    }

    fn clone_value(&self, frame: &Frame<'ctx, '_>, ty: &Type, value: BasicValueEnum<'ctx>) -> BasicValueEnum<'ctx> {
        let Some(&(_, clone)) = self.glue.get(ty) else {
            return value;
        };
        if self.is_real_aggregate(ty) {
            let (size, align) = self.size_align(ty);
            let copy = self.entry_alloca(frame.entry, size, align);
            self.memcpy(copy, self.as_ptr(value), size);
            self.call(clone, &[copy.into()]);
            copy.into()
        } else {
            let address = self.value_address(frame, ty, value);
            self.call(clone, &[address.into()]);
            value
        }
    }

    fn owned_operand(&self, frame: &Frame<'ctx, '_>, operand: &Operand) -> BasicValueEnum<'ctx> {
        let ty = self.operand_ty(frame, operand);
        let value = self.operand(frame, operand);
        if !self.glue.contains_key(&ty) {
            return value;
        }
        match operand {
            Operand::Move(Place::Local(local)) => {
                self.set_flag(frame, *local, false);
                value
            }
            Operand::Move(Place::VariantField { base, .. })
                if matches!(base.as_ref(), Place::Local(local)
                    if frame.body.locals[local.0 as usize].name.as_deref() == Some(paco_mir::TRY_TEMP)) =>
            {
                let Place::Local(local) = base.as_ref() else { unreachable!() };
                self.set_flag(frame, *local, false);
                value
            }
            _ => self.clone_value(frame, &ty, value),
        }
    }

    /// Like [`Self::owned_operand`], but a borrow of `expected` is copied
    /// out of its referent (the type checker's implicit deref).
    fn owned_as(&self, frame: &Frame<'ctx, '_>, operand: &Operand, expected: &Type) -> BasicValueEnum<'ctx> {
        if let Type::Borrow { ty: inner, .. } = self.operand_ty(frame, operand)
            && *inner == *expected
        {
            let pointer = self.operand(frame, operand);
            let value = match self.llvm_ty(expected) {
                Some(ty) if !self.is_real_aggregate(expected) => self.load(ty, self.as_ptr(pointer), 0),
                _ => pointer,
            };
            return self.clone_value(frame, expected, value);
        }
        self.owned_operand(frame, operand)
    }

    fn drop_local_if_owned(&self, frame: &Frame<'ctx, '_>, local: Local) {
        let Some(flag) = frame.flags[local.0 as usize] else { return };
        let owned = self.load(self.i8().into(), flag, 0).into_int_value();
        let drop_block = self.block(frame.function);
        let done = self.block(frame.function);
        self.branch_if(owned, drop_block, done);
        self.builder.position_at_end(drop_block);
        let ty = frame.body.locals[local.0 as usize].ty.clone();
        let value = self.read_place(frame, &Place::Local(local));
        self.drop_value(frame, &ty, value);
        self.set_flag(frame, local, false);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
    }

    fn drop_all_owned(&self, frame: &Frame<'ctx, '_>) {
        for index in (0..frame.body.locals.len()).rev() {
            self.drop_local_if_owned(frame, Local(index as u32));
        }
    }

    fn string_constant(&mut self, text: &str) -> PointerValue<'ctx> {
        if let Some(pointer) = self.strings.get(text) {
            return *pointer;
        }
        let bytes = self.context.const_string(text.as_bytes(), false);
        let global = self.module.add_global(bytes.get_type(), None, "");
        global.set_initializer(&bytes);
        global.set_constant(true);
        global.set_linkage(Linkage::Private);
        let pointer = global.as_pointer_value();
        self.strings.insert(text.to_string(), pointer);
        pointer
    }

    fn operand(&self, frame: &Frame<'ctx, '_>, operand: &Operand) -> BasicValueEnum<'ctx> {
        match operand {
            Operand::Constant(constant) => self.constant(frame, constant),
            Operand::Copy(place) | Operand::Move(place) => self.read_place(frame, place),
        }
    }

    fn constant(&self, frame: &Frame<'ctx, '_>, constant: &Constant) -> BasicValueEnum<'ctx> {
        match constant {
            Constant::Int(value, width) => {
                self.llvm_ty(&Type::Int(*width)).expect("int").into_int_type().const_int(*value as u64, true).into()
            }
            Constant::Float(bits, width) => match width {
                FloatWidth::F64 => self.context.f64_type().const_float(f64::from_bits(*bits)).into(),
                FloatWidth::F32 => self.context.f32_type().const_float(f64::from(f64::from_bits(*bits) as f32)).into(),
                small => self
                    .float_storage(*small)
                    .into_int_type()
                    .const_int(small.encode(f64::from_bits(*bits)), false)
                    .into(),
            },
            Constant::Bool(value) => self.i8().const_int(u64::from(*value), false).into(),
            Constant::Char(value) => self.context.i32_type().const_int(u64::from(u32::from(*value)), false).into(),
            Constant::Str(text) => {
                let data = self.strings[text];
                let descriptor = self.entry_alloca(frame.entry, 16, 8);
                self.store(descriptor, i64::from(SLICE_DATA_OFFSET), data.into());
                self.store(descriptor, i64::from(SLICE_LEN_OFFSET), self.int(text.len() as i64).into());
                descriptor.into()
            }
            Constant::Unit => panic!("a unit value has no runtime representation"),
            Constant::Type(_) => panic!("a `type` value has no runtime representation"),
        }
    }

    fn statement(&self, frame: &Frame<'ctx, '_>, statement: &Statement) {
        match statement {
            Statement::Assign(place, rvalue) => {
                if matches!(place, Place::Local(local) if matches!(frame.body.locals[local.0 as usize].ty, Type::Unit | Type::Never)) {
                    return;
                }
                let value = match rvalue {
                    Rvalue::Use(operand) => self.owned_as(frame, operand, &self.place_ty(frame, place)),
                    _ => self.rvalue(frame, rvalue),
                };
                if matches!(rvalue, Rvalue::Load { .. }) {
                    self.write_unowned(frame, place, value);
                } else {
                    self.write_place(frame, place, value);
                }
            }
            Statement::Store { address, value, ty } => {
                if matches!(ty, Type::Unit) {
                    return;
                }
                let address = self.as_ptr(self.operand(frame, address));
                let mut value = self.owned_operand(frame, value);
                if self.is_real_aggregate(ty) {
                    let (size, _) = self.size_align(ty);
                    let boxed = self
                        .call_runtime("paco_alloc", &[self.int(size.max(1) as i64).into()], Some(self.ptr().into()))
                        .expect("malloc returns");
                    self.memcpy(self.as_ptr(boxed), self.as_ptr(value), size);
                    value = boxed;
                }
                self.store(address, 0, value);
            }
            Statement::FreeBox { address, ty } => {
                if self.is_real_aggregate(ty) {
                    let address = self.as_ptr(self.operand(frame, address));
                    let boxed = self.load(self.ptr().into(), address, 0);
                    self.call_runtime("paco_free", &[boxed], None);
                }
            }
            Statement::Drop(Place::Local(local)) => self.drop_local_if_owned(frame, *local),
            Statement::Drop(_) | Statement::StorageDead(_) => {}
        }
    }

    fn rvalue(&self, frame: &Frame<'ctx, '_>, rvalue: &Rvalue) -> BasicValueEnum<'ctx> {
        match rvalue {
            Rvalue::Use(operand) => self.owned_operand(frame, operand),
            Rvalue::UnaryOp(op, operand) => {
                let ty = self.operand_ty(frame, operand);
                let value = self.operand(frame, operand);
                self.unary(frame, *op, value, &ty)
            }
            Rvalue::BinaryOp(op, left, right) => {
                let ty = self.operand_ty(frame, left);
                let left = self.operand(frame, left);
                let right = self.operand(frame, right);
                if matches!(strip_borrow(&ty), Type::String) {
                    return self.string_binary(frame, *op, left, right);
                }
                if let Type::Float(width) = ty
                    && !matches!(width, FloatWidth::F64 | FloatWidth::F32)
                {
                    let wide_left = self.float_to_f64(left, width);
                    let wide_right = self.float_to_f64(right, width);
                    let result = self.binary(frame, *op, wide_left, wide_right, &Type::Float(FloatWidth::F64));
                    return if result.is_float_value() { self.f64_to_float(result, width) } else { result };
                }
                self.binary(frame, *op, left, right, &ty)
            }
            Rvalue::Cast { operand, target } => {
                let source = self.operand_ty(frame, operand);
                let value = self.operand(frame, operand);
                self.cast(value, &source, target)
            }
            Rvalue::Aggregate { ty, variant, fields } => self.aggregate(frame, ty, variant.as_deref(), fields),
            Rvalue::Math(op, args) => {
                let Type::Float(width) = self.operand_ty(frame, &args[0]) else { panic!("`{}` needs a float receiver", op.name()) };
                let values: Vec<_> = args.iter().map(|arg| self.operand(frame, arg)).collect();
                self.math(*op, &values, width)
            }
            Rvalue::SliceLen(place) => {
                let descriptor = self.as_ptr(self.read_place(frame, place));
                self.load(self.i64().into(), descriptor, i64::from(SLICE_LEN_OFFSET))
            }
            Rvalue::Discriminant(place) => {
                let (address, _) = self.place_address(frame, place);
                self.load(self.i64().into(), address, 0)
            }
            Rvalue::Load { address, ty } => {
                let address = self.as_ptr(self.operand(frame, address));
                self.load(self.llvm_ty(ty).expect("cannot load a unit-typed value"), address, 0)
            }
            Rvalue::RawAlloc { size } => self.entry_alloca(frame.entry, *size, 8).into(),
            Rvalue::Ref { place, .. } => self.place_address(frame, place).0.into(),
            Rvalue::Quote { .. } => panic!("`quote` is evaluated at compile time"),
            Rvalue::FuncAddr(name) => self
                .module
                .get_function(name)
                .unwrap_or_else(|| panic!("undeclared function `{name}`"))
                .as_global_value()
                .as_pointer_value()
                .into(),
        }
    }

    fn aggregate(&self, frame: &Frame<'ctx, '_>, ty: &Type, variant: Option<&str>, fields: &[Operand]) -> BasicValueEnum<'ctx> {
        match (ty, variant) {
            (Type::Struct(name, args), None) => {
                let layout = self.layouts.struct_layout(name, args);
                let address = self.entry_alloca(frame.entry, layout.size, layout.align);
                for ((declared, offset), field) in self.layouts.struct_fields(name, args).into_iter().zip(fields) {
                    let mut field_ty = self.operand_ty(frame, field);
                    if matches!(field_ty, Type::Unit) {
                        continue;
                    }
                    if matches!(&field_ty, Type::Borrow { ty, .. } if **ty == declared) {
                        field_ty = declared.clone();
                    }
                    let value = self.owned_as(frame, field, &declared);
                    self.store_field(value, &field_ty, address, offset as i64);
                }
                self.mark_live(ty, address);
                address.into()
            }
            (Type::Enum(name, args), Some(variant)) => {
                let layout = self.layouts.enum_layout(name, args);
                let address = self.entry_alloca(frame.entry, layout.size, layout.align);
                let tag = self.layouts.enum_variant_index(name, args, variant);
                self.store(address, 0, self.int(tag as i64).into());
                for (index, field) in fields.iter().enumerate() {
                    let (_, offset) = self.layouts.enum_variant_field(name, args, variant, index);
                    let field_ty = self.operand_ty(frame, field);
                    if matches!(field_ty, Type::Unit) {
                        continue;
                    }
                    let value = self.owned_operand(frame, field);
                    self.store_field(value, &field_ty, address, offset as i64);
                }
                self.mark_live(ty, address);
                address.into()
            }
            (Type::Tuple(items), None) => {
                let layout = self.layouts.tuple_layout(items);
                let address = self.entry_alloca(frame.entry, layout.size, layout.align);
                for (index, field) in fields.iter().enumerate() {
                    let field_ty = self.operand_ty(frame, field);
                    if matches!(field_ty, Type::Unit) {
                        continue;
                    }
                    let (_, offset) = self.layouts.tuple_field(items, index);
                    let value = self.owned_operand(frame, field);
                    self.store_field(value, &field_ty, address, offset as i64);
                }
                address.into()
            }
            _ => panic!("aggregate codegen is not implemented yet: {ty:?} / {variant:?}"),
        }
    }

    fn mark_live(&self, ty: &Type, address: PointerValue<'ctx>) {
        if let Some(offset) = self.layouts.live_offset(ty) {
            self.store(address, offset as i64, self.i8().const_int(1, false).into());
        }
    }

    fn unary(&self, frame: &Frame<'ctx, '_>, op: UnOp, value: BasicValueEnum<'ctx>, ty: &Type) -> BasicValueEnum<'ctx> {
        let builder = &self.builder;
        if let (UnOp::Neg, Type::Int(width), Profile::Debug) = (op, ty, frame.body.profile)
            && width.is_signed()
        {
            let int = value.into_int_value();
            let min = int.get_type().const_int(width.range().0 as u64, true);
            let overflow = builder.build_int_compare(IntPredicate::EQ, int, min, "").expect("icmp");
            self.panic_if(frame.function, overflow, "attempt to negate with overflow");
        }
        match (op, ty) {
            (UnOp::Not, _) => {
                let int = value.into_int_value();
                let zero = int.get_type().const_zero();
                self.bool_value(builder.build_int_compare(IntPredicate::EQ, int, zero, "").expect("icmp"))
            }
            (UnOp::Neg, Type::Float(FloatWidth::F64 | FloatWidth::F32)) => {
                builder.build_float_neg(value.into_float_value(), "").expect("fneg").into()
            }
            (UnOp::Neg, Type::Float(width)) => {
                let int = value.into_int_value();
                let sign = int.get_type().const_int(1u64 << (width.bytes() * 8 - 1), false);
                builder.build_xor(int, sign, "").expect("xor").into()
            }
            (UnOp::Neg, _) => builder.build_int_neg(value.into_int_value(), "").expect("neg").into(),
            (UnOp::BitNot, _) => builder.build_not(value.into_int_value(), "").expect("not").into(),
        }
    }

    fn binary(
        &self,
        frame: &Frame<'ctx, '_>,
        op: BinOp,
        left: BasicValueEnum<'ctx>,
        right: BasicValueEnum<'ctx>,
        ty: &Type,
    ) -> BasicValueEnum<'ctx> {
        let builder = &self.builder;
        match ty {
            Type::Float(_) => {
                let (left, right) = (left.into_float_value(), right.into_float_value());
                let compare = |predicate| {
                    self.bool_value(builder.build_float_compare(predicate, left, right, "").expect("fcmp"))
                };
                match op {
                    BinOp::Add => builder.build_float_add(left, right, "").expect("fadd").into(),
                    BinOp::Sub => builder.build_float_sub(left, right, "").expect("fsub").into(),
                    BinOp::Mul => builder.build_float_mul(left, right, "").expect("fmul").into(),
                    BinOp::Div => builder.build_float_div(left, right, "").expect("fdiv").into(),
                    BinOp::Rem => panic!("float remainder codegen is not implemented yet"),
                    BinOp::Eq => compare(FloatPredicate::OEQ),
                    BinOp::Ne => compare(FloatPredicate::UNE),
                    BinOp::Lt => compare(FloatPredicate::OLT),
                    BinOp::Le => compare(FloatPredicate::OLE),
                    BinOp::Gt => compare(FloatPredicate::OGT),
                    BinOp::Ge => compare(FloatPredicate::OGE),
                    _ => panic!("operator `{op:?}` is not valid over float operands"),
                }
            }
            Type::Bool | Type::Char | Type::Int(_) => {
                let (left, right) = (left.into_int_value(), right.into_int_value());
                let signed = matches!(ty, Type::Int(width) if width.is_signed());
                let compare = |signed_predicate, unsigned_predicate| {
                    let predicate = if signed { signed_predicate } else { unsigned_predicate };
                    self.bool_value(builder.build_int_compare(predicate, left, right, "").expect("icmp"))
                };
                match (op, ty) {
                    (BinOp::Eq, _) => compare(IntPredicate::EQ, IntPredicate::EQ),
                    (BinOp::Ne, _) => compare(IntPredicate::NE, IntPredicate::NE),
                    (BinOp::And, Type::Bool) => builder.build_and(left, right, "").expect("and").into(),
                    (BinOp::Or, Type::Bool) => builder.build_or(left, right, "").expect("or").into(),
                    (_, Type::Bool) => panic!("operator `{op:?}` is not valid over bool operands"),
                    (BinOp::Lt, _) => compare(IntPredicate::SLT, IntPredicate::ULT),
                    (BinOp::Le, _) => compare(IntPredicate::SLE, IntPredicate::ULE),
                    (BinOp::Gt, _) => compare(IntPredicate::SGT, IntPredicate::UGT),
                    (BinOp::Ge, _) => compare(IntPredicate::SGE, IntPredicate::UGE),
                    (_, Type::Char) => panic!("operator `{op:?}` is not valid over char operands"),
                    (BinOp::Add | BinOp::Sub | BinOp::Mul, _) => self.checked(frame, op, left, right, signed),
                    (BinOp::Div | BinOp::Rem, _) => self.divide(frame, op, left, right, signed),
                    (BinOp::BitAnd, _) => builder.build_and(left, right, "").expect("and").into(),
                    (BinOp::BitOr, _) => builder.build_or(left, right, "").expect("or").into(),
                    (BinOp::BitXor, _) => builder.build_xor(left, right, "").expect("xor").into(),
                    (BinOp::Shl | BinOp::Shr, _) => self.shift(frame, op, left, right, signed),
                    (BinOp::WrappingAdd, _) => builder.build_int_add(left, right, "").expect("add").into(),
                    (BinOp::WrappingSub, _) => builder.build_int_sub(left, right, "").expect("sub").into(),
                    (BinOp::WrappingMul, _) => builder.build_int_mul(left, right, "").expect("mul").into(),
                    (BinOp::AddOverflows | BinOp::SubOverflows | BinOp::MulOverflows, _) => {
                        let (_, overflow) = self.with_overflow(op, left, right, signed);
                        self.bool_value(overflow)
                    }
                    (BinOp::And | BinOp::Or, _) => panic!("logical operator over int operands is not valid"),
                }
            }
            other => panic!("codegen for binary operands of type {other:?} is not implemented yet"),
        }
    }

    /// Wrapping in release, panicking on overflow in debug.
    fn checked(&self, frame: &Frame<'ctx, '_>, op: BinOp, left: IntValue<'ctx>, right: IntValue<'ctx>, signed: bool) -> BasicValueEnum<'ctx> {
        let builder = &self.builder;
        if frame.body.profile == Profile::Release {
            return match op {
                BinOp::Add => builder.build_int_add(left, right, ""),
                BinOp::Sub => builder.build_int_sub(left, right, ""),
                _ => builder.build_int_mul(left, right, ""),
            }
            .expect("arith")
            .into();
        }
        let (result, overflow) = self.with_overflow(op, left, right, signed);
        let message = match op {
            BinOp::Add => "attempt to add with overflow",
            BinOp::Sub => "attempt to subtract with overflow",
            _ => "attempt to multiply with overflow",
        };
        self.panic_if(frame.function, overflow, message);
        result
    }

    /// The wrapped result and the overflow flag of an add, subtract or multiply.
    fn with_overflow(&self, op: BinOp, left: IntValue<'ctx>, right: IntValue<'ctx>, signed: bool) -> (BasicValueEnum<'ctx>, IntValue<'ctx>) {
        let name = match (op, signed) {
            (BinOp::Add | BinOp::AddOverflows, true) => "llvm.sadd.with.overflow",
            (BinOp::Add | BinOp::AddOverflows, false) => "llvm.uadd.with.overflow",
            (BinOp::Sub | BinOp::SubOverflows, true) => "llvm.ssub.with.overflow",
            (BinOp::Sub | BinOp::SubOverflows, false) => "llvm.usub.with.overflow",
            (_, true) => "llvm.smul.with.overflow",
            (_, false) => "llvm.umul.with.overflow",
        };
        let intrinsic = Intrinsic::find(name)
            .expect("overflow intrinsic")
            .get_declaration(self.module, &[left.get_type().into()])
            .expect("overflow intrinsic declaration");
        let pair = self.call(intrinsic, &[left.into(), right.into()]).expect("pair").into_struct_value();
        let result = self.builder.build_extract_value(pair, 0, "").expect("result");
        let overflow = self.builder.build_extract_value(pair, 1, "").expect("overflow").into_int_value();
        (result, overflow)
    }

    /// Panics in debug on an amount outside `0..bits`; masks it in release.
    fn shift(&self, frame: &Frame<'ctx, '_>, op: BinOp, left: IntValue<'ctx>, right: IntValue<'ctx>, signed: bool) -> BasicValueEnum<'ctx> {
        let builder = &self.builder;
        let bits = left.get_type().get_bit_width();
        let amount_ty = right.get_type();
        if frame.body.profile == Profile::Debug {
            let limit = amount_ty.const_int(u64::from(bits), false);
            let overflow = builder.build_int_compare(IntPredicate::UGE, right, limit, "").expect("icmp");
            let message = if op == BinOp::Shl { "attempt to shift left with overflow" } else { "attempt to shift right with overflow" };
            self.panic_if(frame.function, overflow, message);
        }
        let masked = builder.build_and(right, amount_ty.const_int(u64::from(bits - 1), false), "").expect("and");
        let amount = builder.build_int_cast_sign_flag(masked, left.get_type(), false, "").expect("cast");
        if op == BinOp::Shl {
            builder.build_left_shift(left, amount, "").expect("shl").into()
        } else {
            builder.build_right_shift(left, amount, signed, "").expect("shr").into()
        }
    }

    /// Panics on a zero divisor and on signed `MIN / -1`; `MIN % -1` is 0.
    fn divide(&self, frame: &Frame<'ctx, '_>, op: BinOp, left: IntValue<'ctx>, right: IntValue<'ctx>, signed: bool) -> BasicValueEnum<'ctx> {
        let builder = &self.builder;
        let ty = left.get_type();
        let zero = builder.build_int_compare(IntPredicate::EQ, right, ty.const_zero(), "").expect("icmp");
        self.panic_if(frame.function, zero, if op == BinOp::Div { "division by zero" } else { "remainder by zero" });
        if !signed {
            return match op {
                BinOp::Div => builder.build_int_unsigned_div(left, right, ""),
                _ => builder.build_int_unsigned_rem(left, right, ""),
            }
            .expect("udiv")
            .into();
        }
        let minus_one = builder.build_int_compare(IntPredicate::EQ, right, ty.const_all_ones(), "").expect("icmp");
        let min = ty.const_int(1u64 << (ty.get_bit_width() - 1), false);
        let is_min = builder.build_int_compare(IntPredicate::EQ, left, min, "").expect("icmp");
        let overflow = builder.build_and(minus_one, is_min, "").expect("and");
        if op == BinOp::Div {
            self.panic_if(frame.function, overflow, "attempt to divide with overflow");
            return builder.build_int_signed_div(left, right, "").expect("sdiv").into();
        }
        let safe_right = builder.build_select(minus_one, ty.const_int(1, false), right, "").expect("select").into_int_value();
        let remainder = builder.build_int_signed_rem(left, safe_right, "").expect("srem");
        builder.build_select(minus_one, ty.const_zero(), remainder, "").expect("select")
    }

    fn math(&self, op: MathOp, values: &[BasicValueEnum<'ctx>], width: FloatWidth) -> BasicValueEnum<'ctx> {
        if !matches!(width, FloatWidth::F64 | FloatWidth::F32) {
            let wide: Vec<_> = values.iter().map(|value| self.float_to_f64(*value, width)).collect();
            let result = self.math(op, &wide, FloatWidth::F64);
            return self.f64_to_float(result, width);
        }
        let intrinsic = match op {
            MathOp::Sqrt => Some("llvm.sqrt"),
            MathOp::Abs => Some("llvm.fabs"),
            MathOp::Min => Some("llvm.minimum"),
            MathOp::Max => Some("llvm.maximum"),
            _ => None,
        };
        if let Some(name) = intrinsic {
            let function = Intrinsic::find(name)
                .expect("float intrinsic")
                .get_declaration(self.module, &[values[0].get_type()])
                .expect("float intrinsic declaration");
            return self.call(function, values).expect("float");
        }
        let symbol = op.runtime_symbol().expect("a runtime math function");
        let wide: Vec<_> = values.iter().map(|value| self.float_to_f64(*value, width)).collect();
        let result = self.call_runtime(symbol, &wide, Some(self.context.f64_type().into())).expect("f64");
        self.f64_to_float(result, width)
    }

    fn float_to_f64(&self, value: BasicValueEnum<'ctx>, width: FloatWidth) -> BasicValueEnum<'ctx> {
        let f64_ty = self.context.f64_type();
        match width {
            FloatWidth::F64 => value,
            FloatWidth::F32 => self.builder.build_float_ext(value.into_float_value(), f64_ty, "").expect("fpext").into(),
            small => {
                let i32_ty = self.context.i32_type();
                let bits = self.builder.build_int_z_extend(value.into_int_value(), i32_ty, "").expect("zext");
                let code = i32_ty.const_int(small_float_code(small), false);
                self.call_runtime("paco_float_to_f64", &[bits.into(), code.into()], Some(f64_ty.into())).expect("f64")
            }
        }
    }

    fn f64_to_float(&self, value: BasicValueEnum<'ctx>, width: FloatWidth) -> BasicValueEnum<'ctx> {
        match width {
            FloatWidth::F64 => value,
            FloatWidth::F32 => {
                self.builder.build_float_trunc(value.into_float_value(), self.context.f32_type(), "").expect("fptrunc").into()
            }
            small => {
                let i32_ty = self.context.i32_type();
                let code = i32_ty.const_int(small_float_code(small), false);
                let bits = self
                    .call_runtime("paco_float_from_f64", &[value, code.into()], Some(i32_ty.into()))
                    .expect("bits")
                    .into_int_value();
                let storage = self.float_storage(small).into_int_type();
                self.builder.build_int_truncate(bits, storage, "").expect("trunc").into()
            }
        }
    }

    /// Int narrowing truncates, widening extends by the source's sign;
    /// float-to-int saturates.
    fn cast(&self, value: BasicValueEnum<'ctx>, source: &Type, target: &Type) -> BasicValueEnum<'ctx> {
        let builder = &self.builder;
        let target_llvm = self.llvm_ty(target).expect("cast target is never unit-typed");
        let signed = |ty: &Type| matches!(ty, Type::Int(width) if width.is_signed());
        if let (Type::Float(from), Type::Float(to)) = (source, target) {
            if from == to {
                return value;
            }
            let wide = self.float_to_f64(value, *from);
            return self.f64_to_float(wide, *to);
        }
        if let Type::Float(from) = source {
            let wide = self.float_to_f64(value, *from);
            let name = if signed(target) { "llvm.fptosi.sat" } else { "llvm.fptoui.sat" };
            let intrinsic = Intrinsic::find(name)
                .expect("saturating cast")
                .get_declaration(self.module, &[target_llvm, self.context.f64_type().into()])
                .expect("saturating cast declaration");
            return self.call(intrinsic, &[wide]).expect("int");
        }
        if let Type::Float(to) = target {
            let f64_ty = self.context.f64_type();
            let int = value.into_int_value();
            let wide = if signed(source) {
                builder.build_signed_int_to_float(int, f64_ty, "")
            } else {
                builder.build_unsigned_int_to_float(int, f64_ty, "")
            }
            .expect("int to float");
            return self.f64_to_float(wide.into(), *to);
        }
        let int = value.into_int_value();
        let target_int = target_llvm.into_int_type();
        let (from_bits, to_bits) = (int.get_type().get_bit_width(), target_int.get_bit_width());
        if from_bits == to_bits {
            value
        } else if to_bits < from_bits {
            builder.build_int_truncate(int, target_int, "").expect("trunc").into()
        } else if signed(source) {
            builder.build_int_s_extend(int, target_int, "").expect("sext").into()
        } else {
            builder.build_int_z_extend(int, target_int, "").expect("zext").into()
        }
    }

    fn terminator(&self, frame: &Frame<'ctx, '_>, terminator: &Terminator) {
        let builder = &self.builder;
        match terminator {
            Terminator::Goto(target) => {
                builder.build_unconditional_branch(frame.blocks[block_index(*target)]).expect("br");
            }
            Terminator::SwitchInt { discriminant, targets, otherwise } => {
                let value = self.operand(frame, discriminant).into_int_value();
                let cases: Vec<_> = targets
                    .iter()
                    .map(|(case, target)| (value.get_type().const_int(*case as u64, true), frame.blocks[block_index(*target)]))
                    .collect();
                builder.build_switch(value, frame.blocks[block_index(*otherwise)], &cases).expect("switch");
            }
            Terminator::Call { target, args, destination, resume } => {
                let name = target.0.as_str();
                if name == paco_mir::PANIC_SYMBOL {
                    let message = self.operand(frame, &args[0]);
                    self.panic_call(name, &[message]);
                    return;
                }
                if name == "print" {
                    self.print(frame, args);
                } else if self.builtin(frame, name, args, destination.as_ref()) {
                } else if name == "slice_of_zeros" {
                    self.slice_of_zeros(frame, args, destination.as_ref().expect("returns a value"));
                } else if name == "slice_as_ptr" || name == "slice_as_mut_ptr" {
                    let base = self.as_ptr(self.operand(frame, &args[0]));
                    let data = self.load(self.ptr().into(), base, i64::from(SLICE_DATA_OFFSET));
                    self.write_place(frame, destination.as_ref().expect("returns a value"), data);
                } else {
                    let function = self.module.get_function(name).unwrap_or_else(|| panic!("undeclared function `{name}`"));
                    let mut values = Vec::with_capacity(args.len() + 1);
                    if let Some(place) = destination {
                        let result_ty = self.place_ty(frame, place);
                        if self.is_real_aggregate(&result_ty) {
                            let (size, align) = self.size_align(&result_ty);
                            values.push(self.entry_alloca(frame.entry, size, align).into());
                        }
                    }
                    let params = self.params.get(name);
                    for (index, arg) in args.iter().enumerate() {
                        let param_ty = params.and_then(|params| params.get(index));
                        let auto_ref = matches!(param_ty, Some(Type::Borrow { .. }))
                            && !matches!(self.operand_ty(frame, arg), Type::Borrow { .. });
                        values.push(match param_ty.filter(|_| !auto_ref) {
                            Some(param_ty) => self.owned_as(frame, arg, param_ty),
                            None => self.operand(frame, arg),
                        });
                    }
                    let result = self.call(function, &values);
                    if let Some(place) = destination {
                        self.write_place(frame, place, result.expect("call returns a value"));
                    }
                }
                builder.build_unconditional_branch(frame.blocks[block_index(*resume)]).expect("br");
            }
            Terminator::CallIndirect { callee, args, destination, resume } => {
                let callee = self.as_ptr(self.operand(frame, callee));
                let result_ty = destination.as_ref().map(|place| self.place_ty(frame, place));
                let mut values: Vec<BasicValueEnum<'ctx>> = Vec::with_capacity(args.len() + 1);
                if let Some(result_ty) = &result_ty
                    && self.is_real_aggregate(result_ty)
                {
                    let (size, align) = self.size_align(result_ty);
                    values.push(self.entry_alloca(frame.entry, size, align).into());
                }
                for (index, arg) in args.iter().enumerate() {
                    values.push(if index == 0 { self.operand(frame, arg) } else { self.owned_operand(frame, arg) });
                }
                let param_tys: Vec<_> = values.iter().map(|value| value.get_type()).collect();
                let fn_type = self.fn_type(&param_tys, result_ty.as_ref().and_then(|ty| self.llvm_ty(ty)));
                let args: Vec<BasicMetadataValueEnum<'ctx>> = values.iter().map(|value| (*value).into()).collect();
                let call = builder.build_indirect_call(fn_type, callee, &args, "").expect("call");
                if let Some(place) = destination {
                    let result = call.try_as_basic_value().basic().expect("indirect call returns a value");
                    self.write_place(frame, place, result);
                }
                builder.build_unconditional_branch(frame.blocks[block_index(*resume)]).expect("br");
            }
            Terminator::Return(operand) => {
                let return_ty = &frame.body.return_ty;
                match (self.llvm_ty(return_ty), operand) {
                    (Some(_), Operand::Constant(Constant::Unit)) => self.trap(),
                    (Some(_), operand) if self.is_real_aggregate(return_ty) => {
                        let value = self.owned_as(frame, operand, return_ty);
                        let sret = frame.sret.expect("aggregate-returning function has an sret address");
                        let (size, _) = self.size_align(return_ty);
                        self.memcpy(sret, self.as_ptr(value), size);
                        self.drop_all_owned(frame);
                        builder.build_return(Some(&sret)).expect("ret");
                    }
                    (Some(ty), operand) => {
                        let value = self.coerce(self.owned_as(frame, operand, return_ty), ty);
                        self.drop_all_owned(frame);
                        builder.build_return(Some(&value)).expect("ret");
                    }
                    (None, _) => {
                        self.drop_all_owned(frame);
                        builder.build_return(None).expect("ret");
                    }
                }
            }
            Terminator::Unreachable => self.trap(),
        }
    }

    fn print(&self, frame: &Frame<'ctx, '_>, args: &[Operand]) {
        let ty = self.operand_ty(frame, &args[0]);
        let mut value = self.operand(frame, &args[0]);
        let inner = strip_borrow(&ty).clone();
        if matches!(ty, Type::Borrow { .. }) && !self.is_real_aggregate(&inner) {
            value = self.load(self.llvm_ty(&inner).expect("printed values are never unit"), self.as_ptr(value), 0);
        }
        let mut code = None;
        let name = match inner {
            Type::Float(width) => {
                value = self.float_to_f64(value, width);
                code = Some(self.context.i32_type().const_int(float_format_code(width), false).into());
                "paco_print_float"
            }
            Type::Bool => "paco_print_bool",
            Type::Char => "paco_print_char",
            Type::String => "paco_print_str",
            Type::Int(IntWidth::U64) => "paco_print_uint",
            Type::Int(width) => {
                if width.bytes() < 8 {
                    let int = value.into_int_value();
                    value = if width.is_signed() {
                        self.builder.build_int_s_extend(int, self.i64(), "")
                    } else {
                        self.builder.build_int_z_extend(int, self.i64(), "")
                    }
                    .expect("extend")
                    .into();
                }
                "paco_print_int"
            }
            other => panic!("`print` codegen is not implemented for {other:?}"),
        };
        let args: Vec<BasicValueEnum<'ctx>> = std::iter::once(value).chain(code).collect();
        self.call_runtime(name, &args, None);
    }

    fn string_binary(
        &self,
        frame: &Frame<'ctx, '_>,
        op: BinOp,
        left: BasicValueEnum<'ctx>,
        right: BasicValueEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let (left, right) = (self.as_ptr(left).into(), self.as_ptr(right).into());
        match op {
            BinOp::Eq | BinOp::Ne => {
                let equal = self
                    .call_runtime("paco_string_eq", &[left, right], Some(self.i8().into()))
                    .expect("bool")
                    .into_int_value();
                if op == BinOp::Eq {
                    return equal.into();
                }
                let bit = self.builder.build_int_compare(IntPredicate::EQ, equal, self.i8().const_zero(), "").expect("icmp");
                self.bool_value(bit)
            }
            BinOp::Add => {
                let out = self.entry_alloca(frame.entry, 16, 8);
                self.call_runtime("paco_string_concat", &[left, right, out.into()], None);
                out.into()
            }
            other => panic!("operator `{other:?}` is not supported over strings"),
        }
    }

    fn builtin(&self, frame: &Frame<'ctx, '_>, name: &str, args: &[Operand], destination: Option<&Place>) -> bool {
        if let Some(op) = name.strip_prefix("$cell::") {
            self.cell(frame, op, args, destination);
            return true;
        }
        if let Some(entry) = network_entry(name) {
            let first = if name == "tcp_listen" { self.operand(frame, &args[0]) } else { self.cell_pointer(frame, &args[0]).into() };
            match name {
                "tcp_listen" | "TcpListener::accept" => {
                    let handle = self.call_runtime(entry, &[first], Some(self.ptr().into())).expect("a handle");
                    self.write_place(frame, destination.expect("returns a handle"), handle);
                }
                "TcpStream::read" => {
                    let max_len = self.operand(frame, &args[1]);
                    let out = self.entry_alloca(frame.entry, 16, 8);
                    self.call_runtime(entry, &[first, max_len, out.into()], None);
                    self.write_place(frame, destination.expect("returns a string"), out.into());
                }
                _ => {
                    let text = self.as_ptr(self.operand(frame, &args[1]));
                    self.call_runtime(entry, &[first, text.into()], None);
                }
            }
            return true;
        }
        if !matches!(
            name,
            "string_len_bytes"
                | "string_next_char_boundary"
                | "string_char_at"
                | "string_byte_at"
                | "string_slice_utf8"
                | "fs_read_to_string"
                | "stderr_write"
                | "string_concat"
                | "int_to_string"
                | "uint_to_string"
                | "bool_to_string"
                | "float_to_string"
                | "char_to_string"
                | "arg_count"
                | "arg_at"
                | "string_to_bytes"
                | "string_from_bytes"
                | "bytes_write_string"
                | "string_hash"
                | "slice_sort"
        ) {
            return false;
        }
        let runtime_name = format!("paco_{name}");
        let mut values = Vec::with_capacity(args.len() + 1);
        for arg in args {
            let value = self.operand(frame, arg);
            if let Type::Float(width) = self.operand_ty(frame, arg) {
                values.push(self.float_to_f64(value, width));
                values.push(self.context.i32_type().const_int(float_format_code(width), false).into());
            } else {
                values.push(value);
            }
        }
        let destination = || destination.expect("returns a value");
        match name {
            "string_len_bytes" => {
                let len = self.load(self.i64().into(), self.as_ptr(values[0]), i64::from(SLICE_LEN_OFFSET));
                self.write_place(frame, destination(), len);
            }
            "string_next_char_boundary" | "arg_count" | "string_hash" => {
                let next = self.call_runtime(&runtime_name, &values, Some(self.i64().into())).expect("i64");
                self.write_place(frame, destination(), next);
            }
            "bytes_write_string" => {
                let written = self.call_runtime(&runtime_name, &values, Some(self.i8().into())).expect("i8");
                self.write_place(frame, destination(), written);
            }
            "string_char_at" | "string_byte_at" | "string_slice_utf8" | "fs_read_to_string" | "string_from_bytes" => {
                self.option_builtin(frame, &runtime_name, values, destination());
            }
            "stderr_write" | "slice_sort" => {
                self.call_runtime(&runtime_name, &values, None);
            }
            _ => {
                let out = self.entry_alloca(frame.entry, 16, 8);
                values.push(out.into());
                self.call_runtime(&runtime_name, &values, None);
                self.write_place(frame, destination(), out.into());
            }
        }
        true
    }

    fn option_builtin(&self, frame: &Frame<'ctx, '_>, runtime_name: &str, mut values: Vec<BasicValueEnum<'ctx>>, destination: &Place) {
        let (name, args) = enum_name_of(&self.place_ty(frame, destination));
        let layout = self.layouts.enum_layout(&name, &args);
        let address = self.entry_alloca(frame.entry, layout.size, layout.align);
        let (_, payload) = self.layouts.enum_variant_field(&name, &args, "Some", 0);
        values.push(self.offset(address, payload as i64).into());
        let found = self
            .call_runtime(runtime_name, &values, Some(self.context.i32_type().into()))
            .expect("status")
            .into_int_value();
        let some = self.int(self.layouts.enum_variant_index(&name, &args, "Some") as i64);
        let none = self.int(self.layouts.enum_variant_index(&name, &args, "None") as i64);
        let is_found = self.builder.build_int_compare(IntPredicate::NE, found, found.get_type().const_zero(), "").expect("icmp");
        let tag = self.builder.build_select(is_found, some, none, "").expect("select");
        self.store(address, 0, tag);
        self.write_place(frame, destination, address.into());
    }

    fn value_size(&self, ty: &Type) -> u64 {
        match ty {
            Type::Unit => 0,
            Type::Struct(name, _) if !self.layouts.has_struct(name) => 8,
            _ if self.is_real_aggregate(ty) => self.size_align(ty).0,
            _ => paco_mir::scalar_layout(ty).unwrap_or_else(|| panic!("no layout for {ty:?}")).size,
        }
    }

    fn cell_pointer(&self, frame: &Frame<'ctx, '_>, operand: &Operand) -> PointerValue<'ctx> {
        let value = self.as_ptr(self.operand(frame, operand));
        if matches!(self.operand_ty(frame, operand), Type::Borrow { .. }) {
            self.as_ptr(self.load(self.ptr().into(), value, 0))
        } else {
            value
        }
    }

    fn lock_cell(&self, frame: &Frame<'ctx, '_>, cell: PointerValue<'ctx>) {
        let lock = self.offset(cell, i64::from(CELL_LOCK_OFFSET));
        let spin = self.block(frame.function);
        let acquired = self.block(frame.function);
        self.builder.build_unconditional_branch(spin).expect("br");
        self.builder.position_at_end(spin);
        let exchange = self
            .builder
            .build_cmpxchg(
                lock,
                self.int(0),
                self.int(1),
                AtomicOrdering::SequentiallyConsistent,
                AtomicOrdering::SequentiallyConsistent,
            )
            .expect("cmpxchg");
        let success = self.builder.build_extract_value(exchange, 1, "").expect("success").into_int_value();
        self.builder.build_conditional_branch(success, acquired, spin).expect("br");
        self.builder.position_at_end(acquired);
    }

    fn unlock_cell(&self, cell: PointerValue<'ctx>) {
        let lock = self.offset(cell, i64::from(CELL_LOCK_OFFSET));
        let store = self.builder.build_store(lock, self.int(0)).expect("store");
        store.set_alignment(8).expect("alignment");
        store.set_atomic_ordering(AtomicOrdering::SequentiallyConsistent).expect("atomic");
    }

    pub(crate) fn atomic_add(&self, address: PointerValue<'ctx>, delta: i64) -> IntValue<'ctx> {
        self.builder
            .build_atomicrmw(AtomicRMWBinOp::Add, address, self.int(delta), AtomicOrdering::SequentiallyConsistent)
            .expect("atomicrmw")
    }

    fn cell(&self, frame: &Frame<'ctx, '_>, op: &str, args: &[Operand], destination: Option<&Place>) {
        if op == "new" {
            let ty = self.operand_ty(frame, &args[0]);
            let size = i64::from(CELL_VALUE_OFFSET) + self.value_size(&ty) as i64;
            let cell = self
                .call_runtime("paco_calloc", &[self.int(1).into(), self.int(size).into()], Some(self.i64().into()))
                .expect("paco_calloc");
            let cell = self.as_ptr(cell);
            self.store(cell, i64::from(CELL_COUNT_OFFSET), self.int(1).into());
            if ty != Type::Unit {
                let value = self.owned_operand(frame, &args[0]);
                self.store_field(value, &ty, cell, i64::from(CELL_VALUE_OFFSET));
            }
            self.write_place(frame, destination.expect("`new` returns a value"), cell.into());
            return;
        }
        let cell = self.cell_pointer(frame, &args[0]);
        match op {
            "get" => {
                self.lock_cell(frame, cell);
                if let Some(place) = destination {
                    let ty = self.place_ty(frame, place);
                    let value = if self.is_real_aggregate(&ty) {
                        let (size, align) = self.size_align(&ty);
                        let copy = self.entry_alloca(frame.entry, size, align);
                        self.memcpy(copy, self.offset(cell, i64::from(CELL_VALUE_OFFSET)), size);
                        copy.into()
                    } else {
                        self.load(self.llvm_ty(&ty).expect("non-unit"), cell, i64::from(CELL_VALUE_OFFSET))
                    };
                    let value = self.clone_value(frame, &ty, value);
                    self.unlock_cell(cell);
                    self.write_place(frame, place, value);
                } else {
                    self.unlock_cell(cell);
                }
            }
            "set" => {
                let ty = self.operand_ty(frame, &args[1]);
                if ty != Type::Unit {
                    let value = self.owned_operand(frame, &args[1]);
                    self.lock_cell(frame, cell);
                    let old = self.glue.contains_key(&ty).then(|| {
                        let size = self.value_size(&ty);
                        let old = self.entry_alloca(frame.entry, size.max(8), 8);
                        self.memcpy(old, self.offset(cell, i64::from(CELL_VALUE_OFFSET)), size);
                        old
                    });
                    self.store_field(value, &ty, cell, i64::from(CELL_VALUE_OFFSET));
                    self.unlock_cell(cell);
                    if let (Some(old), Some(&(drop, _))) = (old, self.glue.get(&ty)) {
                        self.call(drop, &[old.into()]);
                    }
                }
            }
            "clone" => {
                self.atomic_add(cell, 1);
                self.write_place(frame, destination.expect("`clone` returns a value"), cell.into());
            }
            "strong_count" => {
                let load = self.builder.build_load(self.i64(), cell, "").expect("load");
                let instruction = load.as_instruction_value().expect("load instruction");
                instruction.set_alignment(8).expect("alignment");
                instruction.set_atomic_ordering(AtomicOrdering::SequentiallyConsistent).expect("atomic");
                self.write_place(frame, destination.expect("`strong_count` returns a value"), load);
            }
            other => panic!("unknown shared-cell operation `{other}`"),
        }
    }

    fn slice_of_zeros(&self, frame: &Frame<'ctx, '_>, args: &[Operand], destination: &Place) {
        let elem_ty = slice_elem_of(&self.place_ty(frame, destination));
        let size = element_size(&elem_ty, self.layouts);
        let len = self.coerce(self.operand(frame, &args[0]), self.i64().into());
        let data = self
            .call_runtime("paco_calloc", &[len, self.int(size as i64).into()], Some(self.i64().into()))
            .expect("paco_calloc");
        let descriptor = self.entry_alloca(frame.entry, 16, 8);
        self.store(descriptor, i64::from(SLICE_DATA_OFFSET), self.as_ptr(data).into());
        self.store(descriptor, i64::from(SLICE_LEN_OFFSET), len);
        self.write_place(frame, destination, descriptor.into());
    }
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

fn float_format_code(width: FloatWidth) -> u64 {
    match width {
        FloatWidth::F32 => 4,
        FloatWidth::F64 => 5,
        small => small_float_code(small),
    }
}

fn small_float_code(width: FloatWidth) -> u64 {
    match width {
        FloatWidth::F16 => 0,
        FloatWidth::BF16 => 1,
        FloatWidth::F8E4M3 => 2,
        FloatWidth::F8E5M2 => 3,
        FloatWidth::F64 | FloatWidth::F32 => unreachable!("not a small float"),
    }
}

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

fn strip_borrow(ty: &Type) -> &Type {
    match ty {
        Type::Borrow { ty, .. } => strip_borrow(ty),
        other => other,
    }
}

fn slice_elem_of(ty: &Type) -> Type {
    match ty {
        Type::Slice(elem) => elem.as_ref().clone(),
        Type::Borrow { ty, .. } => slice_elem_of(ty),
        other => panic!("expected a slice type, found {other:?}"),
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

fn block_index(id: BasicBlockId) -> usize {
    id.0 as usize
}

fn body_strings(body: &Body) -> Vec<String> {
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
            Terminator::SwitchInt { discriminant: operand, .. } | Terminator::Return(operand) => {
                operand_strings(operand, &mut out)
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

fn operand_strings(operand: &Operand, out: &mut Vec<String>) {
    match operand {
        Operand::Constant(Constant::Str(text)) => out.push(text.clone()),
        Operand::Copy(place) | Operand::Move(place) => place_strings(place, out),
        Operand::Constant(_) => {}
    }
}

fn place_strings(place: &Place, out: &mut Vec<String>) {
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

fn rvalue_strings(rvalue: &Rvalue, out: &mut Vec<String>) {
    match rvalue {
        Rvalue::Use(operand) | Rvalue::UnaryOp(_, operand) | Rvalue::Cast { operand, .. } | Rvalue::Load { address: operand, .. } => {
            operand_strings(operand, out)
        }
        Rvalue::BinaryOp(_, left, right) => {
            operand_strings(left, out);
            operand_strings(right, out);
        }
        Rvalue::Aggregate { fields, .. } | Rvalue::Math(_, fields) => fields.iter().for_each(|field| operand_strings(field, out)),
        Rvalue::Ref { place, .. } | Rvalue::Discriminant(place) | Rvalue::SliceLen(place) => place_strings(place, out),
        Rvalue::RawAlloc { .. } | Rvalue::FuncAddr(_) | Rvalue::Quote { .. } => {}
    }
}

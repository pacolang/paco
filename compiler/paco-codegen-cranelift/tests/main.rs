mod aggregates;
mod borrow_of_scalar;
mod channel_ffi;
mod iter_fn_ffi;
mod jit_arithmetic;
mod object_emission;
mod overflow_profile;
mod runtime_allocator;
mod select_ffi;
mod slice_index;
mod spawn_ffi;

static RUNTIME_INIT: std::sync::Once = std::sync::Once::new();

fn ensure_runtime_init() {
    RUNTIME_INIT.call_once(|| paco_runtime_ffi::paco_rt_init());
}

fn with_panic_symbols(mut builder: cranelift_jit::JITBuilder) -> cranelift_jit::JITBuilder {
    builder.symbol("paco_rt_panic", paco_runtime_ffi::paco_rt_panic as *const u8);
    builder.symbol("paco_rt_panic_str", paco_runtime_ffi::paco_rt_panic_str as *const u8);
    builder.symbol("paco_rt_panic_bounds", paco_runtime_ffi::paco_rt_panic_bounds as *const u8);
    builder
}

fn symbol_names<'data>(
    object: &object::File<'data>,
    keep: impl Fn(&object::Symbol<'data, '_>) -> bool,
) -> Vec<&'data str> {
    use object::{Object, ObjectSymbol};
    let macho = object.format() == object::BinaryFormat::MachO;
    object
        .symbols()
        .filter(|symbol| keep(symbol))
        .filter_map(|symbol| symbol.name().ok())
        .map(|name| if macho { name.strip_prefix('_').unwrap_or(name) } else { name })
        .collect()
}

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use paco_mir::glue::{
    CELL_COUNT_OFFSET, CELL_VALUE_OFFSET, CLOSURE_DROP_FN, CLOSURE_HEADER, SLICE_DATA_OFFSET, SLICE_LEN_OFFSET,
    children, element_size, glue_fields, handle_fns, is_cell, unresolved,
};
use paco_mir::{
    BinOp, Body, ComptimeValue, Constant, Local, Operand, Place, Profile, Rvalue, Statement, Terminator, TypeLayouts,
    UnOp,
};
use paco_span::Span;
use paco_syntax::ast::QuoteBody;
use paco_types::{FloatWidth, IntWidth, Type};

use crate::{Bodies, Error, Limits, Program};

const FUNCTION_TAG: u64 = 1 << 62;

struct Allocation {
    bytes: Vec<u8>,
    freed: bool,
    fixed: bool,
}

/// Byte-addressed memory: an address is `(allocation index + 1) << 32 |
/// offset`, so pointer arithmetic in MIR works unchanged.
#[derive(Default)]
struct Memory {
    allocations: Vec<Allocation>,
}

impl Memory {
    fn alloc(&mut self, size: usize, fixed: bool) -> u64 {
        self.allocations.push(Allocation { bytes: vec![0; size], freed: false, fixed });
        (self.allocations.len() as u64) << 32
    }

    fn locate(&self, address: u64, len: usize) -> Result<(usize, usize), Error> {
        let index = (address >> 32) as usize;
        let offset = (address & 0xFFFF_FFFF) as usize;
        let allocation = index
            .checked_sub(1)
            .and_then(|index| self.allocations.get(index))
            .filter(|allocation| !allocation.freed)
            .ok_or_else(|| Error::new(format!("invalid memory access at {address:#x}")))?;
        if offset + len > allocation.bytes.len() {
            return Err(Error::new(format!("out-of-bounds memory access at {address:#x} ({len} bytes)")));
        }
        Ok((index - 1, offset))
    }

    fn read(&self, address: u64, len: usize) -> Result<Vec<u8>, Error> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let (index, offset) = self.locate(address, len)?;
        Ok(self.allocations[index].bytes[offset..offset + len].to_vec())
    }

    fn write(&mut self, address: u64, bytes: &[u8]) -> Result<(), Error> {
        if bytes.is_empty() {
            return Ok(());
        }
        let (index, offset) = self.locate(address, bytes.len())?;
        self.allocations[index].bytes[offset..offset + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }

    fn read_u64(&self, address: u64) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.read(address, 8)?.try_into().expect("eight bytes")))
    }

    fn write_u64(&mut self, address: u64, value: u64) -> Result<(), Error> {
        self.write(address, &value.to_le_bytes())
    }

    fn free(&mut self, address: u64) -> Result<(), Error> {
        if address == 0 {
            return Ok(());
        }
        let (index, offset) = self.locate(address, 0)?;
        let allocation = &mut self.allocations[index];
        if offset != 0 || allocation.fixed {
            return Err(Error::new(format!("freeing memory that was not allocated at {address:#x}")));
        }
        allocation.freed = true;
        allocation.bytes = Vec::new();
        Ok(())
    }
}

struct Frame {
    body: Rc<Body>,
    locals: Vec<u64>,
    flags: Vec<bool>,
    borrowed: Rc<HashSet<u32>>,
    scratch: Vec<u64>,
    span: Span,
}

pub struct Machine<'p, 'b> {
    program: &'p Program<'p>,
    bodies: &'b mut dyn Bodies,
    layouts: &'p TypeLayouts<'p>,
    limits: Limits,
    memory: Memory,
    types: Vec<Type>,
    pub(crate) codes: Vec<QuoteBody>,
    functions: Vec<String>,
    function_ids: HashMap<String, u64>,
    statics: HashMap<String, u64>,
    pub output: String,
    pub stderr: String,
    executed: usize,
    depth: usize,
    drops: HashMap<Type, bool>,
    borrowed: HashMap<String, Rc<HashSet<u32>>>,
    frames: Vec<Frame>,
}

fn int_value(bytes: &[u8], width: IntWidth) -> i128 {
    let mut raw = [0u8; 8];
    raw[..bytes.len()].copy_from_slice(bytes);
    let bits = u64::from_le_bytes(raw);
    let size = width.bytes() * 8;
    if width.is_signed() {
        let shift = 64 - size;
        i128::from(((bits << shift) as i64) >> shift)
    } else {
        i128::from(if size == 64 { bits } else { bits & ((1 << size) - 1) })
    }
}

fn int_bytes(value: i128, bytes: u64) -> Vec<u8> {
    (value as u64).to_le_bytes()[..bytes as usize].to_vec()
}

fn small_code(width: FloatWidth) -> Option<i32> {
    match width {
        FloatWidth::F16 => Some(0),
        FloatWidth::BF16 => Some(1),
        FloatWidth::F8E4M3 => Some(2),
        FloatWidth::F8E5M2 => Some(3),
        FloatWidth::F32 | FloatWidth::F64 => None,
    }
}

fn format_code(width: FloatWidth) -> i32 {
    small_code(width).unwrap_or(if width == FloatWidth::F32 { paco_runtime::FLOAT_CODE_F32 } else { paco_runtime::FLOAT_CODE_F64 })
}

fn float_value(bytes: &[u8], width: FloatWidth) -> f64 {
    match width {
        FloatWidth::F64 => f64::from_le_bytes(bytes.try_into().expect("eight bytes")),
        FloatWidth::F32 => f64::from(f32::from_le_bytes(bytes.try_into().expect("four bytes"))),
        small => {
            let mut raw = [0u8; 4];
            raw[..bytes.len()].copy_from_slice(bytes);
            paco_runtime::float_to_f64(u32::from_le_bytes(raw), small_code(small).expect("small float"))
        }
    }
}

fn float_bytes(value: f64, width: FloatWidth) -> Vec<u8> {
    match width {
        FloatWidth::F64 => value.to_le_bytes().to_vec(),
        FloatWidth::F32 => (value as f32).to_le_bytes().to_vec(),
        small => {
            let bits = paco_runtime::float_from_f64(value, small_code(small).expect("small float"));
            bits.to_le_bytes()[..small.bytes() as usize].to_vec()
        }
    }
}

fn strip_borrow(ty: &Type) -> &Type {
    match ty {
        Type::Borrow { ty, .. } => strip_borrow(ty),
        other => other,
    }
}

fn slice_elem(ty: &Type) -> Type {
    match strip_borrow(ty) {
        Type::Slice(elem) => elem.as_ref().clone(),
        other => panic!("expected a slice type, found {other:?}"),
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

fn not_allowed(operation: &str) -> Error {
    Error::new(format!("{operation} is not allowed inside `comptime`"))
}

impl<'p, 'b> Machine<'p, 'b> {
    pub fn new(program: &'p Program<'p>, bodies: &'b mut dyn Bodies, limits: Limits) -> Self {
        Self {
            program,
            bodies,
            layouts: program.layouts,
            limits,
            memory: Memory::default(),
            types: Vec::new(),
            codes: Vec::new(),
            functions: Vec::new(),
            function_ids: HashMap::new(),
            statics: HashMap::new(),
            output: String::new(),
            stderr: String::new(),
            executed: 0,
            depth: 0,
            drops: HashMap::new(),
            borrowed: HashMap::new(),
            frames: Vec::new(),
        }
    }

    /// Calls `entry` and reads its result back as a value.
    pub fn run(&mut self, entry: &str, args: &[(Type, ComptimeValue)]) -> Result<ComptimeValue, Error> {
        let body = self.body(entry)?;
        let mut values = Vec::with_capacity(args.len());
        for (ty, value) in args {
            values.push(self.encode(ty, value)?);
        }
        let result = self.call(entry, values)?;
        self.decode(&body.return_ty, &result)
    }

    fn body(&mut self, name: &str) -> Result<Rc<Body>, Error> {
        self.bodies.body(name).ok_or_else(|| Error::new(format!("`{name}` has no body to evaluate at compile time")))
    }

    fn frame(&self) -> &Frame {
        self.frames.last().expect("a frame is running")
    }

    fn frame_mut(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("a frame is running")
    }

    fn fail<T>(&self, message: impl Into<String>) -> Result<T, Error> {
        Err(Error { message: message.into(), span: self.frames.last().map(|frame| frame.span) })
    }

    fn located(&self, mut error: Error) -> Error {
        if error.span.is_none() {
            error.span = self.frames.last().map(|frame| frame.span);
        }
        error
    }

    fn tick(&mut self) -> Result<(), Error> {
        self.executed += 1;
        if self.executed > self.limits.instructions {
            return self.fail(format!("exceeded the comptime instruction budget ({} instructions)", self.limits.instructions));
        }
        Ok(())
    }

    // ---------------------------------------------------------------
    // types and layouts
    // ---------------------------------------------------------------

    fn is_real_aggregate(&self, ty: &Type) -> bool {
        matches!(ty, Type::Struct(name, _) if self.layouts.has_struct(name))
            || matches!(ty, Type::Enum(..) | Type::Tuple(_) | Type::Slice(_) | Type::String)
    }

    fn size_of(&self, ty: &Type) -> usize {
        match ty {
            Type::Unit | Type::Never => 0,
            Type::Struct(name, args) if self.layouts.has_struct(name) => self.layouts.struct_layout(name, args).size as usize,
            Type::Enum(name, args) => self.layouts.enum_layout(name, args).size as usize,
            Type::Tuple(items) => self.layouts.tuple_layout(items).size as usize,
            Type::Slice(_) | Type::String => 16,
            Type::Int(width) => width.bytes() as usize,
            Type::Float(width) => width.bytes() as usize,
            Type::Bool => 1,
            Type::Char => 4,
            _ => 8,
        }
    }

    fn field_of(&self, base: &Type, field: &str) -> (Type, u64) {
        match base {
            Type::Borrow { ty, .. } => self.field_of(ty, field),
            Type::Tuple(items) => self.layouts.tuple_field(items, field.parse().expect("tuple field index")),
            Type::Struct(name, args) => self.layouts.struct_field(name, args, field),
            other => panic!("expected a struct type, found {other:?}"),
        }
    }

    fn variant_field(&self, base: &Type, variant: &str, index: usize) -> (Type, u64) {
        match strip_borrow(base) {
            Type::Enum(name, args) => self.layouts.enum_variant_field(name, args, variant, index),
            other => panic!("expected an enum type, found {other:?}"),
        }
    }

    fn place_ty(&self, place: &Place) -> Type {
        match place {
            Place::Local(local) => self.frame().body.locals[local.0 as usize].ty.clone(),
            Place::Field { base, field } => self.field_of(&self.place_ty(base), field).0,
            Place::VariantField { base, variant, index } => self.variant_field(&self.place_ty(base), variant, *index).0,
            Place::Index { base, .. } => slice_elem(&self.place_ty(base)),
            Place::Deref { ty, .. } => ty.clone(),
        }
    }

    fn operand_ty(&self, operand: &Operand) -> Type {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => self.place_ty(place),
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

    fn user_drop(&mut self, ty: &Type) -> Option<String> {
        let (Type::Struct(owner, args) | Type::Enum(owner, args)) = ty else { return None };
        let name = paco_mir::mangled_name(&format!("{owner}::drop"), args);
        let body = self.bodies.body(&name)?;
        let takes_self = matches!(&body.locals[..body.param_count], [local] if matches!(&local.ty, Type::Borrow { mutable: true, ty: inner } if **inner == *ty));
        (takes_self && body.return_ty == Type::Unit).then_some(name)
    }

    fn needs_drop(&mut self, ty: &Type) -> bool {
        if let Some(known) = self.drops.get(ty) {
            return *known;
        }
        self.drops.insert(ty.clone(), false);
        let result = match ty {
            Type::String | Type::Slice(_) | Type::Fn(..) => true,
            _ if is_cell(ty, self.layouts) || handle_fns(ty, self.layouts).is_some() => true,
            _ if unresolved(ty) => false,
            Type::Struct(name, _) if !self.layouts.has_struct(name) => false,
            Type::Struct(..) | Type::Enum(..) | Type::Tuple(_) => {
                self.user_drop(ty).is_some() || children(ty, self.layouts).iter().any(|child| self.needs_drop(child))
            }
            _ => false,
        };
        self.drops.insert(ty.clone(), result);
        result
    }

    // ---------------------------------------------------------------
    // calls and frames
    // ---------------------------------------------------------------

    fn call(&mut self, name: &str, args: Vec<Vec<u8>>) -> Result<Vec<u8>, Error> {
        let body = self.body(name)?;
        if self.depth >= self.limits.call_depth {
            return self.fail(format!(
                "exceeded the comptime call-depth limit ({} nested calls) — this usually means unbounded recursion",
                self.limits.call_depth
            ));
        }
        let borrowed = self
            .borrowed
            .entry(name.to_string())
            .or_insert_with(|| Rc::new(borrowed_locals(&body)))
            .clone();
        let mut locals = Vec::with_capacity(body.locals.len());
        for local in &body.locals {
            locals.push(self.memory.alloc(self.size_of(&local.ty), false));
        }
        let flags = vec![false; body.locals.len()];
        let span = body.span;
        self.frames.push(Frame { body: body.clone(), locals, flags, borrowed, scratch: Vec::new(), span });
        self.depth += 1;
        let result = self.run_frame(args);
        self.depth -= 1;
        let frame = self.frames.pop().expect("the frame just run");
        for address in frame.locals.into_iter().chain(frame.scratch) {
            self.memory.free(address)?;
        }
        result
    }

    fn run_frame(&mut self, args: Vec<Vec<u8>>) -> Result<Vec<u8>, Error> {
        let body = self.frame().body.clone();
        for (index, value) in args.into_iter().enumerate().take(body.param_count) {
            let address = self.frame().locals[index];
            self.memory.write(address, &value).map_err(|error| self.located(error))?;
            let ty = body.locals[index].ty.clone();
            if self.needs_drop(&ty) {
                self.frame_mut().flags[index] = true;
            }
        }
        let mut block = 0;
        loop {
            let data = &body.blocks[block];
            for (index, statement) in data.statements.iter().enumerate() {
                self.frame_mut().span = body.statement_span(block, index);
                self.tick()?;
                self.statement(statement).map_err(|error| self.located(error))?;
            }
            self.frame_mut().span = body.terminator_span(block);
            self.tick()?;
            match self.terminator(&data.terminator).map_err(|error| self.located(error))? {
                Flow::Goto(next) => block = next,
                Flow::Return(value) => return Ok(value),
            }
        }
    }

    fn scratch(&mut self, size: usize) -> u64 {
        let address = self.memory.alloc(size, false);
        self.frame_mut().scratch.push(address);
        address
    }

    // ---------------------------------------------------------------
    // places and operands
    // ---------------------------------------------------------------

    fn local_address(&self, local: Local) -> u64 {
        self.frame().locals[local.0 as usize]
    }

    fn place_address(&mut self, place: &Place) -> Result<(u64, Type), Error> {
        match place {
            Place::Local(local) => {
                let ty = self.frame().body.locals[local.0 as usize].ty.clone();
                let storage = self.local_address(*local);
                let address = if self.is_real_aggregate(&ty) || self.frame().borrowed.contains(&local.0) {
                    storage
                } else {
                    self.memory.read_u64(storage)?
                };
                Ok((address, ty))
            }
            Place::Field { base, field } => {
                let (base_address, base_ty) = self.place_address(base)?;
                let (ty, offset) = self.field_of(&base_ty, field);
                Ok((base_address + offset, ty))
            }
            Place::VariantField { base, variant, index } => {
                let (base_address, base_ty) = self.place_address(base)?;
                let (ty, offset) = self.variant_field(&base_ty, variant, *index);
                Ok((base_address + offset, ty))
            }
            Place::Index { base, index } => {
                let (base_address, base_ty) = self.place_address(base)?;
                let elem = slice_elem(&base_ty);
                let data = self.memory.read_u64(base_address + SLICE_DATA_OFFSET as u64)?;
                let len = self.memory.read_u64(base_address + SLICE_LEN_OFFSET as u64)?;
                let index_ty = self.operand_ty(index);
                let index = self.int_operand(index, &index_ty)? as u64;
                if index >= len {
                    return self.fail(format!("index {} out of bounds for length {}", index as i64, len as i64));
                }
                Ok((data + index * element_size(&elem, self.layouts), elem))
            }
            Place::Deref { address, ty } => {
                let address = self.pointer(address)?;
                Ok((address, ty.clone()))
            }
        }
    }

    fn read_place(&mut self, place: &Place) -> Result<Vec<u8>, Error> {
        match place {
            Place::Local(local) => {
                let ty = self.frame().body.locals[local.0 as usize].ty.clone();
                self.memory.read(self.local_address(*local), self.size_of(&ty))
            }
            _ => {
                let (address, ty) = self.place_address(place)?;
                self.memory.read(address, self.size_of(&ty))
            }
        }
    }

    fn constant(&mut self, constant: &Constant) -> Result<Vec<u8>, Error> {
        Ok(match constant {
            Constant::Int(value, width) => int_bytes(i128::from(*value), width.bytes()),
            Constant::Float(bits, width) => {
                let value = f64::from_bits(*bits);
                match width {
                    FloatWidth::F64 => value.to_le_bytes().to_vec(),
                    FloatWidth::F32 => (value as f32).to_le_bytes().to_vec(),
                    small => small.encode(value).to_le_bytes()[..small.bytes() as usize].to_vec(),
                }
            }
            Constant::Bool(value) => vec![u8::from(*value)],
            Constant::Char(value) => u32::from(*value).to_le_bytes().to_vec(),
            Constant::Str(text) => {
                let data = match self.statics.get(text) {
                    Some(address) => *address,
                    None => {
                        let address = self.memory.alloc(text.len(), true);
                        self.memory.write(address, text.as_bytes())?;
                        self.statics.insert(text.clone(), address);
                        address
                    }
                };
                let mut descriptor = data.to_le_bytes().to_vec();
                descriptor.extend((text.len() as u64).to_le_bytes());
                descriptor
            }
            Constant::Unit => Vec::new(),
            Constant::Type(ty) => self.type_handle(ty).to_le_bytes().to_vec(),
        })
    }

    fn type_handle(&mut self, ty: &Type) -> u64 {
        match self.types.iter().position(|known| known == ty) {
            Some(index) => index as u64,
            None => {
                self.types.push(ty.clone());
                self.types.len() as u64 - 1
            }
        }
    }

    pub(crate) fn type_of_handle(&self, handle: u64) -> Result<Type, Error> {
        self.types.get(handle as usize).cloned().ok_or_else(|| Error::new("invalid `type` value"))
    }

    pub(crate) fn code_handle(&mut self, code: QuoteBody) -> u64 {
        self.codes.push(code);
        self.codes.len() as u64 - 1
    }

    fn operand(&mut self, operand: &Operand) -> Result<Vec<u8>, Error> {
        match operand {
            Operand::Constant(constant) => self.constant(constant),
            Operand::Copy(place) | Operand::Move(place) => self.read_place(place),
        }
    }

    fn int_operand(&mut self, operand: &Operand, ty: &Type) -> Result<i128, Error> {
        let bytes = self.operand(operand)?;
        Ok(match ty {
            Type::Int(width) => int_value(&bytes, *width),
            _ => int_value(&bytes, IntWidth::U64.min_bytes(bytes.len())),
        })
    }

    fn pointer(&mut self, operand: &Operand) -> Result<u64, Error> {
        let bytes = self.operand(operand)?;
        let mut raw = [0u8; 8];
        raw[..bytes.len().min(8)].copy_from_slice(&bytes[..bytes.len().min(8)]);
        Ok(u64::from_le_bytes(raw))
    }

    /// The address of `operand`'s value: its place, or a scratch copy.
    fn operand_address(&mut self, operand: &Operand) -> Result<u64, Error> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => Ok(self.place_address(place)?.0),
            Operand::Constant(constant) => {
                let bytes = self.constant(constant)?;
                let address = self.scratch(bytes.len());
                self.memory.write(address, &bytes)?;
                Ok(address)
            }
        }
    }

    fn owned_operand(&mut self, operand: &Operand) -> Result<Vec<u8>, Error> {
        let ty = self.operand_ty(operand);
        let value = self.operand(operand)?;
        if !self.needs_drop(&ty) {
            return Ok(value);
        }
        match operand {
            Operand::Move(Place::Local(local)) => {
                self.frame_mut().flags[local.0 as usize] = false;
                Ok(value)
            }
            Operand::Move(Place::VariantField { base, .. })
                if matches!(base.as_ref(), Place::Local(local)
                    if self.frame().body.locals[local.0 as usize].name.as_deref() == Some(paco_mir::TRY_TEMP)) =>
            {
                let Place::Local(local) = base.as_ref() else { unreachable!() };
                self.frame_mut().flags[local.0 as usize] = false;
                Ok(value)
            }
            _ => self.clone_value(&ty, value),
        }
    }

    fn owned_as(&mut self, operand: &Operand, expected: &Type) -> Result<Vec<u8>, Error> {
        if let Type::Borrow { ty: inner, .. } = self.operand_ty(operand)
            && *inner == *expected
        {
            let pointer = self.pointer(operand)?;
            let value = self.memory.read(pointer, self.size_of(expected))?;
            return self.clone_value(expected, value);
        }
        self.owned_operand(operand)
    }

    fn write_place(&mut self, place: &Place, value: &[u8]) -> Result<(), Error> {
        match place {
            Place::Local(local) => {
                self.drop_local(*local)?;
                self.memory.write(self.local_address(*local), value)?;
                let ty = self.frame().body.locals[local.0 as usize].ty.clone();
                if self.needs_drop(&ty) {
                    self.frame_mut().flags[local.0 as usize] = true;
                }
                Ok(())
            }
            _ => {
                let (address, ty) = self.place_address(place)?;
                if self.needs_drop(&ty) {
                    self.drop_at(&ty, address)?;
                }
                self.memory.write(address, value)
            }
        }
    }

    fn write_unowned(&mut self, place: &Place, value: &[u8]) -> Result<(), Error> {
        match place {
            Place::Local(local) => self.memory.write(self.local_address(*local), value),
            _ => {
                let (address, _) = self.place_address(place)?;
                self.memory.write(address, value)
            }
        }
    }

    // ---------------------------------------------------------------
    // drop and clone glue
    // ---------------------------------------------------------------

    fn drop_local(&mut self, local: Local) -> Result<(), Error> {
        if !self.frame().flags[local.0 as usize] {
            return Ok(());
        }
        let ty = self.frame().body.locals[local.0 as usize].ty.clone();
        let address = self.local_address(local);
        self.drop_at(&ty, address)?;
        self.frame_mut().flags[local.0 as usize] = false;
        Ok(())
    }

    fn drop_all_owned(&mut self) -> Result<(), Error> {
        for index in (0..self.frame().flags.len()).rev() {
            self.drop_local(Local(index as u32))?;
        }
        Ok(())
    }

    /// Releases what the `ty` value at `address` owns.
    fn drop_at(&mut self, ty: &Type, address: u64) -> Result<(), Error> {
        if !self.needs_drop(ty) {
            return Ok(());
        }
        if let Some(user) = self.user_drop(ty) {
            let live = match self.layouts.live_offset(ty) {
                Some(offset) => self.memory.read(address + offset, 1)?[0] != 0,
                None => true,
            };
            if live {
                self.call(&user, vec![address.to_le_bytes().to_vec()])?;
            }
        }
        if handle_fns(ty, self.layouts).is_some() {
            return Ok(());
        }
        match ty {
            Type::Fn(..) => {
                let env = self.memory.read_u64(address)?;
                if env == 0 {
                    return Ok(());
                }
                let header = env - CLOSURE_HEADER as u64;
                let count = self.memory.read_u64(header)?;
                self.memory.write_u64(header, count.wrapping_sub(1))?;
                if count == 1 {
                    let drop_fn = self.memory.read_u64(env + CLOSURE_DROP_FN as u64)?;
                    if drop_fn != 0 {
                        let name = self.function_name(drop_fn)?;
                        self.call(&name, vec![env.to_le_bytes().to_vec()])?;
                    }
                    self.memory.free(header)?;
                }
            }
            Type::String | Type::Slice(_) => {
                let data = self.memory.read_u64(address + SLICE_DATA_OFFSET as u64)?;
                if let Type::Slice(elem) = ty
                    && self.needs_drop(elem)
                {
                    let len = self.memory.read_u64(address + SLICE_LEN_OFFSET as u64)?;
                    let size = element_size(elem, self.layouts);
                    for index in 0..len {
                        self.drop_at(elem, data + index * size)?;
                    }
                }
                self.memory.free(data)?;
            }
            _ if is_cell(ty, self.layouts) => {
                let cell = self.memory.read_u64(address)?;
                if cell == 0 {
                    return Ok(());
                }
                let count = self.memory.read_u64(cell + CELL_COUNT_OFFSET as u64)?;
                self.memory.write_u64(cell + CELL_COUNT_OFFSET as u64, count.wrapping_sub(1))?;
                if count == 1 {
                    if let Type::Struct(_, args) = ty
                        && let Some(inner) = args.first()
                    {
                        self.drop_at(inner, cell + CELL_VALUE_OFFSET as u64)?;
                    }
                    self.memory.free(cell)?;
                }
            }
            Type::Enum(name, args) => {
                let tag = self.memory.read_u64(address)?;
                for (index, fields) in self.layouts.enum_variants(name, args) {
                    if index == tag {
                        for (field_ty, offset) in fields {
                            self.drop_at(&field_ty, address + offset)?;
                        }
                    }
                }
            }
            _ => {
                for (field_ty, offset) in glue_fields(ty, self.layouts) {
                    self.drop_at(&field_ty, address + offset)?;
                }
            }
        }
        Ok(())
    }

    /// Makes the fresh bitwise copy of a `ty` value at `address` own its own
    /// heap storage.
    fn clone_at(&mut self, ty: &Type, address: u64) -> Result<(), Error> {
        if !self.needs_drop(ty) || handle_fns(ty, self.layouts).is_some() {
            return Ok(());
        }
        match ty {
            Type::Fn(..) => {
                let env = self.memory.read_u64(address)?;
                if env != 0 {
                    let header = env - CLOSURE_HEADER as u64;
                    let count = self.memory.read_u64(header)?;
                    self.memory.write_u64(header, count + 1)?;
                }
            }
            Type::String | Type::Slice(_) => {
                let data = self.memory.read_u64(address + SLICE_DATA_OFFSET as u64)?;
                let len = self.memory.read_u64(address + SLICE_LEN_OFFSET as u64)?;
                let size = match ty {
                    Type::Slice(elem) => element_size(elem, self.layouts),
                    _ => 1,
                };
                let bytes = self.memory.read(data, (len * size) as usize)?;
                let copy = self.memory.alloc(bytes.len(), false);
                self.memory.write(copy, &bytes)?;
                self.memory.write_u64(address + SLICE_DATA_OFFSET as u64, copy)?;
                if let Type::Slice(elem) = ty {
                    for index in 0..len {
                        self.clone_at(elem, copy + index * size)?;
                    }
                }
            }
            _ if is_cell(ty, self.layouts) => {
                let cell = self.memory.read_u64(address)?;
                if cell != 0 {
                    let count = self.memory.read_u64(cell + CELL_COUNT_OFFSET as u64)?;
                    self.memory.write_u64(cell + CELL_COUNT_OFFSET as u64, count + 1)?;
                }
            }
            Type::Enum(name, args) => {
                let tag = self.memory.read_u64(address)?;
                for (index, fields) in self.layouts.enum_variants(name, args) {
                    if index == tag {
                        for (field_ty, offset) in fields {
                            self.clone_at(&field_ty, address + offset)?;
                        }
                    }
                }
            }
            _ => {
                for (field_ty, offset) in glue_fields(ty, self.layouts) {
                    self.clone_at(&field_ty, address + offset)?;
                }
            }
        }
        Ok(())
    }

    fn clone_value(&mut self, ty: &Type, value: Vec<u8>) -> Result<Vec<u8>, Error> {
        if !self.needs_drop(ty) {
            return Ok(value);
        }
        let address = self.memory.alloc(value.len(), false);
        self.memory.write(address, &value)?;
        self.clone_at(ty, address)?;
        let copy = self.memory.read(address, value.len())?;
        self.memory.free(address)?;
        Ok(copy)
    }

    fn function_pointer(&mut self, name: &str) -> u64 {
        if let Some(id) = self.function_ids.get(name) {
            return *id;
        }
        let id = FUNCTION_TAG | self.functions.len() as u64;
        self.functions.push(name.to_string());
        self.function_ids.insert(name.to_string(), id);
        id
    }

    fn function_name(&self, pointer: u64) -> Result<String, Error> {
        (pointer & FUNCTION_TAG != 0)
            .then(|| self.functions.get((pointer & !FUNCTION_TAG) as usize).cloned())
            .flatten()
            .ok_or_else(|| Error::new(format!("calling an invalid function pointer {pointer:#x}")))
    }

    // ---------------------------------------------------------------
    // statements
    // ---------------------------------------------------------------

    fn statement(&mut self, statement: &Statement) -> Result<(), Error> {
        match statement {
            Statement::Assign(place, rvalue) => {
                if matches!(place, Place::Local(local) if matches!(self.frame().body.locals[local.0 as usize].ty, Type::Unit | Type::Never)) {
                    return Ok(());
                }
                let value = match rvalue {
                    Rvalue::Use(operand) => {
                        let expected = self.place_ty(place);
                        self.owned_as(operand, &expected)?
                    }
                    _ => self.rvalue(rvalue)?,
                };
                if matches!(rvalue, Rvalue::Load { .. }) {
                    self.write_unowned(place, &value)
                } else {
                    self.write_place(place, &value)
                }
            }
            Statement::Store { address, value, ty } => {
                if *ty == Type::Unit {
                    return Ok(());
                }
                let address = self.pointer(address)?;
                let mut value = self.owned_operand(value)?;
                if self.is_real_aggregate(ty) {
                    let boxed = self.memory.alloc(self.size_of(ty).max(1), false);
                    self.memory.write(boxed, &value)?;
                    value = boxed.to_le_bytes().to_vec();
                }
                self.memory.write(address, &value)
            }
            Statement::FreeBox { address, ty } => {
                if self.is_real_aggregate(ty) {
                    let address = self.pointer(address)?;
                    let boxed = self.memory.read_u64(address)?;
                    self.memory.free(boxed)?;
                }
                Ok(())
            }
            Statement::Drop(Place::Local(local)) => self.drop_local(*local),
            Statement::Drop(_) | Statement::StorageDead(_) => Ok(()),
        }
    }

    fn rvalue(&mut self, rvalue: &Rvalue) -> Result<Vec<u8>, Error> {
        match rvalue {
            Rvalue::Use(operand) => self.owned_operand(operand),
            Rvalue::UnaryOp(op, operand) => {
                let ty = self.operand_ty(operand);
                let value = self.operand(operand)?;
                self.unary(*op, &value, &ty)
            }
            Rvalue::BinaryOp(op, left, right) => {
                let ty = self.operand_ty(left);
                if matches!(strip_borrow(&ty), Type::String) {
                    let left = self.string(left)?;
                    let right = self.string(right)?;
                    return match op {
                        BinOp::Eq => Ok(vec![u8::from(left == right)]),
                        BinOp::Ne => Ok(vec![u8::from(left != right)]),
                        BinOp::Add => self.new_string(&[left, right].concat()),
                        other => self.fail(format!("operator `{other:?}` is not supported over strings")),
                    };
                }
                let amount_ty = self.operand_ty(right);
                let left = self.operand(left)?;
                let right = self.operand(right)?;
                if let (BinOp::Shl | BinOp::Shr, Type::Int(width), Type::Int(amount_width)) = (op, &ty, &amount_ty) {
                    return self.shift(*op, int_value(&left, *width), int_value(&right, *amount_width), *width);
                }
                self.binary(*op, &left, &right, &ty)
            }
            Rvalue::Cast { operand, target } => {
                let source = self.operand_ty(operand);
                let value = self.operand(operand)?;
                Ok(cast(&value, &source, target))
            }
            Rvalue::Math(op, args) => {
                let Type::Float(width) = self.operand_ty(&args[0]) else {
                    return self.fail(format!("`{}` needs a float receiver", op.name()));
                };
                let x = float_value(&self.operand(&args[0])?, width);
                let y = match args.get(1) {
                    Some(operand) => float_value(&self.operand(operand)?, width),
                    None => 0.0,
                };
                Ok(float_bytes(op.eval(x, y), width))
            }
            Rvalue::Aggregate { ty, variant, fields } => self.aggregate(ty, variant.as_deref(), fields),
            Rvalue::SliceLen(place) => {
                let descriptor = self.descriptor(place)?;
                Ok(descriptor[8..16].to_vec())
            }
            Rvalue::Discriminant(place) => {
                let (address, _) = self.place_address(place)?;
                self.memory.read(address, 8)
            }
            Rvalue::Load { address, ty } => {
                let address = self.pointer(address)?;
                self.memory.read(address, self.size_of(ty))
            }
            Rvalue::RawAlloc { size } => Ok(self.scratch(*size as usize).to_le_bytes().to_vec()),
            Rvalue::Ref { place, .. } => Ok(self.place_address(place)?.0.to_le_bytes().to_vec()),
            Rvalue::FuncAddr(name) => Ok(self.function_pointer(name).to_le_bytes().to_vec()),
            Rvalue::Quote { template, splices } => {
                let mut values = HashMap::new();
                for (span, operand) in splices {
                    let ty = self.operand_ty(operand);
                    let value = self.operand(operand)?;
                    values.insert(*span, self.splice_value(&ty, &value)?);
                }
                let code = crate::quote::substitute(&template.0, &values).map_err(Error::new)?;
                Ok(self.code_handle(code).to_le_bytes().to_vec())
            }
        }
    }

    /// The 16-byte `[data, len]` descriptor of the string or slice `place`
    /// holds, through a borrow if it is one.
    fn descriptor(&mut self, place: &Place) -> Result<Vec<u8>, Error> {
        let ty = self.place_ty(place);
        let value = self.read_place(place)?;
        if matches!(ty, Type::Borrow { .. }) {
            let pointer = u64::from_le_bytes(value[..8].try_into().expect("a pointer"));
            return self.memory.read(pointer, 16);
        }
        Ok(value)
    }

    fn operand_descriptor(&mut self, operand: &Operand) -> Result<Vec<u8>, Error> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => self.descriptor(place),
            Operand::Constant(constant) => self.constant(constant),
        }
    }

    /// The bytes of the string `operand` holds or points to.
    pub(crate) fn string(&mut self, operand: &Operand) -> Result<Vec<u8>, Error> {
        let descriptor = self.operand_descriptor(operand)?;
        self.descriptor_bytes(&descriptor)
    }

    fn descriptor_bytes(&self, descriptor: &[u8]) -> Result<Vec<u8>, Error> {
        let data = u64::from_le_bytes(descriptor[..8].try_into().expect("a pointer"));
        let len = u64::from_le_bytes(descriptor[8..16].try_into().expect("a length"));
        self.memory.read(data, len as usize)
    }

    pub(crate) fn new_string(&mut self, bytes: &[u8]) -> Result<Vec<u8>, Error> {
        let data = self.memory.alloc(bytes.len(), false);
        self.memory.write(data, bytes)?;
        let mut descriptor = data.to_le_bytes().to_vec();
        descriptor.extend((bytes.len() as u64).to_le_bytes());
        Ok(descriptor)
    }

    fn aggregate(&mut self, ty: &Type, variant: Option<&str>, fields: &[Operand]) -> Result<Vec<u8>, Error> {
        let mut bytes = vec![0u8; self.size_of(ty)];
        match (ty, variant) {
            (Type::Struct(name, args), None) => {
                for ((declared, offset), field) in self.layouts.struct_fields(name, args).into_iter().zip(fields) {
                    if self.operand_ty(field) == Type::Unit {
                        continue;
                    }
                    let value = self.owned_as(field, &declared)?;
                    bytes[offset as usize..offset as usize + value.len()].copy_from_slice(&value);
                }
            }
            (Type::Enum(name, args), Some(variant)) => {
                let tag = self.layouts.enum_variant_index(name, args, variant);
                bytes[..8].copy_from_slice(&tag.to_le_bytes());
                for (index, field) in fields.iter().enumerate() {
                    if self.operand_ty(field) == Type::Unit {
                        continue;
                    }
                    let (_, offset) = self.layouts.enum_variant_field(name, args, variant, index);
                    let value = self.owned_operand(field)?;
                    bytes[offset as usize..offset as usize + value.len()].copy_from_slice(&value);
                }
            }
            (Type::Tuple(items), None) => {
                for (index, field) in fields.iter().enumerate() {
                    if self.operand_ty(field) == Type::Unit {
                        continue;
                    }
                    let (_, offset) = self.layouts.tuple_field(items, index);
                    let value = self.owned_operand(field)?;
                    bytes[offset as usize..offset as usize + value.len()].copy_from_slice(&value);
                }
            }
            _ => return self.fail(format!("cannot build a value of {ty:?}")),
        }
        if let Some(offset) = self.layouts.live_offset(ty) {
            bytes[offset as usize] = 1;
        }
        Ok(bytes)
    }

    fn unary(&mut self, op: UnOp, value: &[u8], ty: &Type) -> Result<Vec<u8>, Error> {
        match (op, ty) {
            (UnOp::Not, _) => Ok(vec![u8::from(value.iter().all(|byte| *byte == 0))]),
            (UnOp::Neg, Type::Float(FloatWidth::F64 | FloatWidth::F32)) => {
                let Type::Float(width) = ty else { unreachable!() };
                Ok(float_bytes(-float_value(value, *width), *width))
            }
            (UnOp::Neg, Type::Float(width)) => {
                let mut bytes = value.to_vec();
                let last = width.bytes() as usize - 1;
                bytes[last] ^= 0x80;
                Ok(bytes)
            }
            (UnOp::Neg, Type::Int(width)) => {
                let operand = int_value(value, *width);
                if self.frame().body.profile == Profile::Debug && width.is_signed() && operand == width.range().0 {
                    return self.fail("attempt to negate with overflow");
                }
                Ok(int_bytes(-operand, width.bytes()))
            }
            (UnOp::BitNot, Type::Int(width)) => Ok(int_bytes(!int_value(value, *width), width.bytes())),
            (UnOp::BitNot, _) => Ok(value.iter().map(|byte| !byte).collect()),
            (UnOp::Neg, _) => {
                let operand = i128::from(i64::from_le_bytes(value.try_into().unwrap_or([0; 8])));
                Ok(int_bytes(-operand, value.len() as u64))
            }
        }
    }

    fn binary(&mut self, op: BinOp, left: &[u8], right: &[u8], ty: &Type) -> Result<Vec<u8>, Error> {
        let flag = |value: bool| Ok(vec![u8::from(value)]);
        match ty {
            Type::Float(width) => {
                let (left, right) = (float_value(left, *width), float_value(right, *width));
                let arithmetic = |value: f64| Ok(float_bytes(value, *width));
                match (op, width) {
                    (BinOp::Add, FloatWidth::F32) => arithmetic(f64::from(left as f32 + right as f32)),
                    (BinOp::Sub, FloatWidth::F32) => arithmetic(f64::from(left as f32 - right as f32)),
                    (BinOp::Mul, FloatWidth::F32) => arithmetic(f64::from(left as f32 * right as f32)),
                    (BinOp::Div, FloatWidth::F32) => arithmetic(f64::from(left as f32 / right as f32)),
                    (BinOp::Add, _) => arithmetic(left + right),
                    (BinOp::Sub, _) => arithmetic(left - right),
                    (BinOp::Mul, _) => arithmetic(left * right),
                    (BinOp::Div, _) => arithmetic(left / right),
                    (BinOp::Rem, _) => self.fail("float remainder is not supported"),
                    (BinOp::Eq, _) => flag(left == right),
                    (BinOp::Ne, _) => flag(left != right),
                    (BinOp::Lt, _) => flag(left < right),
                    (BinOp::Le, _) => flag(left <= right),
                    (BinOp::Gt, _) => flag(left > right),
                    (BinOp::Ge, _) => flag(left >= right),
                    (other, _) => self.fail(format!("operator `{other:?}` is not valid over float operands")),
                }
            }
            Type::Bool => {
                let (left, right) = (left[0] != 0, right[0] != 0);
                match op {
                    BinOp::And => flag(left && right),
                    BinOp::Or => flag(left || right),
                    BinOp::Eq => flag(left == right),
                    BinOp::Ne => flag(left != right),
                    other => self.fail(format!("operator `{other:?}` is not valid over bool operands")),
                }
            }
            Type::Char => {
                let (left, right) = (int_value(left, IntWidth::U32), int_value(right, IntWidth::U32));
                match op {
                    BinOp::Eq => flag(left == right),
                    BinOp::Ne => flag(left != right),
                    BinOp::Lt => flag(left < right),
                    BinOp::Le => flag(left <= right),
                    BinOp::Gt => flag(left > right),
                    BinOp::Ge => flag(left >= right),
                    other => self.fail(format!("operator `{other:?}` is not valid over char operands")),
                }
            }
            Type::Int(width) => self.int_binary(op, int_value(left, *width), int_value(right, *width), *width),
            _ => {
                let width = IntWidth::U64.min_bytes(left.len());
                self.int_binary(op, int_value(left, width), int_value(right, width), width)
            }
        }
    }

    fn int_binary(&mut self, op: BinOp, left: i128, right: i128, width: IntWidth) -> Result<Vec<u8>, Error> {
        let (min, max) = width.range();
        let debug = self.frame().body.profile == Profile::Debug;
        let checked = |result: i128, message: &str| -> Result<Vec<u8>, Error> {
            if debug && (result < min || result > max) {
                return Err(Error::new(message));
            }
            Ok(int_bytes(result, width.bytes()))
        };
        let flag = |value: bool| Ok(vec![u8::from(value)]);
        match op {
            BinOp::Add => checked(left + right, "attempt to add with overflow"),
            BinOp::Sub => checked(left - right, "attempt to subtract with overflow"),
            BinOp::Mul => checked(left.wrapping_mul(right), "attempt to multiply with overflow"),
            BinOp::Div => {
                if right == 0 {
                    return self.fail("division by zero");
                }
                if width.is_signed() && left == min && right == -1 {
                    return self.fail("attempt to divide with overflow");
                }
                Ok(int_bytes(left / right, width.bytes()))
            }
            BinOp::Rem => {
                if right == 0 {
                    return self.fail("remainder by zero");
                }
                if width.is_signed() && right == -1 {
                    return Ok(int_bytes(0, width.bytes()));
                }
                Ok(int_bytes(left % right, width.bytes()))
            }
            BinOp::Eq => flag(left == right),
            BinOp::Ne => flag(left != right),
            BinOp::Lt => flag(left < right),
            BinOp::Le => flag(left <= right),
            BinOp::Gt => flag(left > right),
            BinOp::Ge => flag(left >= right),
            BinOp::BitAnd => Ok(int_bytes(left & right, width.bytes())),
            BinOp::BitOr => Ok(int_bytes(left | right, width.bytes())),
            BinOp::BitXor => Ok(int_bytes(left ^ right, width.bytes())),
            BinOp::Shl | BinOp::Shr => return self.shift(op, left, right, width),
            BinOp::WrappingAdd => Ok(int_bytes(left + right, width.bytes())),
            BinOp::WrappingSub => Ok(int_bytes(left - right, width.bytes())),
            BinOp::WrappingMul => Ok(int_bytes(left.wrapping_mul(right), width.bytes())),
            BinOp::AddOverflows => flag(!(min..=max).contains(&(left + right))),
            BinOp::SubOverflows => flag(!(min..=max).contains(&(left - right))),
            BinOp::MulOverflows => flag(left.checked_mul(right).is_none_or(|product| !(min..=max).contains(&product))),
            BinOp::And | BinOp::Or => self.fail("logical operator over int operands is not valid"),
        }
        .map_err(|error: Error| self.located(error))
    }

    /// Panics in debug on an amount outside `0..bits`; masks it in release.
    fn shift(&mut self, op: BinOp, value: i128, amount: i128, width: IntWidth) -> Result<Vec<u8>, Error> {
        let bits = (width.bytes() * 8) as i128;
        if self.frame().body.profile == Profile::Debug && !(0..bits).contains(&amount) {
            let message = if op == BinOp::Shl { "attempt to shift left with overflow" } else { "attempt to shift right with overflow" };
            return Err(self.located(Error::new(message)));
        }
        let amount = amount & (bits - 1);
        let shifted = if op == BinOp::Shl { value.wrapping_shl(amount as u32) } else { value >> amount };
        Ok(int_bytes(shifted, width.bytes()))
    }

    // ---------------------------------------------------------------
    // terminators
    // ---------------------------------------------------------------

    fn terminator(&mut self, terminator: &Terminator) -> Result<Flow, Error> {
        match terminator {
            Terminator::Goto(target) => Ok(Flow::Goto(target.0 as usize)),
            Terminator::SwitchInt { discriminant, targets, otherwise } => {
                let bytes = self.operand(discriminant)?;
                let mut raw = [0u8; 16];
                raw[..bytes.len()].copy_from_slice(&bytes);
                let value = u128::from_le_bytes(raw);
                let mask = if bytes.len() >= 16 { u128::MAX } else { (1u128 << (bytes.len() * 8)) - 1 };
                let target = targets
                    .iter()
                    .find(|(case, _)| (*case as u128) & mask == value)
                    .map_or(*otherwise, |(_, target)| *target);
                Ok(Flow::Goto(target.0 as usize))
            }
            Terminator::Call { target, args, destination, resume } => {
                self.call_terminator(&target.0, args, destination.as_ref())?;
                Ok(Flow::Goto(resume.0 as usize))
            }
            Terminator::CallIndirect { callee, args, destination, resume } => {
                let pointer = self.pointer(callee)?;
                let name = self.function_name(pointer)?;
                let mut values = Vec::with_capacity(args.len());
                for (index, arg) in args.iter().enumerate() {
                    values.push(if index == 0 { self.operand(arg)? } else { self.owned_operand(arg)? });
                }
                let result = self.call(&name, values)?;
                if let Some(place) = destination {
                    self.write_place(place, &result)?;
                }
                Ok(Flow::Goto(resume.0 as usize))
            }
            Terminator::Return(operand) => {
                let return_ty = self.frame().body.return_ty.clone();
                if self.size_of(&return_ty) == 0 && return_ty == Type::Unit {
                    self.drop_all_owned()?;
                    return Ok(Flow::Return(Vec::new()));
                }
                if matches!(operand, Operand::Constant(Constant::Unit)) {
                    return self.fail("reached code with no value to return");
                }
                let value = self.owned_as(operand, &return_ty)?;
                self.drop_all_owned()?;
                Ok(Flow::Return(value))
            }
            Terminator::Unreachable => self.fail("entered unreachable code"),
        }
    }

    fn call_terminator(&mut self, name: &str, args: &[Operand], destination: Option<&Place>) -> Result<(), Error> {
        if let Some(result) = self.builtin(name, args, destination)? {
            if let (Some(place), Some(value)) = (destination, result) {
                self.write_place(place, &value)?;
            }
            return Ok(());
        }
        if self.program.externs.contains(name) {
            return Err(not_allowed("calling an `extern` (FFI) function"));
        }
        let body = self.body(name)?;
        let mut values = Vec::with_capacity(args.len());
        for (index, arg) in args.iter().enumerate() {
            let param = body.locals.get(index).filter(|_| index < body.param_count).map(|local| local.ty.clone());
            let auto_ref = matches!(param, Some(Type::Borrow { .. })) && !matches!(self.operand_ty(arg), Type::Borrow { .. });
            values.push(match param {
                Some(param) if !auto_ref => self.owned_as(arg, &param)?,
                Some(_) => self.operand_address(arg)?.to_le_bytes().to_vec(),
                None => self.operand(arg)?,
            });
        }
        let result = self.call(name, values)?;
        if let Some(place) = destination {
            self.write_place(place, &result)?;
        }
        Ok(())
    }

    /// Runs `name` when it is a built-in or runtime entry point; the inner
    /// `Option` is the value for the destination, if any.
    fn builtin(&mut self, name: &str, args: &[Operand], destination: Option<&Place>) -> Result<Option<Option<Vec<u8>>>, Error> {
        let dest_ty = destination.map(|place| self.place_ty(place));
        let value = match name {
            _ if name == paco_mir::PANIC_SYMBOL => {
                let message = self.string(&args[0])?;
                return self.fail(format!("panicked: {}", String::from_utf8_lossy(&message)));
            }
            "print" => {
                let line = self.display(&args[0])?;
                self.output.push_str(&line);
                self.output.push('\n');
                None
            }
            "stderr_write" => {
                let text = self.string(&args[0])?;
                self.stderr.push_str(&String::from_utf8_lossy(&text));
                self.stderr.push('\n');
                None
            }
            "fs_read_to_string" => return Err(not_allowed("file I/O")),
            "arg_count" | "arg_at" => return Err(not_allowed("reading the process arguments")),
            "tcp_listen" | "TcpListener::accept" | "TcpStream::read" | "TcpStream::write" => return Err(not_allowed("network I/O")),
            "paco_rt_spawn" | "paco_rt_spawn_blocking" | "paco_rt_join" => return Err(not_allowed("spawning a task")),
            "paco_rt_channel" | "paco_rt_send" | "paco_rt_recv" | "paco_rt_sender_close" | "paco_rt_receiver_close"
            | "paco_rt_receiver_is_ready" => return Err(not_allowed("using a channel")),
            "paco_rt_generator_new" | "paco_rt_generator_next" | "paco_rt_generator_yield" => {
                return Err(not_allowed("running a generator (`iter fn`)"));
            }
            "paco_calloc" | "paco_alloc" => {
                let size = match args {
                    [count, size] => (self.int_operand(count, &Type::Int(IntWidth::I64))? * self.int_operand(size, &Type::Int(IntWidth::I64))?) as usize,
                    [size] => self.int_operand(size, &Type::Int(IntWidth::I64))? as usize,
                    _ => return self.fail(format!("`{name}` expects a size")),
                };
                Some(self.memory.alloc(size, false).to_le_bytes().to_vec())
            }
            "paco_free" => {
                let pointer = self.pointer(&args[0])?;
                self.memory.free(pointer)?;
                None
            }
            "string_len_bytes" => {
                let descriptor = self.operand_descriptor(&args[0])?;
                Some(descriptor[8..16].to_vec())
            }
            "string_concat" => {
                let (left, right) = (self.string(&args[0])?, self.string(&args[1])?);
                Some(self.new_string(&[left, right].concat())?)
            }
            "string_next_char_boundary" => {
                let text = self.string(&args[0])?;
                let pos = self.int_operand(&args[1], &Type::Int(IntWidth::I64))? as i64;
                Some(paco_runtime::text::next_char_boundary(&text, pos).to_le_bytes().to_vec())
            }
            "string_char_at" | "string_byte_at" | "string_slice_utf8" => {
                let text = self.string(&args[0])?;
                let pos = self.int_operand(&args[1], &Type::Int(IntWidth::I64))? as i64;
                let payload = match name {
                    "string_char_at" => paco_runtime::text::char_at(&text, pos).map(|c| c.to_le_bytes().to_vec()),
                    "string_byte_at" => paco_runtime::text::byte_at(&text, pos).map(|b| b.to_le_bytes().to_vec()),
                    _ => {
                        let end = self.int_operand(&args[2], &Type::Int(IntWidth::I64))? as i64;
                        match paco_runtime::text::slice_utf8(&text, pos, end).map(<[u8]>::to_vec) {
                            Some(slice) => Some(self.new_string(&slice)?),
                            None => None,
                        }
                    }
                };
                Some(self.option(dest_ty.as_ref().expect("returns an Option"), payload)?)
            }
            "string_to_bytes" => {
                let text = self.string(&args[0])?;
                Some(self.new_string(&text)?)
            }
            "string_from_bytes" => {
                let bytes = self.string(&args[0])?;
                let start = self.int_operand(&args[1], &Type::Int(IntWidth::I64))? as i64;
                let end = self.int_operand(&args[2], &Type::Int(IntWidth::I64))? as i64;
                let payload = match paco_runtime::text::from_utf8_range(&bytes, start, end) {
                    Some(text) => Some(self.new_string(text)?),
                    None => None,
                };
                Some(self.option(dest_ty.as_ref().expect("returns an Option"), payload)?)
            }
            "bytes_write_string" => {
                let descriptor = self.operand_descriptor(&args[0])?;
                let at = self.int_operand(&args[1], &Type::Int(IntWidth::I64))? as i64;
                let text = self.string(&args[2])?;
                let data = u64::from_le_bytes(descriptor[..8].try_into().expect("a pointer"));
                let len = i64::from_le_bytes(descriptor[8..16].try_into().expect("a length"));
                let fits = at >= 0 && at.checked_add(text.len() as i64).is_some_and(|end| end <= len);
                if fits {
                    self.memory.write(data + at as u64, &text)?;
                }
                Some(vec![u8::from(fits)])
            }
            "slice_sort" => {
                let descriptor = self.operand_descriptor(&args[0])?;
                let len = self.int_operand(&args[1], &Type::Int(IntWidth::I64))?.max(0) as u64;
                let kind = self.int_operand(&args[2], &Type::Int(IntWidth::I32))? as i32;
                let data = u64::from_le_bytes(descriptor[..8].try_into().expect("a pointer"));
                let available = u64::from_le_bytes(descriptor[8..16].try_into().expect("a length"));
                let size = paco_runtime::sort::sort_element_size(kind) as u64;
                let mut bytes = self.memory.read(data, (len.min(available) * size) as usize)?;
                paco_runtime::sort::sort_bytes(&mut bytes, kind);
                self.memory.write(data, &bytes)?;
                None
            }
            "string_hash" => {
                let text = self.string(&args[0])?;
                Some(paco_runtime::text::hash_bytes(&text).to_le_bytes().to_vec())
            }
            "uint_to_string" => {
                let value = self.int_operand(&args[0], &Type::Int(IntWidth::U64))? as u64;
                Some(self.new_string(paco_runtime::text::uint_to_string(value).as_bytes())?)
            }
            "int_to_string" => {
                let value = self.int_operand(&args[0], &Type::Int(IntWidth::I64))? as i64;
                Some(self.new_string(paco_runtime::text::int_to_string(value).as_bytes())?)
            }
            "bool_to_string" => {
                let value = self.operand(&args[0])?[0] != 0;
                Some(self.new_string(paco_runtime::text::bool_to_string(value).as_bytes())?)
            }
            "float_to_string" => {
                let ty = self.operand_ty(&args[0]);
                let Type::Float(width) = strip_borrow(&ty).clone() else { return self.fail("float_to_string expects a float") };
                let value = float_value(&self.operand(&args[0])?, width);
                Some(self.new_string(paco_runtime::format_float_code(value, format_code(width)).as_bytes())?)
            }
            "char_to_string" => {
                let value = int_value(&self.operand(&args[0])?, IntWidth::U32) as u32;
                let (buf, len) = paco_runtime::text::encode_char(value);
                Some(self.new_string(&buf[..len])?)
            }
            "slice_of_zeros" => {
                let elem = slice_elem(dest_ty.as_ref().expect("returns a slice"));
                let len = self.int_operand(&args[0], &Type::Int(IntWidth::I64))? as u64;
                let data = self.memory.alloc((len * element_size(&elem, self.layouts)) as usize, false);
                let mut descriptor = data.to_le_bytes().to_vec();
                descriptor.extend(len.to_le_bytes());
                Some(descriptor)
            }
            "slice_as_ptr" | "slice_as_mut_ptr" => {
                let descriptor = self.operand_descriptor(&args[0])?;
                Some(descriptor[..8].to_vec())
            }
            "fields_of" => Some(self.fields_of(&args[0], dest_ty.as_ref().expect("returns a FieldIter"))?),
            "type_name" => {
                let handle = self.pointer(&args[0])?;
                let ty = self.type_of_handle(handle)?;
                Some(self.new_string(ty.name().as_bytes())?)
            }
            "code_to_string" => {
                let handle = self.pointer(&args[0])?;
                let text = crate::quote::render(self.code(handle)?);
                Some(self.new_string(text.as_bytes())?)
            }
            "Code::join" => Some(self.code_join(&args[0], &args[1])?),
            _ if name.starts_with("$cell::") => return self.cell(&name["$cell::".len()..], args, dest_ty.as_ref()).map(Some),
            _ => return Ok(None),
        };
        Ok(Some(value))
    }

    fn option(&mut self, ty: &Type, payload: Option<Vec<u8>>) -> Result<Vec<u8>, Error> {
        let Type::Enum(name, args) = ty else { return self.fail("expected an Option") };
        let mut bytes = vec![0u8; self.size_of(ty)];
        let variant = if payload.is_some() { "Some" } else { "None" };
        let tag = self.layouts.enum_variant_index(name, args, variant);
        bytes[..8].copy_from_slice(&tag.to_le_bytes());
        if let Some(payload) = payload {
            let (_, offset) = self.layouts.enum_variant_field(name, args, "Some", 0);
            bytes[offset as usize..offset as usize + payload.len()].copy_from_slice(&payload);
        }
        Ok(bytes)
    }

    fn cell(&mut self, op: &str, args: &[Operand], dest_ty: Option<&Type>) -> Result<Option<Vec<u8>>, Error> {
        if op == "new" {
            let ty = self.operand_ty(&args[0]);
            let size = CELL_VALUE_OFFSET as usize + self.size_of(&ty);
            let cell = self.memory.alloc(size, false);
            self.memory.write_u64(cell + CELL_COUNT_OFFSET as u64, 1)?;
            if ty != Type::Unit {
                let value = self.owned_operand(&args[0])?;
                self.memory.write(cell + CELL_VALUE_OFFSET as u64, &value)?;
            }
            return Ok(Some(cell.to_le_bytes().to_vec()));
        }
        let mut cell = self.pointer(&args[0])?;
        if matches!(self.operand_ty(&args[0]), Type::Borrow { .. }) {
            cell = self.memory.read_u64(cell)?;
        }
        match op {
            "get" => {
                let Some(ty) = dest_ty else { return Ok(None) };
                let value = self.memory.read(cell + CELL_VALUE_OFFSET as u64, self.size_of(ty))?;
                Ok(Some(self.clone_value(ty, value)?))
            }
            "set" => {
                let ty = self.operand_ty(&args[1]);
                if ty != Type::Unit {
                    let value = self.owned_operand(&args[1])?;
                    let address = cell + CELL_VALUE_OFFSET as u64;
                    let old = self.memory.read(address, value.len())?;
                    self.memory.write(address, &value)?;
                    if self.needs_drop(&ty) {
                        let scratch = self.memory.alloc(old.len(), false);
                        self.memory.write(scratch, &old)?;
                        self.drop_at(&ty, scratch)?;
                        self.memory.free(scratch)?;
                    }
                }
                Ok(None)
            }
            "clone" => {
                let count = self.memory.read_u64(cell + CELL_COUNT_OFFSET as u64)?;
                self.memory.write_u64(cell + CELL_COUNT_OFFSET as u64, count + 1)?;
                Ok(Some(cell.to_le_bytes().to_vec()))
            }
            "strong_count" => Ok(Some(self.memory.read(cell + CELL_COUNT_OFFSET as u64, 8)?)),
            other => self.fail(format!("unknown shared-cell operation `{other}`")),
        }
    }

    /// What `print` writes for `operand`, without the newline.
    fn display(&mut self, operand: &Operand) -> Result<String, Error> {
        let ty = self.operand_ty(operand);
        let inner = strip_borrow(&ty).clone();
        let mut value = self.operand(operand)?;
        if matches!(ty, Type::Borrow { .. }) && !self.is_real_aggregate(&inner) {
            let pointer = u64::from_le_bytes(value[..8].try_into().expect("a pointer"));
            value = self.memory.read(pointer, self.size_of(&inner))?;
        }
        Ok(match &inner {
            Type::Float(width) => paco_runtime::format_float_code(float_value(&value, *width), format_code(*width)),
            Type::Bool => paco_runtime::text::bool_to_string(value[0] != 0).to_string(),
            Type::Char => {
                let (buf, len) = paco_runtime::text::encode_char(int_value(&value, IntWidth::U32) as u32);
                String::from_utf8_lossy(&buf[..len]).into_owned()
            }
            Type::String => {
                let descriptor = if matches!(ty, Type::Borrow { .. }) {
                    let pointer = u64::from_le_bytes(value[..8].try_into().expect("a pointer"));
                    self.memory.read(pointer, 16)?
                } else {
                    value
                };
                String::from_utf8_lossy(&self.descriptor_bytes(&descriptor)?).into_owned()
            }
            Type::Int(IntWidth::U64) => paco_runtime::text::uint_to_string(int_value(&value, IntWidth::U64) as u64),
            Type::Int(width) => paco_runtime::text::int_to_string(int_value(&value, *width) as i64),
            Type::TypeValue(_) => self.type_of_handle(u64::from_le_bytes(value[..8].try_into().expect("a handle")))?.name(),
            Type::Code => crate::quote::render(self.code(u64::from_le_bytes(value[..8].try_into().expect("a handle")))?),
            other => return self.fail(format!("`print` is not supported for {}", other.name())),
        })
    }

    pub(crate) fn code(&self, handle: u64) -> Result<&QuoteBody, Error> {
        self.codes.get(handle as usize).ok_or_else(|| Error::new("invalid `Code` value"))
    }

    fn splice_value(&mut self, ty: &Type, value: &[u8]) -> Result<crate::quote::Splice, Error> {
        use crate::quote::Splice;
        let handle = || u64::from_le_bytes(value[..8].try_into().expect("a handle"));
        Ok(match strip_borrow(ty) {
            Type::TypeValue(_) => Splice::Type(self.type_of_handle(handle())?),
            Type::Code => Splice::Code(Box::new(self.code(handle())?.clone())),
            Type::String => {
                let descriptor = if matches!(ty, Type::Borrow { .. }) { self.memory.read(handle(), 16)? } else { value.to_vec() };
                Splice::String(String::from_utf8_lossy(&self.descriptor_bytes(&descriptor)?).into_owned())
            }
            Type::Int(width) => Splice::Int(int_value(value, *width) as i64),
            Type::Float(width) => Splice::Float(float_value(value, *width)),
            Type::Bool => Splice::Bool(value[0] != 0),
            Type::Char => Splice::Char(char::from_u32(int_value(value, IntWidth::U32) as u32).unwrap_or('\u{FFFD}')),
            other => return self.fail(format!("a {} value cannot be spliced into code", other.name())),
        })
    }

    fn fields_of(&mut self, operand: &Operand, iter_ty: &Type) -> Result<Vec<u8>, Error> {
        let handle = self.pointer(operand)?;
        let ty = self.type_of_handle(handle)?;
        let Type::Struct(struct_name, _) = &ty else {
            return self.fail(format!("fields_of expects a struct type, found `{}`", ty.name()));
        };
        let declared = self.program.structs.get(struct_name).cloned().unwrap_or_default();
        let (items_ty, items_offset) = self.field_of(iter_ty, "items");
        let (_, pos_offset) = self.field_of(iter_ty, "pos");
        let (data_ty, data_offset) = self.field_of(&items_ty, "data");
        let (_, len_offset) = self.field_of(&items_ty, "len");
        let (_, cap_offset) = self.field_of(&items_ty, "cap");
        let info_ty = slice_elem(&data_ty);
        let (_, name_offset) = self.field_of(&info_ty, "name");
        let (_, ty_offset) = self.field_of(&info_ty, "ty");
        let info_size = element_size(&info_ty, self.layouts);
        let data = self.memory.alloc(info_size as usize * declared.len(), false);
        for (index, (field_name, field_ty)) in declared.iter().enumerate() {
            let name = self.new_string(field_name.as_bytes())?;
            let field_ty = crate::quote::resolve_ty(field_ty, &self.program.enums);
            let handle = self.type_handle(&field_ty);
            let base = data + index as u64 * info_size;
            self.memory.write(base + name_offset, &name)?;
            self.memory.write(base + ty_offset, &handle.to_le_bytes())?;
        }
        let mut bytes = vec![0u8; self.size_of(iter_ty)];
        let len = declared.len() as u64;
        let items = items_offset as usize;
        bytes[items + data_offset as usize..items + data_offset as usize + 8].copy_from_slice(&data.to_le_bytes());
        bytes[items + data_offset as usize + 8..items + data_offset as usize + 16].copy_from_slice(&len.to_le_bytes());
        bytes[items + len_offset as usize..items + len_offset as usize + 8].copy_from_slice(&len.to_le_bytes());
        bytes[items + cap_offset as usize..items + cap_offset as usize + 8].copy_from_slice(&len.to_le_bytes());
        bytes[pos_offset as usize..pos_offset as usize + 8].copy_from_slice(&0u64.to_le_bytes());
        Ok(bytes)
    }

    fn code_join(&mut self, pieces: &Operand, separator: &Operand) -> Result<Vec<u8>, Error> {
        let (data_address, vec_ty) = match pieces {
            Operand::Copy(place) | Operand::Move(place) => self.place_address(place)?,
            Operand::Constant(_) => return self.fail("Code::join expects a `Vec<Code>`"),
        };
        let (_, data_offset) = self.field_of(&vec_ty, "data");
        let (_, len_offset) = self.field_of(&vec_ty, "len");
        let data = self.memory.read_u64(data_address + data_offset)?;
        let len = self.memory.read_u64(data_address + len_offset)?;
        let mut codes = Vec::with_capacity(len as usize);
        for index in 0..len {
            let handle = self.memory.read_u64(data + index * 8)?;
            codes.push(self.code(handle)?.clone());
        }
        let separator = String::from_utf8_lossy(&self.string(separator)?).into_owned();
        let joined = crate::quote::join(codes, &separator).map_err(Error::new)?;
        Ok(self.code_handle(joined).to_le_bytes().to_vec())
    }

    // ---------------------------------------------------------------
    // values in and out
    // ---------------------------------------------------------------

    fn encode(&mut self, ty: &Type, value: &ComptimeValue) -> Result<Vec<u8>, Error> {
        Ok(match value {
            ComptimeValue::Scalar(constant) => self.constant(constant)?,
            ComptimeValue::Type(inner) => self.type_handle(inner).to_le_bytes().to_vec(),
            ComptimeValue::Code(code) => self.code_handle(code.as_ref().clone()).to_le_bytes().to_vec(),
            _ => return Err(Error::new(format!("cannot pass a {} argument to compile-time code", ty.name()))),
        })
    }

    fn decode(&mut self, ty: &Type, bytes: &[u8]) -> Result<ComptimeValue, Error> {
        let embed = |what: &str| Error::new(format!("a compile-time value of type `{what}` cannot be embedded in the program"));
        Ok(match ty {
            Type::Unit => ComptimeValue::Scalar(Constant::Unit),
            Type::Int(width) => {
                let value = int_value(bytes, *width);
                ComptimeValue::Scalar(Constant::Int(value as i64, *width))
            }
            Type::Float(width) => ComptimeValue::Scalar(Constant::Float(float_value(bytes, *width).to_bits(), *width)),
            Type::Bool => ComptimeValue::Scalar(Constant::Bool(bytes[0] != 0)),
            Type::Char => ComptimeValue::Scalar(Constant::Char(
                char::from_u32(int_value(bytes, IntWidth::U32) as u32).ok_or_else(|| embed("char"))?,
            )),
            Type::String => {
                ComptimeValue::Scalar(Constant::Str(String::from_utf8_lossy(&self.descriptor_bytes(bytes)?).into_owned()))
            }
            Type::TypeValue(_) => ComptimeValue::Type(self.type_of_handle(u64::from_le_bytes(bytes[..8].try_into().expect("a handle")))?),
            Type::Code => ComptimeValue::Code(Box::new(self.code(u64::from_le_bytes(bytes[..8].try_into().expect("a handle")))?.clone())),
            Type::Slice(elem) => {
                let data = u64::from_le_bytes(bytes[..8].try_into().expect("a pointer"));
                let len = u64::from_le_bytes(bytes[8..16].try_into().expect("a length"));
                let size = element_size(elem, self.layouts);
                let mut items = Vec::with_capacity(len as usize);
                for index in 0..len {
                    let item = self.memory.read(data + index * size, self.size_of(elem))?;
                    items.push(self.decode(elem, &item)?);
                }
                ComptimeValue::Slice(ty.clone(), items)
            }
            Type::Tuple(items) => {
                let mut fields = Vec::with_capacity(items.len());
                for index in 0..items.len() {
                    let (field_ty, offset) = self.layouts.tuple_field(items, index);
                    let size = self.size_of(&field_ty);
                    fields.push(self.decode(&field_ty, &bytes[offset as usize..offset as usize + size])?);
                }
                ComptimeValue::Record(ty.clone(), fields)
            }
            Type::Struct(name, args) if self.layouts.has_struct(name) => {
                let mut fields = Vec::new();
                for (field_ty, offset) in self.layouts.struct_fields(name, args) {
                    let size = self.size_of(&field_ty);
                    fields.push(self.decode(&field_ty, &bytes[offset as usize..offset as usize + size])?);
                }
                ComptimeValue::Record(ty.clone(), fields)
            }
            Type::Enum(name, args) => {
                let tag = u64::from_le_bytes(bytes[..8].try_into().expect("a tag"));
                let names = self.layouts.enum_variant_names(name);
                let (_, layout) = self
                    .layouts
                    .enum_variants(name, args)
                    .into_iter()
                    .enumerate()
                    .find(|(_, (index, _))| *index == tag)
                    .ok_or_else(|| Error::new(format!("invalid `{name}` tag {tag}")))?;
                let variant = names
                    .into_iter()
                    .find(|variant| self.layouts.enum_variant_index(name, args, variant) == tag)
                    .ok_or_else(|| Error::new(format!("invalid `{name}` tag {tag}")))?;
                let mut fields = Vec::new();
                for (field_ty, offset) in layout.1 {
                    let size = self.size_of(&field_ty);
                    fields.push(self.decode(&field_ty, &bytes[offset as usize..offset as usize + size])?);
                }
                ComptimeValue::Variant(ty.clone(), variant, fields)
            }
            other => return Err(embed(&other.name())),
        })
    }
}

enum Flow {
    Goto(usize),
    Return(Vec<u8>),
}

trait MinBytes {
    fn min_bytes(self, len: usize) -> IntWidth;
}

impl MinBytes for IntWidth {
    fn min_bytes(self, len: usize) -> IntWidth {
        match len {
            1 => IntWidth::U8,
            2 => IntWidth::U16,
            4 => IntWidth::U32,
            _ => IntWidth::U64,
        }
    }
}

fn cast(value: &[u8], source: &Type, target: &Type) -> Vec<u8> {
    let signed = |ty: &Type| matches!(ty, Type::Int(width) if width.is_signed());
    let width_of = |ty: &Type, len: usize| match ty {
        Type::Int(width) => *width,
        Type::Char => IntWidth::U32,
        Type::Bool => IntWidth::U8,
        _ => IntWidth::U64.min_bytes(len),
    };
    let target_bytes = |ty: &Type| match ty {
        Type::Int(width) => width.bytes(),
        Type::Char => 4,
        Type::Bool => 1,
        Type::Float(width) => width.bytes(),
        _ => 8,
    };
    match (source, target) {
        (Type::Float(from), Type::Float(to)) => {
            if from == to {
                value.to_vec()
            } else {
                float_bytes(float_value(value, *from), *to)
            }
        }
        (Type::Float(from), _) => {
            let wide = float_value(value, *from);
            let to = width_of(target, target_bytes(target) as usize);
            let (min, max) = to.range();
            let result = if wide.is_nan() {
                0
            } else if signed(target) {
                (wide.trunc() as i128).clamp(min, max)
            } else {
                (wide.trunc().max(0.0) as i128).clamp(min, max)
            };
            int_bytes(result, target_bytes(target))
        }
        (_, Type::Float(to)) => {
            let from = width_of(source, value.len());
            let int = int_value(value, from);
            let wide = if signed(source) { int as i64 as f64 } else { int as u64 as f64 };
            float_bytes(wide, *to)
        }
        _ => {
            let from = width_of(source, value.len());
            let int = if signed(source) { int_value(value, from) } else { int_value(value, IntWidth::U64.min_bytes(value.len())) };
            int_bytes(int, target_bytes(target))
        }
    }
}

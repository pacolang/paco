use cranelift_module::FuncId;
use cranelift_object::ObjectProduct;
use cranelift_object::object::write::{Relocation, SectionId, SymbolId, SymbolSection};
use cranelift_object::object::write::StandardSegment;
use cranelift_object::object::{BinaryFormat, RelocationEncoding, RelocationFlags, RelocationKind, SectionKind};
use gimli::write::{Address, AttributeValue, DwarfUnit, EndianVec, LineProgram, LineString, Sections, Writer};
use gimli::{Encoding, Format, LineEncoding, RunTimeEndian};

/// One defined function's code size and `(code offset, location index)`
/// rows, in address order.
pub(crate) struct DebugFunction {
    pub(crate) id: FuncId,
    pub(crate) size: u32,
    pub(crate) rows: Vec<(u32, u32)>,
}

pub(crate) const PRODUCER: &[u8] = b"paco";

#[derive(Clone)]
enum Target {
    Symbol(usize),
    Section(gimli::SectionId),
}

#[derive(Clone)]
struct Reloc {
    offset: u64,
    size: u8,
    target: Target,
    addend: i64,
}

#[derive(Clone)]
struct RelocWriter {
    data: EndianVec<RunTimeEndian>,
    relocs: Vec<Reloc>,
}

impl Writer for RelocWriter {
    type Endian = RunTimeEndian;

    fn endian(&self) -> RunTimeEndian {
        self.data.endian()
    }

    fn len(&self) -> usize {
        self.data.len()
    }

    fn write(&mut self, bytes: &[u8]) -> gimli::write::Result<()> {
        self.data.write(bytes)
    }

    fn write_at(&mut self, offset: usize, bytes: &[u8]) -> gimli::write::Result<()> {
        self.data.write_at(offset, bytes)
    }

    fn write_address(&mut self, address: Address, size: u8) -> gimli::write::Result<()> {
        match address {
            Address::Constant(value) => self.write_udata(value, size),
            Address::Symbol { symbol, addend } => {
                self.relocs.push(Reloc { offset: self.len() as u64, size, target: Target::Symbol(symbol), addend });
                self.write_udata(0, size)
            }
        }
    }

    fn write_offset(&mut self, value: usize, section: gimli::SectionId, size: u8) -> gimli::write::Result<()> {
        let offset = self.len();
        self.relocs.push(Reloc { offset: offset as u64, size, target: Target::Section(section), addend: value as i64 });
        self.write_udata(0, size)
    }

    fn write_offset_at(&mut self, offset: usize, value: usize, section: gimli::SectionId, size: u8) -> gimli::write::Result<()> {
        self.relocs.push(Reloc { offset: offset as u64, size, target: Target::Section(section), addend: value as i64 });
        self.write_udata_at(offset, 0, size)
    }
}

/// Adds `.debug_info` and `.debug_line` sections describing `functions`
/// (a line table from each row's `locations` entry) to `product`.
pub(crate) fn emit(product: &mut ObjectProduct, functions: &[DebugFunction], locations: &[(String, u32, u32)]) -> Result<(), String> {
    if functions.is_empty() {
        return Ok(());
    }
    let endian = RunTimeEndian::Little;
    let encoding = Encoding { format: Format::Dwarf32, version: 4, address_size: 8 };
    let mut dwarf = DwarfUnit::new(encoding);
    let mut program = LineProgram::new(
        encoding,
        LineEncoding::default(),
        LineString::String(b".".to_vec()),
        None,
        LineString::String(PRODUCER.to_vec()),
        None,
    );
    let directory = program.default_directory();
    let symbols: Vec<SymbolId> = functions.iter().map(|function| product.function_symbol(function.id)).collect();
    for (index, function) in functions.iter().enumerate() {
        program.begin_sequence(Some(Address::Symbol { symbol: index, addend: 0 }));
        for &(offset, location) in &function.rows {
            let (file, line, column) = &locations[location as usize];
            let file = program.add_file(LineString::String(file.as_bytes().to_vec()), directory, None);
            let row = program.row();
            row.address_offset = u64::from(offset);
            row.file = file;
            row.line = u64::from(*line);
            row.column = u64::from(*column);
            program.generate_row();
        }
        program.end_sequence(u64::from(function.size));
    }
    dwarf.unit.line_program = program;
    let root = dwarf.unit.root();
    let entry = dwarf.unit.get_mut(root);
    entry.set(gimli::DW_AT_producer, AttributeValue::String(PRODUCER.to_vec()));
    entry.set(gimli::DW_AT_language, AttributeValue::Language(gimli::DW_LANG_C));
    let name = locations.first().map_or("paco", |(file, ..)| file.as_str());
    entry.set(gimli::DW_AT_name, AttributeValue::String(name.as_bytes().to_vec()));
    entry.set(gimli::DW_AT_comp_dir, AttributeValue::String(b".".to_vec()));
    for (index, function) in functions.iter().enumerate() {
        let id = dwarf.unit.add(root, gimli::DW_TAG_subprogram);
        let subprogram = dwarf.unit.get_mut(id);
        subprogram.set(gimli::DW_AT_low_pc, AttributeValue::Address(Address::Symbol { symbol: index, addend: 0 }));
        subprogram.set(gimli::DW_AT_high_pc, AttributeValue::Udata(u64::from(function.size)));
    }

    let writer = RelocWriter { data: EndianVec::new(endian), relocs: Vec::new() };
    let mut sections = Sections::new(writer);
    dwarf.write(&mut sections).map_err(|error| format!("failed to write debug info: {error}"))?;

    let format = product.object.format();
    let segment = product.object.segment_name(StandardSegment::Debug).to_vec();
    let mut section_ids: Vec<(gimli::SectionId, SectionId, Vec<Reloc>)> = Vec::new();
    let mut failure = None;
    sections
        .for_each_mut(|id, writer| {
            let mut taken = std::mem::take(&mut writer.relocs);
            if writer.data.len() == 0 {
                return Ok::<(), ()>(());
            }
            let mut name = id.name().to_string();
            if format == BinaryFormat::MachO {
                for reloc in taken.extract_if(.., |reloc| matches!(reloc.target, Target::Section(_))) {
                    writer.data.write_udata_at(reloc.offset as usize, reloc.addend as u64, reloc.size).map_err(|_| ())?;
                }
                name = name.replacen('.', "__", 1);
            }
            let section = product.object.add_section(segment.clone(), name.into_bytes(), SectionKind::Debug);
            product.object.append_section_data(section, writer.data.slice(), 1);
            section_ids.push((id, section, taken));
            Ok(())
        })
        .ok();
    for (_, section, relocs) in &section_ids {
        for reloc in relocs {
            let mut addend = reloc.addend;
            let symbol = match reloc.target {
                Target::Symbol(index) if format == BinaryFormat::MachO => {
                    let function = product.object.symbol(symbols[index]);
                    let (value, SymbolSection::Section(text)) = (function.value, function.section) else {
                        failure = Some("a function symbol has no section".to_string());
                        continue;
                    };
                    addend += value as i64;
                    product.object.section_symbol(text)
                }
                Target::Symbol(index) => symbols[index],
                Target::Section(target) => {
                    let Some((_, target, _)) = section_ids.iter().find(|(id, ..)| *id == target) else {
                        failure = Some(format!("debug section {} is missing", target.name()));
                        continue;
                    };
                    product.object.section_symbol(*target)
                }
            };
            let kind = match reloc.target {
                Target::Section(_) if format == BinaryFormat::Coff => RelocationKind::SectionOffset,
                _ => RelocationKind::Absolute,
            };
            let flags = RelocationFlags::Generic {
                kind,
                encoding: RelocationEncoding::Generic,
                size: reloc.size * 8,
            };
            product
                .object
                .add_relocation(*section, Relocation { offset: reloc.offset, symbol, addend, flags })
                .map_err(|error| format!("failed to relocate debug info: {error}"))?;
        }
    }
    failure.map_or(Ok(()), Err)
}

/// dsymutil reads the addresses in a Mach-O object's DWARF as they sit in
/// the file, as the object's own addresses, the way LLVM writes them: each
/// section-relative relocation's value gains its target section's address.
pub(crate) fn place_macho_debug_addresses(bytes: &mut [u8]) -> Result<(), String> {
    use object::{Object, ObjectSection, RelocationTarget};
    let file = object::File::parse(&*bytes).map_err(|error| error.to_string())?;
    let mut patches = Vec::new();
    for section in file.sections().filter(|section| section.name().is_ok_and(|name| name.starts_with("__debug_"))) {
        let Some((start, _)) = section.file_range() else { continue };
        for (offset, relocation) in section.relocations() {
            if let RelocationTarget::Section(index) = relocation.target() {
                let target = file.section_by_index(index).map_err(|error| error.to_string())?;
                patches.push(((start + offset) as usize, usize::from(relocation.size() / 8), target.address()));
            }
        }
    }
    for (at, size, address) in patches {
        let field = &mut bytes[at..at + size];
        let mut value = [0u8; 8];
        value[..size].copy_from_slice(field);
        let placed = u64::from_le_bytes(value).wrapping_add(address).to_le_bytes();
        field.copy_from_slice(&placed[..size]);
    }
    Ok(())
}

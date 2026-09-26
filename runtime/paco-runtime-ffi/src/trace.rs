use std::io::Write;

use object::{Object, ObjectSection, ObjectSymbol};

type Reader<'a> = gimli::EndianSlice<'a, gimli::RunTimeEndian>;

struct Row {
    address: u64,
    end: u64,
    file: String,
    line: u64,
    column: u64,
}

/// The Paco line tables and symbols of the running executable, with
/// addresses already relocated to where it is loaded.
struct Symbols {
    rows: Vec<Row>,
    functions: Vec<(u64, u64, String)>,
}

const MAX_FRAMES: usize = 256;

impl Symbols {
    fn load() -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        let dsym = std::path::PathBuf::from(format!("{}.dwarf", exe.display()));
        let bytes = if cfg!(target_os = "macos") { std::fs::read(dsym) } else { std::fs::read(&exe) }.ok()?;
        let file = object::File::parse(bytes.as_slice()).ok()?;
        let macho = file.format() == object::BinaryFormat::MachO;
        let sizeless = macho || file.format() == object::BinaryFormat::Pe;
        let anchor = crate::paco_rt_panic as *const () as usize as u64;
        let bias = file.symbols().find(|symbol| symbol_name(symbol, macho) == Some("paco_rt_panic"))?.address().wrapping_neg().wrapping_add(anchor);
        let endian = if file.is_little_endian() { gimli::RunTimeEndian::Little } else { gimli::RunTimeEndian::Big };
        let section = |id: gimli::SectionId| -> Result<Reader<'_>, gimli::Error> {
            let data = file.section_by_name(id.name()).and_then(|section| section.data().ok()).unwrap_or(&[]);
            Ok(gimli::EndianSlice::new(data, endian))
        };
        let dwarf = gimli::Dwarf::load(section).ok()?;
        let mut rows = Vec::new();
        let mut units = dwarf.units();
        while let Ok(Some(header)) = units.next() {
            let Ok(unit) = dwarf.unit(header) else { continue };
            if !is_paco_unit(&dwarf, &unit) {
                continue;
            }
            collect_rows(&dwarf, &unit, bias, &mut rows);
        }
        rows.sort_by_key(|row| row.address);
        let mut functions: Vec<(u64, u64, String)> = file
            .symbols()
            .filter(|symbol| symbol.kind() == object::SymbolKind::Text && (symbol.size() > 0 || sizeless))
            .filter_map(|symbol| {
                let start = symbol.address().wrapping_add(bias);
                Some((start, start + symbol.size(), symbol_name(&symbol, macho)?.to_string()))
            })
            .collect();
        functions.sort();
        if sizeless {
            for index in 1..functions.len() {
                functions[index - 1].1 = functions[index].0;
            }
        }
        Some(Self { rows, functions })
    }

    fn line(&self, address: u64) -> Option<&Row> {
        let index = self.rows.partition_point(|row| row.address <= address).checked_sub(1)?;
        let row = &self.rows[index];
        (address < row.end).then_some(row)
    }

    fn function(&self, address: u64) -> &str {
        let index = self.functions.partition_point(|(start, ..)| *start <= address);
        let name = index
            .checked_sub(1)
            .map(|index| &self.functions[index])
            .filter(|(_, end, _)| address < *end)
            .map_or("?", |(_, _, name)| name.as_str());
        match name {
            "__paco_entry" => "main",
            thunk if thunk.starts_with("__paco_spawn_thunk_") => "<spawned task>",
            thunk if thunk.starts_with("__paco_closure_thunk_") => "<closure>",
            name => name,
        }
    }
}

fn symbol_name<'data>(symbol: &object::Symbol<'data, '_>, macho: bool) -> Option<&'data str> {
    let name = symbol.name().ok()?;
    Some(if macho { name.strip_prefix('_').unwrap_or(name) } else { name })
}

fn is_paco_unit(dwarf: &gimli::Dwarf<Reader<'_>>, unit: &gimli::Unit<Reader<'_>>) -> bool {
    let mut entries = unit.entries();
    let Ok(Some(root)) = entries.next_dfs() else {
        return false;
    };
    root.attr_value(gimli::DW_AT_producer)
        .and_then(|value| dwarf.attr_string(unit, value).ok())
        .is_some_and(|producer| producer.slice().starts_with(b"paco"))
}

fn collect_rows(dwarf: &gimli::Dwarf<Reader<'_>>, unit: &gimli::Unit<Reader<'_>>, bias: u64, out: &mut Vec<Row>) {
    let Some(program) = unit.line_program.clone() else { return };
    let mut rows = program.rows();
    let mut sequence: Vec<Row> = Vec::new();
    while let Ok(Some((header, row))) = rows.next_row() {
        let address = row.address().wrapping_add(bias);
        if let Some(previous) = sequence.last_mut() {
            previous.end = address;
        }
        if row.end_sequence() {
            out.append(&mut sequence);
            continue;
        }
        let file = row.file(header).map(|entry| file_name(dwarf, unit, header, entry)).unwrap_or_default();
        let line = row.line().map_or(0, |line| line.get());
        let column = match row.column() {
            gimli::ColumnType::LeftEdge => 1,
            gimli::ColumnType::Column(column) => column.get(),
        };
        sequence.push(Row { address, end: address, file, line, column });
    }
}

fn file_name(
    dwarf: &gimli::Dwarf<Reader<'_>>,
    unit: &gimli::Unit<Reader<'_>>,
    header: &gimli::LineProgramHeader<Reader<'_>>,
    entry: &gimli::FileEntry<Reader<'_>>,
) -> String {
    let text = |value| {
        dwarf.attr_string(unit, value).map(|name| String::from_utf8_lossy(name.slice()).into_owned()).unwrap_or_default()
    };
    let name = text(entry.path_name());
    let directory = entry.directory(header).map(text).unwrap_or_default();
    if directory.is_empty() || directory == "." || name.starts_with('/') { name } else { format!("{directory}/{name}") }
}

/// Writes one `at <fn> (<file>:<line>:<column>)` line per active Paco
/// frame, innermost first: the panicking `function` at the panic site, then
/// each caller found by following frame pointers from `frame`.
pub(crate) fn write(out: &mut impl Write, frame: usize, function: usize, site: (&str, u32, u32)) {
    let Some(symbols) = Symbols::load() else { return };
    let (file, line, column) = site;
    let _ = writeln!(out, "   at {} ({file}:{line}:{column})", symbols.function(function as u64));
    let mut frame = frame;
    for _ in 0..MAX_FRAMES {
        if frame == 0 || !frame.is_multiple_of(std::mem::align_of::<usize>()) {
            return;
        }
        let (caller_frame, return_address) = unsafe { (*(frame as *const usize), *((frame + 8) as *const usize)) };
        let call_site = (return_address as u64).wrapping_sub(1);
        let Some(row) = symbols.line(call_site) else { return };
        let _ = writeln!(out, "   at {} ({}:{}:{})", symbols.function(call_site), row.file, row.line, row.column);
        frame = caller_frame;
    }
}

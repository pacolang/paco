use paco_span::{SourceMap, Span};

#[test]
fn resolves_byte_spans_to_one_based_line_columns() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "first\ncafe\nlast");

    let location = sources.location(Span::new(file, 6, 10)).unwrap();

    assert_eq!(location.file_id, file);
    assert_eq!(location.file_name, "main.paco");
    assert_eq!(location.start.line, 2);
    assert_eq!(location.start.column, 1);
    assert_eq!(location.end.line, 2);
    assert_eq!(location.end.column, 5);
}

#[test]
fn resolves_zero_length_span_at_end_of_file() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "first\ncafe\nlast");

    let location = sources.location(Span::new(file, 15, 15)).unwrap();

    assert_eq!(location.start.line, 3);
    assert_eq!(location.start.column, 5);
    assert_eq!(location.end.line, 3);
    assert_eq!(location.end.column, 5);
}

#[test]
fn resolves_columns_by_utf8_characters_not_bytes() {
    let mut sources = SourceMap::new();
    let source = "a\ncafe\u{301}\nlast";
    let file = sources.add_file("main.paco", source);
    let offset = "a\ncafe\u{301}".len();

    let location = sources.location(Span::new(file, offset, offset)).unwrap();

    assert_eq!(location.start.line, 2);
    assert_eq!(location.start.column, 6);
}

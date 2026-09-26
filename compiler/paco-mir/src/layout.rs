//! Byte layout for MIR types.

use paco_types::Type;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Repr {
    Default,
    C,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldLayout {
    pub name: String,
    pub offset: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Layout {
    pub size: u64,
    pub align: u64,
    pub fields: Vec<FieldLayout>,
}

pub fn scalar_layout(ty: &Type) -> Option<Layout> {
    let leaf = |size, align| {
        Some(Layout {
            size,
            align,
            fields: Vec::new(),
        })
    };
    match ty {
        Type::Unit | Type::Never | Type::Dim(_) | Type::Pack(_) | Type::Spread(_) => leaf(0, 1),
        Type::Bool => leaf(1, 1),
        Type::Int(width) => leaf(width.bytes(), width.bytes()),
        Type::Char => leaf(4, 4),
        Type::Float(width) => leaf(width.bytes(), width.bytes()),
        Type::String => leaf(16, 8),
        Type::Slice(_) => leaf(16, 8),
        Type::RawPointer { .. } | Type::Fn(..) => leaf(8, 8),
        Type::Borrow { ty, .. } => {
            if matches!(ty.as_ref(), Type::Slice(_)) {
                leaf(16, 8)
            } else {
                leaf(8, 8)
            }
        }
        // `type` (`phase-9-comptime` Decision 5): never actually read or
        // written by compiled code (see `type_layout.rs::builtin_ty`'s own
        // comment) — an 8-byte placeholder, matching `RawPointer`'s own
        // size, is enough for the compiler's own static layout analysis
        // of a prelude struct that happens to have a `type`-typed field.
        // `Code` (`phase-9-comptime` Decision 7): same reasoning as
        // `TypeValue` above — comptime-only, never actually materialized
        // in compiled code.
        Type::TypeValue(_) | Type::Code => leaf(8, 8),
        Type::Struct(_, _)
        | Type::Enum(_, _)
        | Type::Tuple(_)
        | Type::Generic(_)
        | Type::Unknown
        | Type::Error => {
            None
        }
    }
}

pub(crate) fn align_up(offset: u64, align: u64) -> u64 {
    offset.div_ceil(align) * align
}

pub fn struct_layout(fields: &[(String, Layout)], repr: Repr) -> Layout {
    let mut ordered: Vec<&(String, Layout)> = fields.iter().collect();
    if repr == Repr::Default {
        ordered.sort_by_key(|(_, layout)| std::cmp::Reverse(layout.align));
    }

    let mut offset = 0u64;
    let mut align = 1u64;
    let mut field_layouts = Vec::with_capacity(ordered.len());
    for (name, layout) in ordered {
        align = align.max(layout.align);
        offset = align_up(offset, layout.align);
        field_layouts.push(FieldLayout {
            name: name.clone(),
            offset,
        });
        offset += layout.size;
    }

    Layout {
        size: align_up(offset, align),
        align,
        fields: field_layouts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paco_types::IntWidth;

    fn field(name: &str, layout: Layout) -> (String, Layout) {
        (name.to_string(), layout)
    }

    #[test]
    fn repr_c_matches_hand_computed_c_layout() {
        let fields = vec![
            field("a", scalar_layout(&Type::Bool).unwrap()),
            field("b", scalar_layout(&Type::Int(IntWidth::I64)).unwrap()),
            field("c", scalar_layout(&Type::Bool).unwrap()),
        ];

        let layout = struct_layout(&fields, Repr::C);

        assert_eq!(layout.align, 8);
        assert_eq!(layout.size, 24);
        assert_eq!(
            layout.fields,
            vec![
                FieldLayout {
                    name: "a".to_string(),
                    offset: 0
                },
                FieldLayout {
                    name: "b".to_string(),
                    offset: 8
                },
                FieldLayout {
                    name: "c".to_string(),
                    offset: 16
                },
            ]
        );
    }

    #[test]
    fn default_repr_reorders_to_minimize_padding() {
        let fields = vec![
            field("a", scalar_layout(&Type::Bool).unwrap()),
            field("b", scalar_layout(&Type::Int(IntWidth::I64)).unwrap()),
            field("c", scalar_layout(&Type::Bool).unwrap()),
        ];

        let layout = struct_layout(&fields, Repr::Default);

        assert_eq!(layout.align, 8);
        assert_eq!(layout.size, 16);
        assert_eq!(
            layout.fields,
            vec![
                FieldLayout {
                    name: "b".to_string(),
                    offset: 0
                },
                FieldLayout {
                    name: "a".to_string(),
                    offset: 8
                },
                FieldLayout {
                    name: "c".to_string(),
                    offset: 9
                },
            ]
        );
    }

    #[test]
    fn owned_slice_has_no_capacity_field() {
        let layout = scalar_layout(&Type::Slice(Box::new(Type::Float(paco_types::FloatWidth::F64)))).unwrap();
        assert_eq!(layout.size, 16);
        assert_eq!(layout.align, 8);
        assert!(layout.fields.is_empty());
    }

    #[test]
    fn borrowed_slice_is_a_fat_pointer() {
        let shared = scalar_layout(&Type::Borrow {
            mutable: false,
            ty: Box::new(Type::Slice(Box::new(Type::Float(paco_types::FloatWidth::F64)))),
        })
        .unwrap();
        let mutable = scalar_layout(&Type::Borrow {
            mutable: true,
            ty: Box::new(Type::Slice(Box::new(Type::Float(paco_types::FloatWidth::F64)))),
        })
        .unwrap();

        for layout in [shared, mutable] {
            assert_eq!(layout.size, 16);
            assert_eq!(layout.align, 8);
        }
    }

    #[test]
    fn borrow_of_a_non_slice_is_a_thin_pointer() {
        let layout = scalar_layout(&Type::Borrow {
            mutable: false,
            ty: Box::new(Type::Int(IntWidth::I64)),
        })
        .unwrap();

        assert_eq!(layout.size, 8);
        assert_eq!(layout.align, 8);
    }
}

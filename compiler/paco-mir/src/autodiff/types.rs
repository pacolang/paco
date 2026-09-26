//! Where the derivative of a value lives: its float leaves (statically named
//! paths into the value), its opaque tangent (a type whose `Tangent` is not
//! itself) and its heap elements (slices, whose adjoints are shadow buffers).

use paco_types::{FloatWidth, Type};

use crate::TypeLayouts;
use crate::body::Place;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Proj {
    Field(String),
    Variant(String, usize),
}

pub type Path = Vec<Proj>;

#[derive(Clone, Debug, PartialEq)]
pub enum LeafKind {
    Float(FloatWidth),
    /// A value whose `Tangent` differs from its own type: its adjoint is a
    /// whole `tangent` value, accumulated with `add`.
    Opaque { primal: Type, tangent: Type },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Leaf {
    pub path: Path,
    pub kind: LeafKind,
}

pub fn project(base: Place, path: &[Proj]) -> Place {
    path.iter().fold(base, |place, proj| match proj {
        Proj::Field(field) => Place::Field { base: Box::new(place), field: field.clone() },
        Proj::Variant(variant, index) => Place::VariantField { base: Box::new(place), variant: variant.clone(), index: *index },
    })
}

pub fn strip_borrow(ty: &Type) -> &Type {
    match ty {
        Type::Borrow { ty, .. } => strip_borrow(ty),
        other => other,
    }
}

pub struct Types<'l, 'a> {
    pub layouts: &'l TypeLayouts<'a>,
}

impl Types<'_, '_> {
    /// `Some(tangent)` when `ty` declares a `Tangent` other than itself.
    pub fn opaque_tangent(&self, ty: &Type) -> Option<Type> {
        let Type::Struct(name, args) = ty else { return None };
        let tangent = self.layouts.struct_assoc(name, args, "Tangent")?;
        (!self.same_type(&tangent, ty)).then_some(tangent)
    }

    /// Type equality where a struct named with or without its module is the
    /// same struct.
    pub fn same_type(&self, left: &Type, right: &Type) -> bool {
        match (left, right) {
            (Type::Struct(a, left_args), Type::Struct(b, right_args)) | (Type::Enum(a, left_args), Type::Enum(b, right_args)) => {
                (a == b || self.layouts.same_struct(a, b) || a.rsplit("::").next() == b.rsplit("::").next() && (a.contains("::") || b.contains("::")))
                    && left_args.len() == right_args.len()
                    && left_args.iter().zip(right_args).all(|(left, right)| self.same_type(left, right))
            }
            (Type::Tuple(left), Type::Tuple(right)) | (Type::Pack(left), Type::Pack(right)) => {
                left.len() == right.len() && left.iter().zip(right).all(|(left, right)| self.same_type(left, right))
            }
            (Type::Borrow { mutable: a, ty: left }, Type::Borrow { mutable: b, ty: right }) => a == b && self.same_type(left, right),
            (Type::Slice(left), Type::Slice(right)) => self.same_type(left, right),
            _ => left == right,
        }
    }

    pub fn leaves(&self, ty: &Type) -> Vec<Leaf> {
        let mut out = Vec::new();
        self.collect(ty, &mut Vec::new(), &mut out, 0);
        out
    }

    fn collect(&self, ty: &Type, path: &mut Path, out: &mut Vec<Leaf>, depth: usize) {
        if depth > 16 {
            return;
        }
        match ty {
            Type::Float(width) if width.has_arithmetic() => out.push(Leaf { path: path.clone(), kind: LeafKind::Float(*width) }),
            Type::Struct(name, args) => {
                if let Some(tangent) = self.opaque_tangent(ty) {
                    out.push(Leaf { path: path.clone(), kind: LeafKind::Opaque { primal: ty.clone(), tangent } });
                    return;
                }
                if !self.layouts.has_struct(name) {
                    return;
                }
                for (field, field_ty) in self.layouts.struct_field_list(name, args) {
                    path.push(Proj::Field(field));
                    self.collect(&field_ty, path, out, depth + 1);
                    path.pop();
                }
            }
            Type::Tuple(items) => {
                for (index, item) in items.iter().enumerate() {
                    path.push(Proj::Field(index.to_string()));
                    self.collect(item, path, out, depth + 1);
                    path.pop();
                }
            }
            Type::Enum(name, args) => {
                for (variant, (_, fields)) in self.layouts.enum_variant_names(name).into_iter().zip(self.layouts.enum_variants(name, args)) {
                    for (index, (field_ty, _)) in fields.iter().enumerate() {
                        path.push(Proj::Variant(variant.clone(), index));
                        self.collect(field_ty, path, out, depth + 1);
                        path.pop();
                    }
                }
            }
            _ => {}
        }
    }

    /// Whether a value of `ty` can carry a derivative at all.
    pub fn can_be_active(&self, ty: &Type) -> bool {
        self.carries(ty, 0)
    }

    fn carries(&self, ty: &Type, depth: usize) -> bool {
        if depth > 16 {
            return false;
        }
        match ty {
            Type::Float(width) => width.has_arithmetic(),
            Type::Borrow { ty, .. } | Type::Slice(ty) => self.carries(ty, depth + 1),
            Type::Struct(name, args) => {
                self.opaque_tangent(ty).is_some()
                    || self.layouts.has_struct(name)
                        && self.layouts.struct_field_list(name, args).iter().any(|(_, field)| self.carries(field, depth + 1))
            }
            Type::Tuple(items) => items.iter().any(|item| self.carries(item, depth + 1)),
            Type::Enum(name, args) => {
                self.layouts.enum_variants(name, args).iter().any(|(_, fields)| fields.iter().any(|(field, _)| self.carries(field, depth + 1)))
            }
            _ => false,
        }
    }

    /// Whether `ty` holds a slice whose elements can carry derivatives.
    pub fn has_heap(&self, ty: &Type) -> bool {
        self.heap(ty, 0)
    }

    fn heap(&self, ty: &Type, depth: usize) -> bool {
        if depth > 16 {
            return false;
        }
        match ty {
            Type::Slice(elem) => self.carries(elem, depth + 1),
            Type::Borrow { ty, .. } => self.heap(ty, depth + 1),
            Type::Struct(name, args) => {
                self.opaque_tangent(ty).is_none()
                    && self.layouts.has_struct(name)
                    && self.layouts.struct_field_list(name, args).iter().any(|(_, field)| self.heap(field, depth + 1))
            }
            Type::Tuple(items) => items.iter().any(|item| self.heap(item, depth + 1)),
            Type::Enum(name, args) => {
                self.layouts.enum_variants(name, args).iter().any(|(_, fields)| fields.iter().any(|(field, _)| self.heap(field, depth + 1)))
            }
            _ => false,
        }
    }

    /// Paths to every slice field of `ty` whose elements carry derivatives.
    pub fn heap_paths(&self, ty: &Type) -> Vec<(Path, Type)> {
        let mut out = Vec::new();
        self.collect_heap(ty, &mut Vec::new(), &mut out, 0);
        out
    }

    fn collect_heap(&self, ty: &Type, path: &mut Path, out: &mut Vec<(Path, Type)>, depth: usize) {
        if depth > 16 {
            return;
        }
        match ty {
            Type::Slice(elem) if self.carries(elem, depth + 1) => out.push((path.clone(), (**elem).clone())),
            Type::Struct(name, args) if self.opaque_tangent(ty).is_none() && self.layouts.has_struct(name) => {
                for (field, field_ty) in self.layouts.struct_field_list(name, args) {
                    path.push(Proj::Field(field));
                    self.collect_heap(&field_ty, path, out, depth + 1);
                    path.pop();
                }
            }
            Type::Tuple(items) => {
                for (index, item) in items.iter().enumerate() {
                    path.push(Proj::Field(index.to_string()));
                    self.collect_heap(item, path, out, depth + 1);
                    path.pop();
                }
            }
            _ => {}
        }
    }

    pub fn field_type(&self, ty: &Type, proj: &Proj) -> Type {
        match (strip_borrow(ty), proj) {
            (Type::Struct(name, args), Proj::Field(field)) => self
                .layouts
                .struct_field_list(name, args)
                .into_iter()
                .find(|(name, _)| name == field)
                .map(|(_, ty)| ty)
                .unwrap_or(Type::Unknown),
            (Type::Tuple(items), Proj::Field(field)) => field.parse::<usize>().ok().and_then(|index| items.get(index).cloned()).unwrap_or(Type::Unknown),
            (Type::Enum(name, args), Proj::Variant(variant, index)) => self.layouts.enum_variant_field(name, args, variant, *index).0,
            _ => Type::Unknown,
        }
    }

    pub fn path_type(&self, ty: &Type, path: &[Proj]) -> Type {
        path.iter().fold(strip_borrow(ty).clone(), |ty, proj| self.field_type(&ty, proj))
    }
}

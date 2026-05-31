//! Tree-walking evaluator for executable frontend features.

use std::collections::{BTreeMap, HashMap, HashSet};

use paco_syntax::ast::{
    BinaryOp, Block, Expr, FnDecl, Item, Literal, MatchArm, Module, Pat, Stmt, Ty, UnaryOp,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Struct {
        name: String,
        fields: BTreeMap<String, Value>,
    },
    Enum {
        name: String,
        variant: String,
        values: Vec<Value>,
    },
    Unit,
}

pub fn evaluate_module(module: &Module) -> Result<String, String> {
    let program = RuntimeProgram::from_module(module);
    let mut evaluator = Evaluator::new(program);
    evaluator.call_function("main", Vec::new())?;
    Ok(evaluator.output)
}

#[derive(Clone, Debug, Default)]
struct RuntimeProgram {
    functions: HashMap<String, FnDecl>,
    methods: HashMap<(String, String), FnDecl>,
    associated: HashMap<(String, String), FnDecl>,
    enum_variants: HashSet<(String, String)>,
}

impl RuntimeProgram {
    fn from_module(module: &Module) -> Self {
        let mut program = Self::default();
        for item in &module.items {
            match item {
                Item::Fn(function) => {
                    program
                        .functions
                        .insert(function.name.clone(), function.clone());
                }
                Item::Struct(decl) => {
                    for method in &decl.methods {
                        program.insert_attached_function(&decl.name, method);
                    }
                }
                Item::Enum(decl) => {
                    for variant in &decl.variants {
                        program
                            .enum_variants
                            .insert((decl.name.clone(), variant.name.clone()));
                    }
                    for method in &decl.methods {
                        program.insert_attached_function(&decl.name, method);
                    }
                }
                Item::Methods(block) => {
                    if let Some(name) = type_name(&block.target) {
                        for method in &block.methods {
                            program.insert_attached_function(&name, method);
                        }
                    }
                }
                Item::Trait(_) | Item::Use(_) => {}
            }
        }
        program
    }

    fn insert_attached_function(&mut self, type_name: &str, function: &FnDecl) {
        let key = (type_name.to_string(), function.name.clone());
        if has_self_receiver(function) {
            self.methods.insert(key, function.clone());
        } else {
            self.associated.insert(key, function.clone());
        }
    }
}

#[derive(Debug)]
struct Evaluator {
    program: RuntimeProgram,
    scopes: Vec<HashMap<String, Value>>,
    output: String,
}

impl Evaluator {
    fn new(program: RuntimeProgram) -> Self {
        Self {
            program,
            scopes: Vec::new(),
            output: String::new(),
        }
    }

    fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        let function = self
            .program
            .functions
            .get(name)
            .cloned()
            .ok_or_else(|| format!("function `{name}` was not found"))?;
        self.call_decl(&function, args)
    }

    fn call_decl(&mut self, function: &FnDecl, args: Vec<Value>) -> Result<Value, String> {
        self.call_decl_with_bindings(function, args)
            .map(|(value, _)| value)
    }

    fn call_decl_with_bindings(
        &mut self,
        function: &FnDecl,
        args: Vec<Value>,
    ) -> Result<(Value, HashMap<String, Value>), String> {
        if function.params.len() != args.len() {
            return Err(format!(
                "function `{}` expected {} arguments, found {}",
                function.name,
                function.params.len(),
                args.len()
            ));
        }

        self.scopes.push(HashMap::new());
        for (param, value) in function.params.iter().zip(args) {
            let Pat::Ident(name, _) = &param.pattern else {
                continue;
            };
            self.define(name.clone(), value);
        }

        let flow = match self.eval_block(&function.body) {
            Ok(flow) => flow,
            Err(error) => {
                self.scopes.pop();
                return Err(error);
            }
        };
        let bindings = self.scopes.pop().unwrap_or_default();

        let value = match flow {
            Flow::Value(value) | Flow::Return(value) => Ok(value),
            Flow::Break(_) => Err("break cannot escape a function".to_string()),
            Flow::Continue => Err("continue cannot escape a function".to_string()),
        }?;
        Ok((value, bindings))
    }

    fn eval_block(&mut self, block: &Block) -> Result<Flow, String> {
        self.scopes.push(HashMap::new());
        for statement in &block.stmts {
            match statement {
                Stmt::Let(statement) => {
                    let value = if let Some(value) = &statement.value {
                        match self.eval_value(value)? {
                            Ok(value) => value,
                            Err(flow) => {
                                self.scopes.pop();
                                return Ok(flow);
                            }
                        }
                    } else {
                        Value::Unit
                    };
                    let Pat::Ident(name, _) = &statement.pattern else {
                        continue;
                    };
                    self.define(name.clone(), value);
                }
                Stmt::Expr(expr) => match self.eval_expr(expr)? {
                    Flow::Value(_) => {}
                    flow => {
                        self.scopes.pop();
                        return Ok(flow);
                    }
                },
                Stmt::Item(_) => {}
            }
        }

        let flow = if let Some(tail) = &block.tail {
            self.eval_expr(tail)?
        } else {
            Flow::Value(Value::Unit)
        };
        self.scopes.pop();
        Ok(flow)
    }

    fn eval_expr(&mut self, expr: &Expr) -> Result<Flow, String> {
        match expr {
            Expr::Literal(literal, _) => Ok(Flow::Value(value_from_literal(literal))),
            Expr::Ident(name, _) => Ok(Flow::Value(
                self.lookup(name)
                    .ok_or_else(|| format!("name `{name}` was not found"))?,
            )),
            Expr::Block(block) => self.eval_block(block),
            Expr::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                let condition = match self.eval_value(condition)? {
                    Ok(value) => value,
                    Err(flow) => return Ok(flow),
                };
                let Value::Bool(condition) = condition else {
                    return Err("if condition must evaluate to bool".to_string());
                };
                if condition {
                    self.eval_block(then_branch)
                } else if let Some(else_branch) = else_branch {
                    self.eval_expr(else_branch)
                } else {
                    Ok(Flow::Value(Value::Unit))
                }
            }
            Expr::Loop { body, .. } => loop {
                match self.eval_block(body)? {
                    Flow::Value(_) | Flow::Continue => {}
                    Flow::Return(value) => break Ok(Flow::Return(value)),
                    Flow::Break(value) => {
                        break Ok(Flow::Value(value.unwrap_or(Value::Unit)));
                    }
                }
            },
            Expr::While {
                condition, body, ..
            } => {
                loop {
                    let condition = match self.eval_value(condition)? {
                        Ok(value) => value,
                        Err(flow) => return Ok(flow),
                    };
                    let Value::Bool(condition) = condition else {
                        return Err("while condition must evaluate to bool".to_string());
                    };
                    if !condition {
                        break;
                    }
                    match self.eval_block(body)? {
                        Flow::Value(_) | Flow::Continue => {}
                        Flow::Return(value) => return Ok(Flow::Return(value)),
                        Flow::Break(_) => break,
                    }
                }
                Ok(Flow::Value(Value::Unit))
            }
            Expr::Call { callee, args, .. } => self.eval_call(callee, args),
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => self.eval_method_call(receiver, method, args),
            Expr::AssociatedCall {
                ty, function, args, ..
            } => self.eval_associated_call(ty, function, args),
            Expr::Binary {
                op, left, right, ..
            } => {
                let left = match self.eval_value(left)? {
                    Ok(value) => value,
                    Err(flow) => return Ok(flow),
                };
                let right = match self.eval_value(right)? {
                    Ok(value) => value,
                    Err(flow) => return Ok(flow),
                };
                eval_binary(*op, left, right).map(Flow::Value)
            }
            Expr::Unary { op, expr, .. } => {
                let value = match self.eval_value(expr)? {
                    Ok(value) => value,
                    Err(flow) => return Ok(flow),
                };
                eval_unary(*op, value).map(Flow::Value)
            }
            Expr::Assign { target, value, .. } => {
                let value = match self.eval_value(value)? {
                    Ok(value) => value,
                    Err(flow) => return Ok(flow),
                };
                match target.as_ref() {
                    Expr::Ident(_, _) | Expr::Field { .. } => self.assign_place(target, value)?,
                    _ => return Err("assignment target must be an identifier or field".to_string()),
                }
                Ok(Flow::Value(Value::Unit))
            }
            Expr::Field { base, field, .. } => self.eval_field(base, field),
            Expr::Return(value, _) => {
                let value = if let Some(value) = value {
                    match self.eval_value(value)? {
                        Ok(value) => value,
                        Err(flow) => return Ok(flow),
                    }
                } else {
                    Value::Unit
                };
                Ok(Flow::Return(value))
            }
            Expr::Break(value, _) => {
                let value = if let Some(value) = value {
                    match self.eval_value(value)? {
                        Ok(value) => Some(value),
                        Err(flow) => return Ok(flow),
                    }
                } else {
                    None
                };
                Ok(Flow::Break(value))
            }
            Expr::Continue(_) => Ok(Flow::Continue),
            Expr::StructLiteral { ty, fields, .. } => self.eval_struct_literal(ty, fields),
            Expr::Match {
                scrutinee, arms, ..
            } => self.eval_match(scrutinee, arms),
            Expr::Index { .. }
            | Expr::Spawn { .. }
            | Expr::Select { .. }
            | Expr::Comptime { .. }
            | Expr::Yield(_, _)
            | Expr::Borrow { .. } => Err("expression is not executable yet".to_string()),
        }
    }

    fn eval_call(&mut self, callee: &Expr, args: &[Expr]) -> Result<Flow, String> {
        let Expr::Ident(name, _) = callee else {
            return Err("callee must be an identifier".to_string());
        };

        let values = match self.eval_values(args)? {
            Ok(values) => values,
            Err(flow) => return Ok(flow),
        };

        if name == "print" {
            if values.len() != 1 {
                return Err(format!("print expected 1 argument, found {}", values.len()));
            }
            self.output.push_str(&values[0].display());
            self.output.push('\n');
            return Ok(Flow::Value(Value::Unit));
        }

        self.call_function(name, values).map(Flow::Value)
    }

    fn eval_method_call(
        &mut self,
        receiver_expr: &Expr,
        method: &str,
        args: &[Expr],
    ) -> Result<Flow, String> {
        let receiver = match self.eval_value(receiver_expr)? {
            Ok(value) => value,
            Err(flow) => return Ok(flow),
        };
        let Some(type_name) = value_type_name(&receiver) else {
            return Err(format!("method `{method}` receiver is not a nominal value"));
        };
        let function = self
            .program
            .methods
            .get(&(type_name.clone(), method.to_string()))
            .cloned()
            .ok_or_else(|| format!("method `{method}` was not found for `{type_name}`"))?;
        let mut values = Vec::with_capacity(args.len() + 1);
        values.push(receiver);
        match self.eval_values(args)? {
            Ok(args) => values.extend(args),
            Err(flow) => return Ok(flow),
        }
        if has_mutable_self_receiver(&function) {
            let (result, bindings) = self.call_decl_with_bindings(&function, values)?;
            if let Some(updated_receiver) = bindings.get("self") {
                self.assign_place(receiver_expr, updated_receiver.clone())?;
            }
            return Ok(Flow::Value(result));
        }
        self.call_decl(&function, values).map(Flow::Value)
    }

    fn eval_associated_call(
        &mut self,
        ty: &Ty,
        function: &str,
        args: &[Expr],
    ) -> Result<Flow, String> {
        let Some(type_name) = type_name(ty) else {
            return Err("associated call target must be a nominal type".to_string());
        };
        let values = match self.eval_values(args)? {
            Ok(values) => values,
            Err(flow) => return Ok(flow),
        };
        if let Some(function_decl) = self
            .program
            .associated
            .get(&(type_name.clone(), function.to_string()))
            .cloned()
        {
            return self.call_decl(&function_decl, values).map(Flow::Value);
        }
        if self
            .program
            .enum_variants
            .contains(&(type_name.clone(), function.to_string()))
        {
            return Ok(Flow::Value(Value::Enum {
                name: type_name,
                variant: function.to_string(),
                values,
            }));
        }
        Err(format!(
            "associated function `{function}` was not found for `{type_name}`"
        ))
    }

    fn eval_struct_literal(&mut self, ty: &Ty, fields: &[(String, Expr)]) -> Result<Flow, String> {
        let Some(name) = type_name(ty) else {
            return Err("struct literal target must be a nominal type".to_string());
        };
        let mut values = BTreeMap::new();
        for (field, expr) in fields {
            let value = match self.eval_value(expr)? {
                Ok(value) => value,
                Err(flow) => return Ok(flow),
            };
            values.insert(field.clone(), value);
        }
        Ok(Flow::Value(Value::Struct {
            name,
            fields: values,
        }))
    }

    fn eval_match(&mut self, scrutinee: &Expr, arms: &[MatchArm]) -> Result<Flow, String> {
        let value = match self.eval_value(scrutinee)? {
            Ok(value) => value,
            Err(flow) => return Ok(flow),
        };

        for arm in arms {
            let Some(bindings) = pattern_bindings(&arm.pattern, &value)? else {
                continue;
            };

            self.scopes.push(bindings);
            let guard_matches = match &arm.guard {
                Some(guard) => match self.eval_value(guard)? {
                    Ok(Value::Bool(value)) => value,
                    Ok(_) => {
                        self.scopes.pop();
                        return Err("match guard must evaluate to bool".to_string());
                    }
                    Err(flow) => {
                        self.scopes.pop();
                        return Ok(flow);
                    }
                },
                None => true,
            };

            if guard_matches {
                let result = self.eval_expr(&arm.body);
                self.scopes.pop();
                return result;
            }
            self.scopes.pop();
        }

        Err("non-exhaustive match reached at runtime".to_string())
    }

    fn eval_field(&mut self, base: &Expr, field: &str) -> Result<Flow, String> {
        let value = match self.eval_value(base)? {
            Ok(value) => value,
            Err(flow) => return Ok(flow),
        };
        let Value::Struct { fields, .. } = value else {
            return Err(format!("field `{field}` receiver is not a struct"));
        };
        fields
            .get(field)
            .cloned()
            .map(Flow::Value)
            .ok_or_else(|| format!("field `{field}` was not found"))
    }

    fn eval_value(&mut self, expr: &Expr) -> Result<Result<Value, Flow>, String> {
        Ok(Self::value_or_flow(self.eval_expr(expr)?))
    }

    fn eval_values(&mut self, args: &[Expr]) -> Result<Result<Vec<Value>, Flow>, String> {
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            match self.eval_value(arg)? {
                Ok(value) => values.push(value),
                Err(flow) => return Ok(Err(flow)),
            }
        }
        Ok(Ok(values))
    }

    fn value_or_flow(flow: Flow) -> Result<Value, Flow> {
        match flow {
            Flow::Value(value) => Ok(value),
            flow => Err(flow),
        }
    }

    fn define(&mut self, name: String, value: Value) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, value);
        }
    }

    fn assign(&mut self, name: &str, value: Value) -> Result<(), String> {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), value);
                return Ok(());
            }
        }
        Err(format!("name `{name}` was not found"))
    }

    fn assign_field(&mut self, target: &Expr, value: Value) -> Result<(), String> {
        let Some((root, path)) = field_path(target) else {
            return Err("field assignment target must start with a binding".to_string());
        };
        for scope in self.scopes.iter_mut().rev() {
            if let Some(root_value) = scope.get_mut(root) {
                return assign_nested_field(root_value, &path, value);
            }
        }
        Err(format!("name `{root}` was not found"))
    }

    fn assign_place(&mut self, target: &Expr, value: Value) -> Result<(), String> {
        match target {
            Expr::Ident(name, _) => self.assign(name, value),
            Expr::Field { .. } => self.assign_field(target, value),
            _ => Err("assignment target must be an identifier or field".to_string()),
        }
    }

    fn lookup(&self, name: &str) -> Option<Value> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Flow {
    Value(Value),
    Return(Value),
    Break(Option<Value>),
    Continue,
}

fn value_from_literal(literal: &Literal) -> Value {
    match literal {
        Literal::Int(value) => Value::Int(*value),
        Literal::Float(value) => Value::Float(*value),
        Literal::Bool(value) => Value::Bool(*value),
        Literal::String(value) => Value::String(value.clone()),
        Literal::Char(value) => Value::String(value.to_string()),
    }
}

fn eval_binary(op: BinaryOp, left: Value, right: Value) -> Result<Value, String> {
    match (op, left, right) {
        // Int arithmetic
        (BinaryOp::Add, Value::Int(left), Value::Int(right)) => {
            checked_int(left.checked_add(right))
        }
        (BinaryOp::Sub, Value::Int(left), Value::Int(right)) => {
            checked_int(left.checked_sub(right))
        }
        (BinaryOp::Mul, Value::Int(left), Value::Int(right)) => {
            checked_int(left.checked_mul(right))
        }
        (BinaryOp::Div, Value::Int(_), Value::Int(0)) => Err("division by zero".to_string()),
        (BinaryOp::Div, Value::Int(left), Value::Int(right)) => {
            checked_int(left.checked_div(right))
        }
        (BinaryOp::Rem, Value::Int(_), Value::Int(0)) => Err("division by zero".to_string()),
        (BinaryOp::Rem, Value::Int(left), Value::Int(right)) => {
            checked_int(left.checked_rem(right))
        }
        // Float arithmetic
        (BinaryOp::Add, Value::Float(left), Value::Float(right)) => {
            Ok(Value::Float(left + right))
        }
        (BinaryOp::Sub, Value::Float(left), Value::Float(right)) => {
            Ok(Value::Float(left - right))
        }
        (BinaryOp::Mul, Value::Float(left), Value::Float(right)) => {
            Ok(Value::Float(left * right))
        }
        (BinaryOp::Div, Value::Float(_), Value::Float(0.0)) => {
            Err("division by zero".to_string())
        }
        (BinaryOp::Div, Value::Float(left), Value::Float(right)) => {
            Ok(Value::Float(left / right))
        }
        (BinaryOp::Rem, Value::Float(_), Value::Float(0.0)) => {
            Err("division by zero".to_string())
        }
        (BinaryOp::Rem, Value::Float(left), Value::Float(right)) => {
            Ok(Value::Float(left % right))
        }
        // Comparisons
        (BinaryOp::Eq, left, right) => Ok(Value::Bool(left == right)),
        (BinaryOp::Ne, left, right) => Ok(Value::Bool(left != right)),
        (BinaryOp::Lt, Value::Int(left), Value::Int(right)) => Ok(Value::Bool(left < right)),
        (BinaryOp::Le, Value::Int(left), Value::Int(right)) => Ok(Value::Bool(left <= right)),
        (BinaryOp::Gt, Value::Int(left), Value::Int(right)) => Ok(Value::Bool(left > right)),
        (BinaryOp::Ge, Value::Int(left), Value::Int(right)) => Ok(Value::Bool(left >= right)),
        (BinaryOp::Lt, Value::Float(left), Value::Float(right)) => Ok(Value::Bool(left < right)),
        (BinaryOp::Le, Value::Float(left), Value::Float(right)) => Ok(Value::Bool(left <= right)),
        (BinaryOp::Gt, Value::Float(left), Value::Float(right)) => Ok(Value::Bool(left > right)),
        (BinaryOp::Ge, Value::Float(left), Value::Float(right)) => Ok(Value::Bool(left >= right)),
        // Logical
        (BinaryOp::And, Value::Bool(left), Value::Bool(right)) => Ok(Value::Bool(left && right)),
        (BinaryOp::Or, Value::Bool(left), Value::Bool(right)) => Ok(Value::Bool(left || right)),
        _ => Err("binary operator received unsupported operands".to_string()),
    }
}

fn eval_unary(op: UnaryOp, value: Value) -> Result<Value, String> {
    match (op, value) {
        (UnaryOp::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
        (UnaryOp::Neg, Value::Int(value)) => checked_int(value.checked_neg()),
        (UnaryOp::Neg, Value::Float(value)) => Ok(Value::Float(-value)),
        _ => Err("unary operator received an unsupported operand".to_string()),
    }
}

fn pattern_bindings(
    pattern: &Pat,
    value: &Value,
) -> Result<Option<HashMap<String, Value>>, String> {
    let mut bindings = HashMap::new();
    if match_pattern(pattern, value, &mut bindings)? {
        Ok(Some(bindings))
    } else {
        Ok(None)
    }
}

fn match_pattern(
    pattern: &Pat,
    value: &Value,
    bindings: &mut HashMap<String, Value>,
) -> Result<bool, String> {
    let mut candidate = bindings.clone();
    if match_pattern_inner(pattern, value, &mut candidate)? {
        *bindings = candidate;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn match_pattern_inner(
    pattern: &Pat,
    value: &Value,
    bindings: &mut HashMap<String, Value>,
) -> Result<bool, String> {
    match pattern {
        Pat::Ident(name, _) => {
            bindings.insert(name.clone(), value.clone());
            Ok(true)
        }
        Pat::Wildcard(_) => Ok(true),
        Pat::Literal(literal, _) => Ok(value_matches_literal(literal, value)),
        Pat::Binding { name, pattern, .. } => {
            bindings.insert(name.clone(), value.clone());
            match_pattern(pattern, value, bindings)
        }
        Pat::Range {
            start,
            end,
            inclusive,
            ..
        } => {
            let Value::Int(value) = value else {
                return Ok(false);
            };
            let start = range_bound(start)?;
            let end = range_bound(end)?;
            if *inclusive {
                Ok(start <= *value && *value <= end)
            } else {
                Ok(start <= *value && *value < end)
            }
        }
        Pat::Enum { path, fields, .. } => {
            let Value::Enum {
                name,
                variant,
                values,
            } = value
            else {
                return Ok(false);
            };
            if path.first() != Some(name) || path.last() != Some(variant) {
                return Ok(false);
            }
            if fields.len() != values.len() {
                return Ok(false);
            }
            for (pattern, value) in fields.iter().zip(values) {
                if !match_pattern(pattern, value, bindings)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Pat::Struct { path, fields, .. } => {
            let Value::Struct {
                name,
                fields: values,
            } = value
            else {
                return Ok(false);
            };
            if path.first() != Some(name) {
                return Ok(false);
            }
            for (field, pattern) in fields {
                let Some(value) = values.get(field) else {
                    return Ok(false);
                };
                if !match_pattern(pattern, value, bindings)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Pat::Tuple(_, _) => Ok(false),
        Pat::Or(patterns, _) => {
            for pattern in patterns {
                if match_pattern(pattern, value, bindings)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

fn value_matches_literal(literal: &Literal, value: &Value) -> bool {
    match (literal, value) {
        (Literal::Int(left), Value::Int(right)) => left == right,
        (Literal::Float(left), Value::Float(right)) => left == right,
        (Literal::Bool(left), Value::Bool(right)) => left == right,
        (Literal::String(left), Value::String(right)) => left == right,
        (Literal::Char(left), Value::String(right)) => right == &left.to_string(),
        _ => false,
    }
}

fn range_bound(pattern: &Pat) -> Result<i64, String> {
    match pattern {
        Pat::Literal(Literal::Int(value), _) => Ok(*value),
        _ => Err("range pattern bounds must be integer literals".to_string()),
    }
}

fn checked_int(value: Option<i64>) -> Result<Value, String> {
    value
        .map(Value::Int)
        .ok_or_else(|| "integer overflow".to_string())
}

impl Value {
    fn display(&self) -> String {
        match self {
            Self::Int(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Bool(value) => value.to_string(),
            Self::String(value) => value.clone(),
            Self::Struct { name, fields } => {
                let fields = fields
                    .iter()
                    .map(|(name, value)| format!("{name}: {}", value.display()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name} {{ {fields} }}")
            }
            Self::Enum {
                name,
                variant,
                values,
            } if values.is_empty() => format!("{name}::{variant}"),
            Self::Enum {
                name,
                variant,
                values,
            } => {
                let values = values
                    .iter()
                    .map(Value::display)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}::{variant}({values})")
            }
            Self::Unit => "()".to_string(),
        }
    }
}

fn type_name(ty: &Ty) -> Option<String> {
    match ty {
        Ty::Path(path, _) | Ty::Generic { path, .. } => path.first().cloned(),
        _ => None,
    }
}

fn value_type_name(value: &Value) -> Option<String> {
    match value {
        Value::Struct { name, .. } | Value::Enum { name, .. } => Some(name.clone()),
        _ => None,
    }
}

fn has_self_receiver(function: &FnDecl) -> bool {
    function
        .params
        .first()
        .is_some_and(|param| matches!(&param.pattern, Pat::Ident(name, _) if name == "self"))
}

fn has_mutable_self_receiver(function: &FnDecl) -> bool {
    function.params.first().is_some_and(|param| {
        matches!(&param.pattern, Pat::Ident(name, _) if name == "self")
            && matches!(&param.ty, Ty::Borrow { mutable: true, .. })
    })
}

fn field_path(expr: &Expr) -> Option<(&str, Vec<&str>)> {
    match expr {
        Expr::Field { base, field, .. } => {
            let (root, mut path) = field_path(base)?;
            path.push(field.as_str());
            Some((root, path))
        }
        Expr::Ident(name, _) => Some((name.as_str(), Vec::new())),
        _ => None,
    }
}

fn assign_nested_field(value: &mut Value, path: &[&str], new_value: Value) -> Result<(), String> {
    let Some((field, rest)) = path.split_first() else {
        *value = new_value;
        return Ok(());
    };
    let Value::Struct { fields, .. } = value else {
        return Err(format!("field `{field}` receiver is not a struct"));
    };
    if rest.is_empty() {
        if !fields.contains_key(*field) {
            return Err(format!("field `{field}` was not found"));
        }
        fields.insert((*field).to_string(), new_value);
        return Ok(());
    }
    let field_value = fields
        .get_mut(*field)
        .ok_or_else(|| format!("field `{field}` was not found"))?;
    assign_nested_field(field_value, rest, new_value)
}

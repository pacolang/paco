//! Tree-walking evaluator for the Phase 1 executable subset.

use std::collections::HashMap;

use paco_syntax::ast::{BinaryOp, Block, Expr, FnDecl, Item, Literal, Module, Pat, Stmt, UnaryOp};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Unit,
}

pub fn evaluate_module(module: &Module) -> Result<String, String> {
    let functions = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(function) => Some((function.name.clone(), function.clone())),
            _ => None,
        })
        .collect();
    let mut evaluator = Evaluator::new(functions);
    evaluator.call_function("main", Vec::new())?;
    Ok(evaluator.output)
}

#[derive(Debug)]
struct Evaluator {
    functions: HashMap<String, FnDecl>,
    scopes: Vec<HashMap<String, Value>>,
    output: String,
}

impl Evaluator {
    fn new(functions: HashMap<String, FnDecl>) -> Self {
        Self {
            functions,
            scopes: Vec::new(),
            output: String::new(),
        }
    }

    fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        let function = self
            .functions
            .get(name)
            .cloned()
            .ok_or_else(|| format!("function `{name}` was not found"))?;
        if function.params.len() != args.len() {
            return Err(format!(
                "function `{name}` expected {} arguments, found {}",
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

        let flow = self.eval_block(&function.body)?;
        self.scopes.pop();

        match flow {
            Flow::Value(value) | Flow::Return(value) => Ok(value),
            Flow::Break(_) => Err("break cannot escape a function".to_string()),
            Flow::Continue => Err("continue cannot escape a function".to_string()),
        }
    }

    fn eval_block(&mut self, block: &Block) -> Result<Flow, String> {
        self.scopes.push(HashMap::new());
        for statement in &block.stmts {
            match statement {
                Stmt::Let(statement) => {
                    let value = if let Some(value) = &statement.value {
                        match Self::value_or_flow(self.eval_expr(value)?) {
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
                let condition = match Self::value_or_flow(self.eval_expr(condition)?) {
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
                    let condition = match Self::value_or_flow(self.eval_expr(condition)?) {
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
            Expr::Binary {
                op, left, right, ..
            } => {
                let left = match Self::value_or_flow(self.eval_expr(left)?) {
                    Ok(value) => value,
                    Err(flow) => return Ok(flow),
                };
                let right = match Self::value_or_flow(self.eval_expr(right)?) {
                    Ok(value) => value,
                    Err(flow) => return Ok(flow),
                };
                eval_binary(*op, left, right).map(Flow::Value)
            }
            Expr::Unary { op, expr, .. } => {
                let value = match Self::value_or_flow(self.eval_expr(expr)?) {
                    Ok(value) => value,
                    Err(flow) => return Ok(flow),
                };
                eval_unary(*op, value).map(Flow::Value)
            }
            Expr::Assign { target, value, .. } => {
                let Expr::Ident(name, _) = target.as_ref() else {
                    return Err("assignment target must be an identifier".to_string());
                };
                let value = match Self::value_or_flow(self.eval_expr(value)?) {
                    Ok(value) => value,
                    Err(flow) => return Ok(flow),
                };
                self.assign(name, value.clone())?;
                Ok(Flow::Value(value))
            }
            Expr::Return(value, _) => {
                let value = if let Some(value) = value {
                    match Self::value_or_flow(self.eval_expr(value)?) {
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
                    match Self::value_or_flow(self.eval_expr(value)?) {
                        Ok(value) => Some(value),
                        Err(flow) => return Ok(flow),
                    }
                } else {
                    None
                };
                Ok(Flow::Break(value))
            }
            Expr::Continue(_) => Ok(Flow::Continue),
            Expr::Match { .. }
            | Expr::MethodCall { .. }
            | Expr::AssociatedCall { .. }
            | Expr::Field { .. }
            | Expr::Index { .. }
            | Expr::Spawn { .. }
            | Expr::Select { .. }
            | Expr::Comptime { .. }
            | Expr::Yield(_, _)
            | Expr::StructLiteral { .. }
            | Expr::Borrow { .. } => Err("expression is not executable in Phase 1".to_string()),
        }
    }

    fn eval_call(&mut self, callee: &Expr, args: &[Expr]) -> Result<Flow, String> {
        let Expr::Ident(name, _) = callee else {
            return Err("callee must be an identifier".to_string());
        };

        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            let value = match Self::value_or_flow(self.eval_expr(arg)?) {
                Ok(value) => value,
                Err(flow) => return Ok(flow),
            };
            values.push(value);
        }

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
        (BinaryOp::Eq, left, right) => Ok(Value::Bool(left == right)),
        (BinaryOp::Ne, left, right) => Ok(Value::Bool(left != right)),
        (BinaryOp::Lt, Value::Int(left), Value::Int(right)) => Ok(Value::Bool(left < right)),
        (BinaryOp::Le, Value::Int(left), Value::Int(right)) => Ok(Value::Bool(left <= right)),
        (BinaryOp::Gt, Value::Int(left), Value::Int(right)) => Ok(Value::Bool(left > right)),
        (BinaryOp::Ge, Value::Int(left), Value::Int(right)) => Ok(Value::Bool(left >= right)),
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
            Self::Unit => "()".to_string(),
        }
    }
}

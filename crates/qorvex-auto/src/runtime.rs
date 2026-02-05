use std::collections::HashMap;
use std::fmt;

use crate::ast::*;
use crate::error::AutoError;

#[derive(Debug, Clone)]
pub enum Value {
    String(String),
    Number(i64),
    List(Vec<Value>),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::String(s) => write!(f, "{}", s),
            Value::Number(n) => write!(f, "{}", n),
            Value::List(items) => {
                write!(f, "[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", item)?;
                }
                write!(f, "]")
            }
        }
    }
}

impl Value {
    pub fn as_string(&self) -> String {
        match self {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::List(_) => self.to_string(),
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::String(s) => !s.is_empty(),
            Value::Number(n) => *n != 0,
            Value::List(items) => !items.is_empty(),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::String(a), Value::Number(b)) => *a == b.to_string(),
            (Value::Number(a), Value::String(b)) => a.to_string() == *b,
            _ => false,
        }
    }
}

pub struct Runtime {
    variables: HashMap<String, Value>,
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
        }
    }

    pub fn set(&mut self, name: String, value: Value) {
        self.variables.insert(name, value);
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.variables.get(name)
    }

    pub fn eval_expression(&self, expr: &Expression, line: usize) -> Result<Value, AutoError> {
        match expr {
            Expression::String(s) => Ok(Value::String(s.clone())),
            Expression::Number(n) => Ok(Value::Number(*n)),
            Expression::Variable(name) => {
                self.get(name).cloned().ok_or_else(|| AutoError::Runtime {
                    message: format!("Undefined variable: {}", name),
                    line,
                })
            }
            Expression::List(items) => {
                let values: Result<Vec<Value>, _> = items
                    .iter()
                    .map(|item| self.eval_expression(item, line))
                    .collect();
                Ok(Value::List(values?))
            }
            Expression::BinaryOp { op, left, right } => {
                let lhs = self.eval_expression(left, line)?;
                let rhs = self.eval_expression(right, line)?;
                match op {
                    BinOp::Add => {
                        // If both are numbers, do numeric add
                        if let (Value::Number(a), Value::Number(b)) = (&lhs, &rhs) {
                            Ok(Value::Number(a + b))
                        } else {
                            // Otherwise string concatenation
                            Ok(Value::String(format!("{}{}", lhs.as_string(), rhs.as_string())))
                        }
                    }
                    BinOp::Eq => Ok(Value::Number(if lhs == rhs { 1 } else { 0 })),
                    BinOp::NotEq => Ok(Value::Number(if lhs != rhs { 1 } else { 0 })),
                }
            }
            Expression::CommandCapture(_) => {
                // CommandCapture is handled by the executor, not the runtime directly
                Err(AutoError::Runtime {
                    message: "Command capture must be handled by executor".to_string(),
                    line,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_value_display() {
        assert_eq!(Value::String("hello".to_string()).to_string(), "hello");
        assert_eq!(Value::Number(42).to_string(), "42");
        assert_eq!(
            Value::List(vec![Value::Number(1), Value::Number(2)]).to_string(),
            "[1, 2]"
        );
    }

    #[test]
    fn test_value_as_string() {
        assert_eq!(Value::Number(42).as_string(), "42");
        assert_eq!(Value::String("hi".to_string()).as_string(), "hi");
    }

    #[test]
    fn test_value_equality() {
        assert_eq!(Value::String("42".to_string()), Value::Number(42));
        assert_eq!(Value::Number(42), Value::String("42".to_string()));
        assert_ne!(Value::String("hello".to_string()), Value::Number(42));
    }

    #[test]
    fn test_value_truthy() {
        assert!(Value::String("hello".to_string()).is_truthy());
        assert!(!Value::String("".to_string()).is_truthy());
        assert!(Value::Number(1).is_truthy());
        assert!(!Value::Number(0).is_truthy());
        assert!(Value::List(vec![Value::Number(1)]).is_truthy());
        assert!(!Value::List(vec![]).is_truthy());
    }

    #[test]
    fn test_runtime_set_get() {
        let mut rt = Runtime::new();
        rt.set("x".to_string(), Value::Number(42));
        assert_eq!(rt.get("x"), Some(&Value::Number(42)));
        assert_eq!(rt.get("y"), None);
    }

    #[test]
    fn test_eval_string() {
        let rt = Runtime::new();
        let val = rt.eval_expression(&Expression::String("hello".to_string()), 1).unwrap();
        assert_eq!(val.as_string(), "hello");
    }

    #[test]
    fn test_eval_number() {
        let rt = Runtime::new();
        let val = rt.eval_expression(&Expression::Number(42), 1).unwrap();
        match val {
            Value::Number(n) => assert_eq!(n, 42),
            _ => panic!("Expected Number"),
        }
    }

    #[test]
    fn test_eval_variable() {
        let mut rt = Runtime::new();
        rt.set("x".to_string(), Value::String("hello".to_string()));
        let val = rt.eval_expression(&Expression::Variable("x".to_string()), 1).unwrap();
        assert_eq!(val.as_string(), "hello");
    }

    #[test]
    fn test_eval_undefined_variable() {
        let rt = Runtime::new();
        let result = rt.eval_expression(&Expression::Variable("x".to_string()), 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_eval_add_numbers() {
        let rt = Runtime::new();
        let expr = Expression::BinaryOp {
            op: BinOp::Add,
            left: Box::new(Expression::Number(1)),
            right: Box::new(Expression::Number(2)),
        };
        match rt.eval_expression(&expr, 1).unwrap() {
            Value::Number(n) => assert_eq!(n, 3),
            _ => panic!("Expected Number"),
        }
    }

    #[test]
    fn test_eval_add_strings() {
        let rt = Runtime::new();
        let expr = Expression::BinaryOp {
            op: BinOp::Add,
            left: Box::new(Expression::String("hello".to_string())),
            right: Box::new(Expression::String(" world".to_string())),
        };
        assert_eq!(rt.eval_expression(&expr, 1).unwrap().as_string(), "hello world");
    }

    #[test]
    fn test_eval_add_string_number() {
        let rt = Runtime::new();
        let expr = Expression::BinaryOp {
            op: BinOp::Add,
            left: Box::new(Expression::String("step-".to_string())),
            right: Box::new(Expression::Number(3)),
        };
        assert_eq!(rt.eval_expression(&expr, 1).unwrap().as_string(), "step-3");
    }

    #[test]
    fn test_eval_eq() {
        let rt = Runtime::new();
        let expr = Expression::BinaryOp {
            op: BinOp::Eq,
            left: Box::new(Expression::String("Ready".to_string())),
            right: Box::new(Expression::String("Ready".to_string())),
        };
        assert!(rt.eval_expression(&expr, 1).unwrap().is_truthy());
    }

    #[test]
    fn test_eval_neq() {
        let rt = Runtime::new();
        let expr = Expression::BinaryOp {
            op: BinOp::NotEq,
            left: Box::new(Expression::String("Ready".to_string())),
            right: Box::new(Expression::String("Error".to_string())),
        };
        assert!(rt.eval_expression(&expr, 1).unwrap().is_truthy());
    }

    #[test]
    fn test_eval_list() {
        let rt = Runtime::new();
        let expr = Expression::List(vec![
            Expression::String("a".to_string()),
            Expression::String("b".to_string()),
        ]);
        match rt.eval_expression(&expr, 1).unwrap() {
            Value::List(items) => assert_eq!(items.len(), 2),
            _ => panic!("Expected List"),
        }
    }
}

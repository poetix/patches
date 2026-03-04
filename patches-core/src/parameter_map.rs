use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum ParameterValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    Enum(&'static str),
    // Array parameter values own their strings; patterns come from the DSL at runtime
    // and cannot be required to be 'static (unlike Enum variants, which are a closed
    // compile-time set declared in the descriptor).
    Array(Vec<String>),
}

impl ParameterValue {
    pub fn kind_name(&self) -> &'static str {
        match self {
            ParameterValue::Float(_) => "float",
            ParameterValue::Int(_) => "int",
            ParameterValue::Bool(_) => "bool",
            ParameterValue::Enum(_) => "enum",
            ParameterValue::Array(_) => "array",
        }
    }
}

pub type ParameterMap = HashMap<String, ParameterValue>;
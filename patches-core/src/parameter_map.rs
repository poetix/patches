use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum ParameterValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    Enum(&'static str),
    Array(Vec<&'static str>),
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
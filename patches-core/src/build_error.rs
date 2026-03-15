use std::fmt;

#[derive(Debug)]
pub enum BuildError {
    UnknownModule {
        name: String,
    },

    InvalidShape {
        module: &'static str,
        reason: String,
    },

    MissingParameter {
        module: &'static str,
        parameter: &'static str,
    },

    InvalidParameterType {
        module: &'static str,
        parameter: &'static str,
        expected: &'static str,
        found: &'static str,
    },

    ParameterOutOfRange {
        module: &'static str,
        parameter: &'static str,
        min: f32,
        max: f32,
        found: f32,
    },

    Custom {
        module: &'static str,
        message: String,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::UnknownModule { name } =>
                write!(f, "unknown module '{name}'"),

            BuildError::InvalidShape { module, reason } =>
                write!(f, "invalid shape for module '{module}': {reason}"),

            BuildError::MissingParameter { module, parameter } =>
                write!(f, "module '{module}' missing parameter '{parameter}'"),

            BuildError::InvalidParameterType {
                module, parameter, expected, found
            } =>
                write!(
                    f,
                    "module '{module}' parameter '{parameter}' expected {expected}, found {found}"
                ),

            BuildError::ParameterOutOfRange {
                module, parameter, min, max, found
            } =>
                write!(
                    f,
                    "module '{module}' parameter '{parameter}' out of range [{min}, {max}], found {found}"
                ),

            BuildError::Custom { module, message } =>
                write!(f, "module '{module}': {message}"),
        }
    }
}
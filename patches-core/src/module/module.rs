use crate::audio_environment::AudioEnvironment;
use crate::build_error::BuildError;
use crate::instance_id::InstanceId;
use crate::module_descriptor::{ModuleDescriptor, ModuleShape, ParameterKind};
use crate::parameter_map::{ParameterMap, ParameterValue};

/// A non-audio-rate control signal delivered to a module via [`Module::receive_signal`].
///
/// Signals are passed by value; modules that need to store the payload can do so directly.
/// New variants may be added in future; module implementations should use a wildcard arm
/// (`_ => {}`) to stay forward-compatible.
///
/// `ControlSignal` implements [`Send`] so it can be queued in the engine's ring buffer
/// (see T-0038). All current variants use only `Send`-safe types (`&'static str`, `f32`).
#[derive(Debug, Clone)]
pub enum ControlSignal {
    /// A single named float parameter update (e.g. frequency, gain).
    Float { name: &'static str, value: f32 },
}

/// Validate `params` against `descriptor`.
///
/// Returns an error if:
/// - Any key in `params` is not declared in `descriptor.parameters`.
/// - Any supplied value has the wrong type for its parameter.
/// - Any supplied numeric value is outside the bounds declared in its [`ParameterKind`].
/// - Any supplied enum value is not among the declared variants.
///
/// Missing parameters are not an error; they are simply left unchanged (or filled with
/// defaults by [`Module::build`] before this is called for the first time).
pub fn validate_parameters(
    params: &ParameterMap,
    descriptor: &ModuleDescriptor,
) -> Result<(), BuildError> {
    // Reject any key not declared in the descriptor.
    for name in params.keys() {
        if !descriptor.parameters.iter().any(|p| p.name == name.as_str()) {
            return Err(BuildError::Custom {
                module: descriptor.module_name,
                message: format!("unknown parameter '{name}'"),
            });
        }
    }

    // Validate type and bounds for each supplied parameter.
    for param_desc in &descriptor.parameters {
        let Some(value) = params.get(param_desc.name) else {
            continue;
        };
        match (&param_desc.parameter_type, value) {
            (ParameterKind::Float { min, max, .. }, ParameterValue::Float(v)) => {
                if *v < *min || *v > *max {
                    return Err(BuildError::ParameterOutOfRange {
                        module: descriptor.module_name,
                        parameter: param_desc.name,
                        min: *min,
                        max: *max,
                        found: *v,
                    });
                }
            }
            (ParameterKind::Int { min, max, .. }, ParameterValue::Int(v)) => {
                if *v < *min || *v > *max {
                    return Err(BuildError::ParameterOutOfRange {
                        module: descriptor.module_name,
                        parameter: param_desc.name,
                        min: *min as f32,
                        max: *max as f32,
                        found: *v as f32,
                    });
                }
            }
            (ParameterKind::Bool { .. }, ParameterValue::Bool(_)) => {}
            (ParameterKind::Enum { variants, .. }, ParameterValue::Enum(v)) => {
                if !variants.contains(v) {
                    return Err(BuildError::Custom {
                        module: descriptor.module_name,
                        message: format!(
                            "parameter '{}' has unrecognised value '{v}'",
                            param_desc.name
                        ),
                    });
                }
            }
            (ParameterKind::Array { length, .. }, ParameterValue::Array(v)) => {
                if v.len() > *length {
                    return Err(BuildError::Custom {
                        module: descriptor.module_name,
                        message: format!(
                            "parameter '{}' has {} elements but capacity is {}",
                            param_desc.name,
                            v.len(),
                            length,
                        ),
                    });
                }
            }
            _ => {
                return Err(BuildError::InvalidParameterType {
                    module: descriptor.module_name,
                    parameter: param_desc.name,
                    expected: param_desc.parameter_type.kind_name(),
                    found: value.kind_name(),
                });
            }
        }
    }

    Ok(())
}

/// Describes which input and output ports of a module are connected in the current patch.
///
/// `inputs[i]` is `true` if the i-th input port (as listed in [`ModuleDescriptor::inputs`])
/// has at least one incoming cable; `outputs[i]` is `true` if the i-th output port has at
/// least one outgoing cable.
///
/// Construct with [`PortConnectivity::new`], which fills both slices with `false`.
#[derive(Debug, Clone, PartialEq)]
pub struct PortConnectivity {
    pub inputs: Box<[bool]>,
    pub outputs: Box<[bool]>,
}

impl PortConnectivity {
    /// Create an all-`false` instance sized for a module with `n_inputs` input ports and
    /// `n_outputs` output ports.
    pub fn new(n_inputs: usize, n_outputs: usize) -> Self {
        Self {
            inputs: vec![false; n_inputs].into_boxed_slice(),
            outputs: vec![false; n_outputs].into_boxed_slice(),
        }
    }
}

/// The core trait all audio modules implement.
///
/// Construction follows a two-phase protocol:
///
/// 1. [`prepare`](Module::prepare) — allocates and initialises the instance with the audio
///    environment and descriptor. Other fields are set to their defaults. Infallible.
/// 2. [`update_validated_parameters`](Module::update_validated_parameters) — applies a
///    pre-validated [`ParameterMap`] to the instance.
///
/// [`build`](Module::build) has a default implementation that:
/// - Calls [`describe`](Module::describe) to get the descriptor.
/// - Calls [`prepare`](Module::prepare).
/// - Fills in any missing parameters from the descriptor's declared defaults.
/// - Calls [`update_parameters`](Module::update_parameters) (which validates then delegates).
///
/// [`update_parameters`](Module::update_parameters) has a default implementation that
/// validates via [`validate_parameters`] and, on success, calls
/// [`update_validated_parameters`](Module::update_validated_parameters).
///
/// `process` is called once per sample. Both `inputs` and `outputs` are indexed
/// according to the module's [`ModuleDescriptor`].
///
/// `as_any` enables downcasting from `&dyn Module` to a concrete type.
/// `as_sink` lets the patch builder detect a sink node — see [`Sink`].
pub trait Module: Send {
    /// Return the static descriptor for this module type, computed from the given shape.
    fn describe(shape: &ModuleShape) -> ModuleDescriptor
    where
        Self: Sized;

    /// Allocate and initialise a new instance, storing `audio_environment`, `descriptor`,
    /// and the externally-minted `instance_id`. All other fields should be set to their
    /// default/zero values.
    ///
    /// This is infallible; parameter validation is deferred to
    /// [`update_validated_parameters`](Module::update_validated_parameters).
    fn prepare(
        audio_environment: &AudioEnvironment,
        descriptor: ModuleDescriptor,
        instance_id: InstanceId,
    ) -> Self
    where
        Self: Sized;

    /// Apply an already-validated `params` to this instance, updating fields derived from
    /// parameters.
    ///
    /// Only called by the default [`update_parameters`](Module::update_parameters) after
    /// validation passes. All keys are guaranteed to be declared in the descriptor and their
    /// values are guaranteed to be correctly typed and within bounds.
    fn update_validated_parameters(&mut self, params: &ParameterMap);

    /// Validate `params` against the module's descriptor, then apply them.
    ///
    /// The default implementation calls [`validate_parameters`] and, on success, forwards
    /// to [`update_validated_parameters`](Module::update_validated_parameters). Override only
    /// if custom validation beyond what [`validate_parameters`] provides is needed.
    fn update_parameters(&mut self, params: &ParameterMap) -> Result<(), BuildError> {
        validate_parameters(params, self.descriptor())?;
        self.update_validated_parameters(params);
        Ok(())
    }

    /// Construct a fully initialised instance from an audio environment, shape, parameters,
    /// and an externally-minted `instance_id`.
    ///
    /// The default implementation:
    /// 1. Calls [`describe`](Module::describe) to obtain the descriptor.
    /// 2. Calls [`prepare`](Module::prepare) with the given `instance_id`.
    /// 3. Fills any missing parameters using the defaults declared in the descriptor.
    /// 4. Calls [`update_parameters`](Module::update_parameters) (validates then applies).
    ///
    /// Module implementations should not need to override this.
    fn build(
        audio_environment: &AudioEnvironment,
        shape: &ModuleShape,
        params: &ParameterMap,
        instance_id: InstanceId,
    ) -> Result<Self, BuildError>
    where
        Self: Sized,
    {
        let descriptor = Self::describe(shape);
        let mut instance = Self::prepare(audio_environment, descriptor, instance_id);

        // Fill in any missing parameters using the descriptor's declared defaults.
        let mut filled = params.clone();
        for param_desc in instance.descriptor().parameters.iter() {
            filled
                .entry(param_desc.name.to_string())
                .or_insert_with(|| param_desc.parameter_type.default_value());
        }

        instance.update_parameters(&filled)?;
        Ok(instance)
    }

    fn descriptor(&self) -> &ModuleDescriptor;

    /// The stable identity of this module instance.
    ///
    /// Must be assigned at construction time (e.g. via [`InstanceId::next()`]) and
    /// return the same value for the lifetime of the instance.
    fn instance_id(&self) -> InstanceId;

    fn process(&mut self, inputs: &[f32], outputs: &mut [f32]);

    /// Deliver a non-audio-rate control signal to this module.
    ///
    /// The default implementation is a no-op; modules that respond to control signals
    /// override this method. Unknown signal variants or parameter names should be
    /// silently ignored.
    fn receive_signal(&mut self, _signal: ControlSignal) {}

    /// Inform the module which of its ports are connected in the current patch.
    ///
    /// Called by the engine whenever the patch topology changes (e.g. after a hot-reload).
    /// Implementations may use this to skip computation for unconnected ports, cache
    /// derived coefficients, or mirror state across channels.
    ///
    /// **Implementations must not allocate, block, or perform I/O.** This method may be
    /// called on the audio thread immediately before the next `process` call.
    ///
    /// The default implementation is a no-op.
    fn set_connectivity(&mut self, _connectivity: PortConnectivity) {}

    fn as_any(&self) -> &dyn std::any::Any;

    /// Returns `Some(self)` if this module is a [`Sink`], `None` otherwise.
    fn as_sink(&self) -> Option<&dyn Sink> {
        None
    }
}

/// Marker trait for modules that act as the final audio output in a patch.
///
/// The patch builder uses this to locate the sink node without knowledge of
/// any concrete type. Implement this alongside [`Module`] and override
/// [`Module::as_sink`] to return `Some(self)`.
pub trait Sink: Module {
    /// Left-channel sample stored during the most recent [`Module::process`] call.
    fn last_left(&self) -> f32;
    /// Right-channel sample stored during the most recent [`Module::process`] call.
    fn last_right(&self) -> f32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_error::BuildError;
    use crate::module_descriptor::{ModuleDescriptor, ModuleShape, ParameterDescriptor, ParameterKind};
    use crate::parameter_map::{ParameterMap, ParameterValue};

    fn array_descriptor() -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "TestArrayModule",
            shape: ModuleShape { channels: 0, length: 16 },
            inputs: vec![],
            outputs: vec![],
            parameters: vec![ParameterDescriptor {
                name: "steps",
                index: 0,
                parameter_type: ParameterKind::Array { default: &[], length: 16 },
            }],
            is_sink: false,
        }
    }

    #[test]
    fn array_value_passes_validation_for_array_parameter() {
        let mut params = ParameterMap::new();
        params.insert(
            "steps".to_string(),
            ParameterValue::Array(vec!["C3".to_string()]),
        );
        let desc = array_descriptor();
        assert!(validate_parameters(&params, &desc).is_ok());
    }

    #[test]
    fn array_value_exceeding_length_returns_error() {
        let mut params = ParameterMap::new();
        params.insert(
            "steps".to_string(),
            ParameterValue::Array(vec!["C3".to_string(); 17]),
        );
        let desc = array_descriptor();
        let err = validate_parameters(&params, &desc).unwrap_err();
        assert!(
            matches!(err, BuildError::Custom { module: "TestArrayModule", .. }),
            "expected Custom error for capacity exceeded, got: {err:?}"
        );
    }

    #[test]
    fn float_value_against_array_descriptor_returns_invalid_parameter_type() {
        let mut params = ParameterMap::new();
        params.insert("steps".to_string(), ParameterValue::Float(1.0));
        let desc = array_descriptor();
        let err = validate_parameters(&params, &desc).unwrap_err();
        assert!(
            matches!(err, BuildError::InvalidParameterType { parameter: "steps", .. }),
            "expected InvalidParameterType, got: {err:?}"
        );
    }
}

use std::collections::HashMap;
use std::marker::PhantomData;
use crate::audio_environment::AudioEnvironment;
use crate::build_error::BuildError;
use crate::modules::{InstanceId, Module, ModuleDescriptor, ModuleShape, ParameterMap};
use super::module_builder::{Builder, ModuleBuilder};

pub struct Registry {
    builders: HashMap<String, Box<dyn ModuleBuilder>>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub fn new() -> Self {
        Self { builders: HashMap::new() }
    }

    pub fn register<T>(&mut self)
    where
        T: Module + 'static,
    {
        let name = T::describe(&ModuleShape { channels: 0, length: 0 }).module_name;
        self.builders
            .insert(name.to_string(), Box::new(Builder::<T>(PhantomData)));
    }

    pub fn describe(&self, name: &str, shape: &ModuleShape) -> Result<ModuleDescriptor, BuildError> {
        self.builders
            .get(name)
            .map(|builder| builder.describe(shape))
            .ok_or_else(|| BuildError::UnknownModule { name: name.to_string() })
    }

    pub fn create(
        &self,
        name: &str,
        audio_environment: &AudioEnvironment,
        shape: &ModuleShape,
        params: &ParameterMap,
        instance_id: InstanceId,
    ) -> Result<Box<dyn Module>, BuildError> {
        let builder = self
            .builders
            .get(name)
            .ok_or_else(|| BuildError::UnknownModule { name: name.to_string() })?;

        builder.build(audio_environment, shape, params, instance_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{InstanceId, ModuleDescriptor};

    struct TestModule {
        instance_id: InstanceId,
        descriptor: ModuleDescriptor,
    }

    impl Module for TestModule {
        fn describe(shape: &ModuleShape) -> ModuleDescriptor {
            ModuleDescriptor {
                module_name: "TestModule",
                shape: shape.clone(),
                inputs: vec![],
                outputs: vec![],
                parameters: vec![],
                is_sink: false,
            }
        }

        fn prepare(
            _audio_environment: &AudioEnvironment,
            descriptor: ModuleDescriptor,
            instance_id: InstanceId,
        ) -> Self {
            Self {
                instance_id,
                descriptor,
            }
        }

        fn update_validated_parameters(&mut self, _params: &ParameterMap) {
        }

        fn descriptor(&self) -> &ModuleDescriptor {
            &self.descriptor
        }

        fn instance_id(&self) -> InstanceId {
            self.instance_id
        }

        fn process(&mut self, _pool: &mut crate::cable_pool::CablePool<'_>) {}

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[test]
    fn build_a_module() {
        let mut registry = Registry::new();
        registry.register::<TestModule>();

        let shape = ModuleShape { channels: 2, length: 0 };
        let params = ParameterMap::new();
        let audio_environment = AudioEnvironment { sample_rate: 44100.0 };
        let module = registry.create("TestModule", &audio_environment, &shape, &params, InstanceId::next()).unwrap();

        assert_eq!(module.descriptor().module_name, "TestModule");
        assert_eq!(module.descriptor().shape.channels, 2);
    }
}

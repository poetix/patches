use std::marker::PhantomData;
use crate::audio_environment::AudioEnvironment;
use crate::build_error::BuildError;
use crate::modules::{InstanceId, Module, ModuleDescriptor, ModuleShape, ParameterMap};

pub trait ModuleBuilder: Send + Sync {
    fn describe(&self, shape: &ModuleShape) -> ModuleDescriptor;

    fn build(
        &self,
        audio_environment: &AudioEnvironment,
        shape: &ModuleShape,
        params: &ParameterMap,
        instance_id: InstanceId,
    ) -> Result<Box<dyn Module>, BuildError>;
}

pub struct Builder<T>(pub PhantomData<fn() -> T>);

impl<T> ModuleBuilder for Builder<T>
where
    T: Module + 'static,
{
    fn describe(&self, shape: &ModuleShape) -> ModuleDescriptor {
        T::describe(shape)
    }

    fn build(
        &self,
        audio_environment: &AudioEnvironment,
        shape: &ModuleShape,
        params: &ParameterMap,
        instance_id: InstanceId,
    ) -> Result<Box<dyn Module>, BuildError> {
        Ok(Box::new(T::build(audio_environment, shape, params, instance_id)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::InstanceId;

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
        let audio_environment = AudioEnvironment { sample_rate: 44100.0, poly_voices: 16 };
        let shape = ModuleShape { channels: 2, length: 0 };
        let params = ParameterMap::new();
        let builder = Builder::<TestModule>(PhantomData);
        let module = builder.build(&audio_environment, &shape, &params, InstanceId::next()).unwrap();

        assert_eq!(module.descriptor().module_name, "TestModule");
        assert_eq!(module.descriptor().shape.channels, 2);
    }
}
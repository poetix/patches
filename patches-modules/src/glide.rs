use patches_core::{
    AudioEnvironment, ControlSignal, InstanceId, Module,
    ModuleDescriptor, PortDescriptor
};

pub struct Glide {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    log_freq: f64,
    alpha: f64,
    beta: f64,
    glide_ms: f64,
    sample_rate: f64,
}

impl Glide {
    pub fn new(glide_ms: f64) -> Self {
        let mut glide = Self {
            instance_id: InstanceId::next(),
            descriptor: ModuleDescriptor {
                inputs: vec![
                    PortDescriptor { name: "in", index: 0 },
                ],
                outputs: vec![PortDescriptor { name: "out", index: 0 }],
            },
            log_freq: 0.0,
            alpha: 0.01,
            beta: 0.0,
            glide_ms: glide_ms,
            sample_rate: 44100.0,
        };
        glide.update_beta();
        glide
    }

    fn initialise(&mut self, env: &AudioEnvironment) {
        self.sample_rate = env.sample_rate;
        self.update_beta();
    }

    fn update_beta(&mut self) {
        let n_samples = self.sample_rate * self.glide_ms / 1000.0;
        self.beta = 1.0 - self.alpha.powf(1.0 / n_samples);
    }

    fn set_glide_ms(&mut self, glide_ms: f64) {
        self.glide_ms = glide_ms;
        self.update_beta();
    }
}

impl Module for Glide {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn receive_signal(&mut self, signal: ControlSignal) {
        if let ControlSignal::Float { name: "glide_ms", value } = signal {
            self.set_glide_ms(value);
        };
    }

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]) {
        let log_target = if inputs[0] > 0.0 { inputs[0].ln() } else { self.log_freq };
        self.log_freq += self.beta * (log_target - self.log_freq);
        outputs[0] = self.log_freq.exp();
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
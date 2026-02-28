pub fn module_message() -> String {
    patches_core::greet()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_returns_core_message() {
        assert_eq!(module_message(), "Hello from patches-core");
    }
}

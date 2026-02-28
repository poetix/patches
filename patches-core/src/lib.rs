pub fn greet() -> String {
    "Hello from patches-core".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_greets() {
        assert_eq!(greet(), "Hello from patches-core");
    }
}

/// Lowercase, ASCII letters and digits only, words joined by single dashes,
/// no leading or trailing dashes. Non-ASCII letters are dropped.
pub fn slugify(input: &str) -> String {
    todo!("slugify {input}")
}

#[cfg(test)]
mod tests {
    use super::slugify;

    #[test]
    fn basics() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("  many   spaces  "), "many-spaces");
        assert_eq!(slugify("Rust 2024 edition"), "rust-2024-edition");
        assert_eq!(slugify("--already--slugged--"), "already-slugged");
        assert_eq!(slugify("Crème brûlée"), "crme-brle");
        assert_eq!(slugify("!!!"), "");
    }
}

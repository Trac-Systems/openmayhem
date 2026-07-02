#![forbid(unsafe_code)]

pub const CRATE_NAME: &str = "mayhem-bridge";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_crate_name() {
        assert_eq!(CRATE_NAME, "mayhem-bridge");
    }
}

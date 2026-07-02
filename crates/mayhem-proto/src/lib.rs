#![forbid(unsafe_code)]

pub const CRATE_NAME: &str = "mayhem-proto";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_crate_name() {
        assert_eq!(CRATE_NAME, "mayhem-proto");
    }
}

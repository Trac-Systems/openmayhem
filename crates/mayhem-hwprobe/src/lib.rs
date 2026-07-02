#![forbid(unsafe_code)]

pub const CRATE_NAME: &str = "mayhem-hwprobe";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_crate_name() {
        assert_eq!(CRATE_NAME, "mayhem-hwprobe");
    }
}

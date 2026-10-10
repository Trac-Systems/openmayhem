use std::process::{Command, Stdio};

#[test]
fn metadata_is_available_without_starting_a_stdio_session() {
    let worker = env!("CARGO_BIN_EXE_mayhem-proxy-worker");
    for flag in ["--help", "-h", "--version", "-V"] {
        let output = Command::new(worker)
            .arg(flag)
            .env_clear()
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "{flag}");
        assert!(output.stderr.is_empty());
        let text = String::from_utf8(output.stdout).unwrap();
        if flag == "--version" || flag == "-V" {
            assert_eq!(
                text,
                format!("mayhem-proxy-worker {}\n", env!("CARGO_PKG_VERSION"))
            );
        } else {
            assert!(text.contains("--stdio-v1"));
            assert!(text.contains("--tokenizer-stdio-v2"));
        }
    }
    for args in [vec!["--invalid"], vec!["--help", "--stdio-v1"], vec![]] {
        let output = Command::new(worker)
            .args(args)
            .env_clear()
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
}

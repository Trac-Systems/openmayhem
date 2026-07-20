use std::{
    io::{self, Read, Write},
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use mayhem_attestation_verifier::{
    rejected_verdict, verifier_identity, verify_json_at, MAX_VERIFIER_INPUT_BYTES,
};
use serde::Serialize;

fn main() -> ExitCode {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() == 1 && args[0] == "--identity" {
        return emit(verifier_identity());
    }
    if !args.is_empty() {
        return emit(rejected_verdict(
            "invalid",
            "",
            0,
            "the verifier only accepts --identity; verification is stdin-only",
        ));
    }

    let mut input = Vec::new();
    match io::stdin()
        .take((MAX_VERIFIER_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut input)
    {
        Ok(_) if input.len() <= MAX_VERIFIER_INPUT_BYTES => {}
        Ok(_) => {
            return emit(rejected_verdict(
                "invalid",
                "",
                0,
                "verifier input exceeds the 8 MiB limit",
            ));
        }
        Err(error) => {
            return emit(rejected_verdict(
                "invalid",
                "",
                0,
                format!("could not read verifier input: {error}"),
            ));
        }
    }

    let now_unix = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => {
            return emit(rejected_verdict(
                "invalid",
                "",
                0,
                "system clock precedes the Unix epoch",
            ));
        }
    };
    emit(verify_json_at(&input, now_unix))
}

fn emit(value: impl Serialize) -> ExitCode {
    let mut stdout = io::stdout().lock();
    match serde_json::to_writer(&mut stdout, &value)
        .and_then(|_| stdout.write_all(b"\n").map_err(serde_json::Error::io))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

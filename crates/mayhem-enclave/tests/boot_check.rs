use std::fs;
use std::process::Command;

use mayhem_enclave::build_merkle_manifest;

const SECRET_HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

#[test]
fn boot_check_rejects_bit_flipped_sealed_chunk() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let artifact = temp.path().join("artifact.bin");
    let sealed_store = temp.path().join("sealed");
    fs::write(&artifact, b"tiny model artifact for cli boot check")?;
    let merkle = build_merkle_manifest(&artifact, 8)?;

    let seal = Command::new(env!("CARGO_BIN_EXE_mayhem-enclave"))
        .arg("seal-local")
        .arg("--artifact")
        .arg(&artifact)
        .arg("--sealed-store")
        .arg(&sealed_store)
        .arg("--provider-secret-hex")
        .arg(SECRET_HEX)
        .arg("--provider-id")
        .arg("provider-cli")
        .arg("--enclave-id")
        .arg("enclave-cli")
        .arg("--artifact-root")
        .arg(&merkle.root)
        .arg("--manifest-hash")
        .arg("manifest-cli")
        .arg("--chunk-size")
        .arg("8")
        .output()?;
    assert!(
        seal.status.success(),
        "seal-local failed: {}",
        String::from_utf8_lossy(&seal.stderr)
    );

    let chunk = sealed_store.join("chunks/00000000.seal");
    let mut bytes = fs::read(&chunk)?;
    bytes[0] ^= 0x40;
    fs::write(&chunk, bytes)?;

    let boot = Command::new(env!("CARGO_BIN_EXE_mayhem-enclave"))
        .arg("boot-check")
        .arg("--sealed-store")
        .arg(&sealed_store)
        .arg("--provider-secret-hex")
        .arg(SECRET_HEX)
        .arg("--provider-id")
        .arg("provider-cli")
        .arg("--enclave-id")
        .arg("enclave-cli")
        .arg("--artifact-root")
        .arg(&merkle.root)
        .arg("--manifest-hash")
        .arg("manifest-cli")
        .output()?;

    assert!(!boot.status.success(), "tampered boot unexpectedly passed");
    let stderr = String::from_utf8_lossy(&boot.stderr);
    assert!(
        stderr.contains("sealed artifact authentication failed at chunk 0"),
        "unexpected boot-check stderr: {stderr}"
    );
    Ok(())
}

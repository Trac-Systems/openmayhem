use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use mayhem_attestation::ValidatedAttestationPolicy;
use sev::{
    certs::snp::{Chain, Verifiable},
    firmware::guest::AttestationReport,
    parser::ByteParser,
};
use subtle::ConstantTimeEq;
use x509_cert::{der::Decode, Certificate};

use crate::{
    collateral::AuthenticatedCollateral, device_digest, evidence_gap, reject, Result, VerifiedCpu,
    VerifyRequest,
};

const MAX_SNP_REPORT_BYTES: usize = 1184;
const MAX_VCEK_BYTES: usize = 32 * 1024;
const AMD_BL_SPL_OID: &str = "1.3.6.1.4.1.3704.1.3.1";
const AMD_TEE_SPL_OID: &str = "1.3.6.1.4.1.3704.1.3.2";
const AMD_SNP_SPL_OID: &str = "1.3.6.1.4.1.3704.1.3.3";
const AMD_UCODE_SPL_OID: &str = "1.3.6.1.4.1.3704.1.3.8";
const AMD_HWID_OID: &str = "1.3.6.1.4.1.3704.1.4";

pub(crate) fn verify(
    request: &VerifyRequest,
    policy: &ValidatedAttestationPolicy,
    collateral: &AuthenticatedCollateral,
    report_b64: &str,
    vcek_der_b64: &str,
    now_unix: u64,
) -> Result<VerifiedCpu> {
    let report_bytes = decode_bounded(report_b64, MAX_SNP_REPORT_BYTES, "SNP report")?;
    if report_bytes.len() != MAX_SNP_REPORT_BYTES {
        return reject("SNP report is not exactly 1184 bytes");
    }
    let vcek_der = decode_bounded(vcek_der_b64, MAX_VCEK_BYTES, "AMD VCEK")?;
    let report = AttestationReport::from_bytes(&report_bytes)
        .map_err(|error| crate::VerifyError::Rejected(format!("SNP report is invalid: {error}")))?;
    if report.vmpl != 0 {
        return reject("SNP report was not issued at VMPL 0");
    }
    if report.policy.debug_allowed() {
        return reject("SNP guest policy permits debugging");
    }
    if report.policy.migrate_ma_allowed() {
        return reject("SNP guest policy permits a migration agent");
    }
    if !tcb_at_least(report.current_tcb, report.reported_tcb) {
        return reject("SNP reported TCB exceeds the signed current platform TCB");
    }
    let expected_binding = hex::decode(&request.quote.binding)
        .map_err(|_| crate::VerifyError::Rejected("quote binding is not hexadecimal".into()))?;
    if !bool::from(report.report_data[..32].ct_eq(&expected_binding)) {
        return reject("SNP REPORT_DATA does not bind the Mayhem hardware quote");
    }
    let device_id = device_digest(&report.chip_id)?;
    if device_id != request.evidence_binding.device_id {
        return reject("SNP chip identity does not match the selected route device");
    }

    let vcek = Certificate::from_der(&vcek_der)
        .map_err(|error| crate::VerifyError::Rejected(format!("VCEK is invalid DER: {error}")))?;
    verify_certificate_time(&vcek, now_unix, "VCEK")?;
    verify_vcek_extensions(&vcek, &report)?;

    let certs = collateral
        .for_kind(policy, request.kind)
        .filter(|item| item.media_type == "application/pkix-cert")
        .collect::<Vec<_>>();
    if certs.len() < 2 {
        return evidence_gap(
            "AMD native verification needs admin-authenticated ARK and ASK certificates",
        );
    }
    let mut verified_chain = false;
    for ark in &certs {
        for ask in &certs {
            if ark.id == ask.id {
                continue;
            }
            let Ok(ark_cert) = Certificate::from_der(&ark.bytes) else {
                continue;
            };
            let Ok(ask_cert) = Certificate::from_der(&ask.bytes) else {
                continue;
            };
            if verify_certificate_time(&ark_cert, now_unix, "ARK").is_err()
                || verify_certificate_time(&ask_cert, now_unix, "ASK").is_err()
            {
                continue;
            }
            let Ok(chain) = Chain::from_der(&ark.bytes, &ask.bytes, &vcek_der) else {
                continue;
            };
            if chain.verify().is_ok() && (&chain, &report).verify().is_ok() {
                verified_chain = true;
                break;
            }
        }
        if verified_chain {
            break;
        }
    }
    if !verified_chain {
        return reject(
            "SNP VCEK/report chain did not verify against admin-authenticated AMD ARK/ASK",
        );
    }

    let launch = hex::encode(report.measurement);
    let mut measurements = std::collections::BTreeMap::new();
    measurements.insert("snp_launch_measurement".to_owned(), launch);
    Ok(VerifiedCpu {
        roots: vec!["amd_sev_snp_vcek".to_owned()],
        cpu_measurements: measurements.clone(),
        workload_measurements: measurements,
        device_id,
        snp_chip_family: Some("amd-sev-snp".to_owned()),
        snp_chip_id: Some(hex::encode(report.chip_id)),
        snp_tcb: Some(format!(
            "bl{}-tee{}-snp{}-ucode{}",
            report.reported_tcb.bootloader,
            report.reported_tcb.tee,
            report.reported_tcb.snp,
            report.reported_tcb.microcode
        )),
    })
}

fn tcb_at_least(
    actual: sev::firmware::host::TcbVersion,
    required: sev::firmware::host::TcbVersion,
) -> bool {
    actual.fmc.unwrap_or_default() >= required.fmc.unwrap_or_default()
        && actual.bootloader >= required.bootloader
        && actual.tee >= required.tee
        && actual.snp >= required.snp
        && actual.microcode >= required.microcode
}

fn verify_vcek_extensions(certificate: &Certificate, report: &AttestationReport) -> Result<()> {
    let extensions = certificate
        .tbs_certificate
        .extensions
        .as_ref()
        .ok_or_else(|| crate::VerifyError::Rejected("VCEK has no extensions".into()))?;
    let mut spl = std::collections::BTreeMap::new();
    let mut hwid = None;
    for extension in extensions {
        let oid = extension.extn_id.to_string();
        match oid.as_str() {
            AMD_BL_SPL_OID | AMD_TEE_SPL_OID | AMD_SNP_SPL_OID | AMD_UCODE_SPL_OID => {
                spl.insert(oid, parse_der_u8(extension.extn_value.as_bytes())?);
            }
            AMD_HWID_OID => {
                let bytes = extension.extn_value.as_bytes();
                if bytes.len() != 64 {
                    return reject("VCEK HWID extension is not 64 bytes");
                }
                hwid = Some(bytes.to_vec());
            }
            _ => {}
        }
    }
    let expected = [
        (AMD_BL_SPL_OID, report.reported_tcb.bootloader),
        (AMD_TEE_SPL_OID, report.reported_tcb.tee),
        (AMD_SNP_SPL_OID, report.reported_tcb.snp),
        (AMD_UCODE_SPL_OID, report.reported_tcb.microcode),
    ];
    for (oid, value) in expected {
        if spl.get(oid).copied() != Some(value) {
            return reject(format!(
                "VCEK TCB extension {oid} does not match signed SNP reported TCB"
            ));
        }
    }
    if hwid.as_deref() != Some(report.chip_id.as_slice()) {
        return reject("VCEK HWID does not match the signed SNP chip identity");
    }
    Ok(())
}

fn parse_der_u8(bytes: &[u8]) -> Result<u8> {
    match bytes {
        [0x02, 0x01, value] => Ok(*value),
        [0x02, 0x02, 0x00, value] => Ok(*value),
        _ => reject("AMD VCEK SPL extension is not a canonical one-byte DER integer"),
    }
}

fn verify_certificate_time(certificate: &Certificate, now_unix: u64, label: &str) -> Result<()> {
    let validity = &certificate.tbs_certificate.validity;
    let not_before = validity.not_before.to_unix_duration().as_secs();
    let not_after = validity.not_after.to_unix_duration().as_secs();
    if now_unix < not_before || now_unix > not_after {
        return reject(format!(
            "{label} certificate is stale or not yet valid at verification time"
        ));
    }
    Ok(())
}

fn decode_bounded(value: &str, maximum: usize, field: &str) -> Result<Vec<u8>> {
    if value.len() > maximum.saturating_mul(2) {
        return reject(format!("{field} exceeds its encoded size limit"));
    }
    let bytes = BASE64
        .decode(value)
        .map_err(|_| crate::VerifyError::Rejected(format!("{field} is not base64")))?;
    if bytes.len() > maximum {
        return reject(format!("{field} exceeds its decoded size limit"));
    }
    Ok(bytes)
}

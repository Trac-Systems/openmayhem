use std::collections::BTreeMap;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use dcap_qvl::{verify::QuoteVerifier, QuoteCollateralV3};
use mayhem_attestation::ValidatedAttestationPolicy;
use subtle::ConstantTimeEq;

use crate::{
    collateral::AuthenticatedCollateral, device_digest, evidence_gap, reject, Result, VerifiedCpu,
    VerifyRequest,
};

const MAX_TDX_QUOTE_BYTES: usize = 1024 * 1024;

pub(crate) fn verify(
    request: &VerifyRequest,
    policy: &ValidatedAttestationPolicy,
    collateral: &AuthenticatedCollateral,
    quote_b64: &str,
    dcap: &QuoteCollateralV3,
    now_unix: u64,
) -> Result<VerifiedCpu> {
    if quote_b64.len() > MAX_TDX_QUOTE_BYTES.saturating_mul(2) {
        return reject("TDX quote exceeds its encoded size limit");
    }
    let quote = BASE64
        .decode(quote_b64)
        .map_err(|_| crate::VerifyError::Rejected("TDX quote is not base64".into()))?;
    if quote.is_empty() || quote.len() > MAX_TDX_QUOTE_BYTES {
        return reject("TDX quote exceeds its decoded size limit");
    }

    let roots = collateral
        .for_kind(policy, request.kind)
        .filter(|item| item.media_type == "application/pkix-cert")
        .collect::<Vec<_>>();
    if roots.is_empty() {
        return evidence_gap(
            "Intel TDX verification needs an admin-authenticated Intel DCAP root certificate",
        );
    }
    let mut failures = Vec::new();
    let mut verified = None;
    for root in roots {
        match QuoteVerifier::new(root.bytes.clone()).verify(&quote, dcap, now_unix) {
            Ok(report) => {
                verified = Some(report);
                break;
            }
            Err(error) => failures.push(error.to_string()),
        }
    }
    let verified = verified.ok_or_else(|| {
        crate::VerifyError::Rejected(format!(
            "TDX quote/collateral verification failed against admin root: {}",
            failures
                .first()
                .map(String::as_str)
                .unwrap_or("no root accepted the chain")
        ))
    })?;
    if verified.status != "UpToDate" {
        return reject(format!(
            "TDX platform TCB status is {}, not UpToDate",
            verified.status
        ));
    }
    let td = verified
        .report
        .as_td10()
        .ok_or_else(|| crate::VerifyError::Rejected("DCAP quote is not a TDX report".into()))?;
    let expected_binding = hex::decode(&request.quote.binding)
        .map_err(|_| crate::VerifyError::Rejected("quote binding is not hexadecimal".into()))?;
    if !bool::from(td.report_data[..32].ct_eq(&expected_binding)) {
        return reject("TDX REPORTDATA does not bind the Mayhem hardware quote");
    }
    let device_id = device_digest(&verified.ppid)?;
    if device_id != request.evidence_binding.device_id {
        return reject("TDX PPID identity does not match the selected route device");
    }

    let cpu_measurements = BTreeMap::from([
        ("mr_seam".to_owned(), hex::encode(td.mr_seam)),
        ("mr_signer_seam".to_owned(), hex::encode(td.mr_signer_seam)),
    ]);
    let workload_measurements = BTreeMap::from([
        ("mr_td".to_owned(), hex::encode(td.mr_td)),
        ("mr_config_id".to_owned(), hex::encode(td.mr_config_id)),
        ("mr_owner".to_owned(), hex::encode(td.mr_owner)),
        (
            "mr_owner_config".to_owned(),
            hex::encode(td.mr_owner_config),
        ),
        ("rt_mr0".to_owned(), hex::encode(td.rt_mr0)),
        ("rt_mr1".to_owned(), hex::encode(td.rt_mr1)),
        ("rt_mr2".to_owned(), hex::encode(td.rt_mr2)),
        ("rt_mr3".to_owned(), hex::encode(td.rt_mr3)),
    ]);
    Ok(VerifiedCpu {
        roots: vec!["intel_tdx_dcap".to_owned()],
        cpu_measurements,
        workload_measurements,
        device_id,
        ..VerifiedCpu::default()
    })
}

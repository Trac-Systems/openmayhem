//! One protected signed record and one durable high-water checkpoint. No history
//! traversal, registry lookup, financial access or upstream work during refresh.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclarationSource {
    pub directory: PathBuf,
    pub draft_id: Digest,
    pub subject: declaration::Subject,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Checkpoint {
    schema_version: u32,
    draft_id: Digest,
    signed: declaration::Signed,
    expired: bool,
}
impl Checkpoint {
    fn validate(&self, draft: &Digest, identity: &declaration::Subject) -> Result<()> {
        require(self.schema_version == 1 && &self.draft_id == draft)?;
        self.signed.verify().map_err(|_| Error::Invalid)?;
        require(
            self.signed.body.subject.provider == identity.provider
                && self.signed.body.subject.network == identity.network,
        )
    }
}
// Authoring must advance the durable observed high-water too, even if an older
// signed file was restored. This does not make old checkpoint claims eligible.
pub(super) fn authoring_revision(
    guard: &store::Guard,
    record: &Record,
    previous: u64,
) -> Result<u64> {
    let Some(saved) = guard.read_json::<Checkpoint>("wizard-declaration-observed.json")? else {
        return Ok(previous);
    };
    saved.validate(&record.id, &subject(record)?)?;
    Ok(previous.max(saved.signed.body.revision))
}
pub(super) fn latest_signed(
    guard: &store::Guard,
    record: &Record,
    previous: declaration::Signed,
) -> Result<declaration::Signed> {
    let Some(saved) = guard.read_json::<Checkpoint>("wizard-declaration-observed.json")? else {
        return Ok(previous);
    };
    saved.validate(&record.id, &subject(record)?)?;
    if saved.signed.body.revision == previous.body.revision {
        require(
            saved.signed.digest().map_err(|_| Error::Invalid)?
                == previous.digest().map_err(|_| Error::Invalid)?,
        )?;
    }
    Ok(if saved.signed.body.revision > previous.body.revision {
        saved.signed
    } else {
        previous
    })
}
pub(super) fn observed(
    guard: &store::Guard,
    record: &Record,
    signed: &declaration::Signed,
    now: u64,
) -> Result<bool> {
    let Some(saved) = guard.read_json::<Checkpoint>("wizard-declaration-observed.json")? else {
        return Ok(false);
    };
    saved.validate(&record.id, &subject(record)?)?;
    Ok(!saved.expired
        && saved.signed.signature == signed.signature
        && saved.signed.check(&subject(record)?, now).is_ok())
}
impl DeclarationSource {
    pub(in crate::setup) fn from_record(directory: &Path, record: &Record) -> Result<Self> {
        require(directory.is_absolute())?;
        Ok(Self {
            directory: directory.to_owned(),
            draft_id: record.id.clone(),
            subject: subject(record)?,
        })
    }

    /// Caller runs this outside inference tasks. Missing/invalid input means no
    /// eligible declaration, never reuse the checkpoint as a claims fallback.
    /// Commit the anti-rollback checkpoint before returning a newer signature.
    pub(crate) fn refresh(
        &self,
        now: u64,
        expired_identity: Option<(u64, Digest)>,
    ) -> Result<Option<declaration::Signed>> {
        require(self.directory.is_absolute())?;
        // Guard::open uses a nonblocking exclusive flock, including for readers.
        let guard = store::Guard::open(&self.directory)?;
        let mut previous: Option<Checkpoint> =
            guard.read_json("wizard-declaration-observed.json")?;
        if let Some(previous) = &mut previous {
            previous.validate(&self.draft_id, &self.subject)?;
            let expired_in_memory = expired_identity.as_ref().is_some_and(|(revision, digest)| {
                *revision == previous.signed.body.revision
                    && previous.signed.digest().ok().as_ref() == Some(digest)
            });
            if !previous.expired && (expired_in_memory || now >= previous.signed.body.expires_at_ms)
            {
                previous.expired = true;
                // Persist the expiry even if the input below disappeared or was
                // corrupted. No restart may resurrect this observed promise.
                guard.write_json(
                    "wizard-declaration-observed.json",
                    "wizard-declaration-observed.next",
                    previous,
                )?;
            }
        }
        let retained: Retained = guard
            .read_json("wizard-declaration-signed.json")?
            .ok_or(Error::Missing)?;
        retained.validate()?;
        require(retained.plan.draft_id == self.draft_id)?;
        let signed = retained.signed.ok_or(Error::Invalid)?;
        require(signed.body.subject == self.subject)?;
        let mut expired = now >= signed.body.expires_at_ms;
        if let Some((revision, digest)) = expired_identity {
            if revision == signed.body.revision
                && digest == signed.digest().map_err(|_| Error::Invalid)?
            {
                expired = true;
            }
        }
        if let Some(previous) = &previous {
            previous.validate(&self.draft_id, &self.subject)?;
            if signed.body.revision < previous.signed.body.revision {
                return Err(Error::Conflict);
            }
            if signed.body.revision == previous.signed.body.revision {
                require(
                    signed.digest().map_err(|_| Error::Invalid)?
                        == previous.signed.digest().map_err(|_| Error::Invalid)?,
                )?;
                expired |= previous.expired;
                if expired == previous.expired {
                    return Ok((!expired && now >= signed.body.issued_at_ms).then_some(signed));
                }
            }
        }
        // The single checkpoint keeps the highest revision across subject
        // changes too. An older controller cannot resurrect a prior route.
        guard.write_json(
            "wizard-declaration-observed.json",
            "wizard-declaration-observed.next",
            &Checkpoint {
                schema_version: 1,
                draft_id: self.draft_id.clone(),
                signed: signed.clone(),
                expired,
            },
        )?;
        Ok((!expired && now >= signed.body.issued_at_ms).then_some(signed))
    }
}

#[cfg(all(test, unix))]
mod tests;

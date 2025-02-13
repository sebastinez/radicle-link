// Copyright © 2019-2020 The Radicle Foundation <hello@radicle.foundation>
//
// This file is part of radicle-link, distributed under the GPLv3 with Radicle
// Linking Exception. For full terms see the included LICENSE file.

use std::{convert::TryFrom, fmt::Debug};

use radicle_git_ext::{self as ext, is_not_found_err, OneLevel};

use super::{
    super::{
        refs::Refs,
        storage::{self, ReadOnlyStorage as _, Storage},
        types::Reference,
    },
    common,
    error::Error,
    local::LocalIdentity,
};
use crate::{
    identities::{
        self,
        delegation,
        git::{Identities, Verifying},
        urn,
    },
    peer::PeerId,
};

pub use identities::{
    git::{Person, Urn, VerifiedPerson},
    payload::PersonPayload,
};

/// Read a [`Person`] from the tip of the ref [`Urn::path`] points to.
///
/// If the ref is not found, `None` is returned.
#[tracing::instrument(level = "trace", skip(storage))]
pub fn get<S>(storage: &S, urn: &Urn) -> Result<Option<Person>, Error>
where
    S: AsRef<storage::ReadOnly>,
{
    let storage = storage.as_ref();
    match storage.reference(&Reference::try_from(urn)?) {
        Ok(Some(reference)) => {
            let tip = reference.peel_to_commit()?.id();
            Ok(Some(identities(storage).get(tip)?))
        },

        Ok(None) => Ok(None),
        Err(storage::Error::Git(e)) if is_not_found_err(&e) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Read and verify the [`Person`] pointed to by `urn`.
///
/// If the ref pointed to by [`Urn::path`] is not found, `None` is returned.
///
/// # Caveats
///
/// Keep in mind that the `content_id` of a successfully verified person may
/// not be the same as the tip of the ref [`Urn::path`] points to. That is, this
/// function cannot be used to assert that the state after an [`update`] is
/// valid.
#[tracing::instrument(level = "debug", skip(storage))]
pub fn verify<S>(storage: &S, urn: &Urn) -> Result<Option<VerifiedPerson>, Error>
where
    S: AsRef<storage::ReadOnly>,
{
    let storage = storage.as_ref();
    let branch = Reference::try_from(urn)?;
    tracing::debug!("verifying {} from {}", urn, branch);
    match storage.reference(&branch) {
        Ok(Some(reference)) => {
            let tip = reference.peel_to_commit()?.id();
            identities(storage)
                .verify(tip)
                .map(Some)
                .map_err(|e| Error::Verify(e.into()))
        },

        Ok(None) => Ok(None),
        Err(storage::Error::Git(e)) if is_not_found_err(&e) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Get the root [`Urn`] for the given `payload` and set of `delegations`.
#[tracing::instrument(level = "debug", skip(storage))]
pub fn urn<S, P>(storage: &S, payload: P, delegations: delegation::Direct) -> Result<Urn, Error>
where
    S: AsRef<storage::ReadOnly>,
    P: Into<PersonPayload> + Debug,
{
    let storage = storage.as_ref();
    let (_, revision) = identities(storage).base(payload.into(), delegations)?;
    Ok(Urn::new(revision))
}

/// Create a new [`Person`].
///
/// The `delegations` must include the [`Storage`]'s [`crate::signer::Signer`]
/// key, such that the newly created [`Person`] is also a valid
/// [`LocalIdentity`] -- it is, in fact, its own [`LocalIdentity`]. This can be
/// changed via [`update`].
#[tracing::instrument(level = "debug", skip(storage))]
pub fn create<P>(
    storage: &Storage,
    payload: P,
    delegations: delegation::Direct,
) -> Result<Person, Error>
where
    P: Into<PersonPayload> + Debug,
{
    let person = {
        let person = identities(storage).create(payload.into(), delegations, storage.signer())?;
        let verified = identities(storage)
            .verify(*person.content_id)
            .map_err(|e| Error::Verify(e.into()))?;
        LocalIdentity::valid(verified, storage.signer())
    }?;

    let urn = person.urn();
    common::IdRef::from(&urn).create(storage, person.content_id)?;
    person.link(storage, &urn)?;
    Refs::update(storage, &urn)?;

    Ok(person.into_inner().into_inner())
}

/// Update the [`Person`] at `urn`.
#[tracing::instrument(level = "debug", skip(storage))]
pub fn update<L, P, D>(
    storage: &Storage,
    urn: &Urn,
    whoami: L,
    payload: P,
    delegations: D,
) -> Result<Person, Error>
where
    L: Into<Option<LocalIdentity>> + Debug,
    P: Into<Option<PersonPayload>> + Debug,
    D: Into<Option<delegation::Direct>> + Debug,
{
    let prev = get(storage, urn)?.ok_or_else(|| Error::NotFound(urn.clone()))?;
    let prev = Verifying::from(prev).signed()?;
    let next = identities(storage).update(prev, payload, delegations, storage.signer())?;

    common::IdRef::from(urn).update(storage, next.content_id, "update")?;
    if let Some(local_id) = whoami.into() {
        local_id.link(storage, urn)?;
    }
    Refs::update(storage, urn)?;

    Ok(next)
}

/// Merge and sign the [`Person`] state as seen by `from`.
#[tracing::instrument(level = "debug", skip(storage))]
pub fn merge(storage: &Storage, urn: &Urn, from: PeerId) -> Result<Person, Error> {
    let ours = get(storage, urn)?.ok_or_else(|| Error::NotFound(urn.clone()))?;
    let theirs = {
        let (path, rad) = OneLevel::from_qualified(urn::DEFAULT_PATH.clone());
        let rad = rad.expect("default path should be refs/rad/id");
        let their_urn = Urn {
            id: urn.id,
            path: Some(reflike!("refs/remotes").join(from).join(rad).join(path)),
        };
        get(storage, &their_urn)?.ok_or(Error::NotFound(their_urn))?
    };

    let ours = Verifying::from(ours).signed()?;
    let theirs = Verifying::from(theirs).signed()?;
    let next = identities(storage).update_from(ours, theirs, storage.signer())?;

    common::IdRef::from(urn).update(storage, next.content_id, &format!("merge from {}", from))?;
    Refs::update(storage, urn)?;

    Ok(next)
}

/// Return the newer of `a` and `b`, or an error if their histories are
/// unrelated.
pub fn newer<S>(storage: &S, a: VerifiedPerson, b: VerifiedPerson) -> Result<VerifiedPerson, Error>
where
    S: AsRef<storage::ReadOnly>,
{
    Ok(verified(storage).newer(a, b)?)
}

pub fn fast_forward(storage: &Storage, latest: &VerifiedPerson) -> Result<Option<ext::Oid>, Error> {
    let urn = latest.urn().with_path(None);
    let id_ref = super::common::IdRef::from(&urn);
    let canonical = id_ref.oid(storage)?;
    let tip = latest.content_id;
    Ok(if storage.as_raw().graph_descendant_of(*tip, *canonical)? {
        id_ref
            .update(storage, tip, &format!("fast-forward to {}", tip))
            .map_err(|e| Error::Store(e.into()))?;
        Some(tip)
    } else {
        None
    })
}

fn identities<S>(storage: &S) -> Identities<Person>
where
    S: AsRef<storage::ReadOnly>,
{
    storage.as_ref().identities()
}

fn verified<S>(storage: &S) -> Identities<VerifiedPerson>
where
    S: AsRef<storage::ReadOnly>,
{
    storage.as_ref().identities()
}

// This file is part of radicle-link
// <https://github.com/radicle-dev/radicle-link>
//
// Copyright (C) 2019-2020 The Radicle Team <dev@radicle.xyz>
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License version 3 or
// later as published by the Free Software Foundation.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

use std::ops::Range;

use git_ext::{is_exists_err, is_not_found_err, RefLike};
use std_ext::result::ResultExt as _;
use thiserror::Error;

use super::{
    p2p::url::GitUrlRef,
    storage2::{self, Storage},
};
use crate::{peer::PeerId, signer::Signer};

pub use crate::identities::git::Urn;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("can't track oneself")]
    SelfReferential,

    #[error(transparent)]
    Store(#[from] storage2::Error),

    #[error(transparent)]
    Git(#[from] git2::Error),
}

/// Track the given `peer` in the context of `urn`.
///
/// `true` is returned if the tracking relationship didn't exist before and was
/// created as a side-effect of the function call. Otherwise, `false` is
/// returned.
///
/// # Errors
///
/// Attempting to track oneself (as per the public key of the [`Signer`] is an
/// error.
#[tracing::instrument(skip(storage), err)]
pub fn track<S>(storage: &Storage<S>, urn: &Urn, peer: PeerId) -> Result<bool, Error>
where
    S: Signer,
{
    let local_peer = storage.peer_id();

    if &peer == local_peer {
        return Err(Error::SelfReferential);
    }

    let remote_name = tracking_remote_name(urn, &peer);
    let url = GitUrlRef::from_urn(urn, local_peer, &peer, &[]);

    let was_created = storage
        .as_raw()
        .remote(&remote_name, &url.to_string())
        .map(|_| true)
        .or_matches::<Error, _, _>(is_exists_err, || Ok(false))?;

    if was_created {
        // Remove default fetchspec, as it is almost always invalid (we compute
        // the fetchspecs ourselves). We also don't want libgit2 to prune the
        // remote.
        // FIXME: go through `&mut storage2::Config`
        let mut config = storage.as_raw().config()?;
        config.remove_multivar(&remote_name, ".*")?;
    }

    Ok(was_created)
}

/// Remove the tracking of `peer` in the context of `urn`.
///
/// `true` is returned if the tracking relationship existed and was removed as a
/// side-effect of the function call. Otherwise, `false` is returned.
///
/// # Caveats
///
/// Untracking will also attempt to prune any remote branches associated with
/// `peer` (this mirrors the behaviour of `git`). Since refdb operations are not
/// (yet) atomic, this may fail, leaving "dangling" refs in the storage. It is
/// safe to call this function repeatedly, so as to ensure all remote tracking
/// branches have been pruned.
#[tracing::instrument(skip(storage), err)]
pub fn untrack<S>(storage: &Storage<S>, urn: &Urn, peer: PeerId) -> Result<bool, Error>
where
    S: Signer,
{
    let remote_name = tracking_remote_name(urn, &peer);
    let was_removed = storage
        .as_raw()
        .remote_delete(&remote_name)
        .map(|()| true)
        .or_matches::<Error, _, _>(is_not_found_err, || Ok(false))?;

    // Prune all remote branches
    let prune = storage.references_glob(
        glob::Pattern::new(
            reflike!("refs/namespaces")
                .join(urn)
                .join(reflike!("refs/remotes"))
                .join(peer)
                .with_pattern_suffix(refspec_pattern!("*"))
                .as_str(),
        )
        .expect("RefspecPattern should be a valid glob::Pattern"),
    )?;

    for branch in prune {
        branch?.delete()?;
    }

    Ok(was_removed)
}

/// Obtain an iterator over the 1st degree tracked peers in the context of
/// `urn`.
pub fn tracked<S>(storage: &Storage<S>, urn: &Urn) -> Result<Tracked, Error>
where
    S: Signer,
{
    Ok(Tracked::collect(storage.as_raw(), urn)?)
}

/// Iterator over the 1st degree tracked peers.
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct Tracked {
    remotes: git2::string_array::StringArray,
    range: Range<usize>,
    prefix: String,
}

impl Tracked {
    fn collect(repo: &git2::Repository, context: &Urn) -> Result<Self, git2::Error> {
        let remotes = repo.remotes()?;
        let range = 0..remotes.len();
        let prefix = format!("{}/", RefLike::from(context));
        Ok(Self {
            remotes,
            range,
            prefix,
        })
    }
}

impl Iterator for Tracked {
    type Item = PeerId;

    fn next(&mut self) -> Option<Self::Item> {
        let next = self.range.next().and_then(|i| self.remotes.get(i));
        match next {
            None => None,
            Some(name) => {
                let peer = name
                    .strip_prefix(&self.prefix)
                    .and_then(|peer| peer.parse().ok());
                peer.or_else(|| self.next())
            },
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.range.size_hint()
    }
}

fn tracking_remote_name(urn: &Urn, peer: &PeerId) -> String {
    format!("{}/{}", RefLike::from(urn), peer)
}
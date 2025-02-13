// Copyright © 2019-2020 The Radicle Foundation <hello@radicle.foundation>
//
// This file is part of radicle-link, distributed under the GPLv3 with Radicle
// Linking Exception. For full terms see the included LICENSE file.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use futures::{future, StreamExt as _, TryFutureExt as _, TryStreamExt as _};
use futures_timer::Delay;

use super::protocol::{self, gossip};
use crate::{
    executor,
    git::{self, storage::Fetchers, Urn},
    signer::Signer,
    PeerId,
};

pub use super::protocol::{
    event::{
        self,
        downstream::{MembershipInfo, Stats},
        Upstream as ProtocolEvent,
    },
    Interrogation,
    PeerInfo,
};

pub mod error;
pub mod storage;
pub use storage::Storage as PeerStorage;

#[derive(Clone)]
pub struct Config<Signer> {
    pub signer: Signer,
    pub protocol: protocol::Config,
    pub storage: config::Storage,
}

pub mod config {
    use super::*;

    #[derive(Clone, Copy, Default)]
    pub struct Storage {
        pub user: UserStorage,
        pub protocol: ProtocolStorage,
    }

    /// Settings for the user-facing storage.
    ///
    /// Cf. [`Peer::using_storage`]
    #[derive(Clone, Copy)]
    pub struct UserStorage {
        /// Number of [`git::storage::Storage`] instances to reserve.
        pub pool_size: usize,
    }

    impl Default for UserStorage {
        fn default() -> Self {
            Self {
                pool_size: num_cpus::get_physical(),
            }
        }
    }

    /// Settings for the protocol storage.
    ///
    /// Cf. [`PeerStorage`]
    #[derive(Clone, Copy)]
    pub struct ProtocolStorage {
        /// Number of [`git::storage::Storage`] instances to reserve.
        pub pool_size: usize,
        /// Maximum amount of time to wait until a fetch slot becomes available.
        ///
        /// Applies to fetches initiated by incoming gossip messages.
        pub fetch_slot_wait_timeout: Duration,
    }

    impl Default for ProtocolStorage {
        fn default() -> Self {
            Self {
                pool_size: num_cpus::get_physical(),
                fetch_slot_wait_timeout: Duration::from_secs(20),
            }
        }
    }
}

#[derive(Clone)]
pub struct Peer<S> {
    config: Config<S>,
    phone: protocol::TinCans,
    peer_store: PeerStorage,
    user_store: git::storage::Pool<git::storage::Storage>,
    caches: protocol::Caches,
    spawner: Arc<executor::Spawner>,
}

impl<S> Peer<S>
where
    S: Signer + Clone,
{
    pub fn new(config: Config<S>) -> Result<Self, error::Init> {
        let spawner = executor::Spawner::from_current()
            .map(Arc::new)
            .ok_or(error::Init::Runtime)?;
        let phone = protocol::TinCans::default();
        let storage_lock = git::storage::pool::Initialised::no();
        let fetchers = Fetchers::default();
        let pool = git::storage::Pool::new(
            git::storage::pool::Config::with_fetchers(
                config.protocol.paths.clone(),
                config.signer.clone(),
                storage_lock.clone(),
                fetchers.clone(),
            ),
            config.storage.protocol.pool_size,
        );
        let caches = {
            let store = git::storage::Storage::open(&config.protocol.paths, config.signer.clone())?;
            let phone = phone.clone();
            let urns = protocol::cache::urns::Filter::new(store, move |ev| phone.emit(ev))?;
            protocol::Caches { urns }
        };
        let peer_store = PeerStorage::new(
            spawner.clone(),
            pool,
            storage::Config {
                replication: config.protocol.replication,
                fetch_slot_wait_timeout: config.storage.protocol.fetch_slot_wait_timeout,
                fetch_quota: config.protocol.rate_limits.gossip.fetches_per_peer_and_urn,
            },
            caches.urns.clone(),
        );
        let user_store = git::storage::Pool::new(
            git::storage::pool::Config::with_fetchers(
                config.protocol.paths.clone(),
                config.signer.clone(),
                storage_lock,
                fetchers,
            ),
            config.storage.user.pool_size,
        );

        Ok(Self {
            config,
            phone,
            peer_store,
            user_store,
            caches,
            spawner,
        })
    }

    pub fn signer(&self) -> &S {
        &self.config.signer
    }

    pub fn peer_id(&self) -> PeerId {
        PeerId::from_signer(self.signer())
    }

    pub fn protocol_config(&self) -> &protocol::Config {
        &self.config.protocol
    }

    pub fn announce(&self, have: gossip::Payload) -> Result<(), gossip::Payload> {
        self.phone.announce(have)
    }

    pub fn query(&self, want: gossip::Payload) -> Result<(), gossip::Payload> {
        self.phone.query(want)
    }

    pub fn providers(
        &self,
        urn: Urn,
        timeout: Duration,
    ) -> impl futures::Stream<Item = PeerInfo<SocketAddr>> {
        use protocol::event::{upstream::Gossip, Upstream};

        let events = self.subscribe();
        let providers = futures::stream::select(
            futures::stream::once(async move {
                Delay::new(timeout).await;
                Err("timed out")
            }),
            {
                let urn = urn.clone();
                events
                    .map_err(|_| "network reconnect")
                    .try_filter_map(move |event| {
                        let provider = match event {
                            Upstream::Gossip(box Gossip::Put {
                                provider,
                                payload:
                                    gossip::Payload {
                                        urn: payload_urn, ..
                                    },
                                ..
                            }) if payload_urn == urn => Some(provider),

                            _ => None,
                        };
                        future::ok(provider)
                    })
            },
        )
        .take_while(|x| future::ready(x.is_ok()))
        .map(Result::unwrap);

        match self.query(gossip::Payload {
            urn,
            rev: None,
            origin: None,
        }) {
            Ok(()) => providers.boxed(),
            Err(_) => futures::stream::empty().boxed(),
        }
    }

    pub async fn connected_peers(&self) -> Vec<PeerId> {
        self.phone.connected_peers().await
    }

    pub async fn membership(&self) -> MembershipInfo {
        self.phone.membership().await
    }

    pub async fn stats(&self) -> Stats {
        self.phone.stats().await
    }

    pub fn interrogate(&self, peer: impl Into<(PeerId, Vec<SocketAddr>)>) -> Interrogation {
        self.phone.interrogate(peer)
    }

    pub fn subscribe(
        &self,
    ) -> impl futures::Stream<Item = Result<ProtocolEvent, protocol::RecvError>> {
        self.phone.subscribe()
    }

    /// Borrow a [`git::storage::Storage`] from the pool, and run a blocking
    /// computation on it.
    pub async fn using_storage<F, A>(&self, blocking: F) -> Result<A, error::Storage>
    where
        F: FnOnce(&git::storage::Storage) -> A + Send + 'static,
        A: Send + 'static,
    {
        let storage = self.user_store.get().await?;
        Ok(self.spawner.blocking(move || blocking(&storage)).await)
    }

    /// Borrow a [`git::storage::ReadOnly`] from the pool, and run a blocking
    /// computation on it.
    pub async fn using_read_only<F, A>(&self, blocking: F) -> Result<A, error::Storage>
    where
        F: FnOnce(&git::storage::ReadOnly) -> A + Send + 'static,
        A: Send + 'static,
    {
        let storage = self.user_store.get().await?;
        Ok(self
            .spawner
            .blocking(move || blocking(storage.read_only()))
            .await)
    }

    /// Borrow a [`git::storage::Storage`] from the pool directly.
    ///
    /// # WARNING
    ///
    /// Operations on [`git::storage::Storage`] are ususally blocking, and thus
    /// require to be spawned to a dedicated thread pool in an async
    /// context. [`Self::using_storage`] takes care of that, while the
    /// consumer of this method's return value is responsible for spawning
    /// themselves.
    ///
    /// Also note that the consumer is responsible for dropping the returned
    /// value in a timely fashion after it is no longer needed, in order to
    /// return the [`git::storage::Storage`] to the pool.
    pub async fn storage(
        &self,
    ) -> Result<impl AsRef<git::storage::Storage>, git::storage::pool::PoolError> {
        self.user_store
            .get()
            .map_ok(git::storage::pool::PooledRef::from)
            .await
    }

    pub async fn bind(&self) -> Result<protocol::Bound<PeerStorage>, protocol::error::Bootstrap> {
        protocol::bind(
            self.spawner.clone(),
            self.phone.clone(),
            self.config.protocol.clone(),
            self.config.signer.clone(),
            self.peer_store.clone(),
            self.caches.clone(),
        )
        .await
    }
}

impl<S> git::local::transport::CanOpenStorage for Peer<S>
where
    S: Signer + Clone,
{
    fn open_storage(
        &self,
    ) -> Result<
        Box<dyn AsRef<git::storage::Storage>>,
        Box<dyn std::error::Error + Send + Sync + 'static>,
    > {
        match futures::executor::block_on(self.storage()) {
            Err(e) => Err(Box::new(e)),
            Ok(s) => Ok(Box::new(s)),
        }
    }
}

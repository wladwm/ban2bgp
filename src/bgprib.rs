use crate::*;
use chrono::prelude::*;
use futures::future::join_all;
use futures::stream::{self, StreamExt};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::net::TcpSocket;
use tokio::sync::{Mutex, RwLock};
use tokio::*;
use zettabgp::prelude::*;

#[derive(Debug)]
pub struct BgpRibUpdate {
    pub updates4: BTreeSet<BgpAddrV4>,
    pub withdraws4: BTreeSet<BgpAddrV4>,
    pub updates6: BTreeSet<BgpAddrV6>,
    pub withdraws6: BTreeSet<BgpAddrV6>,
}
impl BgpRibUpdate {
    pub fn new() -> BgpRibUpdate {
        BgpRibUpdate {
            updates4: BTreeSet::new(),
            withdraws4: BTreeSet::new(),
            updates6: BTreeSet::new(),
            withdraws6: BTreeSet::new(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.updates4.is_empty()
            && self.withdraws4.is_empty()
            && self.updates6.is_empty()
            && self.withdraws6.is_empty()
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerState {
    New,
    SendingRIB,
    InSync,
    SendingUpdates,
}
impl std::fmt::Display for PeerState {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            PeerState::New => "New".fmt(f),
            PeerState::SendingRIB => "SendingRIB".fmt(f),
            PeerState::InSync => "InSync".fmt(f),
            PeerState::SendingUpdates => "SendingUpdates".fmt(f),
        }
    }
}
pub struct BgpPeerShell {
    pub peer: BgpPeer,
    pub state: Arc<RwLock<PeerState>>,
}
impl std::fmt::Display for BgpPeerShell {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        self.peer.fmt(f)
    }
}

impl BgpPeerShell {
    pub fn new(peer: BgpPeer) -> Arc<RwLock<BgpPeerShell>> {
        Arc::new(RwLock::new(BgpPeerShell {
            peer,
            state: Arc::new(RwLock::new(PeerState::New)),
        }))
    }
    pub async fn set_state(&self, s: PeerState) {
        *self.state.write().await = s;
    }
    pub async fn is_state(&self, s: PeerState) -> bool {
        *self.state.read().await == s
    }
}
impl std::fmt::Debug for BgpPeerShell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BgpPeerShell")
            .field("peer", &self.peer)
            .finish()
    }
}
pub struct BgpPeers {
    pub peershells: HashMap<IpAddr, Arc<RwLock<BgpPeerShell>>>,
}
impl BgpPeers {
    pub fn new() -> BgpPeers {
        BgpPeers {
            peershells: HashMap::new(),
        }
    }
    pub fn addpeershell(
        &mut self,
        adr: IpAddr,
        peer: Arc<RwLock<BgpPeerShell>>,
    ) -> Option<Arc<RwLock<BgpPeerShell>>> {
        if self.peershells.get(&adr).is_some() {
            error!("Try to add duplicate peer at {}", adr);
            return None;
        }
        self.peershells.insert(adr, peer);
        self.peershells.get(&adr).cloned()
    }
    pub fn removepeershell(&mut self, adr: &IpAddr) {
        self.peershells.remove(adr);
    }
    pub fn hasip(&self, adr: &IpAddr) -> bool {
        self.peershells.get(adr).is_some()
    }
    pub async fn count_peers_in_state(&self, state: PeerState) -> usize {
        stream::iter(self.peershells.iter())
            .filter_map(|x| {
                let temp_st = state.clone();
                async move {
                    let peer = x.1.read().await;
                    if peer.is_state(temp_st).await {
                        Some(1_usize)
                    } else {
                        None
                    }
                }
            })
            .collect::<Vec<_>>()
            .await
            .len()
    }
    pub async fn send_update(&self, upd: Arc<Mutex<BgpRibUpdate>>) {
        stream::iter(self.peershells.iter())
            .filter_map(|x| async move {
                let peer = x.1.read().await;
                if peer.is_state(PeerState::InSync).await {
                    Some(peer)
                } else {
                    None
                }
            })
            .for_each(|p| {
                let upd1 = upd.clone();
                async move {
                    p.set_state(PeerState::SendingUpdates).await;
                    p.peer.send_update(upd1.clone()).await;
                    p.set_state(PeerState::InSync).await;
                }
            })
            .await;
    }
}
pub struct BgpRib {
    pub cfg: Arc<SvcConfig>,
    pub ipv4: BTreeMap<BgpAddrV4, DateTime<Local>>,
    pub ipv6: BTreeMap<BgpAddrV6, DateTime<Local>>,
    pub peers: Arc<RwLock<BgpPeers>>,
    pub cancel: tokio_util::sync::CancellationToken,
}
impl BgpRib {
    pub fn new(cfg: Arc<SvcConfig>, cancel: tokio_util::sync::CancellationToken) -> BgpRib {
        BgpRib {
            cfg,
            cancel,
            ipv4: BTreeMap::new(),
            ipv6: BTreeMap::new(),
            peers: Arc::new(RwLock::new(BgpPeers::new())),
        }
    }
    pub async fn change(&mut self, msg: BgpRibUpdate, till: chrono::DateTime<Local>) {
        let mut checked = BgpRibUpdate::new();
        for adr in msg.withdraws4.iter() {
            for v in self.ipv4.range(adr..=&BgpAddrV4::new(adr.range_last(), 32)) {
                if adr.in_subnet(&v.0.addr) {
                    checked.withdraws4.insert(v.0.clone());
                }
            }
        }
        for adr in msg.withdraws6.iter() {
            for v in self
                .ipv6
                .range(adr..=&BgpAddrV6::new(adr.range_last(), 128))
            {
                if adr.in_subnet(&v.0.addr) {
                    checked.withdraws6.insert(v.0.clone());
                }
            }
        }
        for adr in msg.updates4.iter() {
            if self.ipv4.range(adr..=adr).count() < 1 {
                checked.updates4.insert(adr.clone());
            };
            self.ipv4.insert(adr.clone(), till);
        }
        for adr in msg.updates6.iter() {
            if self.ipv6.range(adr..=adr).count() < 1 {
                checked.updates6.insert(adr.clone());
            };
            self.ipv6.insert(adr.clone(), till);
        }
        for adr in checked.withdraws4.iter() {
            self.ipv4.remove(adr);
        }
        for adr in checked.withdraws6.iter() {
            self.ipv6.remove(adr);
        }
        for adr in checked.withdraws4.iter() {
            self.ipv4.remove(adr);
        }
        for adr in msg.updates4.iter() {
            self.ipv4.insert(adr.clone(), till);
        }
        for adr in msg.updates6.iter() {
            self.ipv6.insert(adr.clone(), till);
        }
        if checked.is_empty() {
            return;
        }
        let upd = Arc::new(Mutex::new(checked));
        self.peers.write().await.send_update(upd.clone()).await;
    }
    async fn listenat(&self, sockaddr: SocketAddr) -> tokio::io::Result<()> {
        let socket = if sockaddr.is_ipv4() {
            TcpSocket::new_v4()?
        } else {
            TcpSocket::new_v6()?
        };
        let routerid = self.cfg.routerid;
        let bgpparams = if sockaddr.is_ipv4() {
            BgpSessionParams::new(
                0,
                180,
                BgpTransportMode::IPv4,
                routerid,
                vec![
                    BgpCapability::SafiIPv4u,
                    BgpCapability::SafiIPv4fu,
                    BgpCapability::SafiIPv6fu,
                    BgpCapability::SafiIPv6lu,
                ]
                .into_iter()
                .collect(),
            )
        } else {
            BgpSessionParams::new(
                0,
                180,
                BgpTransportMode::IPv6,
                routerid,
                vec![
                    BgpCapability::SafiIPv6u,
                    BgpCapability::SafiIPv4fu,
                    BgpCapability::SafiIPv6fu,
                    BgpCapability::SafiIPv4lu,
                ]
                .into_iter()
                .collect(),
            )
        };
        socket.bind(sockaddr)?;
        let listener = socket.listen(1)?;
        let cancel_tok = self.cancel.clone();
        let peers = self.peers.clone();
        let cfg = self.cfg.clone();
        tokio::task::spawn(async move {
            loop {
                let client = select! {
                    _ = cancel_tok.cancelled() => {
                        break;
                    }
                    res = listener.accept() => {
                        match res {
                          Ok(acc) => acc,
                          Err(e) => {
                            error!("Accept error: {}", e);
                            continue;
                          }
                        }
                    }
                };
                info!("Connected from {}", client.1);
                let peershell = BgpPeerShell::new(
                    BgpPeer::new(
                        client.1,
                        PeerMode::Blackhole,
                        cfg.clone(),
                        bgpparams.clone(),
                        client.0,
                    )
                    .await,
                );
                let mut scs: bool = true;
                if let Err(e) = peershell.write().await.peer.start_passive().await {
                    error!("failed to create BGP peer; err = {:?}", e);
                    scs = false;
                }
                if scs {
                    if let Some(peer_shell) = peers
                        .write()
                        .await
                        .addpeershell(client.1.ip(), peershell.clone())
                    {
                        let cancel_tok1 = cancel_tok.clone();
                        let claddr = client.1;
                        let peers1 = peers.clone();
                        tokio::task::spawn(async move {
                            peer_shell.read().await.peer.lifecycle(cancel_tok1).await;
                            peers1.write().await.removepeershell(&claddr.ip());
                            info!("Session done {}", claddr);
                        });
                    } else {
                        scs = false;
                    }
                }
                if !scs {
                    peershell.write().await.peer.close().await;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10000)).await;
            }
        });
        Ok(())
    }
    async fn connectto(
        cfg: Arc<SvcConfig>,
        peers: Arc<RwLock<BgpPeers>>,
        sockaddr: SocketAddr,
        mode: PeerMode,
        asn: u32,
        cancel: tokio_util::sync::CancellationToken,
    ) -> tokio::io::Result<()> {
        info!("Connecting to {}", sockaddr);
        let peertcp = select! {
            _ = cancel.cancelled() => {
                return Ok(());
            }
            r = tokio::net::TcpStream::connect(sockaddr) => {
                    match r {
                        Err(e) => {
                            return Err(e);
                        }
                        Ok(c) => c
                    }
            }
        };
        info!("Connected to {}", sockaddr);
        let routerid = cfg.routerid;
        let bgpparams = match sockaddr {
            SocketAddr::V4(_) => BgpSessionParams::new(
                asn,
                180,
                BgpTransportMode::IPv4,
                routerid,
                vec![
                    BgpCapability::SafiIPv4u,
                    BgpCapability::SafiIPv4fu,
                    BgpCapability::SafiIPv6fu,
                    BgpCapability::SafiIPv6lu,
                    BgpCapability::CapASN32(asn),
                ]
                .into_iter()
                .collect(),
            ),
            SocketAddr::V6(_) => BgpSessionParams::new(
                asn,
                180,
                BgpTransportMode::IPv6,
                routerid,
                vec![
                    BgpCapability::SafiIPv4fu,
                    BgpCapability::SafiIPv6fu,
                    BgpCapability::SafiIPv6u,
                    BgpCapability::SafiIPv4lu,
                    BgpCapability::CapASN32(asn),
                ]
                .into_iter()
                .collect(),
            ),
        };
        let peershell = BgpPeerShell::new(
            BgpPeer::new(sockaddr, mode, cfg.clone(), bgpparams.clone(), peertcp).await,
        );
        let mut scs: bool = true;
        if let Err(e) = peershell.write().await.peer.start_active().await {
            error!("failed to create BGP peer; err = {:?}", e);
            scs = false;
        }
        if scs {
            scs = peers
                .write()
                .await
                .addpeershell(sockaddr.ip(), peershell.clone())
                .is_some();
        };
        if scs {
            peershell.read().await.peer.lifecycle(cancel.clone()).await;
            peers.write().await.removepeershell(&sockaddr.ip());
            info!("Session done {}", sockaddr);
        }
        peershell.write().await.peer.close().await;
        Ok(())
    }
    pub async fn start(slf: Arc<Mutex<Self>>) {
        let _self = slf.lock().await;
        let listenat = _self.cfg.listenat.clone();
        for sockaddr in listenat.into_iter() {
            if let Err(e) = _self.listenat(sockaddr).await {
                error!("Error listen at {} - {}", sockaddr, e);
            };
        }
        let activepeers = _self.cfg.activepeers.clone();
        let peers = _self.peers.write().await;
        for cfgp in activepeers.into_iter() {
            if !peers.hasip(&cfgp.peeraddr) {
                let cfg = _self.cfg.clone();
                let peers = _self.peers.clone();
                let cancel_tok = _self.cancel.clone();
                tokio::task::spawn(async move {
                    loop {
                        select! {
                            _ = cancel_tok.cancelled() => {
                                break;
                            }
                            _ = BgpRib::connectto(cfg.clone(),peers.clone(),SocketAddr::new(cfgp.peeraddr, 179),cfgp.mode.clone(),cfgp.peeras,cancel_tok.clone()) => {
                            }
                        };
                        select! {
                            _ = cancel_tok.cancelled() => {
                                break;
                            }
                            _ = tokio::time::sleep(std::time::Duration::from_millis(10000)) => {

                            }
                        };
                    }
                });
            }
        }
        let cancel_tok = _self.cancel.clone();
        let slf1 = slf.clone();
        tokio::task::spawn(async move {
            loop {
                select! {
                    _ = cancel_tok.cancelled() => {
                        break;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(1000)) => {

                    }
                };
                select! {
                    _ = cancel_tok.cancelled() => {
                        break;
                    }
                    _ = BgpRib::check_peers(slf1.clone()) => {

                    }
                };
                select! {
                    _ = cancel_tok.cancelled() => {
                        break;
                    }
                    _ = BgpRib::check_expires(slf1.clone()) => {

                    }
                };
            }
        });
    }
    async fn check_expires(slf: Arc<Mutex<Self>>) {
        let mut _self = slf.lock().await;
        let mut inupd = BgpRibUpdate::new();
        let now = Local::now();
        for v in _self.ipv4.iter() {
            if v.1 < &now {
                inupd.withdraws4.insert(v.0.clone());
            }
        }
        for v in _self.ipv6.iter() {
            if v.1 < &now {
                inupd.withdraws6.insert(v.0.clone());
            }
        }
        if inupd.is_empty() {
            return;
        }
        for v in inupd.withdraws4.iter() {
            _self.ipv4.remove(v);
        }
        for v in inupd.withdraws6.iter() {
            _self.ipv6.remove(v);
        }
        let upd = Arc::new(Mutex::new(inupd));
        let peers = _self.peers.write().await;
        let upds: Vec<_> = stream::iter(peers.peershells.iter())
            .filter_map(|x| async move {
                let peer = x.1.read().await;
                if peer.is_state(PeerState::InSync).await {
                    Some(peer)
                } else {
                    None
                }
            })
            .map(|p| {
                let upd1 = upd.clone();
                async move {
                    p.set_state(PeerState::SendingRIB).await;
                    p.peer.send_update(upd1).await;
                    p.set_state(PeerState::InSync).await;
                }
            })
            .collect()
            .await;
        join_all(upds).await;
    }
    async fn check_peers(slf: Arc<Mutex<Self>>) {
        let _self = slf.lock().await;
        let peers = _self.peers.write().await;
        let cnt = peers.count_peers_in_state(PeerState::New).await;
        if cnt < 1 {
            return;
        }
        let mut inupd = BgpRibUpdate::new();
        for v in _self.ipv4.iter() {
            inupd.updates4.insert(v.0.clone());
        }
        for v in _self.ipv6.iter() {
            inupd.updates6.insert(v.0.clone());
        }
        let upd = Arc::new(Mutex::new(inupd));
        let peers: Vec<_> = stream::iter(peers.peershells.iter())
            .filter_map(|x| async move {
                let peer = x.1.read().await;
                if peer.is_state(PeerState::New).await {
                    Some(peer)
                } else {
                    None
                }
            })
            .map(|p| {
                let upd1 = upd.clone();
                async move {
                    p.set_state(PeerState::SendingRIB).await;
                    p.peer.send_update(upd1).await;
                    p.set_state(PeerState::InSync).await;
                }
            })
            .collect()
            .await;
        join_all(peers).await;
    }
}

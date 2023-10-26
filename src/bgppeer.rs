use crate::*;
use chrono::prelude::*;
use futures::stream::{self, StreamExt};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::*;
use tokio::sync::{Mutex, RwLock};
use tokio::*;
use zettabgp::prelude::*;

struct UpdCnt {
    upd: BgpUpdateMessage,
    cnt: usize,
}
pub struct BgpPeer {
    cfg: Arc<SvcConfig>,
    peer: std::net::SocketAddr,
    mode: PeerMode,
    nhop: std::net::IpAddr,
    params: BgpSessionParams,
    peersock: Arc<Mutex<tokio::net::TcpStream>>,
    keepalive_sent: Arc<RwLock<DateTime<Local>>>,
    snd: Arc<Mutex<Sender<BgpUpdateMessage>>>,
    rcv: Arc<Mutex<Receiver<BgpUpdateMessage>>>,
    upd: Arc<RwLock<Option<UpdCnt>>>,
}
impl std::fmt::Display for BgpPeer {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        self.peer.fmt(f)
    }
}
impl std::fmt::Debug for BgpPeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BgpPeer")
            .field("peer", &self.peer)
            .field("params", &self.params)
            .finish()
    }
}

impl BgpPeer {
    pub async fn new(
        peer: std::net::SocketAddr,
        mode: PeerMode,
        cfgarc: Arc<SvcConfig>,
        pars: BgpSessionParams,
        stream: tokio::net::TcpStream,
    ) -> BgpPeer {
        let (tx, rx) = channel(100);
        BgpPeer {
            peer,
            mode,
            cfg: cfgarc.clone(),
            nhop: match pars.peer_mode {
                BgpTransportMode::IPv4 => std::net::IpAddr::V4(cfgarc.nexthop4),
                BgpTransportMode::IPv6 => std::net::IpAddr::V6(cfgarc.nexthop6),
            },
            params: pars,
            peersock: Arc::new(Mutex::new(stream)),
            keepalive_sent: Arc::new(RwLock::new(Local::now())),
            snd: Arc::new(Mutex::new(tx)),
            rcv: Arc::new(Mutex::new(rx)),
            upd: Arc::new(RwLock::new(None)),
        }
    }
    async fn msgflush(&self, maxcnt: usize) {
        let mut updq = self.upd.write().await;
        if let Some(u) = updq.take() {
            if u.cnt > maxcnt {
                match self.snd.lock().await.send(u.upd).await {
                    Ok(_) => {}
                    Err(e) => {
                        error!("Error send update: {}", e);
                    }
                }
            } else {
                *updq = Some(u);
                return;
            }
        }
        let mut msg = BgpUpdateMessage::new();
        msg.attrs.push(BgpAttrItem::Origin(BgpOrigin::new(
            BgpAttrOrigin::Incomplete,
        )));
        msg.attrs.push(BgpAttrItem::ASPath(BgpASpath::new()));
        msg.attrs
            .push(BgpAttrItem::LocalPref(BgpLocalpref::new(10)));
        /*
        msg.attrs.push(BgpAttrItem::OriginatorID(BgpOriginatorID::new(
            match self.params.peer_mode {
                BgpTransportMode::IPv4 => { IpAddr::V4(cfg.nexthop4.clone())}
                BgpTransportMode::IPv6 => { IpAddr::V6(cfg.nexthop6.clone())}
            }
        )));
        */
        if !self.cfg.communities.value.is_empty() {
            msg.attrs
                .push(BgpAttrItem::CommunityList(self.cfg.communities.clone()));
        };
        msg.attrs
            .push(BgpAttrItem::NextHop(BgpNextHop::new(self.nhop)));
        *updq = Some(UpdCnt { upd: msg, cnt: 0 });
    }
    async fn upd<F: FnMut(&mut BgpUpdateMessage)>(&self, mut f: F) {
        self.msgflush(100).await;
        let mut updq = self.upd.write().await;
        let mut upd = updq.take().unwrap();
        f(&mut upd.upd);
        upd.cnt += 1;
        *updq = Some(upd);
    }
    async fn update4(&self, u4: &BgpAddrV4) {
        if self.mode == PeerMode::FlowSource
            && self.params.check_capability(&BgpCapability::SafiIPv4fu)
        {
            self.upd(|upd| {
                for i in upd.attrs.iter_mut() {
                    match i {
                        BgpAttrItem::MPUpdates(u) => {
                            if let BgpAddrs::FS4U(ref mut v) = u.addrs {
                                v.push(BgpFlowSpec::PrefixSrc(u4.clone()));
                            }
                            return;
                        }
                        _ => {}
                    }
                }
                upd.attrs.push(BgpAttrItem::MPUpdates(BgpMPUpdates {
                    nexthop: BgpAddr::None,
                    addrs: BgpAddrs::FS4U(vec![BgpFlowSpec::PrefixSrc(u4.clone())]),
                }));
            })
            .await;
            return;
        };
        match self.params.peer_mode {
            BgpTransportMode::IPv4 => {
                self.upd(|upd| {
                    if let BgpAddrs::IPV4U(ref mut x) = upd.updates {
                        x.push(u4.clone());
                    } else {
                        upd.updates = BgpAddrs::IPV4U(vec![u4.clone()]);
                    }
                })
                .await;
            }
            BgpTransportMode::IPv6 => {
                if self.params.check_capability(&BgpCapability::SafiIPv4lu) {
                    let nhop4 = self.cfg.nexthop4;
                    self.upd(|upd| {
                        for i in upd.attrs.iter_mut() {
                            match i {
                                BgpAttrItem::MPUpdates(u) => {
                                    if let BgpAddrs::IPV4LU(ref mut v) = u.addrs {
                                        v.push(Labeled::<BgpAddrV4>::new(
                                            MplsLabels::fromvec(vec![3]),
                                            u4.clone(),
                                        ));
                                    }
                                    return;
                                }
                                _ => {}
                            }
                        }
                        upd.attrs.push(BgpAttrItem::MPUpdates(BgpMPUpdates {
                            nexthop: BgpAddr::V4(nhop4),
                            addrs: BgpAddrs::IPV4LU(vec![Labeled::<BgpAddrV4>::new(
                                MplsLabels::fromvec(vec![3]),
                                u4.clone(),
                            )]),
                        }));
                    })
                    .await;
                };
            }
        };
    }
    async fn withdraw4(&self, u4: &BgpAddrV4) {
        if self.mode == PeerMode::FlowSource
            && self.params.check_capability(&BgpCapability::SafiIPv4fu)
        {
            self.upd(|upd| {
                for i in upd.attrs.iter_mut() {
                    match i {
                        BgpAttrItem::MPWithdraws(u) => {
                            if let BgpAddrs::FS4U(ref mut v) = u.addrs {
                                v.push(BgpFlowSpec::PrefixSrc(u4.clone()));
                            }
                            return;
                        }
                        _ => {}
                    }
                }
                upd.attrs.push(BgpAttrItem::MPWithdraws(BgpMPWithdraws {
                    addrs: BgpAddrs::FS4U(vec![BgpFlowSpec::PrefixSrc(u4.clone())]),
                }));
            })
            .await;
            return;
        };
        match self.params.peer_mode {
            BgpTransportMode::IPv4 => {
                self.upd(|upd| {
                    if let BgpAddrs::IPV4U(ref mut x) = upd.withdraws {
                        x.push(u4.clone());
                    } else {
                        upd.withdraws = BgpAddrs::IPV4U(vec![u4.clone()]);
                    }
                })
                .await;
            }
            BgpTransportMode::IPv6 => {
                if self.params.check_capability(&BgpCapability::SafiIPv4lu) {
                    self.upd(|upd| {
                        for i in upd.attrs.iter_mut() {
                            match i {
                                BgpAttrItem::MPWithdraws(u) => {
                                    if let BgpAddrs::IPV4LU(ref mut v) = u.addrs {
                                        v.push(Labeled::<BgpAddrV4>::new(
                                            MplsLabels::fromvec(vec![3]),
                                            u4.clone(),
                                        ));
                                    }
                                    return;
                                }
                                _ => {}
                            }
                        }
                        upd.attrs.push(BgpAttrItem::MPWithdraws(BgpMPWithdraws {
                            addrs: BgpAddrs::IPV4LU(vec![Labeled::<BgpAddrV4>::new(
                                MplsLabels::fromvec(vec![3]),
                                u4.clone(),
                            )]),
                        }));
                    })
                    .await;
                };
            }
        };
    }
    async fn update6(&self, u6: &BgpAddrV6) {
        if self.params.check_capability(&BgpCapability::SafiIPv6fu) {
            self.upd(|upd| {
                for i in upd.attrs.iter_mut() {
                    match i {
                        BgpAttrItem::MPUpdates(u) => {
                            if let BgpAddrs::FS6U(ref mut v) = u.addrs {
                                v.push(BgpFlowSpec::PrefixSrc(FS6::new(0, u6.clone())));
                            }
                            return;
                        }
                        _ => {}
                    }
                }
                upd.attrs.push(BgpAttrItem::MPUpdates(BgpMPUpdates {
                    nexthop: BgpAddr::None,
                    addrs: BgpAddrs::FS6U(vec![BgpFlowSpec::PrefixSrc(FS6::new(0, u6.clone()))]),
                }));
            })
            .await;
            return;
        };
        match self.params.peer_mode {
            BgpTransportMode::IPv6 => {
                self.upd(|upd| {
                    if let BgpAddrs::IPV6U(ref mut x) = upd.updates {
                        x.push(u6.clone());
                    } else {
                        upd.updates = BgpAddrs::IPV6U(vec![u6.clone()]);
                    }
                })
                .await;
            }
            BgpTransportMode::IPv4 => {
                if self.params.check_capability(&BgpCapability::SafiIPv6lu) {
                    let nhop6 = self.cfg.nexthop6;
                    self.upd(|upd| {
                        for i in upd.attrs.iter_mut() {
                            match i {
                                BgpAttrItem::MPUpdates(u) => {
                                    if let BgpAddrs::IPV6LU(ref mut v) = u.addrs {
                                        v.push(Labeled::<BgpAddrV6>::new(
                                            MplsLabels::fromvec(vec![2]),
                                            u6.clone(),
                                        ));
                                    }
                                    break;
                                }
                                _ => {}
                            }
                        }
                        upd.attrs.push(BgpAttrItem::MPUpdates(BgpMPUpdates {
                            nexthop: BgpAddr::V6(nhop6),
                            addrs: BgpAddrs::IPV6LU(vec![Labeled::<BgpAddrV6>::new(
                                MplsLabels::fromvec(vec![2]),
                                u6.clone(),
                            )]),
                        }));
                    })
                    .await;
                };
            }
        };
    }
    async fn withdraw6(&self, u6: &BgpAddrV6) {
        if self.params.check_capability(&BgpCapability::SafiIPv6fu) {
            self.upd(|upd| {
                for i in upd.attrs.iter_mut() {
                    match i {
                        BgpAttrItem::MPWithdraws(u) => {
                            if let BgpAddrs::FS6U(ref mut v) = u.addrs {
                                v.push(BgpFlowSpec::PrefixSrc(FS6::new(0, u6.clone())));
                            }
                            return;
                        }
                        _ => {}
                    }
                }
                upd.attrs.push(BgpAttrItem::MPWithdraws(BgpMPWithdraws {
                    addrs: BgpAddrs::FS6U(vec![BgpFlowSpec::PrefixSrc(FS6::new(0, u6.clone()))]),
                }));
            })
            .await;
            return;
        };
        match self.params.peer_mode {
            BgpTransportMode::IPv6 => {
                self.upd(|upd| {
                    if let BgpAddrs::IPV6U(ref mut x) = upd.withdraws {
                        x.push(u6.clone());
                    } else {
                        upd.withdraws = BgpAddrs::IPV6U(vec![u6.clone()]);
                    }
                })
                .await;
            }
            BgpTransportMode::IPv4 => {
                if self.params.check_capability(&BgpCapability::SafiIPv6lu) {
                    self.upd(|upd| {
                        for i in upd.attrs.iter_mut() {
                            match i {
                                BgpAttrItem::MPWithdraws(u) => {
                                    if let BgpAddrs::IPV6LU(ref mut v) = u.addrs {
                                        v.push(Labeled::<BgpAddrV6>::new(
                                            MplsLabels::fromvec(vec![2]),
                                            u6.clone(),
                                        ));
                                    }
                                    break;
                                }
                                _ => {}
                            }
                        }
                        upd.attrs.push(BgpAttrItem::MPWithdraws(BgpMPWithdraws {
                            addrs: BgpAddrs::IPV6LU(vec![Labeled::<BgpAddrV6>::new(
                                MplsLabels::fromvec(vec![2]),
                                u6.clone(),
                            )]),
                        }));
                    })
                    .await;
                };
            }
        };
    }
    pub async fn send_update(&self, updarc: Arc<Mutex<BgpRibUpdate>>) {
        let upd = updarc.lock().await;
        stream::iter(upd.withdraws4.iter())
            .for_each(|i| async move {
                self.withdraw4(i).await;
            })
            .await;
        stream::iter(upd.withdraws6.iter())
            .for_each(|i| async move {
                self.withdraw6(i).await;
            })
            .await;
        stream::iter(upd.updates4.iter())
            .for_each(|i| async move {
                self.update4(i).await;
            })
            .await;
        stream::iter(upd.updates6.iter())
            .for_each(|i| async move {
                self.update6(i).await;
            })
            .await;
        self.msgflush(0).await;
    }
    async fn recv_message_head(
        &self,
        sck: &mut tokio::net::TcpStream,
    ) -> Result<(BgpMessageType, usize), BgpError> {
        let mut buf = [0_u8; 19];
        sck.read_exact(&mut buf).await?;
        self.params.decode_message_head(&buf)
    }
    fn get_message_body_ref(buf: &mut [u8]) -> Result<&mut [u8], BgpError> {
        if buf.len() < 19 {
            return Err(BgpError::insufficient_buffer_size());
        }
        Ok(&mut buf[19..])
    }
    async fn send_message_buf(
        &self,
        sck: &mut tokio::net::TcpStream,
        buf: &mut [u8],
        messagetype: BgpMessageType,
        messagelen: usize,
    ) -> Result<(), BgpError> {
        let blen = self
            .params
            .prepare_message_buf(buf, messagetype, messagelen)?;
        match sck.write_all(&buf[0..blen]).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    pub async fn start_passive(&mut self) -> Result<(), BgpError> {
        let mut bom = BgpOpenMessage::new();
        let mut buf = [255_u8; 4096];
        let mut sck = self.peersock.lock().await;
        let msg = match self.recv_message_head(&mut sck).await {
            Err(e) => return Err(e),
            Ok(msg) => msg,
        };
        if msg.0 != BgpMessageType::Open {
            return Err(BgpError::static_str("Invalid state to start_passive"));
        }
        sck.read_exact(&mut buf[0..msg.1]).await?;
        bom.decode_from(&self.params, &buf[0..msg.1])?;
        bom.router_id = self.params.router_id;
        self.params.as_num = bom.as_num;
        self.params.hold_time = bom.hold_time;
        self.params.match_caps(&bom.caps);
        bom = self.params.open_message();
        let sz = match bom.encode_to(&self.params, BgpPeer::get_message_body_ref(&mut buf)?) {
            Err(e) => return Err(e),
            Ok(sz) => sz,
        };
        self.send_message_buf(&mut sck, &mut buf, BgpMessageType::Open, sz)
            .await?;
        Ok(())
    }
    pub async fn start_active(&mut self) -> Result<(), BgpError> {
        let mut bom = self.params.open_message();
        let mut buf = [255_u8; 4096];
        let sz = match bom.encode_to(&self.params, BgpPeer::get_message_body_ref(&mut buf)?) {
            Err(e) => {
                return Err(e);
            }
            Ok(sz) => sz,
        };
        let mut sck = self.peersock.lock().await;
        self.send_message_buf(&mut sck, &mut buf, BgpMessageType::Open, sz)
            .await?;
        let msg = match self.recv_message_head(&mut sck).await {
            Err(e) => {
                return Err(e);
            }
            Ok(msg) => msg,
        };
        if msg.0 != BgpMessageType::Open {
            return Err(BgpError::static_str("Invalid state to start_active"));
        }
        sck.read_exact(&mut buf[0..msg.1]).await?;
        bom.decode_from(&self.params, &buf[0..msg.1])?;
        self.params.hold_time = bom.hold_time;
        self.params.match_caps(&bom.caps);
        Ok(())
    }
    pub async fn send_keepalive(&self) -> Result<(), BgpError> {
        let mut buf = [255_u8; 19];
        let blen = self
            .params
            .prepare_message_buf(&mut buf, BgpMessageType::Keepalive, 0)?;
        let mut sck = self.peersock.lock().await;
        match sck.write_all(&buf[0..blen]).await {
            Ok(_) => {
                let mut kpl = self.keepalive_sent.write().await;
                *kpl = Local::now();
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }
    pub async fn lifecycle(&self, cancel: tokio_util::sync::CancellationToken) {
        let mut buf = Box::new([255_u8; 65535]);
        let keep_interval = chrono::Duration::seconds((self.params.hold_time / 3) as i64);
        loop {
            let mut tosleep = Local::now() - *self.keepalive_sent.read().await;
            if tosleep >= keep_interval {
                match self.send_keepalive().await {
                    Ok(_) => {}
                    Err(e) => {
                        error!("Keepalive send error: {:?}", e);
                    }
                }
                tosleep = Local::now() - *self.keepalive_sent.read().await;
            }
            tosleep = keep_interval - tosleep;
            let tosleepstd = match tosleep.to_std() {
                Ok(s) => s,
                Err(_) => std::time::Duration::from_secs(1),
            };
            let mut need_send: usize = 0;
            let msg = {
                let mut rcv = self.rcv.lock().await;
                let mut sck = self.peersock.lock().await;
                select! {
                    _ = cancel.cancelled() => {
                        break;
                    }
                    _ = tokio::time::sleep(tosleepstd) => {
                        (BgpMessageType::Keepalive,0)
                    }
                    updrcv = rcv.recv() => {
                        if let Some(upd) = updrcv {
                            need_send = match upd.encode_to(&self.params, &mut buf[19..]) {
                                Err(e) => {
                                    error!("bgp update encode error: {}",e);
                                    continue;
                                }
                                Ok(sz) => sz,
                            };
                        }
                        (BgpMessageType::Keepalive,0)
                    }
                    msgin = self.recv_message_head(&mut sck) => {
                        match msgin {
                            Err(e) => {
                                error!("recv_message_head: {:?}", e);
                                break;
                            }
                            Ok(msg) => {
                                msg
                            }
                        }
                    }
                }
            };
            if need_send > 0 {
                let mut sck = self.peersock.lock().await;
                match self
                    .send_message_buf(&mut sck, &mut buf[0..], BgpMessageType::Update, need_send)
                    .await
                {
                    Err(e) => {
                        error!("bgp update encode error: {}", e);
                    }
                    Ok(_) => {}
                }
            }
            if msg.1 > 0 {
                // read message body
                let mut sck = self.peersock.lock().await;
                select! {
                    _ = cancel.cancelled() => {
                        break;
                    }
                    _ = tokio::time::sleep(tosleepstd) => {
                        //timeout, protocol error
                        break;
                    }
                    rs = sck.read_exact(&mut buf[0..msg.1]) => {
                        if let Err(e) = rs {
                            error!("receve message body error: {:?}", e);
                            break;
                        }
                    }
                };
            };
            match msg.0 {
                BgpMessageType::Open => {
                    error!("Incorrect open message!");
                    break;
                }
                BgpMessageType::Keepalive => match self.send_keepalive().await {
                    Ok(_) => {}
                    Err(e) => {
                        error!("Keepalive sending error: {:?}", e);
                    }
                },
                BgpMessageType::Notification => {
                    let mut msgnotification = BgpNotificationMessage::new();
                    match msgnotification.decode_from(&self.params, &buf[0..msg.1]) {
                        Err(e) => {
                            error!("BGP notification decode error: {:?}", e);
                        }
                        Ok(_) => {
                            debug!(
                                "BGP notification: {:?} - {:?}",
                                msgnotification,
                                msgnotification.error_text()
                            );
                        }
                    };
                    break;
                }
                BgpMessageType::Update => {
                    let mut msgupdate = BgpUpdateMessage::new();
                    if let Err(e) = msgupdate.decode_from(&self.params, &buf[0..msg.1]) {
                        warn!("BGP update decode error: {:?}", e);
                        continue;
                    }
                    //ignore incoming update message
                }
            }
        }
    }
    pub async fn close(&self) {
        match self.peersock.lock().await.shutdown().await {
            Ok(_) => {}
            Err(e) => {
                error!("Warning: socket shutdown error: {}", e)
            }
        }
    }
}

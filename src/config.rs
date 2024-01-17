use crate::*;
use std::error::Error;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum PeerMode {
    Blackhole,
    FlowSource,
}
impl FromStr for PeerMode {
    type Err = ErrorConfig;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "blackhole" => Ok(PeerMode::Blackhole),
            "flowsource" => Ok(PeerMode::FlowSource),
            _ => Err(ErrorConfig::from_str("invalid peer mode")),
        }
    }
}
#[derive(Debug, Clone)]
pub struct ConfigPeer {
    pub peeraddr: IpAddr,
    pub peeras: u32,
    pub localas: u32,
    pub mode: PeerMode,
}
#[derive(Debug, Clone)]
pub struct SvcConfig {
    pub routerid: Ipv4Addr,
    pub activepeers: Vec<ConfigPeer>,
    pub listenat: Vec<SocketAddr>,
    pub default_duration: chrono::Duration,
    pub nexthop4: Ipv4Addr,
    pub nexthop6: Ipv6Addr,
    pub skiplist: Vec<BgpNet>,
    pub communities: BgpCommunityList,
    pub httplisten: std::net::SocketAddr,
    pub http_files_root: String,
}
impl ConfigPeer {
    pub fn from_section(sec: &HashMap<String, Option<String>>) -> Result<ConfigPeer, ErrorConfig> {
        if !sec.contains_key("peer") {
            return Err(ErrorConfig::from_str("missing peer IP address"));
        };
        if !sec.contains_key("as") {
            return Err(ErrorConfig::from_str("missing peer as number"));
        };
        let peeraddr: IpAddr = match sec
            .get("peer")
            .map(|s| s.as_ref().map(|s| s.as_str()))
            .flatten()
        {
            Some(s) => match s.parse() {
                Ok(q) => q,
                Err(_) => return Err(ErrorConfig::from_str("invalid peer IP address")),
            },
            None => return Err(ErrorConfig::from_str("missing peer IP address")),
        };
        let peeras: u32 = match sec
            .get("as")
            .map(|s| s.as_ref().map(|s| s.as_str()))
            .flatten()
        {
            Some(s) => match s.parse() {
                Ok(q) => q,
                Err(_) => return Err(ErrorConfig::from_str("invalid peer as number")),
            },
            None => return Err(ErrorConfig::from_str("missing peer as number")),
        };
        let localas: u32 = match sec
            .get("localas")
            .map(|s| s.as_ref().map(|s| s.as_str()))
            .flatten()
        {
            Some(s) => match s.parse() {
                Ok(q) => q,
                Err(_) => return Err(ErrorConfig::from_str("invalid local as number")),
            },
            None => peeras,
        };
        let mode: PeerMode = if sec.contains_key("mode") {
            match sec["mode"] {
                Some(ref s) => match s.parse() {
                    Ok(q) => q,
                    Err(_) => return Err(ErrorConfig::from_str("invalid peer mode")),
                },
                None => PeerMode::Blackhole,
            }
        } else {
            PeerMode::Blackhole
        };
        Ok(ConfigPeer {
            peeraddr,
            peeras,
            localas,
            mode,
        })
    }
}

#[derive(Debug)]
pub enum ErrorConfig {
    Static(&'static str),
    Str(String),
}
impl ErrorConfig {
    pub fn from_str(m: &'static str) -> Self {
        ErrorConfig::Static(m)
    }
    pub fn from_string(m: String) -> Self {
        ErrorConfig::Str(m)
    }
}
impl From<&'static str> for ErrorConfig {
    fn from(m: &'static str) -> Self {
        ErrorConfig::Static(m)
    }
}
impl From<std::net::AddrParseError> for ErrorConfig {
    fn from(m: std::net::AddrParseError) -> Self {
        ErrorConfig::Str(format!("{}", m))
    }
}
impl From<std::num::ParseIntError> for ErrorConfig {
    fn from(m: std::num::ParseIntError) -> Self {
        ErrorConfig::Str(format!("{}", m))
    }
}
impl fmt::Display for ErrorConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ErrorConfig: {}",
            match self {
                ErrorConfig::Static(s) => s,
                ErrorConfig::Str(s) => s.as_str(),
            }
        )
    }
}

impl Error for ErrorConfig {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self)
    }
}

impl SvcConfig {
    pub fn from_inifile(inifile: &str) -> Result<SvcConfig, ErrorConfig> {
        let conf = match ini!(safe inifile) {
            Ok(c) => c,
            Err(e) => return Err(ErrorConfig::from_string(e)),
        };
        if !conf.contains_key("main") {
            return Err(ErrorConfig::from_str("Missing section 'main' in ini file"));
        }
        let mainsection = &conf["main"];
        let routerid: Ipv4Addr = if mainsection.contains_key("routerid") {
            match mainsection["routerid"] {
                Some(ref s) => match s.parse() {
                    Ok(q) => q,
                    Err(_) => Ipv4Addr::new(1, 1, 1, 1),
                },
                None => Ipv4Addr::new(1, 1, 1, 1),
            }
        } else {
            Ipv4Addr::new(1, 1, 1, 1)
        };
        let listen: Vec<SocketAddr> = if mainsection.contains_key("listen") {
            match mainsection["listen"] {
                Some(ref s) => s
                    .split(&[',', ' ', '\t'][..])
                    .map(|cs| cs.parse::<SocketAddr>())
                    .filter_map(|r| r.ok())
                    .collect(),
                None => {
                    vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 179)]
                }
            }
        } else {
            vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 179)]
        };
        let skplist: Vec<BgpNet> = if mainsection.contains_key("skiplist") {
            match mainsection["skiplist"] {
                Some(ref s) => s
                    .split(&[',', ' ', '\t'][..])
                    .map(|cs| cs.parse::<BgpNet>())
                    .filter_map(|r| r.ok())
                    .collect(),
                None => {
                    vec![]
                }
            }
        } else {
            vec![]
        };
        let httplisten: SocketAddr = if mainsection.contains_key("httplisten") {
            match mainsection["httplisten"] {
                Some(ref s) => match s.parse() {
                    Ok(q) => q,
                    Err(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 8081),
                },
                None => SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 8081),
            }
        } else {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 8081)
        };
        let http_files_root: String = if mainsection.contains_key("http_files_root") {
            mainsection["http_files_root"].as_ref().cloned()
        } else {
            None
        }
        .unwrap_or("./contrib/".to_string());
        let peers: Vec<ConfigPeer> = if mainsection.contains_key("peers") {
            match mainsection["peers"] {
                Some(ref s) => s
                    .split(&[',', ' ', '\t'][..])
                    .map(|peername| {
                        if conf.contains_key(peername) {
                            Some(&conf[peername])
                        } else {
                            None
                        }
                    })
                    .filter(|x| x.is_some())
                    .map(|sect| ConfigPeer::from_section(sect.unwrap()))
                    .filter_map(|r| r.ok())
                    .collect(),
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };

        let nh4: Ipv4Addr = if mainsection.contains_key("nexthop") {
            match mainsection["nexthop"] {
                Some(ref s) => match s.parse() {
                    Ok(q) => q,
                    Err(_) => Ipv4Addr::new(1, 1, 1, 1),
                },
                None => Ipv4Addr::new(1, 1, 1, 1),
            }
        } else {
            Ipv4Addr::new(1, 1, 1, 1)
        };
        let nh6: Ipv6Addr = if mainsection.contains_key("nexthop6") {
            match mainsection["nexthop6"] {
                Some(ref s) => match s.parse() {
                    Ok(q) => q,
                    Err(_) => Ipv6Addr::new(0xffff, 0, 0, 0, 0, 0, 0x0101, 0x0101),
                },
                None => Ipv6Addr::new(0xffff, 0, 0, 0, 0, 0, 0x0101, 0x0101),
            }
        } else {
            Ipv6Addr::new(0xffff, 0, 0, 0, 0, 0, 0x0101, 0x0101)
        };
        let cms: BgpCommunityList = if mainsection.contains_key("communities") {
            match mainsection["communities"] {
                Some(ref s) => match s.parse() {
                    Ok(q) => q,
                    Err(_) => BgpCommunityList::new(),
                },
                None => BgpCommunityList::new(),
            }
        } else {
            BgpCommunityList::new()
        };
        let dur: chrono::Duration = if mainsection.contains_key("duration") {
            match mainsection["duration"] {
                Some(ref s) => match s.parse() {
                    Ok(q) => chrono::Duration::seconds(q),
                    Err(_) => chrono::Duration::hours(1),
                },
                None => chrono::Duration::hours(1),
            }
        } else {
            chrono::Duration::hours(1)
        };
        Ok(SvcConfig {
            routerid,
            activepeers: peers,
            listenat: listen,
            nexthop4: nh4,
            nexthop6: nh6,
            skiplist: skplist,
            httplisten,
            communities: cms,
            default_duration: dur,
            http_files_root,
        })
    }
    pub fn in_skiplist(&self, c: &BgpNet) -> bool {
        self.skiplist.iter().any(|x| x.contains(c))
    }
}

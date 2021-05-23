use crate::*;
use std::error::Error;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct ConfigPeer {
    pub peeraddr: IpAddr,
    pub peeras: u32,
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
}
impl FromStr for ConfigPeer {
    type Err = ErrorConfig;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let sp: Vec<&str> = s.split(" AS").collect();
        if sp.len() != 2 {
            return Err(ErrorConfig::from_str("invalid peer format (ip ASn)"));
        }
        Ok(ConfigPeer {
            peeraddr: sp[0].parse()?,
            peeras: sp[1].parse()?,
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
        let conf = ini!(inifile);
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
                    .filter(|x| x.is_ok())
                    .map(|r| r.unwrap())
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
                    .filter(|x| x.is_ok())
                    .map(|r| r.unwrap())
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
        let peers: Vec<ConfigPeer> = if mainsection.contains_key("peers") {
            match mainsection["peers"] {
                Some(ref s) => s
                    .split(&[',', '\t'][..])
                    .map(|cs| cs.parse::<ConfigPeer>())
                    .filter(|x| x.is_ok())
                    .map(|r| r.unwrap())
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
            routerid: routerid,
            activepeers: peers,
            listenat: listen,
            nexthop4: nh4,
            nexthop6: nh6,
            skiplist: skplist,
            httplisten: httplisten,
            communities: cms,
            default_duration: dur,
        })
    }
    pub fn in_skiplist(&self, c: &BgpNet) -> bool {
        self.skiplist.iter().find(|x| x.contains(c)).is_some()
    }
}

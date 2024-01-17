use crate::bgprib::*;
use serde::de::{self, Visitor};
use std::fmt::Debug;
use zettabgp::prelude::*;

const FP_SRC: &str = "src";
const FP_DST: &str = "dst";
const FP_BOTH: &str = "both";

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum FilterPref {
    Src,
    Dst,
    Both,
}
impl std::fmt::Display for FilterPref {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            FilterPref::Src => f.write_str(FP_SRC),
            FilterPref::Dst => f.write_str(FP_DST),
            FilterPref::Both => f.write_str(FP_BOTH),
        }
    }
}
#[derive(Debug, Clone)]
pub struct FilterItem {
    pub pref: FilterPref,
    pub net: BgpNet,
}
impl FilterItem {
    pub fn update_to(&self, upd: &mut BgpRibUpdate) {
        match &self.net {
            BgpNet::V4(a) => {
                if self.pref == FilterPref::Src || self.pref == FilterPref::Both {
                    upd.updates4
                        .insert(FilterTarget::new(FilterDirection::Src, a.clone()));
                }
                if self.pref == FilterPref::Dst || self.pref == FilterPref::Both {
                    upd.updates4
                        .insert(FilterTarget::new(FilterDirection::Dst, a.clone()));
                }
            }
            BgpNet::V6(a) => {
                if self.pref == FilterPref::Src || self.pref == FilterPref::Both {
                    upd.updates6
                        .insert(FilterTarget::new(FilterDirection::Src, a.clone()));
                }
                if self.pref == FilterPref::Dst || self.pref == FilterPref::Both {
                    upd.updates6
                        .insert(FilterTarget::new(FilterDirection::Dst, a.clone()));
                }
            }
            _ => {}
        };
    }
    pub fn withdraw_to(&self, upd: &mut BgpRibUpdate) {
        match &self.net {
            BgpNet::V4(a) => {
                if self.pref == FilterPref::Src || self.pref == FilterPref::Both {
                    upd.withdraws4
                        .insert(FilterTarget::new(FilterDirection::Src, a.clone()));
                }
                if self.pref == FilterPref::Dst || self.pref == FilterPref::Both {
                    upd.withdraws4
                        .insert(FilterTarget::new(FilterDirection::Dst, a.clone()));
                }
            }
            BgpNet::V6(a) => {
                if self.pref == FilterPref::Src || self.pref == FilterPref::Both {
                    upd.withdraws6
                        .insert(FilterTarget::new(FilterDirection::Src, a.clone()));
                }
                if self.pref == FilterPref::Dst || self.pref == FilterPref::Both {
                    upd.withdraws6
                        .insert(FilterTarget::new(FilterDirection::Dst, a.clone()));
                }
            }
            _ => {}
        };
    }
}
impl std::str::FromStr for FilterItem {
    type Err = BgpError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut pref = FilterPref::Both;
        let mut rest_s = s.trim();
        if rest_s.starts_with(FP_SRC) {
            pref = FilterPref::Src;
            rest_s = rest_s.strip_prefix(FP_SRC).unwrap_or(rest_s).trim();
        } else if rest_s.starts_with(FP_DST) {
            pref = FilterPref::Dst;
            rest_s = rest_s.strip_prefix(FP_DST).unwrap_or(rest_s).trim();
        } else if rest_s.starts_with(FP_BOTH) {
            pref = FilterPref::Both;
            rest_s = rest_s.strip_prefix(FP_BOTH).unwrap_or(rest_s).trim();
        }
        if let Ok(ip4) = rest_s.parse::<BgpAddrV4>() {
            return Ok(FilterItem {
                pref,
                net: BgpNet::V4(ip4),
            });
        };
        if let Ok(ip6) = rest_s.parse::<BgpAddrV6>() {
            return Ok(FilterItem {
                pref,
                net: BgpNet::V6(ip6),
            });
        };
        Err(BgpError::Static("Invalid FilterItem"))
    }
}
impl std::fmt::Display for FilterItem {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} {}", self.pref, self.net)
    }
}

impl serde::Serialize for FilterItem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.to_string().as_str())
    }
}

struct FilterItemVisitor;

impl<'de> Visitor<'de> for FilterItemVisitor {
    type Value = FilterItem;
    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a fileritem for ipv4/ipv6 prefix")
    }
    fn visit_str<E>(self, value: &str) -> Result<FilterItem, E>
    where
        E: serde::de::Error,
    {
        value.parse::<FilterItem>().map_err(de::Error::custom)
    }
}
impl<'de> serde::Deserialize<'de> for FilterItem {
    fn deserialize<D>(deserializer: D) -> Result<FilterItem, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_string(FilterItemVisitor)
    }
}

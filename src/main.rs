extern crate futures;
extern crate futures_util;
extern crate hyper;
extern crate tokio;
#[macro_use]
extern crate ini;
#[macro_use]
extern crate log;
extern crate pretty_env_logger;
extern crate url;
extern crate zettabgp;

pub mod bgppeer;
pub mod bgprib;
pub mod config;

pub use bgppeer::*;
pub use bgprib::*;
pub use config::*;

use chrono::prelude::*;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio_util::codec::{BytesCodec, FramedRead};
pub use zettabgp::prelude::*;

static NOTFOUND: &[u8] = b"Not Found";
static DONE: &[u8] = b"Done";
pub const HEADER_CONTENT_TYPE: &[u8] = b"Content-Type";
pub const CONTENT_TYPE_TEXT: &str = "text/plain; charset=utf8";
pub const CONTENT_TYPE_JSON: &str = "application/json; charset=utf8";

/// HTTP status code 404
fn not_found() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_TEXT)
        .body(NOTFOUND.into())
        .unwrap()
}
/// HTTP status code 200
fn request_done() -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_TEXT)
        .body(DONE.into())
        .unwrap()
}
async fn simple_file_send(filename: &str) -> Response<Body> {
    if let Ok(file) = tokio::fs::File::open(filename).await {
        let stream = FramedRead::new(file, BytesCodec::new());
        let body = Body::wrap_stream(stream);
        return Response::builder()
            .status(StatusCode::OK)
            //.header(HEADER_CONTENT_TYPE, CONTENT_TYPE_TEXT)
            .body(body)
            .unwrap();
    }
    warn!("file {} open error", filename);
    not_found()
}
pub fn get_url_params(req: &Request<Body>) -> HashMap<String, String> {
    req.uri()
        .query()
        .map(|v| {
            url::form_urlencoded::parse(v.as_bytes())
                .into_owned()
                .collect()
        })
        .unwrap_or_else(HashMap::new)
}
pub fn get_url_param<T: std::str::FromStr>(
    hashmap: &HashMap<String, String>,
    keyname: &str,
) -> Option<T> {
    match hashmap.get(&keyname.to_string()) {
        None => None,
        Some(vs) => match vs.parse::<T>() {
            Err(_) => None,
            Ok(n) => Some(n),
        },
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct NetsAdd {
    duration: Option<u32>,
    nets: Vec<BgpNet>,
}

#[derive(Debug, Serialize, Deserialize)]
struct HandleNets {
    add: Option<NetsAdd>,
    remove: Option<Vec<BgpNet>>,
}

pub struct Svc {
    pub bgprib: Arc<Mutex<BgpRib>>,
    pub cfg: Arc<SvcConfig>,
    pub cancel: tokio_util::sync::CancellationToken,
}
impl Svc {
    pub fn new(c: Arc<SvcConfig>, cancel_tok: tokio_util::sync::CancellationToken) -> Svc {
        Svc {
            bgprib: Arc::new(Mutex::new(BgpRib::new(c.clone(), cancel_tok.clone()))),
            cfg: c.clone(),
            cancel: cancel_tok,
        }
    }
    pub async fn run(&self) {
        BgpRib::start(self.bgprib.clone()).await;
    }
    async fn handle_json_str_msg(&self, req: &str) -> Result<Response<Body>, BgpError> {
        let q: HandleNets = match serde_json::from_str(req) {
            Ok(r) => r,
            Err(e) => {
                warn!("JSON parse error: {:?}", e);
                return Ok(not_found());
            }
        };
        self.handle_json_msg(&q).await
    }
    async fn handle_json_msg(&self, req: &HandleNets) -> Result<Response<Body>, BgpError> {
        debug!("{:?}", req);
        let mut upd = BgpRibUpdate::new();
        let mut dt = Local::now();
        {
            dt += self.cfg.default_duration;
            if let Some(ref nadd) = req.add {
                if let Some(secs) = nadd.duration {
                    dt = Local::now() + chrono::Duration::seconds(secs as i64);
                };
                nadd.nets.iter().for_each(|x| {
                    if !self.cfg.in_skiplist(x) {
                        match x {
                            BgpNet::V4(a) => {
                                upd.updates4.insert(a.clone());
                            }
                            BgpNet::V6(a) => {
                                upd.updates6.insert(a.clone());
                            }
                            _ => {}
                        };
                    };
                });
            }
            if let Some(ref nrem) = req.remove {
                nrem.iter().for_each(|x| {
                    if !self.cfg.in_skiplist(x) {
                        match x {
                            BgpNet::V4(a) => {
                                upd.withdraws4.insert(a.clone());
                            }
                            BgpNet::V6(a) => {
                                upd.withdraws6.insert(a.clone());
                            }
                            _ => {}
                        }
                    }
                })
            };
        };
        if !upd.is_empty() {
            self.bgprib.lock().await.change(upd, dt).await;
        }
        Ok(request_done())
    }
    pub async fn handle_req(&self, req: Request<Body>) -> Result<Response<Body>, BgpError> {
        let ruri = req.uri().clone();
        let requri = ruri.path();
        if requri.len() > 5 && requri[..5] == "/api/"[..5] {
            let urlparts: Vec<&str> = requri.split('/').collect();
            if urlparts.len() > 2 {
                match urlparts[2] {
                    "status" => {
                        let mut ret = Vec::new();
                        {
                            let rib = self.bgprib.lock().await;
                            let peers = rib.peers.read().await;
                            write!(&mut ret, "{{'peers':{{")?;
                            let mut first: bool = true;
                            for pr in peers.peershells.iter() {
                                if !first {
                                    write!(&mut ret, ",")?;
                                }
                                write!(
                                    &mut ret,
                                    "'{}':'{}'",
                                    pr.0,
                                    pr.1.read().await.state.read().await
                                )?;
                                first = false;
                            }
                            write!(&mut ret, "}}")?;
                            write!(
                                &mut ret,
                                ",'routes':{{'ipv4':{},'ipv6':{}}}",
                                rib.ipv4.len(),
                                rib.ipv6.len()
                            )?;
                            write!(&mut ret, "}}")?;
                        }
                        Ok(Response::builder()
                            .status(StatusCode::OK)
                            .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON)
                            .body(ret.into())
                            .unwrap())
                    }
                    "ping" => Ok(Response::builder()
                        .status(StatusCode::OK)
                        .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_TEXT)
                        .body("pong".into())
                        .unwrap()),
                    "dumprib" => {
                        let rib = self.bgprib.lock().await;
                        let mut ret: String = String::new();
                        for i in rib.ipv4.iter() {
                            ret += format!("{}\t{}\n", i.0, i.1).as_str();
                        }
                        for i in rib.ipv6.iter() {
                            ret += format!("{}\t{}\n", i.0, i.1).as_str();
                        }
                        Ok(Response::builder()
                            .status(StatusCode::OK)
                            .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_TEXT)
                            .body(ret.into())
                            .unwrap())
                    }
                    "dumpribjson" => {
                        let rib = self.bgprib.lock().await;
                        let mut ret: String = String::new();
                        ret += "{";
                        let mut is_first: bool = true;
                        for i in rib.ipv4.iter() {
                            if !is_first {
                                ret.push_str(",\n");
                            }
                            ret += format!("\"{}\":\"{}\"", i.0, i.1).as_str();
                            is_first = false;
                        }
                        for i in rib.ipv6.iter() {
                            if !is_first {
                                ret.push_str(",\n");
                            }
                            ret += format!("\"{}\":\"{}\",\n", i.0, i.1).as_str();
                            is_first = false;
                        }
                        ret += "}";
                        Ok(Response::builder()
                            .status(StatusCode::OK)
                            .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON)
                            .body(ret.into())
                            .unwrap())
                    }
                    "form_json" => {
                        let (_, body) = req.into_parts();
                        let bts = match hyper::body::to_bytes(body).await {
                            Ok(r) => r,
                            Err(e) => {
                                error!("Body get error: {:?}", e);
                                return Ok(not_found());
                            }
                        };
                        let vls = url::form_urlencoded::parse(&bts)
                            .into_owned()
                            .collect::<HashMap<String, String>>();
                        let vl = match vls.get("value") {
                            None => return Ok(not_found()),
                            Some(x) => x,
                        };
                        self.handle_json_str_msg(vl.as_str()).await
                    }
                    "json" => {
                        let (_, body) = req.into_parts();
                        let bts = match hyper::body::to_bytes(body).await {
                            Ok(r) => r,
                            Err(e) => {
                                error!("Body get error: {:?}", e);
                                return Ok(not_found());
                            }
                        };
                        let q: HandleNets = match serde_json::from_slice(&bts) {
                            Ok(r) => r,
                            Err(e) => {
                                warn!("JSON parse error: {:?}", e);
                                return Ok(not_found());
                            }
                        };
                        self.handle_json_msg(&q).await
                    }
                    "add" => {
                        let (_, body) = req.into_parts();
                        let bts = match hyper::body::to_bytes(body).await {
                            Ok(r) => r,
                            Err(e) => {
                                warn!("Body get error: {:?}", e);
                                return Ok(not_found());
                            }
                        };
                        let mut vls = url::form_urlencoded::parse(&bts)
                            .into_owned()
                            .collect::<HashMap<String, String>>();
                        if vls.is_empty() {
                            let qry = ruri.query().unwrap_or("");
                            vls = url::form_urlencoded::parse(qry.as_bytes())
                                .into_owned()
                                .collect::<HashMap<String, String>>();
                        }
                        debug!("{:?}", vls);
                        let net: BgpNet = vls
                            .get("net")
                            .ok_or(BgpError::static_str("missing net"))?
                            .parse()
                            .ok()
                            .ok_or(BgpError::static_str("invalid net"))?;
                        let dur: u32 = vls
                            .get("duration")
                            .or(vls.get("dur"))
                            .map(|s| s.parse().ok())
                            .flatten()
                            .unwrap_or(0);
                        let dt = Local::now()
                            + if dur == 0 {
                                self.cfg.default_duration
                            } else {
                                chrono::Duration::seconds(dur as i64)
                            };
                        let mut upd = BgpRibUpdate::new();
                        match net {
                            BgpNet::V4(v4) => {
                                upd.updates4.insert(v4);
                            }
                            BgpNet::V6(v6) => {
                                upd.updates6.insert(v6);
                            }
                            _ => {}
                        };
                        self.bgprib.lock().await.change(upd, dt).await;
                        Ok(request_done())
                    }
                    "remove" => {
                        let (_, body) = req.into_parts();
                        let bts = match hyper::body::to_bytes(body).await {
                            Ok(r) => r,
                            Err(e) => {
                                error!("Body get error: {:?}", e);
                                return Ok(not_found());
                            }
                        };
                        let mut vls = url::form_urlencoded::parse(&bts)
                            .into_owned()
                            .collect::<HashMap<String, String>>();
                        if vls.is_empty() {
                            let qry = ruri.query().unwrap_or("");
                            vls = url::form_urlencoded::parse(qry.as_bytes())
                                .into_owned()
                                .collect::<HashMap<String, String>>();
                        }
                        debug!("{:?}", vls);
                        let net: BgpNet = vls
                            .get("net")
                            .ok_or(BgpError::static_str("missing net"))?
                            .parse()
                            .ok()
                            .ok_or(BgpError::static_str("invalid net"))?;
                        let dt = Local::now();
                        let mut upd = BgpRibUpdate::new();
                        match net {
                            BgpNet::V4(v4) => {
                                upd.withdraws4.insert(v4);
                            }
                            BgpNet::V6(v6) => {
                                upd.withdraws6.insert(v6);
                            }
                            _ => {}
                        };
                        self.bgprib.lock().await.change(upd, dt).await;
                        Ok(request_done())
                    }
                    _ => Ok(not_found()),
                }
            } else {
                Ok(not_found())
            }
        } else {
            let filepath = String::new()
                + self.cfg.http_files_root.as_str()
                + (match requri {
                    "/" => "/index.html",
                    s => s,
                });
            Ok(simple_file_send(filepath.as_str()).await)
        }
    }
    pub async fn response_fn(&self, req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
        Ok(match self.handle_req(req).await {
            Ok(r) => r,
            Err(e) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(format!("{}", e).into())
                .unwrap(),
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    pretty_env_logger::init_timed();
    let mut cfgfile: String = "ban2bgp.ini".to_string();
    let mut argstate: u8 = 0;
    for stra in std::env::args() {
        if argstate == 1 {
            cfgfile = stra.to_string();
            argstate = 0;
            continue;
        };
        if stra == "-c" {
            argstate = 1;
            continue;
        }
        if stra == "-h" {
            eprintln!("-h - this help\n-c inifile - configuration file, ban2bgp.ini default");
            return Ok(());
        }
        argstate = 0;
    }
    let conf = match SvcConfig::from_inifile(cfgfile.as_str()) {
        Ok(sc) => Arc::new(sc),
        Err(e) => {
            eprintln!("{}", e);
            return Ok(());
        }
    };
    let httplisten = conf.httplisten;
    let token = tokio_util::sync::CancellationToken::new();
    let svc = Arc::new(RwLock::new(Svc::new(conf.clone(), token.clone())));
    let svc1 = svc.clone();
    tokio::task::spawn(async move {
        svc1.read().await.run().await;
    });
    {
        let _svc = svc.clone();
        let service = {
            make_service_fn(|_| {
                let _svc1 = _svc.clone();
                async move {
                    let _svc2 = _svc1.clone();
                    Ok::<_, hyper::Error>(service_fn(move |req: Request<Body>| {
                        let _svc3 = _svc2.clone();
                        async move { _svc3.read().await.response_fn(req).await }
                    }))
                }
            })
        };
        info!("Listening on http://{}", httplisten);
        if let Err(e) = Server::bind(&httplisten).serve(service).await {
            error!("server error: {}", e);
        }
        token.cancel();
    };
    Ok(())
}

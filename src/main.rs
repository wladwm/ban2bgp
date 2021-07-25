extern crate futures;
extern crate futures_util;
extern crate hyper;
extern crate tokio;
#[macro_use]
extern crate ini;
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
/// HTTP status code 404
fn not_found() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(NOTFOUND.into())
        .unwrap()
}
async fn simple_file_send(filename: &str) -> Result<Response<Body>, hyper::Error> {
    if let Ok(file) = tokio::fs::File::open(filename).await {
        let stream = FramedRead::new(file, BytesCodec::new());
        let body = Body::wrap_stream(stream);
        return Ok(Response::new(body));
    }
    Ok(not_found())
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
    pub cfg: Arc<Mutex<SvcConfig>>,
    pub cancel: tokio_util::sync::CancellationToken,
}
impl Svc {
    pub fn new(c: Arc<Mutex<SvcConfig>>, cancel_tok: tokio_util::sync::CancellationToken) -> Svc {
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
                eprintln!("JSON parse error: {:?}", e);
                return Ok(not_found());
            }
        };
        self.handle_json_msg(&q).await
    }
    async fn handle_json_msg(&self, req: &HandleNets) -> Result<Response<Body>, BgpError> {
        eprintln!("{:?}", req);
        let mut upd = BgpRibUpdate::new();
        let mut dt = Local::now();
        {
            let cfg = self.cfg.lock().await;
            dt = dt + cfg.default_duration;
            if let Some(ref nadd) = req.add {
                if let Some(secs) = nadd.duration {
                    dt = Local::now() + chrono::Duration::seconds(secs as i64);
                };
                nadd.nets.iter().for_each(|x| {
                    if !cfg.in_skiplist(x) {
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
                    if !cfg.in_skiplist(x) {
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
        return Ok(Response::new(Body::from("Done")));
    }
    pub async fn handle_req(&self, req: Request<Body>) -> Result<Response<Body>, BgpError> {
        let ruri = req.uri().clone();
        let requri = ruri.path();
        if requri.len() > 5 {
            if requri[..5] == "/api/"[..5] {
                let urlparts: Vec<&str> = requri.split('/').collect();
                if urlparts.len() > 2 {
                    match urlparts[2] {
                        "status" => {
                            let mut ret = Vec::new();
                            {
                                let rib = self.bgprib.lock().await;
                                let peers = rib.peers.read().await;
                                write!(&mut ret, "{}", "{'peers':{")?;
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
                                write!(&mut ret, "{}", "}")?;
                                write!(
                                    &mut ret,
                                    "{}:{},{}:{}{}",
                                    ",'routes':{'ipv4'",
                                    rib.ipv4.iter().count(),
                                    "'ipv6'",
                                    rib.ipv6.iter().count(),
                                    "}"
                                )?;
                                write!(&mut ret, "{}", "}")?;
                            }
                            return Ok(Response::builder()
                                .status(StatusCode::OK)
                                .header("Content-type", "text/json")
                                .body(ret.into())
                                .unwrap());
                        }
                        "ping" => {
                            return Ok(Response::new(Body::from("pong")));
                        }
                        "dumprib" => {
                            let rib = self.bgprib.lock().await;
                            let mut ret: String = String::new();
                            for i in rib.ipv4.iter() {
                                ret += format!("{}\t{}\n", i.0, i.1).as_str();
                            }
                            for i in rib.ipv6.iter() {
                                ret += format!("{}\t{}\n", i.0, i.1).as_str();
                            }
                            return Ok(Response::builder()
                                .status(StatusCode::OK)
                                .body(ret.into())
                                .unwrap());
                        }
                        "form_json" => {
                            let (_, body) = req.into_parts();
                            let bts = match hyper::body::to_bytes(body).await {
                                Ok(r) => r,
                                Err(e) => {
                                    eprintln!("Body get error: {:?}", e);
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
                            return self.handle_json_str_msg(vl.as_str()).await;
                        }
                        "json" => {
                            let (_, body) = req.into_parts();
                            let bts = match hyper::body::to_bytes(body).await {
                                Ok(r) => r,
                                Err(e) => {
                                    eprintln!("Body get error: {:?}", e);
                                    return Ok(not_found());
                                }
                            };
                            let q: HandleNets = match serde_json::from_slice(&bts) {
                                Ok(r) => r,
                                Err(e) => {
                                    eprintln!("JSON parse error: {:?}", e);
                                    return Ok(not_found());
                                }
                            };
                            return self.handle_json_msg(&q).await;
                        }
                        "add" => {
                            let (_, body) = req.into_parts();
                            let bts = match hyper::body::to_bytes(body).await {
                                Ok(r) => r,
                                Err(e) => {
                                    eprintln!("Body get error: {:?}", e);
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
                            eprintln!("{:?}", vls);
                            let net: BgpNet = vls
                                .get("net")
                                .ok_or(BgpError::static_str("missing net"))?
                                .parse()
                                .ok()
                                .ok_or(BgpError::static_str("invalid net"))?;
                            let dur: u32 = match vls.get("duration") {
                                None => 0,
                                Some(s) => s.parse::<u32>().unwrap_or(0),
                            };
                            let dt = Local::now()
                                + if dur == 0 {
                                    self.cfg.lock().await.default_duration
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
                            return Ok(Response::new(Body::from("Done")));
                        }
                        "remove" => {
                            let (_, body) = req.into_parts();
                            let bts = match hyper::body::to_bytes(body).await {
                                Ok(r) => r,
                                Err(e) => {
                                    eprintln!("Body get error: {:?}", e);
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
                            eprintln!("{:?}", vls);
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
                            return Ok(Response::new(Body::from("Done")));
                        }
                        _ => {
                            return Ok(Response::new(Body::from("No service")));
                        }
                    }
                }
            }
        };
        return Ok(Response::new(Body::from("Done")));
    }
    pub async fn response_fn(&self, req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
        let requri = req.uri().path();
        if requri.len() > 5 {
            return Ok(match self.handle_req(req).await {
                Ok(r) => r,
                Err(e) => Response::builder()
                    .status(StatusCode::OK)
                    .body(format!("{}", e).into())
                    .unwrap(),
            });
        }
        let filepath = String::new()
            + "./"
            + (match requri {
                "/" => "/index.html",
                s => s,
            });
        //println!("File {}",filepath);
        simple_file_send(filepath.as_str()).await
        //return Ok(not_found());
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
        Ok(sc) => Arc::new(Mutex::new(sc)),
        Err(e) => {
            eprintln!("{}", e);
            return Ok(());
        }
    };
    let httplisten = conf.lock().await.httplisten.clone();
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
        println!("Listening on http://{}", httplisten);
        if let Err(e) = Server::bind(&httplisten).serve(service).await {
            eprintln!("server error: {}", e);
        }
        token.cancel();
    };
    Ok(())
}

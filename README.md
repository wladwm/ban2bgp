ban2bgp
====================

## Description

This is a network distributed intrusion prevention system controlled via HTTP API.
When bad guys tries to knock your servers or routers, their IP can be immediately blocked for specified amount of time for all your network by two ways:
1. bad IP's announced as as hotroutes to null to your RR's, so bad guys will not got responses.
2. bad IP's announced as flowspec rules, blocking any traffic from these addresses.
Please be caution with attack sources, because some session-less protocols like DNS is not recommended as attack source, because source IP's can be easy spoofed.

When no filtering direction (src/dst) specified, it assumed both.
Blackhole routing obviously supports only dst.
Flowspec filtering supports src and dst filtering.

## Quick start

At first you will need at least a BGP RR to connect ban2bgp with :-). And some PC with Linux/FreeBSD (although Windows or Mac will work too, I hope) to run ban2bgp.

At second you should configure your router to accept bgp connections from your PC.
For example, router you have has IP 10.0.0.1, AS 65535. PC with ban2bgp has IP 10.1.1.1.
In Cisco dialect it will be something like:
```
! Set nexthop for denied routes to null
ip route 198.18.0.1 255.255.255.255 Null0
! Set up the BGP
router bgp 65535
 ! create a neighbor with your own AS, so it will be IBGP
 neighbor 10.1.1.1 remote-as 65535
 ! specify source IP
 neighbor 10.1.1.1 update-source Loopback0
 ! do not attempt to connect from router to PC, only from PC to router
 neighbor 10.1.1.1 transport connection-mode passive
 address-family ipv4
 ! it has ipv4 unicast address family
  neighbor 10.1.1.1 activate
 ! send all routing information to PC
  neighbor 10.1.1.1 route-reflector-client
```
Now on PC with Linux or FreeBSD:
```
$ git clone https://github.com/wladwm/ban2bgp
... git messages
$ cd ban2bgp
$ cargo build
... cargo messages
$ cat > ban2bgp.ini <<EOF
[main]
httplisten=0.0.0.0:8080
listen=0.0.0.0:1179
nexthop=198.18.0.1
communities=666:666
peers=r1
duration=3600
skiplist=10.0.0.0/24

[peer_r1]
peer=10.0.0.1
as=65535
mode=blackhole
EOF

$ RUST_LOG=info cargo run
Listening on http://0.0.0.0:8080
Connecting to 10.0.0.1:179
Connected to 10.0.0.1:179
```

After this you can check on your router that BGP session with 10.1.1.1 is established.
Add denyroute for host 10.2.2.2 for an hour via http request:
/usr/bin/curl http://127.0.0.1:8080/api/add?net=10.2.2.2&dur=3600

Or you can use bundled minimal SPA by pointing your browser to http://127.0.0.1:8080/

In ther contrib folder you can find an example configuration for fail2ban.
when you specify in the jain config 
banaction=ban2bgp
instead of
banaction=iptables...

Bad hosts will be banned not for just single server, but for your whole network.
Also you can easily integrate ban2bgp on your syslog server for all your routers.

## Configuration

ban2bgp looks for configuration in file ban2bgp.ini in current directory.
This file have on [main] section
Main section parameters:
* httplisten - bind address and port for inner http server, default 0.0.0.0:8080.
* http_files_root - http serve files from this location, default ./contrib/
* listen - not necessary BGP listen point, default 0.0.0.0:179, which required root privileges because 179<1024.
* nexthop - ipv4 nexthop for denied routes. All your routers should have this routed to Null.
* nexthop6 - ipv6 nexthop for denied routes.
* communities - spaced-separated communites list for denied routes.
* peers - comma-separated (or space- or tab-separated) list for BGP peers. Each peer should have a separate section in ini file.
* duration - default denied route time to live in seconds.
* skiplist - comma-separated list for networks, which are skipped in add requests.
Peer section parameters:
* peer - peer ip address
* as - peer AS
* localas - unnecessary, local AS number  if it not the same as peer AS
* mode - peer mode - blackhole or flowspec


## API endpoints

* /api/add
  Parameters: 
  * net - ipv4/ipv6 route to anounce
  * dur - ttl in seconds

* /api/remove
  Parameters: 
  * net - ipv4/ipv6 route to be removed from announces

* /api/json
  Block operation. Requires a valid JSON Object in POST body with optional keys "add" and "remove".
  remove - an array of string routes to be removed, add - an object with "duration" and "nets". "nets" - an array of string routes to be added for "duration" seconds.
  Example:
  {"add":{"duration":86400,"nets":["dst 33.3.3.3"]},"remove":["11.1.1.1","22.2.2.2"]}

* /api/ping
  Checks service liveness. Returns "pong"
    
* /api/status
  Returns JSON object with per peer status and count opf ipv4 and ipv6 routes
  {'peers':{'10.0.0.1':'InSync'},'routes':{'ipv4':0,'ipv6':0}}
    
* /api/dumprib
  Returns text table with active routes and times

* /api/dumpribjson
  Returns json object with active routes and times

## Crates.io

https://crates.io/crates/ban2bgp

## Documentation

https://docs.rs/ban2bgp

## License

[MIT OR Apache-2.0](LICENSE)

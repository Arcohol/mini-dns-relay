mod packet;

use std::{
    collections::HashMap,
    env,
    io::BufRead,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
};

use packet::{QuestionEntry, RData, ResourceRecord};
use tokio::net::UdpSocket;
use tracing::{debug, error, info, trace};

pub type MsgMap = Arc<Mutex<HashMap<u16, (u16, SocketAddr)>>>;
pub type Hosts = HashMap<String, IpAddr>;

const BUF_SIZE: usize = 512;
const DEFAULT_TTL: usize = 600;

pub async fn run(config: Config) -> anyhow::Result<()> {
    let local_sock = UdpSocket::bind(&config.local_addr).await?;
    info!("local socket is listening on {}", &config.local_addr);

    let remote_sock = UdpSocket::bind(&config.remote_addr).await?;
    info!("remote socket is listening on {}", &config.remote_addr);

    let hosts = load_hosts(&config.hosts_path)?;
    debug!("hosts: {:?}", hosts);

    let msg_map: MsgMap = Arc::new(Mutex::new(HashMap::new()));

    tokio::try_join!(
        forward(
            &local_sock,
            &remote_sock,
            &hosts,
            msg_map.clone(),
            &config.upstream_addr
        ),
        reply(&local_sock, &remote_sock, msg_map.clone())
    )?;

    Ok(())
}

async fn forward(
    local_sock: &UdpSocket,
    remote_sock: &UdpSocket,
    hosts: &Hosts,
    msg_map: MsgMap,
    upstream: &str,
) -> anyhow::Result<()> {
    'outer: loop {
        let mut buf = [0u8; BUF_SIZE];

        let (len, addr) = local_sock.recv_from(&mut buf).await?;
        trace!("buf: {:x?}", &buf[..len]);

        let mut msg = packet::Message::new(&mut buf, len);
        info!("({:x?}) query received from {}", msg.header.get_id(), addr);

        let queries = msg.question.entries(msg.header.get_qdcount());
        debug!(
            "({:x?}) questions parsed: {:?}",
            msg.header.get_id(),
            queries
        );

        let mut local_answers = Vec::new();
        for query in queries {
            match process(&query, hosts) {
                Ok(Some(rr)) => {
                    debug!("({:x?}) local rr created: {:x?}", msg.header.get_id(), rr);
                    local_answers.push(rr);
                }
                Ok(None) => {}
                Err(e) => {
                    msg.header.set_qr(0b1);
                    msg.header.set_rcode(0b0011);

                    info!(
                        "({:x?}) query is {}, sending response back to {}",
                        msg.header.get_id(),
                        e,
                        addr
                    );
                    let len = msg.len();

                    trace!("buf: {:x?}", &buf[..len]);
                    local_sock.send_to(&buf[..len], addr).await?;

                    continue 'outer;
                }
            }
        }

        let local_ancount = local_answers.len() as u16;
        if local_ancount == msg.header.get_qdcount() {
            debug!(
                "({:x?}) constructed a total of {} local rr(s)",
                msg.header.get_id(),
                local_ancount
            );

            msg.header.set_qr(0b1);
            msg.header.set_ancount(local_ancount);
            msg.header.set_nscount(0);
            msg.header.set_arcount(0);
            msg.answer.add_entries(local_answers);

            info!(
                "({:x?}) query is processed locally, sending response back to {}",
                msg.header.get_id(),
                addr
            );
            let len = msg.len();

            trace!("buf: {:x?}", &buf[..len]);
            local_sock.send_to(&buf[..len], addr).await?;
        } else {
            info!(
                "({:x?}) query cannot be processed locally",
                msg.header.get_id()
            );

            {
                let mut map = msg_map.lock().unwrap();

                // try to generate a new id of 16 bits
                let mut new_id = rand::random::<u16>();
                while map.contains_key(&new_id) {
                    new_id = rand::random::<u16>();
                }

                map.insert(new_id, (msg.header.get_id(), addr));

                info!(
                    "({:x?}) new id generated: {:x?}",
                    msg.header.get_id(),
                    new_id
                );
                msg.header.set_id(new_id);
                // mutex guard dropped here
            }

            info!("({:x?}) query is sending to upstream", msg.header.get_id(),);

            trace!("buf: {:x?}", &buf[..len]);
            remote_sock.send_to(&buf[..len], &upstream).await?;
        }
    }
}

async fn reply(
    local_sock: &UdpSocket,
    remote_sock: &UdpSocket,
    msg_map: MsgMap,
) -> anyhow::Result<()> {
    loop {
        let mut buf = [0u8; BUF_SIZE];

        let (len, _) = remote_sock.recv_from(&mut buf).await?;
        trace!("buf: {:x?}", &buf[..len]);

        let mut msg = packet::Message::new(&mut buf, len);
        info!(
            "({:x?}) response received from upstream",
            msg.header.get_id()
        );

        let origin = msg_map.lock().unwrap().remove(&msg.header.get_id());
        match origin {
            Some((id, addr)) => {
                info!(
                    "({:x?}) the original query id is {:x?}, changing back to it",
                    msg.header.get_id(),
                    id
                );

                msg.header.set_id(id);

                info!(
                    "({:x?}) upstream response is sending back to {}",
                    msg.header.get_id(),
                    addr
                );

                let len = msg.len();
                trace!("buf: {:x?}", &buf[..len]);
                local_sock.send_to(&buf[..len], addr).await?;
            }
            None => {
                error!("({:x?}) no corresponding query found", msg.header.get_id());
            }
        }
    }
}

fn load_hosts(path: &str) -> anyhow::Result<Hosts> {
    let mut hosts = HashMap::new();

    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);

    for line in reader.lines() {
        let line = line?;
        let mut parts = line.split_whitespace();
        let ip = parts.next().ok_or(anyhow::anyhow!("invalid hosts file"))?;
        let ip = ip.parse::<IpAddr>()?;
        for cname in parts {
            hosts.entry(cname.to_owned()).or_insert(ip);
        }
    }

    Ok(hosts)
}

fn process(qe: &QuestionEntry, hosts: &Hosts) -> anyhow::Result<Option<ResourceRecord>> {
    match hosts.get(&qe.qname) {
        Some(ip) => match ip {
            IpAddr::V4(ip) if ip == &Ipv4Addr::UNSPECIFIED => {
                Err(anyhow::anyhow!("blocked"))
            }
            IpAddr::V4(ip) => {
                if qe.qtype != 1 {
                    return Ok(None);
                }
                let rr = ResourceRecord {
                    name: name_compressed(qe),
                    rtype: qe.qtype,
                    rclass: qe.qclass,
                    ttl: DEFAULT_TTL as u32,
                    rdlength: 4,
                    rdata: RData::V4(ip.octets()),
                };
                Ok(Some(rr))
            }
            IpAddr::V6(ip) => {
                if qe.qtype != 28 {
                    return Ok(None);
                }
                let rr = ResourceRecord {
                    name: name_compressed(qe),
                    rtype: qe.qtype,
                    rclass: qe.qclass,
                    ttl: DEFAULT_TTL as u32,
                    rdlength: 16,
                    rdata: RData::V6(ip.octets()),
                };
                Ok(Some(rr))
            }
        },
        None => Ok(None),
    }
}

fn name_compressed(qe: &QuestionEntry) -> u16 {
    0b1100_0000_0000_0000 | (qe.offset as u16)
}

#[derive(Debug)]
pub struct Config {
    pub local_addr: String,
    pub remote_addr: String,
    pub upstream_addr: String,
    pub hosts_path: String,
}

impl Config {
    pub fn from_env() -> Config {
        Config {
            local_addr: env::var("LOCAL_ADDR").unwrap_or("127.0.0.1:53".to_owned()),
            remote_addr: env::var("REMOTE_ADDR").unwrap_or("0.0.0.0:10053".to_owned()),
            upstream_addr: env::var("UPSTREAM_ADDR").unwrap_or("10.3.9.45:53".to_owned()),
            hosts_path: env::var("HOSTS_PATH").unwrap_or("hosts.txt".to_owned()),
        }
    }
}

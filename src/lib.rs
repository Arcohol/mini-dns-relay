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

pub type MsgMap = Arc<Mutex<HashMap<u16, SocketAddr>>>;
pub type Hosts = HashMap<String, IpAddr>;

const BUF_SIZE: usize = 512;
const DEFAULT_TTL: usize = 600;

pub async fn run(config: Config) -> anyhow::Result<()> {
    let local_sock = UdpSocket::bind(&config.local_addr).await?;
    let remote_sock = UdpSocket::bind(&config.remote_addr).await?;

    let hosts = load_hosts(&config.hosts_path)?;
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
        let mut msg = packet::Message::new(&mut buf, len);

        msg_map.lock().unwrap().insert(msg.header.get_id(), addr);

        let mut local_answers = Vec::new();
        for query in msg.question.entries(msg.header.get_qdcount()) {
            if let Some(ip) = hosts.get(&query.qname) {
                if ip == &Ipv4Addr::UNSPECIFIED {
                    msg.header.set_qr(0b1);
                    msg.header.set_rcode(0b0011);

                    msg_map.lock().unwrap().remove(&msg.header.get_id());

                    let len = msg.len();
                    local_sock.send_to(&buf[..len], addr).await?;
                    continue 'outer;
                }

                if query.qtype != 1 && query.qtype != 28 {
                    continue;
                }

                match ip {
                    IpAddr::V4(ip) => {
                        if query.qtype != 1 {
                            continue;
                        }
                        let rr = ResourceRecord {
                            name: name_compressed(&query),
                            rtype: query.qtype,
                            rclass: query.qclass,
                            ttl: DEFAULT_TTL as u32,
                            rdlength: 4,
                            rdata: RData::V4(ip.octets()),
                        };
                        local_answers.push(rr);
                    }
                    IpAddr::V6(ip) => {
                        if query.qtype != 28 {
                            continue;
                        }
                        let rr = ResourceRecord {
                            name: name_compressed(&query),
                            rtype: query.qtype,
                            rclass: query.qclass,
                            ttl: DEFAULT_TTL as u32,
                            rdlength: 16,
                            rdata: RData::V6(ip.octets()),
                        };
                        local_answers.push(rr);
                    }
                }
            }
        }

        let local_ancount = local_answers.len() as u16;
        if local_ancount == msg.header.get_qdcount() {
            msg.header.set_qr(0b1);
            msg.header.set_ancount(local_ancount);
            msg.header.set_nscount(0);
            msg.header.set_arcount(0);
            msg.answer.add_entries(local_answers);

            msg_map.lock().unwrap().remove(&msg.header.get_id());

            let len = msg.len();
            local_sock.send_to(&buf[..len], addr).await?;
        } else {
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
        let msg = packet::Message::new(&mut buf, len);

        if let Some(addr) = msg_map.lock().unwrap().remove(&msg.header.get_id()) {
            let len = msg.len();
            local_sock.send_to(&buf[..len], addr).await?;
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

fn name_compressed(qe: &QuestionEntry) -> u16 {
    0b1100_0000_0000_0000 | (qe.offset as u16)
}

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

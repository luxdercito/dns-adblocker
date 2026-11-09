use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use anyhow::{Context, Result};
use tokio::net::UdpSocket;
use tokio::sync::oneshot;
use tracing::{info, warn};

pub struct DnsAdBlockerServer {
    config: crate::config::Config,
}

impl DnsAdBlockerServer {
    pub fn new(config: &crate::config::Config) -> Self {
        Self {
            config: config.clone(),
        }
    }

    /// Start listening for UDP DNS queries on bind_port.
    /// For each incoming query, ask the engine worker whether the domain is blocked.
    /// Blocked domains get a 0.0.0.0 reply; otherwise the original query is forwarded upstream.
    pub async fn run(
        self,
        filter_manager: crate::filter_lists::FilterListManager,
    ) -> Result<()> {
        let socket = UdpSocket::bind(self.config.bind_socket())
            .await
            .context("Failed to bind UDP socket")?;
        info!("UDP DNS server listening on {}", self.config.bind_socket());

        let socket = Arc::new(socket);
        let upstreams: Vec<SocketAddr> = self.config.upstream_dns.iter()
            .map(|s| s.parse())
            .collect::<Result<Vec<_>, _>>()?;

        let fm_sender = filter_manager.sender.clone();

        let socket_recv = socket.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 65535];
            loop {
                match socket_recv.recv_from(&mut buf).await {
                    Ok((len, addr)) => {
                        // Only process valid DNS queries (QR=0 means standard query)
                        if len < 12 || (buf[2] & 0x80) != 0 { continue; }
                        let query_bytes = buf[..len].to_vec();
                        let sender = fm_sender.clone();
                        let ups = upstreams.clone();
                        let socket_send = socket_recv.clone();
                        tokio::spawn(async move {
                            match handle_query(query_bytes, sender, ups).await {
                                Ok(resp) => {
                                    let _ = socket_send.send_to(&resp, addr).await;
                                }
                                Err(e) => warn!("Error handling DNS query: {}", e),
                            }
                        });
                    }
                    Err(e) => warn!("UDP receive error: {}", e),
                }
            }
        });

        tokio::signal::ctrl_c().await?;
        info!("Shutting down");
        Ok(())
    }
}

/// Handle a single DNS query asynchronously.
/// Extracts the queried domain, asks the engine worker if it's blocked,
/// then either returns a local block response or forwards the
/// original query to an upstream resolver.
async fn handle_query(
    query_bytes: Vec<u8>,
    sender: Arc<Mutex<std::sync::mpsc::Sender<crate::filter_lists::EngineMsg>>>,
    upstreams: Vec<SocketAddr>,
) -> Result<Vec<u8>> {
    let domain = extract_domain(&query_bytes);

    // Ask the engine worker whether this domain is blocked
    let is_blocked = if !domain.is_empty() {
        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let tx = sender.lock().unwrap();
            let _ = tx.send(crate::filter_lists::EngineMsg::IsBlocked {
                domain: domain.clone(),
                reply_tx,
            });
        }
        reply_rx.await.unwrap_or(false)
    } else {
        false
    };

    if is_blocked {
        info!("BLOCKED: {}", domain);
        return Ok(build_block_response(&query_bytes));
    }

    // Not blocked — forward the ORIGINAL query directly to upstream asynchronously.
    resolve_upstream_exact(&query_bytes, &upstreams).await
}

/// Extract the primary domain name from a DNS query packet.
/// Skips any compression pointers and reads the qname portion.
fn extract_domain(query_bytes: &[u8]) -> String {
    if query_bytes.len() < 12 { return String::new(); }

    let qdcount = u16::from_be_bytes([query_bytes[4], query_bytes[5]]) as usize;
    if qdcount == 0 { return String::new(); }

    let mut off = 12usize;
    let mut clean = Vec::new();

    // Read the first label sequence (the domain name in question section)
    while off < query_bytes.len() {
        let b = query_bytes[off];
        if b == 0 {
            break;
        }
        if b & 0xC0 == 0xC0 {
            // Compression pointer — stop here
            break;
        }
        let len = b as usize;
        if off + 1 + len > query_bytes.len() { break; }
        clean.extend_from_slice(&query_bytes[off + 1..off + 1 + len]);
        clean.push(b'.');
        off += 1 + len;
    }

    // Remove trailing dot and convert to string
    if clean.ends_with(&[b'.']) { clean.pop(); }
    String::from_utf8(clean).unwrap_or_default().to_lowercase()
}

/// Build a blocking DNS response for the given query.
/// Returns a proper DNS response with QR=1, RA=1, RD preserved,
/// ANCOUNT=1, plus an A record pointing to 0.0.0.0.
fn build_block_response(query_bytes: &[u8]) -> Vec<u8> {
    let q_end = find_question_end(query_bytes);
    let mut resp = query_bytes[..q_end].to_vec();

    // Set response flags: QR=1 (bit7), RA=1 (bit4), RD preserved, Opcode preserved
    resp[2] |= 0x80; // QR = 1 (response)
    resp[3] |= 0x80; // RA = 1 (recursion available)

    // Set ANCOUNT = 1 at bytes [6:7]
    resp[6] = 0x00;
    resp[7] = 0x01;
    // NSCOUNT = 0 at [8:9]
    resp[8] = 0x00; resp[9] = 0x00;
    // ARCOUNT = 0 at [10:11]
    resp[10] = 0x00; resp[11] = 0x00;

    // Append Answer RR:
    // Name: pointer to question offset (0xC00C = 12)
    resp.push(0xC0);
    resp.push(0x0C);
    // Type: A (1)
    resp.push(0x00);
    resp.push(0x01);
    // Class: IN (1)
    resp.push(0x00);
    resp.push(0x01);
    // TTL: 60s
    resp.push(0x00);
    resp.push(0x00);
    resp.push(0x00);
    resp.push(0x3C);
    // RDLENGTH: 4
    resp.push(0x00);
    resp.push(0x04);
    // RDATA: 0.0.0.0
    resp.push(0x00);
    resp.push(0x00);
    resp.push(0x00);
    resp.push(0x00);

    resp
}

/// Find the end of the question section in a DNS query.
/// Returns the byte offset just past the last qtype+qclass bytes.
fn find_question_end(query: &[u8]) -> usize {
    if query.len() < 12 { return query.len(); }
    let qdcount = u16::from_be_bytes([query[4], query[5]]) as usize;
    let mut off = 12;
    for _ in 0..qdcount {
        while off < query.len() {
            let b = query[off];
            if b == 0 {
                off += 1;
                break;
            }
            if b & 0xC0 == 0xC0 {
                off += 2;
                break;
            }
            off += 1 + b as usize;
        }
        if off + 4 <= query.len() {
            off += 4;
        }
    }
    off
}

/// Forward DNS query asynchronously to upstream resolvers.
async fn resolve_upstream_exact(query: &[u8], upstreams: &[SocketAddr]) -> Result<Vec<u8>> {
    let sock = UdpSocket::bind("0.0.0.0:0").await?;
    for addr in upstreams {
        if sock.send_to(query, *addr).await.is_ok() {
            let mut buf = [0u8; 65535];
            match tokio::time::timeout(Duration::from_secs(3), sock.recv(&mut buf)).await {
                Ok(Ok(len)) if len > 0 => return Ok(buf[..len].to_vec()),
                Ok(Err(e)) => {
                    warn!("Upstream {} recv error: {}", addr, e);
                    continue;
                }
                _ => continue,
            }
        }
    }
    anyhow::bail!("All upstreams failed")
}

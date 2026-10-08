//! A direct TCP connection between a sending and a receiving `qrsend` on the
//! same network (docs/PROTOCOL.md §12.2).
//!
//! The sender listens and says where in a link code mixed into its stream,
//! together with a key. A receiver that reads the code connects; from then on
//! the transfer also travels through the connection, far faster than through
//! a camera. The codes on the screen keep going, and are all there is if the
//! connection never comes up or breaks.
//!
//! Everything on the connection is encrypted with keys derived from the key
//! in the code, so only someone who saw the sender's screen can take part,
//! and nobody else on the network can read or alter it.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use qrsend_core::direct::{self, DirectSender};
use qrsend_core::feedback::{self, Feedback};
use qrsend_core::link::TcpOffer;
use qrsend_core::sender::{SegmentSource, SessionLayout};

const MAGIC: &[u8; 7] = b"QSTCP1\n";
/// The receiver's first line: it is who it should be (it knows the key).
const HELLO: &str = "B1";
/// Symbol size of the frames sent through the connection.
pub const SYMBOL_SIZE: usize = 4096;
/// Plain bytes per record, about.
const MESSAGE_BYTES: usize = 256 << 10;
/// No record is larger than this (a damaged length must not allocate at will).
const MAX_RECORD: usize = 4 << 20;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the receiver reports, at most.
pub const REPLY_EVERY: Duration = Duration::from_millis(100);
/// Everything was sent, something is still missing, and no report settles
/// it: after this long, more is sent regardless.
const IDLE: Duration = Duration::from_secs(3);

/// One direction of the connection: records sealed with a key of its own,
/// numbered so that none can be replayed, dropped or reordered unnoticed.
struct Seal {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

impl Seal {
    fn new(key: [u8; 32]) -> Self {
        Seal {
            cipher: ChaCha20Poly1305::new(&key.into()),
            counter: 0,
        }
    }

    fn nonce(&mut self) -> Nonce {
        let mut nonce = [0u8; 12];
        nonce[..8].copy_from_slice(&self.counter.to_le_bytes());
        self.counter += 1;
        nonce.into()
    }

    fn write(&mut self, to: &mut impl Write, plain: &[u8]) -> Result<()> {
        let nonce = self.nonce();
        let sealed = self
            .cipher
            .encrypt(&nonce, plain)
            .map_err(|_| anyhow::anyhow!("cannot encrypt"))?;
        to.write_all(&(sealed.len() as u32).to_le_bytes())?;
        to.write_all(&sealed)?;
        Ok(())
    }

    /// `None` when the other side closed the connection.
    fn read(&mut self, from: &mut impl Read) -> Result<Option<Vec<u8>>> {
        let mut len = [0u8; 4];
        match from.read_exact(&mut len) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        let len = u32::from_le_bytes(len) as usize;
        if len > MAX_RECORD {
            bail!("the connection carries something else than a QRSend transfer");
        }
        let mut sealed = vec![0u8; len];
        from.read_exact(&mut sealed)?;
        let nonce = self.nonce();
        self.cipher
            .decrypt(&nonce, &sealed[..])
            .map(Some)
            .map_err(|_| anyhow::anyhow!("the other side does not have the key from the code"))
    }
}

/// Keys for the two directions, from the key in the code and what each side
/// contributed to this connection (so no two connections share keys).
fn seals(key: &[u8; 16], from_receiver: &[u8; 16], from_sender: &[u8; 16]) -> (Seal, Seal) {
    let mut material = key.to_vec();
    material.extend_from_slice(from_receiver);
    material.extend_from_slice(from_sender);
    let derive = |context| Seal::new(blake3::derive_key(context, &material));
    (
        derive("qrsend 0.1.3 tcp: sender to receiver"),
        derive("qrsend 0.1.3 tcp: receiver to sender"),
    )
}

// ------------------------------------------------------------------ sender

/// What the connection tells the part of the sender that shows the codes.
pub enum LinkEvent {
    Up,
    Down,
    Feedback(Feedback),
    /// The codes of an offer that replaces the one before it (a WebRTC
    /// offer makes one connection, see [`crate::rtc`]).
    Offer(Vec<String>),
}

/// Addresses of this machine other devices on the network may reach. Asking
/// the system which address it would use toward a few places finds them
/// without sending anything.
pub(crate) fn local_addresses() -> Vec<IpAddr> {
    let mut found = Vec::new();
    for target in [
        "192.168.255.254:9",
        "10.255.255.254:9",
        "172.31.255.254:9",
        "8.8.8.8:9",
    ] {
        let target: SocketAddr = target.parse().unwrap();
        let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
            continue;
        };
        if socket.connect(target).is_err() {
            continue;
        }
        if let Ok(local) = socket.local_addr() {
            let ip = local.ip();
            if !ip.is_unspecified() && !ip.is_loopback() && !found.contains(&ip) {
                found.push(ip);
            }
        }
    }
    found
}

/// Starts listening. `address`: what to announce instead of the addresses
/// found (for a machine with several networks); `port` 0 lets the system
/// choose.
pub fn listen(address: Option<&str>, port: u16) -> Result<(TcpListener, TcpOffer)> {
    // IPv4 unless an IPv6 address is asked for: one socket for both is not
    // something every system does.
    let v6 = address.is_some_and(|a| a.parse::<std::net::Ipv6Addr>().is_ok());
    let listener = TcpListener::bind((if v6 { "::" } else { "0.0.0.0" }, port))
        .context("cannot listen for a network connection")?;
    let addresses: Vec<String> = match address {
        Some(a) => vec![a.to_string()],
        None => local_addresses().iter().map(IpAddr::to_string).collect(),
    };
    if addresses.is_empty() {
        bail!("this machine has no network address to offer (give one with --lan-address)");
    }
    let offer = TcpOffer {
        port: listener.local_addr()?.port(),
        key: rand::random(),
        addresses: addresses.into_iter().take(4).collect(),
    };
    Ok((listener, offer))
}

struct Outgoing<S> {
    direct: DirectSender<S>,
    /// Records written so far.
    sent: u64,
    /// Records the receiver has taken in.
    taken: u64,
    done: bool,
    failed: bool,
}

/// Serves receivers that connect, one at a time, until one reports that it
/// has everything. `source` opens the transfer's data anew (each connection
/// reads on its own).
pub fn serve<S, F>(
    listener: TcpListener,
    offer: TcpOffer,
    layout: SessionLayout,
    source: F,
    events: Sender<LinkEvent>,
) where
    S: SegmentSource + Send + 'static,
    F: Fn() -> Result<S> + Send + 'static,
{
    thread::spawn(move || {
        let mut last: Option<Feedback> = None;
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let Ok(source) = source() else { return };
            let direct = DirectSender::new(layout, source, last.as_ref());
            // (An error: someone without the key, or something else entirely.)
            let feedback = serve_one(stream, &offer, direct, &events).unwrap_or(None);
            let _ = events.send(LinkEvent::Down);
            let complete = feedback.as_ref().is_some_and(|f| f.complete);
            last = feedback.or(last);
            if complete {
                return;
            }
        }
    });
}

/// Returns the last feedback of the connection once it ends.
fn serve_one<S: SegmentSource + Send + 'static>(
    mut stream: TcpStream,
    offer: &TcpOffer,
    direct: DirectSender<S>,
    events: &Sender<LinkEvent>,
) -> Result<Option<Feedback>> {
    // (A system firewall may hand over a connection it has already cut:
    // every call on it then fails, and the next one is waited for.)
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let mut hello = [0u8; MAGIC.len() + 4 + 16];
    stream.read_exact(&mut hello)?;
    let session = u32::from_le_bytes(hello[MAGIC.len()..MAGIC.len() + 4].try_into().unwrap());
    if &hello[..MAGIC.len()] != MAGIC || session != direct.layout().session_id {
        bail!("not for this transfer");
    }
    let from_receiver: [u8; 16] = hello[MAGIC.len() + 4..].try_into().unwrap();
    let from_sender: [u8; 16] = rand::random();
    stream.write_all(&from_sender)?;
    let (mut to_receiver, mut to_sender) = seals(&offer.key, &from_receiver, &from_sender);
    // Nothing is sent before the receiver has shown that it knows the key.
    if to_sender.read(&mut stream)?.as_deref() != Some(HELLO.as_bytes()) {
        bail!("no greeting");
    }
    stream.set_read_timeout(None)?;
    let _ = events.send(LinkEvent::Up);

    let state = Arc::new((
        Mutex::new(Outgoing {
            direct,
            sent: 0,
            taken: 0,
            done: false,
            failed: false,
        }),
        Condvar::new(),
    ));
    let last = Arc::new(Mutex::new(None));
    let reader = {
        let (state, last, events) = (state.clone(), last.clone(), events.clone());
        let mut stream = stream.try_clone()?;
        thread::spawn(move || {
            // Lines: "A<n>" (n records taken in) and feedback codes.
            while let Ok(Some(message)) = to_sender.read(&mut stream) {
                let text = String::from_utf8_lossy(&message);
                let mut out = state.0.lock().unwrap();
                for line in text.lines() {
                    if let Some(n) = line.strip_prefix('A') {
                        out.taken = n.parse().unwrap_or(out.taken);
                    } else if line.starts_with(feedback::PREFIX)
                        && let Ok(f) = Feedback::decode(line)
                    {
                        let settled = out.taken >= out.sent;
                        out.direct.apply_feedback(&f, settled);
                        out.done |= f.complete;
                        *last.lock().unwrap() = Some(f.clone());
                        let _ = events.send(LinkEvent::Feedback(f));
                    }
                }
                drop(out);
                state.1.notify_all();
            }
            state.0.lock().unwrap().failed = true;
            state.1.notify_all();
        })
    };

    let mut idle_since: Option<Instant> = None;
    loop {
        let mut out = state.0.lock().unwrap();
        if out.done || out.failed {
            break;
        }
        let mut message = Vec::with_capacity(MESSAGE_BYTES + SYMBOL_SIZE + 64);
        let mut records = 0;
        while message.len() < MESSAGE_BYTES {
            let Ok(Some(frame)) = out.direct.next_frame() else {
                break;
            };
            direct::pack(&mut message, &frame.encode());
            records += 1;
        }
        if records == 0 {
            // Everything was sent; the receiver's report decides what follows.
            let since = *idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() > IDLE {
                out.direct.send_more();
                idle_since = None;
                continue;
            }
            drop(
                state
                    .1
                    .wait_timeout(out, Duration::from_millis(100))
                    .unwrap(),
            );
            continue;
        }
        idle_since = None;
        out.sent += records;
        drop(out);
        if to_receiver.write(&mut stream, &message).is_err() {
            break;
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let _ = reader.join();
    Ok(last.lock().unwrap().take())
}

// ---------------------------------------------------------------- receiver

/// The receiver's end of a connection.
pub struct Link {
    /// Messages of packed records (see `qrsend_core::direct`). Ends when the
    /// connection does, or when none could be made.
    pub messages: Receiver<Vec<u8>>,
    /// Lines for the sender: "A<n>" and feedback codes.
    pub replies: Sender<String>,
    /// Closes once everything in `replies` went out.
    pub(crate) written: Receiver<()>,
}

impl Link {
    /// Waits (briefly) until the last replies have been sent.
    pub fn finish(self) {
        drop(self.replies);
        let _ = self.written.recv_timeout(Duration::from_secs(2));
    }
}

/// Connects to a sender that offered a connection, in the background.
pub fn connect(offer: TcpOffer, session_id: u32) -> Link {
    let (message_tx, messages) = bounded::<Vec<u8>>(8);
    let (replies, reply_rx) = unbounded::<String>();
    let (written_tx, written) = bounded::<()>(0);
    thread::spawn(move || {
        let Some(mut stream) = dial(&offer) else {
            return;
        };
        let run = || -> Result<()> {
            stream.set_nodelay(true)?;
            let from_receiver: [u8; 16] = rand::random();
            let mut hello = MAGIC.to_vec();
            hello.extend_from_slice(&session_id.to_le_bytes());
            hello.extend_from_slice(&from_receiver);
            stream.write_all(&hello)?;
            stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
            let mut from_sender = [0u8; 16];
            stream.read_exact(&mut from_sender)?;
            stream.set_read_timeout(None)?;
            let (mut to_receiver, mut to_sender) = seals(&offer.key, &from_receiver, &from_sender);
            to_sender.write(&mut stream, HELLO.as_bytes())?;
            let mut writer = stream.try_clone()?;
            thread::spawn(move || {
                for line in reply_rx {
                    if to_sender.write(&mut writer, line.as_bytes()).is_err() {
                        break;
                    }
                }
                // Said what there was to say (the last report included).
                let _ = writer.shutdown(std::net::Shutdown::Write);
                drop(written_tx);
            });
            while let Some(message) = to_receiver.read(&mut stream)? {
                if message_tx.send(message).is_err() {
                    break;
                }
            }
            Ok(())
        };
        let _ = run();
    });
    Link {
        messages,
        replies,
        written,
    }
}

fn dial(offer: &TcpOffer) -> Option<TcpStream> {
    offer.addresses.iter().find_map(|a| {
        let ip: IpAddr = a.parse().ok()?;
        TcpStream::connect_timeout(&SocketAddr::new(ip, offer.port), CONNECT_TIMEOUT).ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_records_round_trip_and_reject_tampering() {
        let key = [3u8; 16];
        let (mut a_out, _) = seals(&key, &[1; 16], &[2; 16]);
        let (mut b_in, _) = seals(&key, &[1; 16], &[2; 16]);
        let mut wire = Vec::new();
        a_out.write(&mut wire, b"first").unwrap();
        a_out.write(&mut wire, b"second").unwrap();
        let mut reader = &wire[..];
        assert_eq!(b_in.read(&mut reader).unwrap().unwrap(), b"first");
        assert_eq!(b_in.read(&mut reader).unwrap().unwrap(), b"second");
        assert!(b_in.read(&mut reader).unwrap().is_none());

        // Another key, another connection (salt), a flipped bit, a record
        // out of order: all refused.
        let (mut other_key, _) = seals(&[4; 16], &[1; 16], &[2; 16]);
        assert!(other_key.read(&mut &wire[..]).is_err());
        let (mut other_salt, _) = seals(&key, &[9; 16], &[2; 16]);
        assert!(other_salt.read(&mut &wire[..]).is_err());
        let mut damaged = wire.clone();
        damaged[10] ^= 1;
        let (mut c, _) = seals(&key, &[1; 16], &[2; 16]);
        assert!(c.read(&mut &damaged[..]).is_err());
        let (mut d, _) = seals(&key, &[1; 16], &[2; 16]);
        let second = &wire[4 + 5 + 16..];
        assert!(d.read(&mut &second[..]).is_err());
        // The two directions do not share a key.
        let (_, mut back) = seals(&key, &[1; 16], &[2; 16]);
        assert!(back.read(&mut &wire[..]).is_err());
    }
}

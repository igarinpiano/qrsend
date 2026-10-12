//! A direct connection between `qrsend` and a browser on the same network: a
//! WebRTC data channel, the only kind of connection a web page can make to
//! another device (docs/PROTOCOL.md §12.4).
//!
//! As with the TCP connection between two `qrsend` ([`crate::net`]), the
//! sender says how to reach it in a link code mixed into its stream, and the
//! receiver connects without anything having to travel back first: what the
//! receiver would have to answer with, both sides work out from the offer.
//! The channel then carries what the browser's own connections carry: packed
//! binary records one way, lines of acknowledgment and feedback the other.

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use qrsend_core::direct::{self, DirectSender};
use qrsend_core::feedback::{self, Feedback, SenderNotice};
use qrsend_core::sender::{SegmentSource, SessionLayout};
use qrsend_core::{link, linkcert};
use sha2::{Digest, Sha256};
use str0m::channel::{ChannelConfig, ChannelId};
use str0m::config::{DtlsCert, Fingerprint};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, IceCreds, Input, Output, Rtc, RtcConfig};

use crate::log;
use crate::net::{
    Link, LinkEvent, SYMBOL_SIZE, address_kind, local_addresses, path_said, within_one_network,
};

/// The receiver's line that shows it read the offer (see [`proof`]).
const PROOF: char = 'K';
/// The receiver's line that says it takes messages of packed binary records.
const HELLO: &str = "B1";
/// Bytes per message, about: the library sends no message larger than
/// 64 KiB unless told the other side takes more.
const MESSAGE_BYTES: usize = 56 << 10;
/// Bytes handed to the connection but not yet sent.
const BUFFER_HIGH: usize = 1 << 20;
/// Every so many records one is a notice: a browser reports back only to a
/// sender that asked for it (as the web sender does on its connections).
const NOTICE_EVERY: u64 = 256;
/// Records on their way (sent, not yet reported as taken in) at the start
/// and at most. In between, half a second's worth of what the receiver has
/// been taking in.
const WINDOW_START: u64 = 256;
const WINDOW_MAX: u64 = 16384;
/// Everything was sent, something is still missing, and no report settles
/// it: after this long, more is sent regardless.
const IDLE: Duration = Duration::from_secs(3);
/// A receiver that has said nothing for this long is gone (it reports at
/// least once a second, whatever it is doing).
const SILENT: Duration = Duration::from_secs(10);

/// While nobody has taken an offer up, it is announced anew this often: the
/// same offer under another number. A browser tries an offer once per
/// number, and what stood in its way the first time (a firewall the user was
/// just being asked about, a network not yet joined) may be gone by now.
const ANNOUNCE_AGAIN: Duration = Duration::from_secs(20);

/// Says what the connection is doing when `QRSEND_TRACE` is set (for
/// finding out why one does not come about).
fn trace(what: impl FnOnce() -> String) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("QRSEND_TRACE").is_some()) {
        eprintln!("[link] {}", what());
    }
}

/// What a peer must know to connect. The layout is the browser's
/// (web/src/lib/lan.ts): ufrag and pwd (each with its length before it), the
/// SHA-256 fingerprint of the DTLS certificate, the number of addresses, and
/// each address (with its length) and port.
#[derive(Debug, Clone, PartialEq)]
pub struct Description {
    pub ufrag: String,
    pub pwd: String,
    pub fingerprint: [u8; 32],
    pub candidates: Vec<(String, u16)>,
    /// What a browser's offer says besides (see [`FLAG_ONE_NETWORK`]); none
    /// are written when there are none.
    pub flags: u8,
}

/// In a browser's offer: connect only if both devices are on one network
/// (its sender chose "Wi-Fi", not "Wi-Fi & Cellular Data"). Nothing is then
/// sent toward an address of the internet at large.
pub const FLAG_ONE_NETWORK: u8 = 2;

impl Description {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let text = |out: &mut Vec<u8>, s: &str| {
            out.push(s.len() as u8);
            out.extend_from_slice(s.as_bytes());
        };
        text(&mut out, &self.ufrag);
        text(&mut out, &self.pwd);
        out.extend_from_slice(&self.fingerprint);
        out.push(self.candidates.len() as u8);
        for (address, port) in &self.candidates {
            text(&mut out, address);
            out.extend_from_slice(&port.to_be_bytes());
        }
        if self.flags != 0 {
            out.push(self.flags);
        }
        out
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        /// Takes the next `n` bytes off the front.
        fn take<'d>(data: &mut &'d [u8], n: usize) -> Result<&'d [u8]> {
            if data.len() < n {
                bail!("damaged connection offer");
            }
            let (part, rest) = data.split_at(n);
            *data = rest;
            Ok(part)
        }
        fn text(data: &mut &[u8]) -> Result<String> {
            let n = take(data, 1)?[0] as usize;
            Ok(std::str::from_utf8(take(data, n)?)
                .context("damaged connection offer")?
                .to_string())
        }
        let mut data = data;
        let ufrag = text(&mut data)?;
        let pwd = text(&mut data)?;
        let fingerprint = take(&mut data, 32)?.try_into().unwrap();
        let mut candidates = Vec::new();
        for _ in 0..take(&mut data, 1)?[0] {
            let address = text(&mut data)?;
            let port = u16::from_be_bytes(take(&mut data, 2)?.try_into().unwrap());
            candidates.push((address, port));
        }
        Ok(Description {
            ufrag,
            pwd,
            fingerprint,
            candidates,
            flags: data.first().copied().unwrap_or(0),
        })
    }
}

fn sha256(context: &str, parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(context.as_bytes());
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The credentials the receiver answers an offer with. Both sides work them
/// out from the offer, so the answer need not travel.
pub fn answer_credentials(offer: &[u8]) -> IceCreds {
    let h = hex(&sha256("qrsend known answer\n", &[offer]));
    IceCreds {
        ufrag: h[..8].to_string(),
        pass: h[8..40].to_string(),
    }
}

/// What the receiver sends first: it knows the key in the offer, and this is
/// the certificate it connected with. The sender cannot know the receiver's
/// certificate beforehand as a browser would from an answer; this ties the
/// connection to someone who read the code all the same.
pub fn proof(key: &[u8; 16], certificate: &[u8]) -> String {
    format!(
        "{PROOF}{}",
        hex(&sha256("qrsend open link\n", &[key, certificate]))
    )
}

/// One UDP socket per local address, read by threads of their own.
struct Sockets {
    list: Vec<(SocketAddr, Arc<UdpSocket>)>,
    /// (to, from, datagram)
    incoming: Receiver<Datagram>,
}

impl Sockets {
    fn open(addresses: &[IpAddr]) -> Result<Sockets> {
        let (tx, incoming) = unbounded();
        let mut list = Vec::new();
        for ip in addresses {
            let Ok(socket) = UdpSocket::bind((*ip, 0)) else {
                continue;
            };
            let local = socket.local_addr()?;
            socket.set_read_timeout(Some(Duration::from_millis(500)))?;
            let socket = Arc::new(socket);
            list.push((local, socket.clone()));
            let tx = tx.clone();
            thread::spawn(move || {
                let mut buf = vec![0u8; 2048];
                loop {
                    match socket.recv_from(&mut buf) {
                        Ok((n, from)) => {
                            if tx.send((local, from, buf[..n].to_vec())).is_err() {
                                return;
                            }
                        }
                        // Nothing came: go on as long as someone listens.
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            if tx.is_empty() && Arc::strong_count(&socket) == 1 {
                                return;
                            }
                        }
                        // (Windows reports an unreachable peer on the socket.)
                        Err(_) => {}
                    }
                }
            });
        }
        if list.is_empty() {
            bail!("no network address to offer a connection at");
        }
        Ok(Sockets { list, incoming })
    }

    fn send(&self, from: SocketAddr, to: SocketAddr, data: &[u8]) {
        if let Some((_, socket)) = self.list.iter().find(|(a, _)| *a == from) {
            let _ = socket.send_to(data, to);
        }
    }
}

fn addresses(given: Option<&str>) -> Result<Vec<IpAddr>> {
    Ok(match given {
        Some(a) => vec![a.parse().context("--lan-address: not an address")?],
        None => local_addresses(),
    })
}

/// A connection offered to whoever reads the offer. Nothing of the
/// connection itself exists yet: its handshake gives up half a minute after
/// it is set up, so one set up with the offer would end, and the offer with
/// it, whether or not anyone had come (a receiver that read the offer late in
/// that time found a connection about to give up). It is set up when someone
/// knocks ([`Offered::begin`]).
struct Offered {
    credentials: IceCreds,
    certificate: DtlsCert,
    key: [u8; 16],
    /// What goes into the link codes: the key, then the description.
    payload: Vec<u8>,
}

fn offer(sockets: &Sockets) -> Result<Offered> {
    let certificate = str0m::crypto::from_feature_flags()
        .dtls_provider
        .generate_certificate()
        .context("no certificate for the connection")?;
    let credentials = IceCreds::new();
    let description = Description {
        ufrag: credentials.ufrag.clone(),
        pwd: credentials.pass.clone(),
        fingerprint: Sha256::digest(&certificate.certificate).into(),
        candidates: sockets
            .list
            .iter()
            .take(4)
            .map(|(a, _)| (a.ip().to_string(), a.port()))
            .collect(),
        flags: 0,
    };
    let key: [u8; 16] = rand::random();
    let mut payload = key.to_vec();
    payload.extend_from_slice(&description.to_bytes());
    Ok(Offered {
        credentials,
        certificate,
        key,
        payload,
    })
}

/// A datagram as it came: (to, from, contents).
type Datagram = (SocketAddr, SocketAddr, Vec<u8>);

/// What the library takes a datagram as; none for what it cannot be.
fn input(datagram: &Datagram) -> Option<Input<'_>> {
    let (to, from, data) = datagram;
    Some(Input::Receive(
        Instant::now(),
        Receive {
            proto: Protocol::Udp,
            source: *from,
            destination: *to,
            contents: data.as_slice().try_into().ok()?,
        },
    ))
}

impl Offered {
    /// This side of the connection, as the offer describes it.
    fn begin(&self, sockets: &Sockets) -> Result<(Rtc, ChannelId)> {
        let mut rtc = RtcConfig::new()
            .set_fingerprint_verification(false)
            .set_local_ice_credentials(self.credentials.clone())
            .set_dtls_cert(self.certificate.clone())
            .build(Instant::now());
        for (address, _) in &sockets.list {
            rtc.add_local_candidate(Candidate::host(*address, "udp")?);
        }
        let mut api = rtc.direct_api();
        api.set_ice_controlling(true);
        api.set_remote_ice_credentials(answer_credentials(&self.payload));
        // Whoever connects is checked by what it sends first (see `proof`); the
        // library wants to be told some fingerprint regardless.
        api.set_remote_fingerprint(Fingerprint {
            hash_func: "sha-256".into(),
            bytes: vec![0; 32],
        });
        // The receiver, answering, takes the active part.
        api.start_dtls(false)?;
        api.start_sctp(false);
        let channel = api.create_data_channel(ChannelConfig {
            label: "qrsend".into(),
            ..Default::default()
        });
        Ok((rtc, channel))
    }

    /// Waits for a receiver to knock: for the first datagram that belongs to
    /// a connection made from this offer. Returns that connection and the
    /// datagram; none when nothing can come any more. `still_waiting` is
    /// called every [`ANNOUNCE_AGAIN`] in the meantime.
    fn knocked(
        &self,
        sockets: &Sockets,
        mut still_waiting: impl FnMut(),
    ) -> Option<(Rtc, ChannelId, Datagram)> {
        let mut said = Instant::now();
        loop {
            if said.elapsed() >= ANNOUNCE_AGAIN {
                still_waiting();
                said = Instant::now();
            }
            let datagram = match sockets.incoming.recv_timeout(Duration::from_secs(1)) {
                Ok(datagram) => datagram,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return None,
            };
            // A check of the path (STUN), which is what a receiver sends
            // first: anything else is not worth setting a connection up for.
            // And it names this offer (the user name in such a check begins
            // with the fragment the offer gave): nothing is set up for
            // someone who has not read the offer, however much they send.
            let data = &datagram.2;
            let name = format!("{}:", self.credentials.ufrag);
            if data.len() < 20
                || data[0] > 1
                || data[4..8] != [0x21, 0x12, 0xa4, 0x42]
                || !data.windows(name.len()).any(|w| w == name.as_bytes())
            {
                continue;
            }
            let Ok((rtc, channel)) = self.begin(sockets) else {
                return None;
            };
            if input(&datagram).is_some_and(|i| rtc.accepts(&i)) {
                return Some((rtc, channel, datagram));
            }
        }
    }
}

/// Offers a connection to a receiving browser and serves whoever takes it,
/// one receiver at a time, until one reports that it has everything. The
/// codes of the offer are reported through `events` (anew for every
/// connection: one offer makes one connection). `room`: characters a code
/// may have.
pub fn serve<S, F>(
    session_id: u32,
    address: Option<&str>,
    room: usize,
    layout: SessionLayout,
    source: F,
    events: Sender<LinkEvent>,
) -> Result<()>
where
    S: SegmentSource + Send + 'static,
    F: Fn() -> Result<S> + Send + 'static,
{
    let sockets = Sockets::open(&addresses(address)?)?;
    let layout = SessionLayout {
        symbol_size: SYMBOL_SIZE,
        ..layout
    };
    let announce = move |offered: &Offered, events: &Sender<LinkEvent>| -> Result<()> {
        let codes = link::split(
            session_id,
            link::KIND_OPEN_OFFER,
            rand::random(),
            &offered.payload,
            room,
        )?;
        let _ = events.send(LinkEvent::Offer(codes));
        Ok(())
    };
    // The first offer is out before this returns: a stream written to a
    // file has it from its first picture.
    let mut offered = offer(&sockets)?;
    announce(&offered, &events)?;
    log::line("link", || {
        let kinds: Vec<&str> = sockets
            .list
            .iter()
            .map(|(a, _)| address_kind(a.ip()))
            .collect();
        format!(
            "WebRTC: offering a connection to browsers addresses={}",
            kinds.join(",")
        )
    });
    thread::spawn(move || {
        let mut last: Option<Feedback> = None;
        loop {
            // (Until someone comes, there is nothing to serve and nothing
            // to give up on: the offer holds as long as it is shown.)
            let again = || {
                let _ = announce(&offered, &events);
            };
            let Some((rtc, channel, first)) = offered.knocked(&sockets, again) else {
                return;
            };
            trace(|| "a receiver knocked".into());
            log::line("link", || {
                "WebRTC: a receiver's first packet arrived".into()
            });
            let Ok(source) = source() else { return };
            let direct = DirectSender::new(layout, source, last.as_ref());
            let feedback = Connection::new(rtc, channel, offered.key, direct, &sockets, &events)
                .run(first)
                .unwrap_or_else(|e| {
                    trace(|| format!("the connection failed: {e:#}"));
                    log::line("link", || {
                        format!(
                            "WebRTC: the connection failed: {}",
                            log::discreet(&format!("{e:#}"))
                        )
                    });
                    None
                });
            log::line("link", || {
                format!(
                    "WebRTC: connection over complete={}",
                    feedback.as_ref().is_some_and(|f| f.complete)
                )
            });
            let _ = events.send(LinkEvent::Down);
            let complete = feedback.as_ref().is_some_and(|f| f.complete);
            last = feedback.or(last);
            if complete {
                return;
            }
            offered = match offer(&sockets) {
                Ok(o) => o,
                Err(_) => return,
            };
            if announce(&offered, &events).is_err() {
                return;
            }
        }
    });
    Ok(())
}

struct Connection<'a, S> {
    rtc: Rtc,
    channel: ChannelId,
    key: [u8; 16],
    direct: DirectSender<S>,
    sockets: &'a Sockets,
    events: &'a Sender<LinkEvent>,
    open: bool,
    /// The receiver showed that it read the offer.
    proven: bool,
    /// … and said how it takes the data.
    greeted: bool,
    /// Records written so far, and records the receiver has taken in.
    sent: u64,
    taken: u64,
    window: u64,
    /// When `taken` was last reported, and what it was.
    reported: (Instant, u64),
    heard: Instant,
    idle_since: Option<Instant>,
    /// A message the connection had no room for yet.
    waiting: VecDeque<Vec<u8>>,
    last: Option<Feedback>,
    /// The two ends of the last packet that came (this side's, the other's).
    ends: Option<(SocketAddr, SocketAddr)>,
}

impl<'a, S: SegmentSource> Connection<'a, S> {
    fn new(
        rtc: Rtc,
        channel: ChannelId,
        key: [u8; 16],
        direct: DirectSender<S>,
        sockets: &'a Sockets,
        events: &'a Sender<LinkEvent>,
    ) -> Self {
        let now = Instant::now();
        Connection {
            rtc,
            channel,
            key,
            direct,
            sockets,
            events,
            open: false,
            proven: false,
            greeted: false,
            sent: 0,
            taken: 0,
            window: WINDOW_START,
            reported: (now, 0),
            heard: now,
            idle_since: None,
            waiting: VecDeque::new(),
            last: None,
            ends: None,
        }
    }

    /// Takes in a datagram that came for this connection.
    fn take(&mut self, datagram: Datagram) -> Result<()> {
        self.ends = Some((datagram.0, datagram.1));
        if !self.open {
            trace(|| {
                format!(
                    "{} B from {} at {}",
                    datagram.2.len(),
                    datagram.1,
                    datagram.0
                )
            });
        }
        // (Whatever is not part of this connection is not for it.)
        if let Some(input) = input(&datagram)
            && self.rtc.accepts(&input)
        {
            self.rtc.handle_input(input)?;
        }
        Ok(())
    }

    /// Runs until the connection ends; returns the receiver's last feedback.
    /// `first`: the datagram the connection was set up for.
    fn run(mut self, first: Datagram) -> Result<Option<Feedback>> {
        self.take(first)?;
        loop {
            if self.last.as_ref().is_some_and(|f| f.complete) {
                break;
            }
            if self.greeted {
                if self.heard.elapsed() > SILENT {
                    break;
                }
                self.pump()?;
            }
            let timeout = loop {
                match self.rtc.poll_output()? {
                    Output::Timeout(t) => break t,
                    Output::Transmit(t) => self.sockets.send(t.source, t.destination, &t.contents),
                    Output::Event(e) => {
                        trace(|| match &e {
                            Event::ChannelData(d) => format!("data: {} B", d.data.len()),
                            other => format!("{other:?}"),
                        });
                        if !self.event(e)? {
                            return Ok(self.last);
                        }
                    }
                }
            };
            if !self.rtc.is_alive() {
                break;
            }
            let wait = timeout
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(50));
            match self.sockets.incoming.recv_timeout(wait) {
                Ok(datagram) => self.take(datagram)?,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    self.rtc.handle_input(Input::Timeout(Instant::now()))?;
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
        self.rtc.disconnect();
        Ok(self.last)
    }

    /// False: the connection is over.
    fn event(&mut self, event: Event) -> Result<bool> {
        match event {
            Event::IceConnectionStateChange(IceConnectionState::Disconnected) => return Ok(false),
            Event::ChannelOpen(id, _) if id == self.channel => {
                self.open = true;
                self.heard = Instant::now();
                if let Some((own, other)) = self.ends {
                    log::line("link", || {
                        format!("WebRTC: a browser connected {}", path_said(own, other))
                    });
                }
            }
            Event::ChannelClose(id) if id == self.channel => return Ok(false),
            Event::ChannelData(data) if data.id == self.channel && !data.binary => {
                self.heard = Instant::now();
                let text = String::from_utf8_lossy(&data.data).into_owned();
                for line in text.lines() {
                    if !self.line(line) {
                        return Ok(false);
                    }
                }
            }
            _ => {}
        }
        Ok(true)
    }

    /// One line from the receiver. False: it is not who it should be.
    fn line(&mut self, line: &str) -> bool {
        if !self.proven {
            let seen = self.rtc.direct_api().remote_dtls_fingerprint().cloned();
            self.proven = seen.is_some_and(|f| line == proof(&self.key, &f.bytes));
            trace(|| format!("the receiver read the offer: {}", self.proven));
            log::line("link", || {
                format!(
                    "WebRTC: the receiver showed that it read the offer: {}",
                    self.proven
                )
            });
            return self.proven;
        }
        if line == HELLO {
            if !self.greeted {
                self.greeted = true;
                let _ = self.events.send(LinkEvent::Up);
            }
        } else if let Some(n) = line.strip_prefix('A') {
            let taken = n.parse().unwrap_or(self.taken).min(self.sent);
            // Half a second's worth of what the receiver takes in may be on
            // its way: more would only pile up in front of it.
            let (at, was) = self.reported;
            let passed = at.elapsed().as_secs_f64();
            if taken > was && passed > 0.05 {
                let rate = (taken - was) as f64 / passed;
                self.window = ((rate * 0.5) as u64).clamp(WINDOW_START, WINDOW_MAX);
                self.reported = (Instant::now(), taken);
            }
            self.taken = taken;
        } else if line.starts_with(feedback::PREFIX)
            && let Ok(f) = Feedback::decode(line)
        {
            let settled = self.taken >= self.sent;
            self.direct.apply_feedback(&f, settled);
            self.last = Some(f.clone());
            let _ = self.events.send(LinkEvent::Feedback(f));
        }
        true
    }

    /// Hands the connection as much as it and the receiver have room for.
    fn pump(&mut self) -> Result<()> {
        loop {
            let Some(mut channel) = self.rtc.channel(self.channel) else {
                return Ok(());
            };
            if channel.buffered_amount() > BUFFER_HIGH {
                return Ok(());
            }
            if let Some(message) = self.waiting.front() {
                if !channel.write(true, message)? {
                    return Ok(());
                }
                self.waiting.pop_front();
                continue;
            }
            if self.sent - self.taken >= self.window {
                return Ok(());
            }
            let mut message = Vec::with_capacity(MESSAGE_BYTES + SYMBOL_SIZE + 64);
            let (mut records, mut frames) = (0u64, 0);
            while message.len() < MESSAGE_BYTES {
                if (self.sent + records).is_multiple_of(NOTICE_EVERY) {
                    let notice = SenderNotice {
                        session_id: self.direct.layout().session_id,
                        wants_feedback: true,
                        hears_sound: false,
                        colors: 0,
                    };
                    direct::pack(&mut message, notice.encode().as_bytes());
                    records += 1;
                    continue;
                }
                let Some(frame) = self.direct.next_frame()? else {
                    break;
                };
                direct::pack(&mut message, &frame.encode());
                records += 1;
                frames += 1;
            }
            if frames == 0 {
                // Everything was sent; the receiver's report decides what follows.
                let since = *self.idle_since.get_or_insert_with(Instant::now);
                if since.elapsed() > IDLE {
                    self.direct.send_more();
                    self.idle_since = None;
                    continue;
                }
                return Ok(());
            }
            self.idle_since = None;
            self.sent += records;
            self.waiting.push_back(message);
        }
    }
}

// ---------------------------------------------------------------- receiver

/// How long a connection may take to come about.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The addresses a name on the local network stands for (multicast DNS).
/// Browsers put such names in place of their addresses unless the page may
/// use the camera or microphone.
fn resolve_local(name: &str, wait: Duration) -> Vec<IpAddr> {
    let mut found = Vec::new();
    let mut sockets = Vec::new();
    for ip in local_addresses() {
        let Ok(socket) = UdpSocket::bind((ip, 0)) else {
            continue;
        };
        let _ = socket.set_read_timeout(Some(Duration::from_millis(100)));
        // One question: the name's address, class IN, answer wanted to this
        // socket ("unicast response").
        let mut query = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        for label in name.trim_end_matches('.').split('.') {
            if label.is_empty() || label.len() > 63 {
                return found;
            }
            query.push(label.len() as u8);
            query.extend_from_slice(label.as_bytes());
        }
        let kind: u16 = if ip.is_ipv4() { 1 } else { 28 };
        query.push(0);
        query.extend_from_slice(&kind.to_be_bytes());
        query.extend_from_slice(&0x8001u16.to_be_bytes());
        let to: SocketAddr = if ip.is_ipv4() {
            "224.0.0.251:5353".parse().unwrap()
        } else {
            "[ff02::fb]:5353".parse().unwrap()
        };
        if socket.send_to(&query, to).is_ok() {
            sockets.push(socket);
        }
    }
    let until = Instant::now() + wait;
    let mut buf = [0u8; 1500];
    while found.is_empty() && Instant::now() < until && !sockets.is_empty() {
        for socket in &sockets {
            if let Ok((n, _)) = socket.recv_from(&mut buf) {
                found.extend(answers(&buf[..n]));
            }
        }
    }
    found
}

/// The addresses in the answers of a DNS message.
fn answers(message: &[u8]) -> Vec<IpAddr> {
    /// Skips a name (labels, or a pointer to one elsewhere).
    fn name(message: &[u8], mut at: usize) -> Option<usize> {
        loop {
            let n = *message.get(at)? as usize;
            if n == 0 {
                return Some(at + 1);
            }
            if n & 0xc0 == 0xc0 {
                return Some(at + 2);
            }
            at += 1 + n;
        }
    }
    let mut found = Vec::new();
    let read = |at: usize| -> Option<u16> {
        Some(u16::from_be_bytes(
            message.get(at..at + 2)?.try_into().ok()?,
        ))
    };
    let mut parse = || -> Option<()> {
        let (questions, records) = (read(4)?, read(6)?);
        let mut at = 12;
        for _ in 0..questions {
            at = name(message, at)? + 4;
        }
        for _ in 0..records {
            at = name(message, at)?;
            let (kind, len) = (read(at)?, read(at + 8)? as usize);
            let data = message.get(at + 10..at + 10 + len)?;
            match (kind, len) {
                (1, 4) => found.push(IpAddr::from(<[u8; 4]>::try_from(data).ok()?)),
                (28, 16) => found.push(IpAddr::from(<[u8; 16]>::try_from(data).ok()?)),
                _ => {}
            }
            at += 10 + len;
        }
        Some(())
    };
    let _ = parse();
    found
}

/// Connects to a sending browser that offered a connection to a receiver
/// which shows no answer (`payload` of a link message of kind 6), in the
/// background. The certificate follows from the seed in the offer, so the
/// browser already knows whom to expect; this side knocks at its addresses.
pub fn connect(payload: Vec<u8>) -> Link {
    let (message_tx, messages) = bounded::<Vec<u8>>(64);
    let (replies, reply_rx) = unbounded::<String>();
    let (written_tx, written) = bounded::<()>(0);
    thread::spawn(move || {
        let result = receive(&payload, message_tx, reply_rx);
        match &result {
            Err(e) => {
                trace(|| format!("the connection failed: {e:#}"));
                log::line("link", || {
                    format!(
                        "WebRTC: no connection: {}",
                        log::discreet(&format!("{e:#}"))
                    )
                });
            }
            Ok(()) => log::line("link", || "WebRTC: connection over".into()),
        }
        drop(written_tx);
    });
    Link {
        messages,
        replies,
        written,
    }
}

fn receive(payload: &[u8], messages: Sender<Vec<u8>>, replies: Receiver<String>) -> Result<()> {
    let (seed, description) = payload
        .split_at_checked(16)
        .context("damaged connection offer")?;
    let description = Description::from_bytes(description)?;
    let one_network = description.flags & FLAG_ONE_NETWORK != 0;
    let mut remote = Vec::new();
    let mut kinds = Vec::new();
    for (address, port) in &description.candidates {
        match address.parse::<IpAddr>() {
            Ok(ip) => {
                kinds.push(address_kind(ip));
                // (This program does not know its own IPv6 addresses, so it
                // cannot tell whether one of the internet at large is on
                // its network: where that matters, such an address is left.)
                if !one_network || within_one_network(ip) {
                    remote.push(SocketAddr::new(ip, *port));
                }
            }
            Err(_) if address.ends_with(".local") => {
                // (Only a device on the same network can look a name up.)
                let found = resolve_local(address, Duration::from_secs(2));
                trace(|| {
                    format!(
                        "a name on the local network stands for {} address(es)",
                        found.len()
                    )
                });
                kinds.push(if found.is_empty() {
                    "name (not found)"
                } else {
                    "name"
                });
                remote.extend(found.into_iter().map(|ip| SocketAddr::new(ip, *port)));
            }
            Err(_) => {}
        }
    }
    log::line("link", || {
        format!(
            "WebRTC: a browser's offer addresses={} oneNetworkOnly={one_network} usable={}",
            kinds.join(","),
            remote.len()
        )
    });
    if remote.is_empty() {
        bail!(if one_network {
            "the sender connects only on its own network, and none of its addresses is on one"
        } else {
            "the offer holds no address that can be reached"
        });
    }
    let mut local = local_addresses();
    if remote.iter().any(|a| a.ip().is_loopback()) {
        local.push(IpAddr::from([127, 0, 0, 1]));
    }
    let sockets = Sockets::open(&local)?;
    let certificate = linkcert::certificate(seed);
    let mut rtc = RtcConfig::new()
        .set_dtls_cert(DtlsCert {
            certificate: certificate.certificate,
            private_key: certificate.private_key,
        })
        .set_local_ice_credentials(answer_credentials(payload))
        .build(Instant::now());
    for (address, _) in &sockets.list {
        rtc.add_local_candidate(Candidate::host(*address, "udp")?);
    }
    for address in remote {
        rtc.add_remote_candidate(Candidate::host(address, "udp")?);
    }
    let mut api = rtc.direct_api();
    api.set_ice_controlling(false);
    api.set_remote_ice_credentials(IceCreds {
        ufrag: description.ufrag,
        pass: description.pwd,
    });
    api.set_remote_fingerprint(Fingerprint {
        hash_func: "sha-256".into(),
        bytes: description.fingerprint.to_vec(),
    });
    // The answer the browser gave itself says this side takes the active part.
    api.start_dtls(true)?;
    api.start_sctp(true);

    let started = Instant::now();
    // The two ends of the last packet that came (this side's, the other's).
    let mut ends: Option<(SocketAddr, SocketAddr)> = None;
    let mut channel: Option<ChannelId> = None;
    let mut replies = Some(replies);
    let mut waiting: VecDeque<String> = VecDeque::new();
    // Once the caller has said all it has to say: when that was.
    let mut closing: Option<Instant> = None;
    loop {
        if channel.is_none() && started.elapsed() > CONNECT_TIMEOUT {
            bail!("no connection came about");
        }
        // What there is to say to the sender.
        if let Some(rx) = &replies {
            loop {
                match rx.try_recv() {
                    Ok(line) => waiting.push_back(line),
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        replies = None;
                        closing = Some(Instant::now());
                        break;
                    }
                }
            }
        }
        if let Some(mut c) = channel.and_then(|id| rtc.channel(id)) {
            while let Some(line) = waiting.front() {
                if !c.write(false, line.as_bytes())? {
                    break;
                }
                waiting.pop_front();
            }
            if let Some(since) = closing
                && ((waiting.is_empty() && c.buffered_amount() == 0)
                    || since.elapsed() > Duration::from_secs(2))
            {
                break;
            }
        } else if closing.is_some() {
            break;
        }
        let timeout = loop {
            match rtc.poll_output()? {
                Output::Timeout(t) => break t,
                Output::Transmit(t) => sockets.send(t.source, t.destination, &t.contents),
                Output::Event(e) => {
                    trace(|| match &e {
                        Event::ChannelData(d) => format!("data: {} B", d.data.len()),
                        other => format!("{other:?}"),
                    });
                    match e {
                        Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                            return Ok(());
                        }
                        Event::ChannelOpen(id, _) => {
                            channel = Some(id);
                            waiting.push_front(HELLO.to_string());
                            if let Some((own, other)) = ends {
                                log::line("link", || {
                                    format!("WebRTC: connected {}", path_said(own, other))
                                });
                            }
                        }
                        Event::ChannelClose(_) => return Ok(()),
                        Event::ChannelData(data) if data.binary => {
                            if messages.send(data.data).is_err() {
                                return Ok(());
                            }
                        }
                        _ => {}
                    }
                }
            }
        };
        if !rtc.is_alive() {
            return Ok(());
        }
        let wait = timeout
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(20));
        match sockets.incoming.recv_timeout(wait) {
            Ok((to, from, data)) => {
                ends = Some((to, from));
                if let Ok(contents) = data.as_slice().try_into() {
                    let input = Input::Receive(
                        Instant::now(),
                        Receive {
                            proto: Protocol::Udp,
                            source: from,
                            destination: to,
                            contents,
                        },
                    );
                    if rtc.accepts(&input) {
                        rtc.handle_input(input)?;
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                rtc.handle_input(Input::Timeout(Instant::now()))?;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
    // The last report is on its way: a moment for it to arrive.
    let until = Instant::now() + Duration::from_millis(300);
    while Instant::now() < until {
        match rtc.poll_output()? {
            Output::Transmit(t) => sockets.send(t.source, t.destination, &t.contents),
            Output::Timeout(_) => {
                thread::sleep(Duration::from_millis(10));
                rtc.handle_input(Input::Timeout(Instant::now()))?;
            }
            Output::Event(_) => {}
        }
    }
    rtc.disconnect();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_roundtrip() {
        let d = Description {
            ufrag: "abcd".into(),
            pwd: "0123456789abcdef0123456789abcdef".into(),
            fingerprint: [7; 32],
            candidates: vec![("192.168.1.20".into(), 50000), ("fd00::1".into(), 9)],
            flags: 0,
        };
        let bytes = d.to_bytes();
        assert_eq!(Description::from_bytes(&bytes).unwrap(), d);
        assert!(Description::from_bytes(&bytes[..bytes.len() - 1]).is_err());
        // What a browser says besides comes last.
        let flagged = Description {
            flags: FLAG_ONE_NETWORK,
            ..d.clone()
        };
        assert_eq!(flagged.to_bytes().len(), bytes.len() + 1);
        assert_eq!(
            Description::from_bytes(&flagged.to_bytes()).unwrap(),
            flagged
        );
    }

    #[test]
    fn addresses_in_a_dns_answer() {
        // A response with the question echoed and one address record whose
        // name points back at the question's.
        let mut m = vec![0, 0, 0x84, 0, 0, 1, 0, 1, 0, 0, 0, 0];
        m.extend_from_slice(b"\x04host\x05local\x00\x00\x01\x00\x01");
        m.extend_from_slice(&[0xc0, 12, 0, 1, 0x80, 1, 0, 0, 0, 120, 0, 4, 192, 168, 1, 7]);
        assert_eq!(answers(&m), ["192.168.1.7".parse::<IpAddr>().unwrap()]);
        assert!(answers(&m[..m.len() - 2]).is_empty());
        assert!(answers(&[]).is_empty());
    }

    #[test]
    fn what_both_sides_work_out() {
        // The same values as the browser's (web/src/lib/lan.ts).
        let creds = answer_credentials(b"offer");
        assert_eq!(creds.ufrag.len(), 8);
        assert_eq!(creds.pass.len(), 32);
        assert_ne!(answer_credentials(b"other").ufrag, creds.ufrag);
        let p = proof(&[1; 16], &[2; 32]);
        assert!(p.starts_with('K') && p.len() == 65);
        assert_ne!(proof(&[1; 16], &[3; 32]), p);
    }

    /// What arrives from others (an offer off the screen, any packet on the
    /// mDNS port) may be anything: no input makes these panic.
    #[test]
    fn damaged_offers_and_answers_are_only_refused() {
        let valid = Description {
            ufrag: "abcd".into(),
            pwd: "0123456789abcdef0123456789abcdef".into(),
            fingerprint: [7; 32],
            candidates: vec![("192.168.1.20".into(), 50000)],
            flags: FLAG_ONE_NETWORK,
        }
        .to_bytes();
        let mut dns = vec![0, 0, 0x84, 0, 0, 1, 0, 1, 0, 0, 0, 0];
        dns.extend_from_slice(b"\x04host\x05local\x00\x00\x01\x00\x01");
        dns.extend_from_slice(&[0xc0, 12, 0, 1, 0x80, 1, 0, 0, 0, 120, 0, 4, 192, 168, 1, 7]);
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for round in 0..20_000 {
            for base in [&valid, &dns] {
                let mut data = base.clone();
                // Cut short, grown, and with bytes changed.
                data.truncate(next() as usize % (data.len() + 1));
                for _ in 0..next() % 4 {
                    data.push(next() as u8);
                }
                for _ in 0..next() % 3 {
                    if !data.is_empty() {
                        let at = next() as usize % data.len();
                        data[at] = next() as u8;
                    }
                }
                let _ = Description::from_bytes(&data);
                let _ = answers(&data);
            }
            let noise: Vec<u8> = (0..round % 300).map(|_| next() as u8).collect();
            let _ = Description::from_bytes(&noise);
            let _ = answers(&noise);
        }
    }
}

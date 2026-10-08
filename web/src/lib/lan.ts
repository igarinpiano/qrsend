// A direct connection over the local network (WebRTC data channel), set up
// through the codes the two devices show each other: the sender's stream
// carries an offer, the receiver shows an answer. No server is involved and
// no address outside the local network is contacted (no STUN/TURN).
//
// The channel carries frames of the same transfer as the screen (in binary,
// and in far larger pieces than a code can hold), and the receiver's feedback
// in the other direction. It is one more way for the data to travel, nothing
// else: if it never comes up or breaks, the screen and the camera carry on.
import { linkCertificateFingerprint, linkParse, linkSplit, ready } from "./core";
import { log } from "./log";

/** What kinds of addresses a description holds (the addresses themselves are nobody's business). */
function kinds(candidates: { address: string }[]): string {
  const kind = (a: string) => (a.endsWith(".local") ? "name" : a.includes(":") ? "v6" : "v4");
  return candidates.map((c) => kind(c.address)).join(",") || "none";
}

/** Logs how a connection in the making gets on. */
function watch(pc: RTCPeerConnection, who: string): void {
  const since = performance.now();
  const say = (what: string, state: string) => log("link", `${who}: ${what} ${state}`, { s: (performance.now() - since) / 1000 });
  pc.addEventListener("icegatheringstatechange", () => say("looking for addresses:", pc.iceGatheringState));
  pc.addEventListener("iceconnectionstatechange", () => say("path:", pc.iceConnectionState));
  pc.addEventListener("connectionstatechange", () => say("connection:", pc.connectionState));
  pc.addEventListener("icecandidateerror", (e) => say("address error:", String((e as RTCPeerConnectionIceErrorEvent).errorCode)));
}

export const LINK_PREFIX = "QSL1-";
const KIND_OFFER = 1;
const KIND_ANSWER = 2;
/** An offer to one device the sender has connected to before: it needs no answer (see `LanSender.start`). */
const KIND_KNOWN_OFFER = 4;
/**
 * An offer to whoever reads it, from a sender that cannot be shown an answer (the command-line program, which has no
 * camera on the receiver): 16 bytes of key, then the description. See `LanReceiver.acceptOpen`.
 */
const KIND_OPEN_OFFER = 5;
/**
 * An offer to a receiver that cannot show an answer either (the command-line program, read by nobody's camera): 16
 * bytes of seed, then the description. The receiver's certificate follows from the seed, so this side knows its
 * fingerprint without being told (see `LanSender.start`).
 */
export const KIND_SEEDED_OFFER = 6;
/** Starts the line with which a receiver shows that it read an open offer. */
const PROOF = "K";
/** In an offer: the sender remembers devices it trusts, to connect without an answer next time. */
const FLAG_REMEMBERS = 1;
const FEEDBACK_PREFIX = "QSF1-";
/** The receiver's first line: it takes messages of packed binary records. */
const HELLO_BINARY = "B1";
/** Starts the line in which a receiver says who it is (see `introduction`). */
const INTRODUCTION = "I";

/** What two peers must know about each other to connect. */
interface Description {
  ufrag: string;
  pwd: string;
  /** SHA-256 fingerprint of the peer's DTLS certificate. */
  fingerprint: Uint8Array;
  candidates: { address: string; port: number }[];
  /** Offers only (see `FLAG_REMEMBERS`). */
  flags?: number;
}

const MAX_CANDIDATES = 4;

function parseSdp(sdp: string): Description {
  const line = (prefix: string) =>
    sdp
      .split(/\r?\n/)
      .find((l) => l.startsWith(prefix))
      ?.slice(prefix.length) ?? "";
  const hex = line("a=fingerprint:sha-256 ").trim();
  const fingerprint = new Uint8Array(hex ? hex.split(":").map((b) => parseInt(b, 16)) : []);
  const candidates: Description["candidates"] = [];
  for (const l of sdp.split(/\r?\n/)) {
    if (!l.startsWith("a=candidate:")) continue;
    const f = l.split(" ");
    // foundation component transport priority address port "typ" type …
    if (f[2]?.toLowerCase() !== "udp" || f[7] !== "host") continue;
    const candidate = { address: f[4], port: Number(f[5]) };
    if (!candidates.some((c) => c.address === candidate.address && c.port === candidate.port)) candidates.push(candidate);
  }
  // Addresses most likely to work first: IPv4 and mDNS names before IPv6.
  candidates.sort((a, b) => Number(a.address.includes(":")) - Number(b.address.includes(":")));
  return { ufrag: line("a=ice-ufrag:"), pwd: line("a=ice-pwd:"), fingerprint, candidates: candidates.slice(0, MAX_CANDIDATES) };
}

function buildSdp(d: Description, type: "offer" | "answer"): string {
  const fingerprint = [...d.fingerprint].map((b) => b.toString(16).padStart(2, "0").toUpperCase()).join(":");
  return [
    "v=0",
    "o=- 1 2 IN IP4 127.0.0.1",
    "s=-",
    "t=0 0",
    "a=group:BUNDLE 0",
    "a=msid-semantic: WMS",
    "m=application 9 UDP/DTLS/SCTP webrtc-datachannel",
    "c=IN IP4 0.0.0.0",
    ...d.candidates.map((c, i) => `a=candidate:${i + 1} 1 udp ${2122260223 - i} ${c.address} ${c.port} typ host generation 0`),
    `a=ice-ufrag:${d.ufrag}`,
    `a=ice-pwd:${d.pwd}`,
    `a=fingerprint:sha-256 ${fingerprint}`,
    `a=setup:${type === "offer" ? "actpass" : "active"}`,
    "a=mid:0",
    "a=sctp-port:5000",
    "a=max-message-size:262144",
    "",
  ].join("\r\n");
}

const text = new TextEncoder();
const untext = new TextDecoder();

/** ufrag, pwd (length-prefixed), 32 fingerprint bytes, then candidates as length-prefixed address + port. */
function pack(d: Description): Uint8Array {
  const out: number[] = [];
  const str = (s: string) => {
    const b = text.encode(s);
    out.push(b.length, ...b);
  };
  str(d.ufrag);
  str(d.pwd);
  out.push(...d.fingerprint);
  out.push(d.candidates.length);
  for (const c of d.candidates) {
    str(c.address);
    out.push(c.port >> 8, c.port & 0xff);
  }
  // Left out when nothing is flagged: the message is then what it always was.
  if (d.flags) out.push(d.flags);
  return new Uint8Array(out);
}

function unpack(bytes: Uint8Array): Description {
  let at = 0;
  const take = (n: number) => {
    if (at + n > bytes.length) throw new Error("the connection offer is damaged");
    at += n;
    return bytes.subarray(at - n, at);
  };
  const str = () => untext.decode(take(take(1)[0]));
  const ufrag = str();
  const pwd = str();
  const fingerprint = take(32).slice();
  const candidates: Description["candidates"] = [];
  for (let n = take(1)[0]; n > 0; n--) {
    const address = str();
    const [hi, lo] = take(2);
    candidates.push({ address, port: (hi << 8) | lo });
  }
  // Only what an SDP line may hold: these strings end up in one.
  const plain = /^[A-Za-z0-9+/=._:-]+$/;
  if (![ufrag, pwd, ...candidates.map((c) => c.address)].every((s) => plain.test(s))) {
    throw new Error("the connection offer is damaged");
  }
  // (Anything after the flags belongs to a later revision.)
  return { ufrag, pwd, fingerprint, candidates, flags: at < bytes.length ? bytes[at] : 0 };
}

const hex = (bytes: Uint8Array) => [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");

async function sha256(context: string, bytes: Uint8Array): Promise<Uint8Array> {
  const data = new Uint8Array([...text.encode(context), ...bytes]);
  return new Uint8Array(await crypto.subtle.digest("SHA-256", data));
}

/** How an offer to a known device names it: the start of a hash of its certificate's fingerprint. */
export async function knownTag(fingerprint: Uint8Array): Promise<Uint8Array> {
  return (await sha256("qrsend known device\n", fingerprint)).slice(0, 8);
}

/**
 * The credentials a known device answers an offer with. Both sides work them out from the offer, so the answer need
 * not travel. (They keep strangers' packets apart, no more: who the other side is, is settled by the certificates.)
 */
async function answerCredentials(offer: Uint8Array): Promise<{ ufrag: string; pwd: string }> {
  const h = hex(await sha256("qrsend known answer\n", offer));
  return { ufrag: h.slice(0, 8), pwd: h.slice(8, 40) };
}

/** After the first local address is known, others get this long to follow. */
const ADDRESSES_SETTLE_MS = 250;
/** Without any address after this long, there is none to wait for. */
const ADDRESSES_TIMEOUT_MS = 3000;

/**
 * Local description once the addresses are known (there is no trickling: it all has to fit into codes).
 *
 * Addresses on the local network are known almost at once. The browser's word that it is done looking can take
 * seconds more (it also looks for kinds of addresses not used here), and both devices wait for it in turn, so the
 * wait ends a moment after the first usable address instead.
 */
async function described(pc: RTCPeerConnection, flags = 0): Promise<Uint8Array> {
  if (pc.iceGatheringState !== "complete") {
    await new Promise<void>((resolve) => {
      let settle: ReturnType<typeof setTimeout> | undefined;
      const done = () => {
        if (pc.iceGatheringState === "complete") finish();
      };
      const found = (e: RTCPeerConnectionIceEvent) => {
        if (e.candidate?.type === "host" && e.candidate.protocol === "udp") settle ??= setTimeout(finish, ADDRESSES_SETTLE_MS);
      };
      const finish = () => {
        pc.removeEventListener("icegatheringstatechange", done);
        pc.removeEventListener("icecandidate", found);
        clearTimeout(timer);
        clearTimeout(settle);
        resolve();
      };
      const timer = setTimeout(finish, ADDRESSES_TIMEOUT_MS);
      pc.addEventListener("icegatheringstatechange", done);
      pc.addEventListener("icecandidate", found);
    });
  }
  const d = parseSdp(pc.localDescription?.sdp ?? "");
  if (!d.ufrag || d.fingerprint.length !== 32) throw new Error("this browser gave no usable connection details");
  log("link", "own description ready", { addresses: kinds(d.candidates), lookedFor: pc.iceGatheringState });
  if (d.candidates.length === 0) throw new Error("no local network address");
  return pack({ ...d, flags });
}

export interface LinkMessage {
  session: string;
  kind: number;
  id: number;
  payload: Uint8Array;
}

/** Puts link messages together from their codes, which arrive in any order and repeatedly. */
export class LinkAssembler {
  private pending = new Map<string, (Uint8Array | undefined)[]>();

  /** Returns the whole message whenever `code` completes (or repeats a part of) one. */
  add(code: string): LinkMessage | null {
    const p = linkParse(code) as { session: string; kind: number; id: number; part: number; parts: number; payload: Uint8Array } | null;
    if (!p) return null;
    const key = `${p.session}:${p.kind}:${p.id}:${p.parts}`;
    let parts = this.pending.get(key);
    if (!parts) {
      if (this.pending.size > 16) this.pending.clear();
      parts = new Array(p.parts).fill(undefined);
      this.pending.set(key, parts);
    }
    parts[p.part] = p.payload;
    if (parts.some((x) => !x)) return null;
    const whole = new Uint8Array(parts.reduce((n, x) => n + x!.length, 0));
    let at = 0;
    for (const x of parts) {
      whole.set(x!, at);
      at += x!.length;
    }
    return { session: p.session, kind: p.kind, id: p.id, payload: whole };
  }
}

export type LinkState = "offering" | "connected" | "closed";

// A record for this channel is a frame with 4 KiB of data.
/** Records per message; a message must stay well below 256 KiB. */
const BATCH = 16;
// How fast to send is found out as the transfer goes. The sender hands
// records to the browser at a pace (records per second) instead of all at
// once: the pace starts low and rises with every round of confirmations
// while the receiver keeps up (by half, but by no more than 4 MiB/s a round),
// and only gently once it is near a pace that caused trouble before.
// "Keeps up" is judged by how much is on its way, i.e. sent but not yet
// confirmed as taken in: more than a third of a second's worth means the
// receiver or the network is slower than the pace, which then drops to what
// actually got through.
//
// Why a pace and not just a limit on what is on its way: whatever is handed
// over at once, the browser sends in one burst, and its own probing for the
// network's capacity doubles those bursts until packets are lost. Measured on
// one machine, that happened about a megabyte into every transfer and cost one
// to three seconds of standstill each time (and with everything poured in at
// once, 359 lost packets and 8 seconds). Fed at a pace, the connection is
// never asked to carry more than it just did, plus a little. (The limit per
// round is what keeps "a little" small at high speeds: the same test lost
// packets again when the pace went from 23 to 115 MiB/s within half a second.)
const RATE_START = 512;
const RATE_MIN = 64;
const RATE_MAX = 65536;
/** The most the pace rises in one round of confirmations (records per second). */
const RATE_STEP_MAX = 1024;
/** What may be on its way at most, in seconds of the current pace. */
const IN_FLIGHT_S = 0.5;
/** More than this on its way: the pace is too high. */
const BEHIND_S = 0.3;
/** Less than this on its way: there is room for more. */
const KEEPING_UP_S = 0.15;
/** The least that may be on its way, whatever the pace. */
const WINDOW_START = 32;
/** Without a confirmation for this long, what is on its way counts as stuck. */
const QUIET_MS = 1500;
/** Bytes handed to the browser but not yet to the network. */
const BUFFER_HIGH = 512 << 10;
/** A connection through which nothing was confirmed for this long counts as lost. */
const STALL_MS = 8000;
/**
 * Everything was sent, the receiver still lacks something and does not say so in a way that settles it: after this
 * long, send more regardless.
 */
const IDLE_MS = 3000;

export interface SenderHooks {
  /** Codes of the offer to mix into the stream (`null`: none any more). */
  offer(payload: Uint8Array | null, id: number): void;
  /**
   * The next records of the stream: one binary message, or codes as text for a receiver that does not take binary.
   * None when everything was sent and the receiver's feedback has to tell what else is wanted (`more`: regardless).
   */
  pull(count: number, binary: boolean, more: boolean): Promise<{ count: number; data?: Uint8Array; texts?: string[] }>;
  /** A feedback code the receiver sent through the connection, having taken in `taken` records by then. */
  feedback(code: string, taken: number): void;
  state(state: LinkState): void;
  /** Codes of the offer to a known device (`null`: none any more). */
  offerKnown?(payload: Uint8Array | null, id: number): void;
  /** Codes of the offer to a receiver that shows no answer (`null`: none any more). */
  offerSeeded?(payload: Uint8Array | null, id: number): void;
  /** The receiver says who it is (see `introduction`); `certificate` is the fingerprint it connected with. */
  introduced?(introduction: string, certificate: Uint8Array): void;
}

/** What a sender does besides offering a connection the usual way. */
export interface SenderOptions {
  /** Tell receivers that this sender remembers devices it trusts. */
  remembers?: boolean;
  /** Certificate fingerprint of a device connected to before: it is offered a connection that needs no answer. */
  known?: Uint8Array;
}

export class LanSender {
  private pc?: RTCPeerConnection;
  private channel?: RTCDataChannel;
  /** The attempt toward a known device, until one of the two connects. */
  private knownPc?: RTCPeerConnection;
  private knownChannel?: RTCDataChannel;
  /** The attempt toward a receiver that shows no answer, likewise. */
  private seededPc?: RTCPeerConnection;
  private seededChannel?: RTCDataChannel;
  /** Certificate fingerprint of whoever answered the usual offer. */
  private answered?: Uint8Array;
  private assembler = new LinkAssembler();
  private id = Math.floor(Math.random() * 256);
  private sent = 0;
  private acked = 0;
  /** Records per second handed to the browser at most. */
  private rate = RATE_START;
  /** Below this pace there was no trouble: up to it the pace rises fast. */
  private ceiling = RATE_MAX;
  /** Since the last look at the pace: it held sending back; the network did. */
  private paced = false;
  private networkBound = false;
  private ratedAt = 0;
  private freshSince = 0;
  private tokens = 0;
  private tokensAt = 0;
  private confirmedAt = 0;
  /** The receiver takes binary messages (it said so). */
  private binary = false;
  private stopped = false;
  private wake?: () => void;

  constructor(
    private session: string,
    private hooks: SenderHooks,
    private options: SenderOptions = {},
  ) {}

  /**
   * Starts offering a connection (again).
   *
   * The usual offer is for anyone, and needs the receiver's answer (a code on its screen, read by this device's
   * camera). A device connected to before can be spared that: what an answer says is its certificate (known from
   * last time), credentials (worked out from the offer by both sides) and its addresses (learned when it knocks). So
   * it gets an offer of its own, to which this side has already written the answer. Whichever connects first carries
   * the transfer; the other attempt ends.
   */
  async start(): Promise<void> {
    this.close();
    if (this.stopped) return;
    this.id = (this.id + 1) % 256;
    const id = this.id;
    const attempt = (peer: () => Uint8Array | undefined, who: string) => {
      const pc = new RTCPeerConnection({ iceServers: [] });
      watch(pc, who);
      const channel = pc.createDataChannel("qrsend");
      channel.bufferedAmountLowThreshold = BUFFER_HIGH / 4;
      channel.onopen = () => {
        if (this.channel !== channel && this.knownChannel !== channel && this.seededChannel !== channel) return;
        const all = [this.channel, this.knownChannel, this.seededChannel, this.pc, this.knownPc, this.seededPc];
        const others = all.filter((x) => x !== channel && x !== pc);
        this.pc = pc;
        this.channel = channel;
        this.knownPc = this.knownChannel = undefined;
        this.seededPc = this.seededChannel = undefined;
        for (const other of others) {
          try {
            other?.close();
          } catch {
            /* already closed */
          }
        }
        this.peer = peer();
        log("link", `${who}: channel open`);
        this.hooks.offer(null, id);
        this.hooks.offerKnown?.(null, id);
        this.hooks.offerSeeded?.(null, id);
        this.hooks.state("connected");
        this.pump(channel);
      };
      channel.onbufferedamountlow = () => this.wake?.();
      channel.onmessage = (e) => {
        for (const line of String(e.data).split("\n")) {
          if (line.startsWith("A")) {
            this.confirmed(Number(line.slice(1)) || 0);
          } else if (line.startsWith(FEEDBACK_PREFIX)) {
            this.hooks.feedback(line, this.acked);
            this.wake?.();
          } else if (line === HELLO_BINARY) {
            this.binary = true;
            this.wake?.();
          } else if (line.startsWith(INTRODUCTION) && this.peer) {
            this.hooks.introduced?.(line.slice(INTRODUCTION.length), this.peer);
          }
        }
      };
      // (An attempt that is not, or no longer, the one in use just ends.)
      const lost = () => this.giveUp(channel);
      channel.onclose = lost;
      pc.onconnectionstatechange = () => {
        if (pc.connectionState === "failed" || pc.connectionState === "closed") lost();
      };
      return { pc, channel };
    };

    const { pc, channel } = attempt(() => this.answered, "offer");
    this.pc = pc;
    this.channel = channel;
    await pc.setLocalDescription(await pc.createOffer());
    const payload = await described(pc, this.options.remembers ? FLAG_REMEMBERS : 0);
    if (this.pc !== pc) return;
    this.hooks.offer(payload, id);
    this.hooks.state("offering");

    await this.offerSeeded(attempt, id);
    const known = this.options.known;
    if (!known) return;
    try {
      const second = attempt(() => known, "offer to known device");
      this.knownPc = second.pc;
      this.knownChannel = second.channel;
      await second.pc.setLocalDescription(await second.pc.createOffer());
      const offer = new Uint8Array([...(await knownTag(known)), ...(await described(second.pc))]);
      const { ufrag, pwd } = await answerCredentials(offer);
      await second.pc.setRemoteDescription({
        type: "answer",
        sdp: buildSdp({ ufrag, pwd, fingerprint: known, candidates: [] }, "answer"),
      });
      if (this.knownPc !== second.pc) return;
      this.hooks.offerKnown?.(offer, id);
    } catch (e) {
      // The usual offer stands on its own.
      console.warn("offer to a known device:", e);
    }
  }

  /**
   * A third attempt, toward a receiver that cannot show an answer (the command-line program): its certificate follows
   * from a seed in the offer, the rest is as for a known device.
   */
  private async offerSeeded(
    attempt: (peer: () => Uint8Array | undefined, who: string) => { pc: RTCPeerConnection; channel: RTCDataChannel },
    id: number,
  ): Promise<void> {
    if (!this.hooks.offerSeeded) return;
    try {
      await ready();
      const seed = crypto.getRandomValues(new Uint8Array(16));
      const fingerprint = linkCertificateFingerprint(seed) as Uint8Array;
      const third = attempt(() => fingerprint, "offer to a command line");
      this.seededPc = third.pc;
      this.seededChannel = third.channel;
      await third.pc.setLocalDescription(await third.pc.createOffer());
      const offer = new Uint8Array([...seed, ...(await described(third.pc))]);
      const { ufrag, pwd } = await answerCredentials(offer);
      await third.pc.setRemoteDescription({ type: "answer", sdp: buildSdp({ ufrag, pwd, fingerprint, candidates: [] }, "answer") });
      if (this.seededPc !== third.pc) return;
      this.hooks.offerSeeded(offer, id);
    } catch (e) {
      // The usual offer stands on its own.
      console.warn("offer to a command line:", e);
    }
  }

  /** Certificate fingerprint of the device connected to. */
  private peer?: Uint8Array;

  /** A link code the camera read off the receiver's screen. */
  async answer(code: string): Promise<void> {
    const msg = this.assembler.add(code);
    const pc = this.pc;
    if (!msg || !pc || msg.kind !== KIND_ANSWER || msg.session !== this.session || msg.id !== this.id) return;
    if (pc.signalingState !== "have-local-offer") return;
    const description = unpack(msg.payload);
    log("link", "answer read", { addresses: kinds(description.candidates) });
    this.answered = description.fingerprint;
    await pc.setRemoteDescription({ type: "answer", sdp: buildSdp(description, "answer") });
  }

  private get window(): number {
    return Math.max(WINDOW_START, Math.round(this.rate * IN_FLIGHT_S));
  }

  /** The receiver has taken in `count` records so far. */
  private confirmed(count: number): void {
    const fresh = count - this.acked;
    if (fresh <= 0) return;
    this.acked = count;
    const now = performance.now();
    this.confirmedAt = now;
    this.freshSince += fresh;
    const elapsed = now - this.ratedAt;
    if (elapsed >= 80) {
      const onTheirWay = this.sent - this.acked;
      if (onTheirWay > Math.max(2 * BATCH, this.rate * BEHIND_S)) {
        // More goes in than comes out: down to what came out (a standstill
        // says little about that, so not below a third at once).
        const delivered = (this.freshSince * 1000) / elapsed;
        this.ceiling = Math.max(RATE_MIN, this.rate * 0.7);
        this.rate = Math.max(RATE_MIN, Math.min(this.rate, Math.max(delivered * 0.9, this.rate * 0.3)));
      } else if (this.networkBound) {
        // The browser could not pass on what it was given: this is the limit.
        this.ceiling = Math.min(this.ceiling, this.rate);
      } else if (this.paced && onTheirWay < Math.max(2 * BATCH, this.rate * KEEPING_UP_S)) {
        const step = this.rate < this.ceiling ? this.rate * 0.5 : this.rate * 0.03;
        this.rate = Math.min(RATE_MAX, this.rate + Math.min(Math.max(step, BATCH), RATE_STEP_MAX));
      }
      this.paced = this.networkBound = false;
      this.freshSince = 0;
      this.ratedAt = now;
    }
    this.wake?.();
  }

  /** Time spent, in milliseconds, since the connection came up (for the measurements display). */
  private spent = { window: 0, buffer: 0, pace: 0, pull: 0, idle: 0, since: 0 };

  /** What the connection is doing: for finding out what holds a transfer back. */
  get measurements(): {
    /** Records per second the sender allows itself at the moment. */
    rate: number;
    window: number;
    onTheirWay: number;
    sent: number;
    binary: boolean;
    waitingForReceiver: number;
    waitingForNetwork: number;
    holdingBack: number;
    preparing: number;
    nothingToSend: number;
  } {
    const total = Math.max(performance.now() - this.spent.since, 1);
    return {
      rate: Math.round(this.rate),
      window: this.window,
      onTheirWay: this.sent - this.acked,
      sent: this.sent,
      binary: this.binary,
      // Shares of the time: the receiver has not confirmed enough, the browser
      // has not handed enough to the network, the sender's own pace, the next
      // records are being made, there is nothing to send.
      waitingForReceiver: this.spent.window / total,
      waitingForNetwork: this.spent.buffer / total,
      holdingBack: this.spent.pace / total,
      preparing: this.spent.pull / total,
      nothingToSend: this.spent.idle / total,
    };
  }

  /** Sends codes as fast as the receiver takes them in. */
  private async pump(channel: RTCDataChannel): Promise<void> {
    // (Asked anew of every receiver; kept after the end for the measurements.)
    this.binary = false;
    this.sent = 0;
    this.acked = 0;
    this.rate = RATE_START;
    this.ceiling = RATE_MAX;
    this.paced = this.networkBound = false;
    this.freshSince = 0;
    this.tokens = BATCH;
    this.confirmedAt = this.ratedAt = this.tokensAt = performance.now();
    this.spent = { window: 0, buffer: 0, pace: 0, pull: 0, idle: 0, since: performance.now() };
    const nap = (ms: number) =>
      new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, ms);
        this.wake = () => {
          clearTimeout(timer);
          resolve();
        };
      }).then(() => (this.wake = undefined));
    // The receiver says at once whether it takes binary messages; one that
    // does not say so (an older version) gets text.
    if (!this.binary) await nap(300);
    let idleSince = 0;
    while (this.channel === channel && channel.readyState === "open") {
      const waiting = this.sent - this.acked;
      const quiet = performance.now() - this.confirmedAt;
      if (waiting > 0 && quiet > STALL_MS) {
        // "Open", yet nothing gets through (a network that drops everything,
        // a peer that went to sleep): the screen has to take over again.
        this.giveUp(channel);
        return;
      }
      if (waiting > 0 && quiet > QUIET_MS && this.rate > RATE_MIN && performance.now() - this.ratedAt > QUIET_MS) {
        // Nothing comes out at all: whatever the pace was, it was too much.
        this.ceiling = Math.max(RATE_MIN, this.rate / 2);
        this.rate = Math.max(RATE_MIN, this.rate / 4);
        this.ratedAt = performance.now();
      }
      const byWindow = waiting >= this.window;
      if (byWindow || channel.bufferedAmount > BUFFER_HIGH) {
        if (!byWindow) this.networkBound = true;
        const waitingSince = performance.now();
        await nap(100);
        this.spent[byWindow ? "window" : "buffer"] += performance.now() - waitingSince;
        continue;
      }
      // The pace: a message goes out when its share of time has come. (A
      // little may be saved up, as timers are coarser than a message's share.)
      const at = performance.now();
      this.tokens = Math.min(this.tokens + ((at - this.tokensAt) * this.rate) / 1000, Math.max(BATCH, this.rate * 0.008));
      this.tokensAt = at;
      if (this.tokens < BATCH) {
        this.paced = true;
        await nap(Math.max(1, ((BATCH - this.tokens) * 1000) / this.rate));
        this.spent.pace += performance.now() - at;
        continue;
      }
      let batch: Awaited<ReturnType<SenderHooks["pull"]>>;
      const pullingSince = performance.now();
      const more = idleSince > 0 && pullingSince - idleSince > IDLE_MS;
      try {
        batch = await this.hooks.pull(BATCH, this.binary, more);
      } catch {
        break;
      }
      this.spent.pull += performance.now() - pullingSince;
      if (this.channel !== channel || channel.readyState !== "open") break;
      if (batch.count === 0) {
        // Everything was sent once. What happens next is up to the receiver:
        // its feedback tells what it still lacks (or that it has it all).
        const idleFrom = performance.now();
        if (!idleSince || more) idleSince = idleFrom;
        await nap(100);
        this.spent.idle += performance.now() - idleFrom;
        continue;
      }
      idleSince = 0;
      // Nothing was waiting: the clock for "no confirmation" starts now.
      if (this.sent === this.acked) this.confirmedAt = performance.now();
      if (batch.data) channel.send(batch.data as Uint8Array<ArrayBuffer>);
      else channel.send((batch.texts ?? []).join("\n"));
      this.sent += batch.count;
      this.tokens -= batch.count;
    }
  }

  /** The connection is of no use any more: drop it and say so. */
  private giveUp(channel: RTCDataChannel): void {
    if (this.channel !== channel) return;
    log("link", "connection given up", { sent: this.sent, confirmed: this.acked });
    this.close();
    this.hooks.state("closed");
  }

  private close(): void {
    const channels = [this.channel, this.knownChannel, this.seededChannel];
    const connections = [this.pc, this.knownPc, this.seededPc];
    this.pc = this.knownPc = this.seededPc = undefined;
    this.channel = this.knownChannel = this.seededChannel = undefined;
    this.answered = this.peer = undefined;
    this.wake?.();
    const shut = (all: (RTCDataChannel | RTCPeerConnection | undefined)[]) => {
      for (const one of all) {
        try {
          one?.close();
        } catch {
          /* already closed */
        }
      }
    };
    // The channel first, the connection a moment later: closing both at once
    // takes the connection down before the other side has been told, and it
    // goes on believing it is connected.
    shut(channels);
    setTimeout(() => shut(connections), 500);
  }

  /** Ends the connection for good (the transfer is over). */
  stop(): void {
    this.stopped = true;
    this.hooks.offer(null, this.id);
    this.hooks.offerKnown?.(null, this.id);
    this.hooks.offerSeeded?.(null, this.id);
    this.close();
  }
}

export interface ReceiverHooks {
  /** Codes that arrived through the connection as text (from a sender that does not send binary). */
  codes(codes: string[]): void;
  /** A message of packed binary records that arrived through the connection. */
  packed(message: ArrayBuffer): void;
  state(state: "connected" | "closed"): void;
}

export class LanReceiver {
  private pc?: RTCPeerConnection;
  private channel?: RTCDataChannel;
  /** The connection uses this device's lasting certificate (so the sender may remember it). */
  lasting = false;
  /** What to send first on a connection made from an open offer. */
  private proof?: string;

  constructor(private hooks: ReceiverHooks) {}

  get connected(): boolean {
    return this.channel?.readyState === "open";
  }

  private open(certificate?: RTCCertificate): RTCPeerConnection {
    this.stop();
    this.proof = undefined;
    this.lasting = !!certificate;
    const pc = new RTCPeerConnection({ iceServers: [], certificates: certificate ? [certificate] : undefined });
    watch(pc, "answer");
    this.pc = pc;
    this.listen(pc);
    return pc;
  }

  /**
   * Accepts an offer; returns the answer as a code to show the sender. For a sender that remembers devices,
   * `lasting` supplies this device's lasting certificate.
   */
  async accept(offer: LinkMessage, lasting?: () => Promise<RTCCertificate | undefined>): Promise<string> {
    const description = unpack(offer.payload);
    const certificate = (description.flags ?? 0) & FLAG_REMEMBERS ? await lasting?.() : undefined;
    log("link", "offer taken up", { id: offer.id, addresses: kinds(description.candidates), senderRemembers: !!description.flags });
    const pc = this.open(certificate);
    await pc.setRemoteDescription({ type: "offer", sdp: buildSdp(description, "offer") });
    await pc.setLocalDescription(await pc.createAnswer());
    const payload = await described(pc);
    const codes = linkSplit(offer.session, KIND_ANSWER, offer.id, payload, 2000) as string[];
    return codes[0];
  }

  /**
   * Takes up an offer made to this device in particular (it has connected to that sender before). Nothing is shown:
   * the sender already has the answer, for which this side uses its lasting certificate and the credentials both work
   * out from the offer, and then knocks at the sender's addresses.
   */
  async acceptKnown(offer: LinkMessage, certificate: RTCCertificate): Promise<void> {
    const pc = this.open(certificate);
    await pc.setRemoteDescription({ type: "offer", sdp: buildSdp(unpack(offer.payload.subarray(8)), "offer") });
    const { ufrag, pwd } = await answerCredentials(offer.payload);
    const answer = await pc.createAnswer();
    const sdp = (answer.sdp ?? "").replace(/a=ice-ufrag:.*/g, `a=ice-ufrag:${ufrag}`).replace(/a=ice-pwd:.*/g, `a=ice-pwd:${pwd}`);
    await pc.setLocalDescription({ type: "answer", sdp });
  }

  /**
   * Takes up an offer made to whoever reads it. Nothing is shown here either: this side answers with the credentials
   * both work out from the offer and knocks at the sender's addresses. The sender cannot know this side's certificate
   * beforehand, so the first thing sent ties it to the key in the offer: only someone who read the code can say it.
   */
  async acceptOpen(offer: LinkMessage): Promise<void> {
    const key = offer.payload.subarray(0, 16);
    const description = unpack(offer.payload.subarray(16));
    log("link", "open offer taken up", { id: offer.id, addresses: kinds(description.candidates) });
    const pc = this.open();
    await pc.setRemoteDescription({ type: "offer", sdp: buildSdp(description, "offer") });
    const { ufrag, pwd } = await answerCredentials(offer.payload);
    const answer = await pc.createAnswer();
    const sdp = (answer.sdp ?? "").replace(/a=ice-ufrag:.*/g, `a=ice-ufrag:${ufrag}`).replace(/a=ice-pwd:.*/g, `a=ice-pwd:${pwd}`);
    const own = parseSdp(sdp).fingerprint;
    if (own.length !== 32) throw new Error("this browser gave no usable connection details");
    await pc.setLocalDescription({ type: "answer", sdp });
    this.proof = PROOF + hex(await sha256("qrsend open link\n", new Uint8Array([...key, ...own])));
  }

  private listen(pc: RTCPeerConnection): void {
    // A sender that goes away without a word (its page closed, the network
    // gone) leaves the channel looking open; the connection itself tells.
    pc.addEventListener("connectionstatechange", () => {
      if (this.pc !== pc || (pc.connectionState !== "failed" && pc.connectionState !== "closed")) return;
      const was = this.channel;
      this.channel = undefined;
      if (was) this.hooks.state("closed");
    });
    pc.ondatachannel = (e) => {
      const channel = e.channel;
      this.channel = channel;
      channel.binaryType = "arraybuffer";
      let greeted = false;
      const opened = () => {
        if (greeted) return;
        greeted = true;
        log("link", "answer: channel open");
        this.hooks.state("connected");
        if (this.proof) channel.send(this.proof);
        channel.send(HELLO_BINARY);
      };
      channel.onopen = opened;
      channel.onmessage = (m) => {
        if (m.data instanceof ArrayBuffer) this.hooks.packed(m.data);
        else this.hooks.codes(String(m.data).split("\n"));
      };
      channel.onclose = () => {
        if (this.channel !== channel) return;
        this.channel = undefined;
        this.hooks.state("closed");
      };
      if (channel.readyState === "open") opened();
    };
  }

  /**
   * Sends lines to the sender: feedback codes, and "A<n>" (n codes taken in so
   * far). Callers gather them and send a few times per second at most.
   */
  send(line: string): void {
    if (this.connected) this.channel!.send(line);
  }

  stop(): void {
    const { pc, channel } = this;
    this.pc = undefined;
    this.channel = undefined;
    try {
      channel?.close();
      pc?.close();
    } catch {
      /* already closed */
    }
  }
}

export const isOffer = (m: LinkMessage) => m.kind === KIND_OFFER;
/** Whether this is an offer to the device with this certificate fingerprint in particular. */
export async function isKnownOfferFor(m: LinkMessage, fingerprint: Uint8Array): Promise<boolean> {
  if (m.kind !== KIND_KNOWN_OFFER || m.payload.length < 8) return false;
  const tag = await knownTag(fingerprint);
  return tag.every((b, i) => b === m.payload[i]);
}
export const KNOWN_OFFER = KIND_KNOWN_OFFER;
/** A command-line sender's offer (a TCP address to connect to): nothing a browser can take. */
export const isTcpOffer = (m: LinkMessage) => m.kind === 3;
export const isOpenOffer = (m: LinkMessage) => m.kind === KIND_OPEN_OFFER && m.payload.length > 16;

/** The line in which a receiver says who it is: its device ID, its certificate, and its signature over both. */
export const introductionLine = (id: string, certificate: string, signature: string) =>
  INTRODUCTION + JSON.stringify({ id, certificate, signature });

/** What an introduction is signed over. */
export const introductionMessage = (certificate: string) => text.encode(`qrsend link v1\n${certificate}`);

export { hex as fingerprintHex };
export const canConnect = typeof RTCPeerConnection === "function";

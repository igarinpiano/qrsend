// A direct connection over the local network (WebRTC data channel), set up
// through the codes the two devices show each other: the sender's stream
// carries an offer, the receiver shows an answer. No server is involved and
// no address outside the local network is contacted (no STUN/TURN).
//
// The channel carries the same codes as the screen, as text, and the
// receiver's feedback in the other direction. It is one more way for codes to
// travel, nothing else: if it never comes up or breaks, the screen and the
// camera carry on.
import { linkParse, linkSplit } from "./core";

export const LINK_PREFIX = "QSL1-";
const KIND_OFFER = 1;
const KIND_ANSWER = 2;
const FEEDBACK_PREFIX = "QSF1-";

/** What two peers must know about each other to connect. */
interface Description {
  ufrag: string;
  pwd: string;
  /** SHA-256 fingerprint of the peer's DTLS certificate. */
  fingerprint: Uint8Array;
  candidates: { address: string; port: number }[];
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
  return { ufrag, pwd, fingerprint, candidates };
}

/** Local description once the addresses are known (there is no trickling: it all has to fit into codes). */
async function described(pc: RTCPeerConnection): Promise<Uint8Array> {
  if (pc.iceGatheringState !== "complete") {
    await new Promise<void>((resolve) => {
      const done = () => {
        if (pc.iceGatheringState === "complete") finish();
      };
      const finish = () => {
        pc.removeEventListener("icegatheringstatechange", done);
        clearTimeout(timer);
        resolve();
      };
      const timer = setTimeout(finish, 3000);
      pc.addEventListener("icegatheringstatechange", done);
    });
  }
  const d = parseSdp(pc.localDescription?.sdp ?? "");
  if (!d.ufrag || d.fingerprint.length !== 32) throw new Error("this browser gave no usable connection details");
  if (d.candidates.length === 0) throw new Error("no local network address");
  return pack(d);
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

// Codes for this channel are about 6 KiB of text each (4 KiB of data).
/** Codes per message; a message must stay well below 256 KiB. */
const BATCH = 16;
// How many codes may be on their way, i.e. sent but not yet confirmed as
// taken in by the receiver, is found out as the transfer goes, the way TCP
// does: start small, double while the receiver keeps up, grow gently after
// the first trouble, and fall back when nothing is confirmed for a while.
// (Pouring everything in at once looks faster but is not: measured on one
// machine, the first megabytes overran the network buffers, 359 packets were
// dropped, and the connection needed 8 seconds to recover.)
const WINDOW_START = 32;
const WINDOW_MAX = 4096;
/** Without a confirmation for this long, what is on its way counts as stuck. */
const QUIET_MS = 1500;
/** Bytes handed to the browser but not yet to the network. */
const BUFFER_HIGH = 512 << 10;
/** A connection through which nothing was confirmed for this long counts as lost. */
const STALL_MS = 8000;

export interface SenderHooks {
  /** Codes of the offer to mix into the stream (`null`: none any more). */
  offer(payload: Uint8Array | null, id: number): void;
  /** The next codes of the stream, as text. */
  pull(count: number): Promise<string[]>;
  /** A feedback code the receiver sent through the connection. */
  feedback(code: string): void;
  state(state: LinkState): void;
}

export class LanSender {
  private pc?: RTCPeerConnection;
  private channel?: RTCDataChannel;
  private assembler = new LinkAssembler();
  private id = Math.floor(Math.random() * 256);
  private sent = 0;
  private acked = 0;
  private window = WINDOW_START;
  /** Above this the window grows gently instead of doubling. */
  private gentleFrom = WINDOW_MAX;
  /** The window, not the browser's buffer, was what held sending back. */
  private windowLimited = false;
  private confirmedAt = 0;
  private stopped = false;
  private wake?: () => void;

  constructor(
    private session: string,
    private hooks: SenderHooks,
  ) {}

  /** Starts offering a connection (again). */
  async start(): Promise<void> {
    this.close();
    if (this.stopped) return;
    this.id = (this.id + 1) % 256;
    const id = this.id;
    const pc = new RTCPeerConnection({ iceServers: [] });
    this.pc = pc;
    const channel = pc.createDataChannel("qrsend");
    this.channel = channel;
    channel.bufferedAmountLowThreshold = BUFFER_HIGH / 4;
    channel.onopen = () => {
      if (this.channel !== channel) return;
      this.hooks.offer(null, id);
      this.hooks.state("connected");
      this.pump(channel);
    };
    channel.onbufferedamountlow = () => this.wake?.();
    channel.onmessage = (e) => {
      for (const line of String(e.data).split("\n")) {
        if (line.startsWith("A")) {
          this.confirmed(Number(line.slice(1)) || 0);
        } else if (line.startsWith(FEEDBACK_PREFIX)) {
          this.hooks.feedback(line);
        }
      }
    };
    const lost = () => this.giveUp(channel);
    channel.onclose = lost;
    pc.onconnectionstatechange = () => {
      if (pc.connectionState === "failed" || pc.connectionState === "closed") lost();
    };
    await pc.setLocalDescription(await pc.createOffer());
    const payload = await described(pc);
    if (this.pc !== pc) return;
    this.hooks.offer(payload, id);
    this.hooks.state("offering");
  }

  /** A link code the camera read off the receiver's screen. */
  async answer(code: string): Promise<void> {
    const msg = this.assembler.add(code);
    const pc = this.pc;
    if (!msg || !pc || msg.kind !== KIND_ANSWER || msg.session !== this.session || msg.id !== this.id) return;
    if (pc.signalingState !== "have-local-offer") return;
    await pc.setRemoteDescription({ type: "answer", sdp: buildSdp(unpack(msg.payload), "answer") });
  }

  /** The receiver has taken in `count` codes so far. */
  private confirmed(count: number): void {
    const fresh = count - this.acked;
    if (fresh <= 0) return;
    this.acked = count;
    this.confirmedAt = performance.now();
    if (this.windowLimited) {
      // Doubling at first; later about one more message per round.
      const step = this.window < this.gentleFrom ? fresh : Math.max(1, Math.round((BATCH * fresh) / this.window));
      this.window = Math.min(WINDOW_MAX, this.window + step);
      this.windowLimited = false;
    }
    this.wake?.();
  }

  /** How many codes may currently be on their way (for display). */
  get windowSize(): number {
    return this.window;
  }

  /** Sends codes as fast as the receiver takes them in. */
  private async pump(channel: RTCDataChannel): Promise<void> {
    this.sent = 0;
    this.acked = 0;
    this.window = WINDOW_START;
    this.gentleFrom = WINDOW_MAX;
    this.confirmedAt = performance.now();
    while (this.channel === channel && channel.readyState === "open") {
      const waiting = this.sent - this.acked;
      const quiet = performance.now() - this.confirmedAt;
      if (waiting > 0 && quiet > STALL_MS) {
        // "Open", yet nothing gets through (a network that drops everything,
        // a peer that went to sleep): the screen has to take over again.
        this.giveUp(channel);
        return;
      }
      if (waiting > 0 && quiet > QUIET_MS && this.window > WINDOW_START) {
        // Too much was on its way at once: start small again, and be
        // careful from half of what was too much.
        this.gentleFrom = Math.max(WINDOW_START, this.window >> 1);
        this.window = WINDOW_START;
      }
      const byWindow = waiting >= this.window;
      if (byWindow || channel.bufferedAmount > BUFFER_HIGH) {
        if (byWindow) this.windowLimited = true;
        await new Promise<void>((resolve) => {
          const timer = setTimeout(resolve, 100);
          this.wake = () => {
            clearTimeout(timer);
            resolve();
          };
        });
        this.wake = undefined;
        continue;
      }
      let codes: string[];
      try {
        codes = await this.hooks.pull(BATCH);
      } catch {
        break;
      }
      if (this.channel !== channel || channel.readyState !== "open") break;
      // Nothing was waiting: the clock for "no confirmation" starts now.
      if (this.sent === this.acked) this.confirmedAt = performance.now();
      channel.send(codes.join("\n"));
      this.sent += codes.length;
    }
  }

  /** The connection is of no use any more: drop it and say so. */
  private giveUp(channel: RTCDataChannel): void {
    if (this.channel !== channel) return;
    this.close();
    this.hooks.state("closed");
  }

  private close(): void {
    const { pc, channel } = this;
    this.pc = undefined;
    this.channel = undefined;
    this.wake?.();
    try {
      channel?.close();
      pc?.close();
    } catch {
      /* already closed */
    }
  }

  /** Ends the connection for good (the transfer is over). */
  stop(): void {
    this.stopped = true;
    this.hooks.offer(null, this.id);
    this.close();
  }
}

export interface ReceiverHooks {
  /** Codes that arrived through the connection. */
  codes(codes: string[]): void;
  state(state: "connected" | "closed"): void;
}

export class LanReceiver {
  private pc?: RTCPeerConnection;
  private channel?: RTCDataChannel;

  constructor(private hooks: ReceiverHooks) {}

  get connected(): boolean {
    return this.channel?.readyState === "open";
  }

  /** Accepts an offer; returns the answer as a code to show the sender. */
  async accept(offer: LinkMessage): Promise<string> {
    this.stop();
    const pc = new RTCPeerConnection({ iceServers: [] });
    this.pc = pc;
    pc.ondatachannel = (e) => {
      const channel = e.channel;
      this.channel = channel;
      channel.onopen = () => this.hooks.state("connected");
      channel.onmessage = (m) => this.hooks.codes(String(m.data).split("\n"));
      channel.onclose = () => {
        if (this.channel !== channel) return;
        this.channel = undefined;
        this.hooks.state("closed");
      };
      if (channel.readyState === "open") this.hooks.state("connected");
    };
    await pc.setRemoteDescription({ type: "offer", sdp: buildSdp(unpack(offer.payload), "offer") });
    await pc.setLocalDescription(await pc.createAnswer());
    const payload = await described(pc);
    const codes = linkSplit(offer.session, KIND_ANSWER, offer.id, payload, 2000) as string[];
    return codes[0];
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
export const canConnect = typeof RTCPeerConnection === "function";

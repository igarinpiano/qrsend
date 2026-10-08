/// <reference lib="webworker" />
// The engine: everything that touches transfer data runs in this worker.
//
// The WebAssembly core calls back into the functions defined here to write
// packed bodies, read segments and write extracted files, and those calls are
// served synchronously from OPFS files. Data therefore flows disk → core →
// disk in small pieces; nothing large is held in memory, on either side.
import { Identity, Receive, SendJob, SendSession, linkSplit, quietModules, ready, stanzaKeys } from "./core";
import * as db from "./db";
import { trustedDevices } from "./devices";
import type {
  EngineApi,
  EngineEvent,
  FrameBatch,
  LinkBatch,
  OutEntry,
  ReceiverReport,
  RecvInfo,
  RecvResult,
  RecvState,
  SendRequest,
  SendStarted,
  SessionRecord,
} from "./engine-types";
import { identityInfo, loadIdentity, sharedSecret, sign, type StoredIdentity } from "./keys";
import { fileStore, type FileStore, type RandomFile } from "./storage";

const READ_CHUNK = 4 << 20;
/** Texts up to this size are shown in the page; larger ones are offered as a file. */
const MAX_INLINE_TEXT = 4 << 20;
const FLAG_ENCRYPTED = 1;
/** Codes in which a sender says what it can do (see `SenderNotice` in the core). */
const NOTICE_PREFIX = "QSC1-";

function emit(event: EngineEvent): void {
  self.postMessage(event);
}

/** A line for the diagnostic log (no file names, no contents). */
function logLine(area: string, what: string, data?: Record<string, string | number | boolean | null | undefined>): void {
  emit({ event: "log", area, what, data });
}

function ranges(indices: number[]): string {
  const sorted = [...indices].sort((a, b) => a - b);
  const out: string[] = [];
  for (let i = 0; i < sorted.length; i++) {
    const start = sorted[i];
    while (i + 1 < sorted.length && sorted[i + 1] === sorted[i] + 1) i++;
    out.push(start === sorted[i] ? `${start}` : `${start}-${sorted[i]}`);
  }
  return out.join(",");
}

function parseRanges(text: string): number[] {
  const out: number[] = [];
  for (const part of text.split(",").filter(Boolean)) {
    const [a, b = a] = part.split("-").map(Number);
    for (let i = a; i <= b; i++) out.push(i);
  }
  return out;
}

// ------------------------------------------------------------------ sending

interface SendState {
  /** Codes of the offers to connect being repeated in the stream: the usual one, and one to a known device. */
  offers: { usual: string[]; known: string[] };
  session: SendSession;
  /** Session id (hex). */
  id: string;
  body: RandomFile;
}

let sending: SendState | undefined;
// Per worker, so two tabs never share a spool (OPFS is shared by the origin).
const SEND_DIR = ["send", crypto.randomUUID()];
const SEND_BODY = [...SEND_DIR, "body"];

async function sendStop(): Promise<void> {
  if (!sending) return;
  sending.session.free();
  sending.body.close();
  sending = undefined;
  await (await fileStore()).remove(SEND_DIR);
}

async function sendStart(req: SendRequest): Promise<SendStarted> {
  await ready();
  await sendStop();
  const packingSince = performance.now();
  const store = await fileStore();
  await store.remove(SEND_DIR);
  const body = await store.open(SEND_BODY);
  try {
    let written = 0;
    const job = new SendJob((chunk: Uint8Array) => {
      body.write(written, chunk);
      written += chunk.length;
    });
    for (const id of req.recipients) job.addRecipient(id);
    const me = await loadIdentity();
    if (me) job.setSenderName(identityInfo(me).name);

    if (req.text !== undefined) {
      job.setText(req.text);
    } else {
      const total = req.items.reduce((sum, i) => sum + (i.file?.size ?? 0), 0);
      let done = 0;
      // Always in the same order, however the files were picked: the same
      // data then packs to the same bytes, which is what lets a transfer be
      // continued by sending it again.
      const items = [...req.items].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
      for (const item of items) {
        if (!item.file) {
          job.addDir(item.path);
          continue;
        }
        job.beginFile(item.path, item.file.lastModified);
        for (let at = 0; at < item.file.size; at += READ_CHUNK) {
          const chunk = new Uint8Array(await item.file.slice(at, at + READ_CHUNK).arrayBuffer());
          job.writeChunk(chunk);
          done += chunk.length;
          emit({ event: "send-progress", done, total });
        }
        job.endFile(item.file.size);
      }
    }

    const message = job.seal();
    let meta: Uint8Array;
    if (me) {
      const { signature, signer } = await sign(me, message);
      meta = job.finish(signature, signer);
    } else {
      meta = job.finish();
    }
    const info = job.info() as {
      session: string;
      flags: number;
      segShift: number;
      metaLen: number;
      bodyLen: number;
      summary: string;
      encrypted: boolean;
    };
    job.free();

    const segSize = 2 ** info.segShift;
    const read = (index: number): Uint8Array => {
      if (index === 0) return meta;
      const start = (index - 1) * segSize;
      return body.read(start, Math.min(segSize, info.bodyLen - start));
    };
    const session = new SendSession(
      info.session,
      info.flags,
      info.segShift,
      info.metaLen,
      info.bodyLen,
      read,
      req.density?.version ?? 0,
      req.density?.ec ?? "L",
      req.redundancy,
    );
    sending = { session, id: info.session, body, offers: { usual: [], known: [] } };
    const params = session.params() as { version: number; ec: string; modules: number; symbolSize: number };
    logLine("tx", "packed", {
      ms: Math.round(performance.now() - packingSince),
      wireBytes: info.bodyLen + info.metaLen,
      fileListBytes: info.metaLen,
      qr: `v${params.version}-${params.ec}`,
      bytesPerCode: params.symbolSize,
      codesPerPass: session.framesPerPass,
      encrypted: info.encrypted,
      signed: !!me,
      recipients: req.recipients.length,
      storage: store.persistent ? "opfs" : "memory",
    });
    return {
      session: info.session,
      summary: info.summary,
      wireBytes: info.bodyLen + info.metaLen,
      encrypted: info.encrypted,
      recipients: req.recipients,
      signed: !!me,
      framesPerPass: session.framesPerPass,
      ...params,
      quiet: quietModules(),
    };
  } catch (e) {
    body.close();
    await store.remove(SEND_DIR);
    throw e;
  }
}

function sendFrames(count: number): FrameBatch {
  if (!sending) throw new Error("no transfer is being sent");
  const data = sending.session.nextBatch(count);
  const { frames, pass, framesPerPass } = sending.session;
  return { data, count, frames, pass, framesPerPass };
}

const LINK_OFFER = 1;
const LINK_KNOWN_OFFER = 4;

function sendLinkOffer(payload: Uint8Array | null, id: number, known = false): void {
  if (!sending) return;
  const { session, offers } = sending;
  const kind = known ? LINK_KNOWN_OFFER : LINK_OFFER;
  offers[known ? "known" : "usual"] = payload ? (linkSplit(sending.id, kind, id, payload, session.codeChars) as string[]) : [];
  session.setLinkCodes([...offers.usual, ...offers.known]);
}

function sendFeedback(text: string, linkTaken?: number): ReceiverReport | null {
  if (!sending) return null;
  return sending.session.applyFeedback(text, linkTaken, performance.now() / 1000) as ReceiverReport | null;
}

// ---------------------------------------------------------------- receiving

interface RecvSession {
  r: Receive;
  me?: StoredIdentity;
  record?: SessionRecord;
  body?: RandomFile;
  done: Set<number>;
  unverified: number[];
  /** Codes kept of segments still incomplete: one file per segment, a code per line. */
  parts: Map<number, { file: RandomFile; size: number }>;
  lastSave: number;
  /** When the first code of the session was read (for the diagnostic log). */
  lockedAt?: number;
  /** Another transfer seen in place of this one, and how far this one was then (see `RecvState.foreign`). */
  foreign?: string;
  foreignAt?: number;
  already?: boolean;
  notice?: string;
  error?: string;
  result?: RecvResult;
}

let receiving: RecvSession | undefined;
let persistent = true;

const dir = (session: string, name: string) => ["recv", session, name];

async function recvStop(): Promise<void> {
  if (!receiving) return;
  const s = receiving;
  receiving = undefined;
  if (s.record && !s.record.extracted) await saveRecord(s, true);
  s.body?.close();
  for (const part of s.parts.values()) part.file.close();
  s.r.free();
}

// A segment is a megabyte, and several are collected at once: through a
// camera that takes minutes, and a transfer stopped before then would have
// nothing complete to continue from. So the codes of segments still
// incomplete are kept as they arrive (a file per segment, a code per line,
// dropped when the segment completes) and read back in when the transfer is
// continued. Only codes of a size a QR code holds: what a network connection
// brings completes a segment in a moment.
const KEPT_CODE_MAX = 4400;
const lines = new TextEncoder();

async function keepCode(s: RecvSession, segment: number, text: string): Promise<void> {
  if (!s.record || s.done.has(segment) || text.length > KEPT_CODE_MAX) return;
  let part = s.parts.get(segment);
  if (!part) {
    const file = await (await fileStore()).open(dir(s.record.session, `part-${segment}`));
    part = { file, size: file.size() };
    s.parts.set(segment, part);
  }
  const line = lines.encode(`${text}\n`);
  part.file.write(part.size, line);
  part.size += line.length;
}

async function dropKept(s: RecvSession, segment: number): Promise<void> {
  const part = s.parts.get(segment);
  if (!part || !s.record) return;
  s.parts.delete(segment);
  part.file.close();
  await (await fileStore()).remove(dir(s.record.session, `part-${segment}`));
}

/** Takes up the codes kept of incomplete segments again. */
async function restoreKept(s: RecvSession, rec: SessionRecord): Promise<void> {
  const store = await fileStore();
  let codes = 0;
  for (const segment of rec.partial ?? []) {
    if (s.done.has(segment)) continue;
    const file = await store.open(dir(rec.session, `part-${segment}`));
    const size = file.size();
    s.parts.set(segment, { file, size });
    // (A last line cut short by a crash fails its checksum and is passed over.)
    for (const text of new TextDecoder().decode(file.read(0, size)).split("\n")) {
      if (!text) continue;
      s.r.push(text);
      codes++;
    }
  }
  if (codes) logLine("rx", "codes of unfinished parts taken up again", { codes, parts: s.parts.size });
}

async function saveRecord(s: RecvSession, force = false): Promise<void> {
  if (!s.record) return;
  const now = Date.now();
  if (!force && now - s.lastSave < 1000) return;
  s.lastSave = now;
  const rec = s.record;
  if (!rec.extracted) {
    const info = s.r.info() as RecvInfo;
    rec.done = ranges([...s.done]);
    rec.unverified = [...s.unverified];
    rec.partial = [...s.parts.keys()];
    rec.doneCount = s.done.size;
    rec.summary = info.summary ?? rec.summary;
    rec.resumeCode = s.r.resumeCode() ?? undefined;
  }
  rec.updated = now;
  await db.put("sessions", rec);
}

function state(s: RecvSession): RecvState {
  const info = s.record?.extracted ? null : (s.r.info() as RecvInfo);
  return {
    info,
    resumeCode: s.record?.extracted ? undefined : (s.r.resumeCode() ?? undefined),
    notice: s.notice,
    error: s.error,
    result: s.result,
    persistent,
    feedbackBySound: s.r.feedbackBySound,
    foreign: s.foreign,
    already: s.already,
    // "Complete" only once everything is verified and unpacked.
    feedback: s.r.feedback(!!s.result) ?? undefined,
  };
}

/** Decrypts (if needed) and checks the meta segment; records a fatal error if it cannot be used. */
async function openMeta(s: RecvSession, meta: Uint8Array): Promise<boolean> {
  try {
    const flags = (s.r.params() as { flags: number }).flags;
    if (flags & FLAG_ENCRYPTED && s.me?.kind === "webcrypto") {
      for (const key of stanzaKeys(meta) as Uint8Array[]) s.r.addSecret(key, await sharedSecret(s.me, key));
    }
    s.r.openMeta(meta);
    return true;
  } catch (e) {
    s.error = e instanceof Error ? e.message : String(e);
    return false;
  }
}

function segmentAt(s: RecvSession, index: number): Uint8Array {
  const segSize = 2 ** s.record!.segShift;
  return s.body!.read((index - 1) * segSize, s.r.segmentLength(index));
}

/** Checks segments that were stored before the manifest was known. */
function verifyPending(s: RecvSession): void {
  let bad = 0;
  for (const index of s.unverified) {
    if (!s.r.verifySegment(index, segmentAt(s, index))) {
      s.r.resetSegment(index);
      s.done.delete(index);
      bad++;
    }
  }
  s.unverified = [];
  if (bad) s.notice = `${bad} segment(s) failed verification and will be received again.`;
}

async function readWhole(store: FileStore, path: string[]): Promise<Uint8Array> {
  const f = await store.open(path);
  try {
    return f.read(0, f.size());
  } finally {
    f.close();
  }
}

async function resultFromRecord(rec: SessionRecord): Promise<RecvResult> {
  if (rec.kind === "text") {
    const size = rec.textSize ?? 0;
    if (size <= MAX_INLINE_TEXT) {
      const bytes = await readWhole(await fileStore(), dir(rec.session, "out"));
      return { kind: "text", session: rec.session, text: new TextDecoder().decode(bytes.subarray(0, size)) };
    }
    const entry: OutEntry = { path: `qrsend-${rec.session}.txt`, dir: false, size, offset: 0 };
    return { kind: "files", session: rec.session, entries: [entry] };
  }
  return { kind: "files", session: rec.session, entries: rec.entries ?? [] };
}

/** Streams the stored body through decryption, decompression and unpacking into the output file. */
async function extract(s: RecvSession): Promise<RecvResult | undefined> {
  const rec = s.record!;
  const store = await fileStore();
  const outPath = dir(rec.session, "out");
  await store.remove(outPath);
  const out = await store.open(outPath);
  try {
    const info = s.r.info() as RecvInfo;
    if (info.encrypted && s.me?.kind === "webcrypto" && rec.segCount > 0) {
      // The body is its own age file; its header is at the start of segment 1.
      const head = segmentAt(s, 1).subarray(0, 1 << 16);
      for (const key of stanzaKeys(head) as Uint8Array[]) s.r.addSecret(key, await sharedSecret(s.me, key));
    }
    let pos = 0;
    const entries: OutEntry[] = [];
    let current: OutEntry | undefined;
    let textSize: number | undefined;
    const sink = {
      dir(path: string) {
        entries.push({ path, dir: true, size: 0, offset: 0 });
      },
      begin(path: string, size: number, mtime?: number) {
        current = { path, dir: false, size, offset: pos, mtime };
      },
      write(chunk: Uint8Array) {
        out.write(pos, chunk);
        pos += chunk.length;
      },
      end() {
        entries.push(current!);
      },
      text(text: string) {
        const bytes = new TextEncoder().encode(text);
        out.write(0, bytes);
        textSize = bytes.length;
      },
    };
    const kind = s.r.extract((index: number) => segmentAt(s, index), sink) as "text" | "files";
    out.close();

    rec.kind = kind;
    rec.entries = kind === "files" ? entries : undefined;
    rec.textSize = textSize;
    rec.complete = true;
    rec.extracted = true;
    rec.doneCount = rec.total;
    rec.resumeCode = undefined;
    rec.summary = info.summary ?? rec.summary;
    await saveRecord(s, true);
    // The packed body is no longer needed; free the space.
    s.body?.close();
    s.body = undefined;
    await store.remove(dir(rec.session, "body"));
    await store.remove(dir(rec.session, "meta"));
    return await resultFromRecord(rec);
  } catch (e) {
    try {
      out.close();
    } catch {
      /* already closed */
    }
    s.error = e instanceof Error ? e.message : String(e);
    return undefined;
  }
}

async function attachIdentity(s: RecvSession): Promise<void> {
  s.me = await loadIdentity();
  if (s.me?.kind === "legacy") s.r.setIdentity(Identity.parse(s.me.secret));
  else if (s.me) s.r.setPublicKey(s.me.x25519Public);
  for (const d of await trustedDevices()) s.r.addTrusted(d.id, d.name);
}

/** Loads what was stored for a session into a fresh receiver. */
async function resume(s: RecvSession, rec: SessionRecord): Promise<void> {
  const store = await fileStore();
  s.record = rec;
  s.body = await store.open(dir(rec.session, "body"));
  for (const index of parseRanges(rec.done)) {
    s.r.markDone(index);
    s.done.add(index);
  }
  s.unverified = [...rec.unverified];
  if (s.done.has(0)) {
    const meta = await readWhole(store, dir(rec.session, "meta"));
    if (await openMeta(s, meta)) verifyPending(s);
  }
  await restoreKept(s, rec);
}

async function finishIfComplete(s: RecvSession): Promise<void> {
  if (!s.result && !s.error && s.r.isComplete() && s.unverified.length === 0) {
    const since = performance.now();
    logLine("rx", "all here", { s: s.lockedAt ? (since - s.lockedAt) / 1000 : undefined });
    s.result = await extract(s);
    logLine("rx", s.result ? "unpacked" : "unpacking failed", {
      ms: Math.round(performance.now() - since),
      kind: s.result?.kind,
      error: s.error?.slice(0, 200),
    });
  }
}

async function recvStart(session?: string): Promise<RecvState> {
  await ready();
  await recvStop();
  const store = await fileStore();
  persistent = store.persistent;
  const s: RecvSession = { r: new Receive(session), done: new Set(), unverified: [], parts: new Map(), lastSave: 0 };
  receiving = s;
  await attachIdentity(s);
  if (session) {
    const rec = await db.get<SessionRecord>("sessions", session);
    if (!rec) throw new Error("This transfer is no longer in the inbox.");
    if (rec.extracted) {
      s.record = rec;
      s.result = await resultFromRecord(rec);
      return state(s);
    }
    s.r.restore(rec.session, rec.flags, rec.segShift, rec.segCount);
    await resume(s, rec);
    await finishIfComplete(s);
  }
  return state(s);
}

type Pushed = { locked?: string; foreign?: string; rejected: number[]; records: number; kept?: number | null };

async function recvPush(
  texts: string[],
  packed: ArrayBuffer[] = [],
  camera?: { reads: number; dot: number },
): Promise<RecvState> {
  const s = receiving;
  if (!s) throw new Error("not receiving");
  if (camera) s.r.setCamera(camera.reads, camera.dot);
  if (s.result || s.error) {
    // The data is in, but the sender may only now say that it reads
    // feedback: it still has to learn that everything arrived.
    if (s.result) for (const text of texts) if (text.startsWith(NOTICE_PREFIX)) s.r.push(text);
    return state(s);
  }
  s.notice = undefined;
  const store = await fileStore();
  let taken = 0;
  const note = (res: Pushed) => {
    if (res.foreign) {
      s.foreign = res.foreign;
      s.foreignAt = s.r.useful;
      logLine("rx", "another transfer is being shown", { waitingFor: s.record?.session, shown: res.foreign });
    }
    if (res.rejected.length) s.notice = `${res.rejected.length} segment(s) failed verification and will be received again.`;
  };
  const kept: [number, string][] = [];
  const rejected: number[] = [];
  for (const text of texts) {
    const res = s.r.push(text) as Pushed;
    note(res);
    if (res.kept != null) kept.push([res.kept, text]);
    rejected.push(...res.rejected);
  }
  for (const message of packed) {
    const res = s.r.pushPacked(new Uint8Array(message)) as Pushed;
    taken += res.records;
    note(res);
  }
  // Codes of this receiver's own transfer again: the other one is gone.
  if (s.foreign && s.r.useful > (s.foreignAt ?? 0)) {
    s.foreign = undefined;
    logLine("rx", "this transfer is being shown again");
  }
  const done = async (): Promise<RecvState> => ({ ...state(s), taken });
  if (!s.record) {
    const p = s.r.params() as { session: string; flags: number; segShift: number; segCount: number } | null;
    if (!p) return done();
    const saved = await db.get<SessionRecord>("sessions", p.session);
    if (saved?.extracted && saved.segCount === p.segCount && saved.flags === p.flags) {
      // Received in full before (the same data sent again is the same
      // transfer): there is nothing to receive, the copy is in the inbox.
      s.record = saved;
      s.result = await resultFromRecord(saved);
      s.already = true;
      logLine("rx", "this transfer was received before: showing the copy from the inbox");
      return done();
    }
    if (saved && !saved.extracted && saved.segCount === p.segCount && saved.flags === p.flags) {
      // Seen before: continue where it stopped.
      await resume(s, saved);
    } else {
      await store.remove(["recv", p.session]);
      const now = Date.now();
      s.record = {
        ...p,
        created: now,
        updated: now,
        done: "",
        unverified: [],
        doneCount: 0,
        total: p.segCount + 1,
        complete: false,
        extracted: false,
      };
      s.body = await store.open(dir(p.session, "body"));
    }
    s.lockedAt = performance.now();
    logLine("rx", saved && s.record === saved ? "transfer seen before, continuing" : "transfer found", {
      segments: p.segCount,
      encrypted: !!(p.flags & FLAG_ENCRYPTED),
      alreadyHere: s.done.size,
      storage: store.persistent ? "opfs" : "memory",
    });
  }
  const rec = s.record!;
  const segSize = 2 ** rec.segShift;
  let important = false;
  for (const [segment, text] of kept) await keepCode(s, segment, text);
  // What failed verification is collected anew: the codes kept of it are no use.
  for (const segment of rejected) await dropKept(s, segment);
  for (const { index, data } of s.r.takeCompleted() as { index: number; data: Uint8Array }[]) {
    if (s.done.has(index)) continue;
    if (index === 0) {
      const meta = await store.open(dir(rec.session, "meta"));
      meta.write(0, data);
      meta.close();
      s.done.add(0);
      important = true;
      const opened = await openMeta(s, data);
      logLine("rx", opened ? "file list" : "file list unusable", {
        bytes: data.length,
        s: s.lockedAt ? (performance.now() - s.lockedAt) / 1000 : undefined,
        error: opened ? undefined : s.error?.slice(0, 200),
      });
      if (!opened) break;
      verifyPending(s);
    } else {
      s.body!.write((index - 1) * segSize, data);
      s.done.add(index);
      if (!s.r.hasManifest()) s.unverified.push(index);
    }
    await dropKept(s, index);
  }
  await saveRecord(s, important || s.r.isComplete());
  await finishIfComplete(s);
  return done();
}

// -------------------------------------------------------------------- inbox

async function inboxList(): Promise<SessionRecord[]> {
  const all = await db.all<SessionRecord>("sessions");
  return all.sort((a, b) => b.updated - a.updated);
}

async function inboxOpen(session: string): Promise<RecvResult> {
  const rec = await db.get<SessionRecord>("sessions", session);
  if (!rec) throw new Error("This transfer is no longer in the inbox.");
  if (rec.extracted) return resultFromRecord(rec);
  // Fully received but not unpacked yet (e.g. the page closed in between).
  const st = await recvStart(session);
  await recvStop();
  if (st.result) return st.result;
  throw new Error(st.error ?? "This transfer is not complete yet.");
}

async function inboxRemove(session: string): Promise<void> {
  if (receiving?.record?.session === session) {
    receiving.record = undefined;
    await recvStop();
  }
  await (await fileStore()).remove(["recv", session]);
  await db.del("sessions", session);
}

async function outBlob(session: string): Promise<Blob> {
  return (await fileStore()).blob(dir(session, "out"));
}

// ---------------------------------------------------------------------- RPC

const api: EngineApi = {
  sendStart,
  sendFrames: async (count) => sendFrames(count),
  sendStop,
  sendLink: async (count, binary, more) => {
    if (!sending) throw new Error("no transfer is being sent");
    return sending.session.nextLink(count, binary, more) as LinkBatch;
  },
  sendTextChannelUp: async (up) => sending?.session.setTextChannelUp(up),
  sendLinkOffer: async (payload, id, known) => sendLinkOffer(payload, id, known),
  sendAskForFeedback: async (on, bySound) => sending?.session.askForFeedback(on, bySound),
  sendFeedback: async (text, linkTaken) => sendFeedback(text, linkTaken),
  sendReceiverSilent: async (forget) => sending?.session.receiverSilent(forget),
  sendTune: async (levels, scales, fps, level) =>
    sending?.session.setTuner(new Uint32Array(levels), new Float64Array(scales), fps, level, performance.now() / 1000),
  recvStart,
  recvPush,
  recvStop,
  inboxList,
  inboxOpen,
  inboxRemove,
  outBlob,
};

// One call at a time: handlers await storage and crypto, and must not interleave.
let queue: Promise<void> = Promise.resolve();

self.onmessage = (e: MessageEvent<{ id: number; method: keyof EngineApi; args: unknown[] }>) => {
  const { id, method, args } = e.data;
  queue = queue.then(async () => {
    try {
      const result = await (api[method] as (...a: unknown[]) => Promise<unknown>)(...args);
      const data = method === "sendFrames" || method === "sendLink" ? (result as FrameBatch | LinkBatch).data : undefined;
      const transfer = data ? [data.buffer] : [];
      self.postMessage({ id, result }, transfer);
    } catch (err) {
      self.postMessage({ id, error: err instanceof Error ? err.message : String(err) });
    }
  });
};

// Spools left behind by closed tabs are useless. (Removal fails harmlessly
// while another tab is sending; it is retried the next time a tab opens.)
fileStore().then((store) => store.remove(["send"]));

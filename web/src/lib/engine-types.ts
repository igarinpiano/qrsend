// Messages between the page and the engine worker.

export interface SendItem {
  path: string;
  /** Absent for directories. */
  file?: File;
}

export interface SendRequest {
  items: SendItem[];
  text?: string;
  /** Device IDs to encrypt for; empty = unencrypted. */
  recipients: string[];
  /** null = choose from the transfer size. */
  density: { version: number; ec: string } | null;
  redundancy: number;
  /** A resume code read off the receiver (`QSR1-…`): send only what it lists. */
  resume?: string;
}

export interface SendStarted {
  session: string;
  summary: string;
  wireBytes: number;
  encrypted: boolean;
  /** Device IDs the transfer is encrypted for (empty: for anyone). */
  recipients: string[];
  /** With a resume code: how many parts (of how many) are sent. */
  resumed?: { parts: number; of: number };
  signed: boolean;
  framesPerPass: number;
  version: number;
  ec: string;
  /** Modules per side of each code. */
  modules: number;
  symbolSize: number;
  /** Gap between codes, in modules. */
  quiet: number;
}

export interface FrameBatch {
  /** count × modules × modules values, 1 = dark. */
  data: Uint8Array;
  count: number;
  frames: number;
  pass: number;
  /** Codes in one pass over what the receiver still needs. */
  framesPerPass: number;
}

/** Records for a network connection: one binary message, or the same as codes in text form. */
export interface LinkBatch {
  /** Records in this batch; 0 when everything was sent and the receiver's feedback has to tell what else is wanted. */
  count: number;
  data?: Uint8Array;
  texts?: string[];
}

/** What the receiver told the sender through a feedback code. */
export interface ReceiverReport {
  /** The receiver has everything: stop sending. */
  complete: boolean;
  /** Bytes on the wire the receiver still lacks, and in total. */
  remainingBytes: number;
  totalBytes: number;
  /** Distinct codes the receiver has read. */
  frames: number;
  /** With `sendTune`: pictures per second and layout to switch to (when they change). */
  fps?: number | null;
  level?: number | null;
  /** With `sendTune`: the share of the shown codes the receiver read when last measured. */
  readShare?: number | null;
  /** What the receiver's camera makes of the colors: bits 0-2 red, green, blue are read; bits 3-5 red and green, red and blue, green and blue look alike (0: not told). */
  colors?: number;
}

export interface RecvInfo {
  session: string | null;
  encrypted: boolean;
  total: number;
  done: number;
  complete: boolean;
  kind: "text" | "files" | null;
  summary: string | null;
  plain_length: number | null;
  wire_length: number | null;
  sender_name: string | null;
  sender_status: "trusted" | "unverified" | "unsigned" | null;
  sender: string | null;
  entries: { path: string; dir: boolean; size: number }[];
  frames: number;
  useful: number;
  /** Bytes on the wire in total and still missing; null before the first code. */
  total_bytes: number | null;
  remaining_bytes: number | null;
  /** Codes the whole transfer takes (without redundancy), and about how many more it takes to finish. */
  total_codes: number | null;
  remaining_codes: number | null;
  /** While the file list is on its way: codes of it read, and needed. */
  list_have: number | null;
  list_need: number | null;
}

/** A received entry inside the session's output file. */
export interface OutEntry {
  path: string;
  dir: boolean;
  size: number;
  offset: number;
  mtime?: number;
}

export type RecvResult =
  | { kind: "text"; session: string; text: string }
  | { kind: "files"; session: string; entries: OutEntry[] };

export interface RecvState {
  info: RecvInfo | null;
  resumeCode?: string;
  notice?: string;
  error?: string;
  result?: RecvResult;
  /** False when received data is only held in memory (no OPFS). */
  persistent: boolean;
  /** The sender also listens for feedback as sound. */
  feedbackBySound: boolean;
  /** The colors the sender says its codes are stacked in (bits: red, green, blue; 0: black and white), -1: not said. */
  senderColors: number;
  /** Feedback code for the sender (`QSF1-…`), when the sender asked for feedback. */
  feedback?: string;
  /** Records in the binary messages of this push. */
  taken?: number;
  /**
   * Another transfer (its session id) is being shown instead of the one this receiver is on: its codes are passed
   * over. Cleared when codes of this receiver's own transfer arrive again.
   */
  foreign?: string;
  /** The transfer shown was received in full before: `result` is the copy from the inbox. */
  already?: boolean;
}

export interface SessionRecord {
  session: string;
  flags: number;
  segShift: number;
  segCount: number;
  created: number;
  updated: number;
  /** Stored segments as ranges, e.g. "0-5,7". */
  done: string;
  /** Body segments stored before the manifest was known. */
  unverified: number[];
  /** Segments of which some codes are kept (in `part-<n>` files), to be taken up again when continuing. */
  partial?: number[];
  doneCount: number;
  total: number;
  complete: boolean;
  /** The output file has been written; body data is gone. */
  extracted: boolean;
  summary?: string;
  resumeCode?: string;
  kind?: "text" | "files";
  entries?: OutEntry[];
  textSize?: number;
}

export interface EngineApi {
  sendStart(req: SendRequest): Promise<SendStarted>;
  sendFrames(count: number): Promise<FrameBatch>;
  sendStop(): Promise<void>;
  /**
   * Up to `count` records for a network connection (`binary`: as one binary message; otherwise as codes in text
   * form). `more`: send on even though the receiver has not said what it still lacks.
   */
  sendLink(count: number, binary: boolean, more: boolean): Promise<LinkBatch>;
  /**
   * Whether a network connection is carrying `sendLink` records right now. While it does, the codes on the screen
   * start from the end of the transfer, so the two channels bring different parts.
   */
  sendTextChannelUp(up: boolean): Promise<void>;
  /**
   * An offer to connect another way, repeated in the stream until replaced (`null`: none). `known`: the offer to a
   * device connected to before (it travels beside the usual one).
   */
  sendLinkOffer(payload: Uint8Array | null, id: number, known?: boolean | number): Promise<void>;
  /**
   * Whether the stream tells the receiver that this sender reads feedback codes, and whether it also listens for
   * them as sound.
   */
  sendAskForFeedback(on: boolean, bySound: boolean): Promise<void>;
  /** The colors the codes are being stacked in (bit 0 red, 1 green, 2 blue; 0: black and white): the receiver is told. */
  sendColors(colors: number): Promise<void>;
  /**
   * A feedback code from the receiver; null unless it is feedback for this transfer. `linkTaken`: when it came
   * through the network connection, the number of records the receiver had taken in from it by then.
   */
  sendFeedback(text: string, linkTaken?: number): Promise<ReceiverReport | null>;
  /**
   * Lets the engine choose speed and layout from the receiver's feedback. `levels`: codes per picture of every
   * layout the screen offers, fewest first (empty: off); `scales`: the size of a dot on the screen in each of them;
   * `fps` and `level`: the setting in use. `sendFeedback` then reports the setting to switch to.
   */
  sendTune(levels: number[], scales: number[], fps: number, level: number): Promise<void>;
  /**
   * No feedback is being read. `forget` false: for a moment (keep leaving out what the receiver has, stop waiting
   * for its answers); true: for long (assume nothing, send everything again).
   */
  sendReceiverSilent(forget: boolean): Promise<void>;
  recvStart(session?: string): Promise<RecvState>;
  /**
   * Codes read by a camera (text) and messages from a network connection (packed records). `camera`: what the
   * camera makes of the codes at the moment (pictures read per second, camera pixels per dot), for the feedback.
   */
  recvPush(texts: string[], packed?: ArrayBuffer[], camera?: { reads: number; dot: number; colors?: number }): Promise<RecvState>;
  recvStop(): Promise<void>;
  inboxList(): Promise<SessionRecord[]>;
  inboxOpen(session: string): Promise<RecvResult>;
  inboxRemove(session: string): Promise<void>;
  /** The session's output file; entries are byte ranges of it. */
  outBlob(session: string): Promise<Blob>;
}

export type EngineEvent =
  | { event: "send-progress"; done: number; total: number }
  /** Something for the diagnostic log (see `log.ts`). */
  | { event: "log"; area: string; what: string; data?: Record<string, string | number | boolean | null | undefined> };

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
}

export interface SendStarted {
  session: string;
  summary: string;
  wireBytes: number;
  encrypted: boolean;
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
  recvStart(session?: string): Promise<RecvState>;
  recvPush(texts: string[]): Promise<RecvState>;
  recvStop(): Promise<void>;
  inboxList(): Promise<SessionRecord[]>;
  inboxOpen(session: string): Promise<RecvResult>;
  inboxRemove(session: string): Promise<void>;
  /** The session's output file; entries are byte ranges of it. */
  outBlob(session: string): Promise<Blob>;
}

export type EngineEvent = { event: "send-progress"; done: number; total: number };

// Feedback as sound: the receiver's speaker, the sender's microphone.
// Slow (a feedback code takes about a second) but it needs no line of sight,
// so it works when the sender has no camera that sees the receiver's screen.
// The modem itself is in the Rust core (crates/qrsend-core/src/sound.rs).
import { SoundDecoder, feedbackBytes, feedbackCode, ready, soundEncode } from "./core";
import tapUrl from "./tap.worklet.js?url";

const PLAY_RATE = 48000;

/** A feedback code as a WAV file (16-bit mono), or null if `code` is none. */
export async function feedbackWav(code: string): Promise<Blob | null> {
  await ready();
  const bytes = feedbackBytes(code) as Uint8Array | undefined;
  if (!bytes) return null;
  const tone = soundEncode(bytes, PLAY_RATE) as Float32Array;
  // A moment of silence around it: speakers and microphones need it to settle.
  const pad = PLAY_RATE / 10;
  const total = tone.length + 2 * pad;
  const wav = new DataView(new ArrayBuffer(44 + total * 2));
  const ascii = (at: number, s: string) => [...s].forEach((c, i) => wav.setUint8(at + i, c.charCodeAt(0)));
  ascii(0, "RIFF");
  wav.setUint32(4, 36 + total * 2, true);
  ascii(8, "WAVEfmt ");
  wav.setUint32(16, 16, true);
  wav.setUint16(20, 1, true); // PCM
  wav.setUint16(22, 1, true); // mono
  wav.setUint32(24, PLAY_RATE, true);
  wav.setUint32(28, PLAY_RATE * 2, true);
  wav.setUint16(32, 2, true);
  wav.setUint16(34, 16, true);
  ascii(36, "data");
  wav.setUint32(40, total * 2, true);
  for (let i = 0; i < tone.length; i++) {
    wav.setInt16(44 + (pad + i) * 2, Math.round(Math.max(-1, Math.min(1, tone[i])) * 32767), true);
  }
  return new Blob([wav], { type: "audio/wav" });
}

export const canListen = typeof AudioWorkletNode === "function" && !!navigator.mediaDevices?.getUserMedia;

/** Listens through the microphone for feedback codes. */
export class Ear {
  private context?: AudioContext;
  private stream?: MediaStream;
  private turn = 0;

  constructor(private onCode: (code: string) => void) {}

  async start(): Promise<void> {
    this.stop();
    const turn = this.turn;
    await ready();
    // The usual clean-up for speech would damage the tones.
    const stream = await navigator.mediaDevices.getUserMedia({
      video: false,
      audio: { echoCancellation: false, noiseSuppression: false, autoGainControl: false, channelCount: 1 },
    });
    if (turn !== this.turn) {
      stream.getTracks().forEach((t) => t.stop());
      return;
    }
    this.stream = stream;
    const context = new AudioContext();
    this.context = context;
    await context.audioWorklet.addModule(tapUrl);
    if (turn !== this.turn) return;
    const decoder = new SoundDecoder(context.sampleRate);
    const tap = new AudioWorkletNode(context, "tap", { numberOfOutputs: 0 });
    tap.port.onmessage = (e: MessageEvent<Float32Array>) => {
      if (turn !== this.turn) return;
      for (const message of decoder.push(e.data) as Uint8Array[]) {
        const code = feedbackCode(message) as string | undefined;
        if (code) this.onCode(code);
      }
    };
    context.createMediaStreamSource(stream).connect(tap);
    await context.resume();
  }

  stop(): void {
    this.turn++;
    this.stream?.getTracks().forEach((t) => t.stop());
    this.stream = undefined;
    this.context?.close().catch(() => {});
    this.context = undefined;
  }
}

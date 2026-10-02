// Camera capture feeding the decoding worker, one frame at a time.
import ScanWorker from "./scan.worker?worker";

export interface ScanStats {
  frames: number;
  codes: number;
  engine: string;
  width: number;
  height: number;
}

type VideoWithRvfc = HTMLVideoElement & {
  requestVideoFrameCallback?: (cb: () => void) => number;
  cancelVideoFrameCallback?: (id: number) => void;
};

export class Scanner {
  private worker = new ScanWorker();
  private stream?: MediaStream;
  private busy = false;
  private running = false;
  stats: ScanStats = { frames: 0, codes: 0, engine: "…", width: 0, height: 0 };

  constructor(
    private video: VideoWithRvfc,
    private onTexts: (texts: string[]) => void,
    private onStats?: (s: ScanStats) => void,
  ) {
    this.worker.onmessage = (e: MessageEvent<{ texts: string[]; engine: string }>) => {
      this.busy = false;
      this.stats.frames++;
      this.stats.codes += e.data.texts.length;
      this.stats.engine = e.data.engine;
      if (e.data.texts.length) this.onTexts(e.data.texts);
      this.onStats?.({ ...this.stats });
    };
  }

  static async cameras(): Promise<MediaDeviceInfo[]> {
    const all = await navigator.mediaDevices.enumerateDevices();
    return all.filter((d) => d.kind === "videoinput");
  }

  async start(deviceId?: string): Promise<void> {
    this.stopStream();
    const video: MediaTrackConstraints = deviceId
      ? { deviceId: { exact: deviceId } }
      : { facingMode: { ideal: "environment" } };
    this.stream = await navigator.mediaDevices.getUserMedia({
      audio: false,
      video: { ...video, width: { ideal: 1920 }, height: { ideal: 1080 }, frameRate: { ideal: 30 } },
    });
    const track = this.stream.getVideoTracks()[0];
    try {
      // Continuous autofocus where supported.
      await track.applyConstraints({ advanced: [{ focusMode: "continuous" } as MediaTrackConstraintSet] });
    } catch {
      /* not supported */
    }
    this.video.srcObject = this.stream;
    this.video.muted = true;
    this.video.playsInline = true;
    await this.video.play();
    this.running = true;
    this.loop();
  }

  useZxing(force: boolean): void {
    this.worker.postMessage({ zxing: force });
  }

  private loop = (): void => {
    if (!this.running) return;
    const v = this.video;
    if (!this.busy && v.readyState >= 2 && v.videoWidth > 0) {
      this.busy = true;
      this.stats.width = v.videoWidth;
      this.stats.height = v.videoHeight;
      createImageBitmap(v)
        .then((bitmap) => this.worker.postMessage({ bitmap }, [bitmap]))
        .catch(() => (this.busy = false));
    }
    if (v.requestVideoFrameCallback) v.requestVideoFrameCallback(this.loop);
    else requestAnimationFrame(this.loop);
  };

  private stopStream(): void {
    this.stream?.getTracks().forEach((t) => t.stop());
    this.stream = undefined;
  }

  stop(): void {
    this.running = false;
    this.stopStream();
    this.video.srcObject = null;
  }

  dispose(): void {
    this.stop();
    this.worker.terminate();
  }
}

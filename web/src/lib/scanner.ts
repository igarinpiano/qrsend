// Feeds pictures to the decoding worker, one at a time: live from a camera,
// or frame by frame from a video file (a recording of a sender's screen).
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
  captureStream?: () => MediaStream;
  mozCaptureStream?: () => MediaStream;
};

export class Scanner {
  private worker = new ScanWorker();
  private stream?: MediaStream;
  private fileUrl?: string;
  private busy = false;
  private running = false;
  private decoded?: () => void;
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
      this.decoded?.();
    };
  }

  static async cameras(): Promise<MediaDeviceInfo[]> {
    const all = await navigator.mediaDevices.enumerateDevices();
    return all.filter((d) => d.kind === "videoinput");
  }

  async start(deviceId?: string): Promise<void> {
    this.stop();
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

  private loop = (): void => {
    if (!this.running || !this.stream) return;
    const v = this.video;
    if (!this.busy && v.readyState >= 2 && v.videoWidth > 0) this.send(v);
    if (v.requestVideoFrameCallback) v.requestVideoFrameCallback(this.loop);
    else requestAnimationFrame(this.loop);
  };

  private send(v: HTMLVideoElement): void {
    this.busy = true;
    this.stats.width = v.videoWidth;
    this.stats.height = v.videoHeight;
    createImageBitmap(v)
      .then((bitmap) => this.worker.postMessage({ bitmap }, [bitmap]))
      .catch(() => {
        this.busy = false;
        this.decoded?.();
      });
  }

  /**
   * Scans a video file frame by frame (no frame is skipped, however slow the
   * decoding is). Resolves when the end is reached or `stop()` is called.
   */
  async scanFile(file: File, onProgress?: (seconds: number, duration: number) => void): Promise<void> {
    this.stop();
    const v = this.video;
    this.fileUrl = URL.createObjectURL(file);
    v.srcObject = null;
    v.src = this.fileUrl;
    v.muted = true;
    v.playsInline = true;
    await new Promise<void>((resolve, reject) => {
      v.onloadedmetadata = () => resolve();
      v.onerror = () => reject(new Error("This video cannot be played in this browser."));
    });
    const step = 1 / (await this.frameRate());
    this.running = true;
    // Sample the middle of each frame interval.
    for (let t = step / 2; this.running && t < v.duration; t += step) {
      await new Promise<void>((resolve) => {
        v.onseeked = () => resolve();
        v.currentTime = t;
      });
      if (!this.running) break;
      await new Promise<void>((resolve) => {
        this.decoded = resolve;
        this.send(v);
      });
      onProgress?.(t, v.duration);
    }
    this.decoded = undefined;
    this.running = false;
  }

  /** Frames per second of the loaded file, as far as the browser tells. */
  private async frameRate(): Promise<number> {
    try {
      const capture = this.video.captureStream ?? this.video.mozCaptureStream;
      const rate = capture?.call(this.video).getVideoTracks()[0]?.getSettings().frameRate;
      if (rate && rate >= 1 && rate <= 240) return rate;
    } catch {
      /* not supported */
    }
    return 30;
  }

  stop(): void {
    this.running = false;
    this.decoded?.();
    this.decoded = undefined;
    this.stream?.getTracks().forEach((t) => t.stop());
    this.stream = undefined;
    this.video.srcObject = null;
    if (this.fileUrl) {
      this.video.removeAttribute("src");
      this.video.load();
      URL.revokeObjectURL(this.fileUrl);
      this.fileUrl = undefined;
    }
  }

  dispose(): void {
    this.stop();
    this.worker.terminate();
  }
}

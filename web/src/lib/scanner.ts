// Feeds pictures to the decoding worker, one at a time: live from a camera
// or a captured screen, or frame by frame from a video file (a recording of a
// sender's screen).
import ScanWorker from "./scan.worker?worker";

export interface ScanStats {
  frames: number;
  codes: number;
  engine: string;
  width: number;
  height: number;
  /** The stream is being read as color codes. */
  colored?: boolean;
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
  /** Counts stop() calls, so slow async steps notice they were superseded. */
  private turn = 0;
  private decoded?: () => void;
  stats: ScanStats = { frames: 0, codes: 0, engine: "…", width: 0, height: 0 };

  constructor(
    private video: VideoWithRvfc,
    private onTexts: (texts: string[]) => void,
    private onStats?: (s: ScanStats) => void,
  ) {
    this.worker.onmessage = (e: MessageEvent<{ texts: string[]; engine: string; colored?: boolean }>) => {
      this.busy = false;
      this.stats.frames++;
      this.stats.codes += e.data.texts.length;
      this.stats.engine = e.data.engine;
      this.stats.colored = !!e.data.colored;
      if (e.data.texts.length) this.onTexts(e.data.texts);
      this.onStats?.({ ...this.stats });
      this.decoded?.();
    };
  }

  static async cameras(): Promise<MediaDeviceInfo[]> {
    const all = await navigator.mediaDevices.enumerateDevices();
    return all.filter((d) => d.kind === "videoinput");
  }

  /** `facing` is used when no particular camera is asked for. */
  async start(deviceId?: string, facing: "environment" | "user" = "environment"): Promise<void> {
    this.stop();
    // Opening a camera takes a while. If something else took over meanwhile
    // (stop(), another start(), a video file), this call must not touch the
    // video element any more.
    const turn = this.turn;
    const video: MediaTrackConstraints = deviceId
      ? { deviceId: { exact: deviceId } }
      : { facingMode: { ideal: facing } };
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: false,
      video: { ...video, width: { ideal: 1920 }, height: { ideal: 1080 }, frameRate: { ideal: 30 } },
    });
    if (turn !== this.turn) {
      stream.getTracks().forEach((t) => t.stop());
      return;
    }
    this.stream = stream;
    const track = stream.getVideoTracks()[0];
    try {
      // Continuous autofocus where supported.
      await track.applyConstraints({ advanced: [{ focusMode: "continuous" } as MediaTrackConstraintSet] });
    } catch {
      /* not supported */
    }
    await this.show(stream, turn);
  }

  static get canCaptureScreen(): boolean {
    return typeof navigator.mediaDevices?.getDisplayMedia === "function";
  }

  /**
   * Reads codes from a window or screen the user picks (a remote desktop, a
   * virtual machine, a shared screen) instead of a camera. `onEnded` runs
   * when the user stops sharing.
   */
  async startScreen(onEnded?: () => void): Promise<void> {
    this.stop();
    const turn = this.turn;
    const stream = await navigator.mediaDevices.getDisplayMedia({ audio: false, video: { frameRate: { ideal: 30 } } });
    if (turn !== this.turn) {
      stream.getTracks().forEach((t) => t.stop());
      return;
    }
    this.stream = stream;
    stream.getVideoTracks()[0].addEventListener("ended", () => {
      if (this.stream !== stream) return;
      this.stop();
      onEnded?.();
    });
    await this.show(stream, turn);
  }

  /** Plays `stream` in the video element and starts reading it. */
  private async show(stream: MediaStream, turn: number): Promise<void> {
    if (turn !== this.turn) return;
    this.video.srcObject = stream;
    this.video.muted = true;
    this.video.playsInline = true;
    try {
      await this.video.play();
    } catch (e) {
      if (turn !== this.turn) return; // interrupted by whatever took over
      throw e;
    }
    if (turn !== this.turn) return;
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
   * decoding is). Resolves with true at the end of the file, or with false
   * when something else took over (`stop()`, the camera, another file).
   */
  async scanFile(file: File, onProgress?: (seconds: number, duration: number) => void): Promise<boolean> {
    this.stop();
    const turn = this.turn;
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
    if (turn !== this.turn) return false;
    this.running = true;
    // Sample the middle of each frame interval.
    for (let t = step / 2; turn === this.turn && t < v.duration; t += step) {
      await new Promise<void>((resolve) => {
        v.onseeked = () => resolve();
        v.currentTime = t;
      });
      if (turn !== this.turn) return false;
      await new Promise<void>((resolve) => {
        this.decoded = resolve;
        this.send(v);
      });
      onProgress?.(t, v.duration);
    }
    if (turn !== this.turn) return false;
    this.decoded = undefined;
    this.running = false;
    return true;
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
    this.turn++;
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

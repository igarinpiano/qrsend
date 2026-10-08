// Advice for the person holding the receiving camera, from what the decoder
// sees: where the codes are in the picture, how large their dots come out,
// and how sharp the picture is. (A preview feature: "Camera guidance".)
//
// Each piece of advice needs its condition to hold for most of the last
// second or so before it shows, and to be mostly gone before it disappears:
// advice that flickers is worse than none.

/** One code found in a picture: its bounding box in camera pixels, and the length of its text. */
export interface Box {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  chars: number;
}

/** What the decoder saw in one picture. */
export interface Look {
  width: number;
  height: number;
  boxes: Box[];
  /** How much fine detail the middle of the picture has (any unit, comparable between pictures). */
  sharp: number;
}

export type Advice = "closer" | "back" | "fewer" | "blurred" | "lost";

export const ADVICE: Record<Advice, string> = {
  closer: "Move closer: the codes are small in the picture.",
  back: "Move back a little: the codes reach the edge of the picture.",
  fewer: "The codes are very small for this camera. Fewer codes at once on the sender would read better.",
  // Nothing is read and the picture has little detail: blurred, or pointing
  // somewhere else (a wall has little detail too). The picture alone cannot
  // tell which, so the advice names both.
  blurred: "Cannot read the codes: they are out of the picture, or it is blurred. Aim at them and hold still.",
  // Nothing is read though the picture is sharp.
  lost: "Cannot read the codes. Check that they are fully in the picture.",
};

// Characters a code holds at most, by version (1–40), at the lowest error
// correction level in alphanumeric mode. A code of some length is at least
// the first version that holds it (a higher level makes it a little larger,
// which only makes the dots smaller than estimated here).
const CAPACITY = [
  25, 47, 77, 114, 154, 195, 224, 279, 335, 395, 468, 535, 619, 667, 758, 854, 938, 1046, 1153, 1249, 1352, 1460, 1588,
  1704, 1853, 1990, 2132, 2223, 2369, 2520, 2677, 2840, 3009, 3183, 3351, 3537, 3729, 3927, 4087, 4296,
];

/** Dots per side of a code holding `chars` characters, at least. */
export function modulesFor(chars: number): number {
  const version = CAPACITY.findIndex((c) => c >= chars) + 1 || 40;
  return 17 + 4 * version;
}

/** Camera pixels per dot below which reading gets unreliable. */
const SMALL_DOT_PX = 3.5;
/** A code this near the edge (as a share of the picture's size) may have neighbors outside. */
const EDGE = 0.015;
/** Codes spanning less than this share of the picture leave room to come closer. */
const ROOM = 0.6;
/** Sharpness below this share of the best seen counts as blurred. */
const BLURRED = 0.4;
const SHOW = 0.6;
const HIDE = 0.3;
/** How fast a condition's score follows the pictures: about the last eight count. */
const FOLLOW = 1 / 8;

const median = (values: number[]) => [...values].sort((a, b) => a - b)[values.length >> 1];

/** Camera pixels per dot of the codes in a picture (0 when none was located). */
export function dotSize(look: Look): number {
  if (look.boxes.length === 0) return 0;
  return median(look.boxes.map((b) => (b.x1 - b.x0 + (b.y1 - b.y0)) / 2 / modulesFor(b.chars)));
}

export class Guide {
  private scores: Record<Advice, number> = { closer: 0, back: 0, fewer: 0, blurred: 0, lost: 0 };
  private shown: Advice | undefined;
  private sharpest = 0;

  /** Takes one picture into account; returns the advice to show, if any. */
  notice(look: Look): Advice | undefined {
    const now: Record<Advice, boolean> = { closer: false, back: false, fewer: false, blurred: false, lost: false };
    if (look.boxes.length) {
      // Whatever was read was sharp enough to read.
      this.sharpest = Math.max(this.sharpest * 0.995, look.sharp);
      const dot = dotSize(look);
      const x0 = Math.min(...look.boxes.map((b) => b.x0));
      const y0 = Math.min(...look.boxes.map((b) => b.y0));
      const x1 = Math.max(...look.boxes.map((b) => b.x1));
      const y1 = Math.max(...look.boxes.map((b) => b.y1));
      const span = Math.max((x1 - x0) / look.width, (y1 - y0) / look.height);
      const atEdge =
        x0 < look.width * EDGE || y0 < look.height * EDGE || x1 > look.width * (1 - EDGE) || y1 > look.height * (1 - EDGE);
      now.back = atEdge;
      now.closer = !atEdge && dot < SMALL_DOT_PX && span < ROOM;
      now.fewer = !atEdge && dot < SMALL_DOT_PX && span >= ROOM;
    } else if (this.sharpest > 0) {
      // Codes were read before and are not now. (Before the first code there
      // is nothing to say: the camera may not be pointed at anything yet.)
      now.blurred = look.sharp < this.sharpest * BLURRED;
      now.lost = !now.blurred;
    }
    for (const key of Object.keys(now) as Advice[]) {
      this.scores[key] += ((now[key] ? 1 : 0) - this.scores[key]) * FOLLOW;
    }
    if (this.shown && this.scores[this.shown] < HIDE) this.shown = undefined;
    if (!this.shown) {
      this.shown = (Object.keys(now) as Advice[]).find((key) => this.scores[key] > SHOW);
    }
    return this.shown;
  }
}

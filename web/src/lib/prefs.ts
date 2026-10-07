// Feature preview: things that are still being worked on. Each one is off
// until the user turns it on (Feature preview page); sending and receiving
// work the established way otherwise. The choice is kept per browser.

export interface PreviewFeature {
  id: string;
  title: string;
  summary: string;
  /** How to use it, step by step. */
  steps: string[];
}

export const PREVIEW_FEATURES = [
  {
    id: "twoWay",
    title: "Two-way transfer",
    summary:
      "The receiver tells this device what is still missing, through a small code on its screen that this device's camera reads. Lost codes are made up for at once, what has arrived is not sent again, and sending stops by itself when everything has arrived.",
    steps: [
      "Place the two devices so that each camera sees the other screen (for phones: face to face, front cameras).",
      "Start sending as usual. The player opens this device's camera and asks the receiver for feedback.",
      "The receiver needs no setting: it shows the feedback code when asked. If nothing comes back, sending simply continues the usual way.",
    ],
  },
] as const satisfies readonly PreviewFeature[];

export type FeatureId = (typeof PREVIEW_FEATURES)[number]["id"];

const key = (id: FeatureId) => `qrsend.preview.${id}`;

export function featureOn(id: FeatureId): boolean {
  try {
    return localStorage.getItem(key(id)) === "1";
  } catch {
    return false;
  }
}

export function setFeature(id: FeatureId, on: boolean): void {
  try {
    if (on) localStorage.setItem(key(id), "1");
    else localStorage.removeItem(key(id));
  } catch {
    /* storage unavailable: nothing to remember */
  }
}

// Feature preview: things that are still being worked on. Each one is off
// until the user turns it on (Feature preview page); sending and receiving
// work the established way otherwise. The choice is kept per browser.

export interface PreviewFeature {
  id: string;
  title: string;
  /** Which role of this device the switch affects. */
  side: "sending" | "receiving" | "sending and receiving";
  summary: string;
  /** How to use it, step by step. */
  steps: string[];
}

export const PREVIEW_FEATURES = [
  {
    id: "twoWay",
    title: "Two-way transfer",
    side: "sending",
    summary:
      "The receiver tells this device what is still missing, through a small code on its screen that this device's camera reads. Lost codes are made up for at once, what has arrived is not sent again, and sending stops by itself when everything has arrived.",
    steps: [
      "Place the two devices so that each camera sees the other screen (for phones: face to face, front cameras).",
      "Start sending as usual. The player opens this device's camera and asks the receiver for feedback.",
      "The receiver needs no setting: it shows the feedback code when asked. If nothing comes back, sending simply continues the usual way.",
    ],
  },
  {
    id: "autoTune",
    title: "Automatic speed",
    side: "sending",
    summary:
      "With two-way transfer on, the sender finds the fastest setting by itself: it tries more pictures per second or more codes per picture, watches in the receiver's feedback whether more codes are read per second, keeps the change if so and takes it back if not. It keeps adjusting as conditions change (distance, light, a steadier hand). The − and + buttons still work; the search carries on from what you set.",
    steps: [
      "Turn on Two-way transfer as well, and start sending as usual.",
      "Hold the devices so that the feedback code stays in view of this device's camera. Without feedback nothing is changed.",
      "Expect a change every three to four seconds. A try that makes things worse is taken back by itself.",
    ],
  },
  {
    id: "sound",
    title: "Feedback by sound",
    side: "sending",
    summary:
      "Like two-way transfer, but the receiver answers with a short run of soft notes from its speaker, which this device hears through its microphone. No camera has to see the receiver's screen, so it works in the usual position: a phone filming a computer screen. The receiver is asked every time before it makes any sound. A feedback code takes about a second and a half.",
    steps: [
      "Start sending as usual; the browser asks for the microphone.",
      "The receiver is asked whether it may answer by sound. Keep the devices close and the room reasonably quiet.",
      "If nothing is heard, sending simply continues the usual way.",
    ],
  },
  {
    id: "lan",
    title: "Local network boost",
    side: "sending",
    summary:
      "Once each device has seen the other's screen, the two also connect directly over the local network (Wi-Fi or cable), which is far faster than a camera. Both ways are then used at once: the connection carries the transfer from its start while the screen carries it from its end, and the receiver keeps whatever arrives first. The connection is arranged through the codes themselves: no server, no account, and nothing outside the local network is contacted. The receiver connects by itself when it sees the offer (and can end the connection). If the connection fails or drops, the screen simply carries on.",
    steps: [
      "Both devices must be on the same network, and — for a moment — this device's camera must see the receiver's screen.",
      "Start sending as usual. The receiver shows a code as soon as it has seen the offer; hold it up to this device's camera.",
      "Once connected, the devices no longer need to see each other (though it helps: the screen keeps contributing).",
    ],
  },
  {
    id: "color",
    title: "Color codes",
    side: "sending",
    summary:
      "Shows three codes in one, as the red, green and blue parts of the picture: up to three times the data per frame. Receivers notice by themselves. Works best where colors arrive unchanged (screen capture, a good camera held steady and close); a washed-out picture reads fewer codes, never wrong ones.",
    steps: [
      "Start sending as usual; the player shows colored codes.",
      "If the receiver's count of scanned codes grows more slowly than without colors, the camera cannot tell the colors apart well enough: turn this off again.",
    ],
  },
  {
    id: "guide",
    title: "Camera guidance",
    side: "receiving",
    summary:
      "While receiving, tells you how to hold the camera when something is off: move closer (the codes are small in the picture and there is room), move back (the codes reach the edge, so some may be outside), ask for fewer codes (they fill the picture and are still too small), or hold still (the picture is blurred). The advice comes from where the decoder finds the codes, how large their dots come out, and how sharp the picture is compared with a moment ago.",
    steps: [
      "Open Receive and point the camera at the sender's screen as usual.",
      "Advice appears over the camera picture only when something is off, and goes away once it is not.",
    ],
  },
  {
    id: "screenCapture",
    title: "Receive from the screen",
    side: "receiving",
    summary:
      "Reads the codes straight from a window or screen of this computer instead of through a camera — for a sender running in a remote desktop, a virtual machine or a shared screen in a video call. Every pixel arrives as shown, so much denser grids work than with a camera.",
    steps: [
      "On the Receive page, choose “Use the screen” and pick the window that shows the codes.",
      "On the sender, try many codes at once (“Fill the screen”).",
    ],
  },
  {
    id: "stats",
    title: "Show measurements",
    side: "sending and receiving",
    summary:
      "Shows what the transfer spends its time on: how long reading a picture takes (with and without codes in it), how fast received codes are taken in, and — with a local network connection — what holds the sender back. For finding out why something is slower than it should be.",
    steps: ["Start a transfer; the numbers appear under the camera picture and in the sender's status line."],
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

import { QrPlayer, QR_CODE_TRADEMARK } from "@qrsend/web/player";

document.getElementById("trademark").textContent = QR_CODE_TRADEMARK;

const $ = (id) => document.getElementById(id);
const player = new QrPlayer($("codes"), { interval: Number($("interval").value) });

player.onpicture = ({ picture, pictures, count, codesPerRound, paused }) => {
  const where = picture === pictures ? `Picture ${picture}` : `Picture ${picture} of ${pictures}`;
  $("status").textContent = `${where} · ${count} codes made (${codesPerRound} hold everything once)${paused ? " · paused" : ""}`;
  $("toggle").textContent = paused ? "Play" : "Pause";
};

$("send").onclick = async () => {
  const files = $("files").files;
  await player.play(files.length ? files : $("text").value);
};
$("stop").onclick = () => {
  player.stop();
  $("status").textContent = "";
};
$("toggle").onclick = () => player.toggle();
$("back").onclick = () => player.stepBack();
$("forward").onclick = () => player.stepForward();
$("interval").onchange = (e) => {
  const ms = Number(e.target.value);
  if (ms > 0) player.interval = ms;
};

// The keyboard: space to pause or play, arrows to step.
document.addEventListener("keydown", (e) => {
  if (!player.playing || e.target instanceof HTMLInputElement) return;
  if (e.key === " ") player.toggle();
  else if (e.key === "ArrowLeft") player.stepBack();
  else if (e.key === "ArrowRight") player.stepForward();
  else return;
  e.preventDefault();
});

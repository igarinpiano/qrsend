import { QrPlayer, QR_CODE_TRADEMARK } from "@qrsend/web/player";

document.getElementById("trademark").textContent = QR_CODE_TRADEMARK;

const player = new QrPlayer(document.getElementById("codes"), { fps: 10 });
const status = document.getElementById("status");
player.onpicture = ({ count, codesPerRound }) => {
  status.textContent = `${count} codes shown (${codesPerRound} hold everything once)`;
};

document.getElementById("send").onclick = async () => {
  const files = document.getElementById("files").files;
  const text = document.getElementById("text").value;
  await player.play(files.length ? files : text);
};
document.getElementById("stop").onclick = () => player.stop();

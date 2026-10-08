import { PNG } from "pngjs";
import { writeFileSync, mkdirSync } from "node:fs";

const size = 1024;
const png = new PNG({ width: size, height: size });
const bg = [0x0f, 0x11, 0x15];
const disc = [0xff, 0x9f, 0x0a];
const radius = size * 0.22;
const corner = size * 0.2;

function insideRoundedSquare(x: number, y: number): boolean {
  const dx = Math.max(corner - x, 0, x - (size - 1 - corner));
  const dy = Math.max(corner - y, 0, y - (size - 1 - corner));
  return dx * dx + dy * dy <= corner * corner;
}

for (let y = 0; y < size; y++) {
  for (let x = 0; x < size; x++) {
    const i = (y * size + x) * 4;
    if (!insideRoundedSquare(x, y)) {
      png.data[i + 3] = 0;
      continue;
    }
    const d = Math.hypot(x - size / 2, y - size / 2);
    const inDisc = d <= radius;
    const [r, g, b] = inDisc ? disc : bg;
    png.data[i] = r;
    png.data[i + 1] = g;
    png.data[i + 2] = b;
    png.data[i + 3] = 255;
  }
}
mkdirSync("../src-tauri/icons", { recursive: true });
writeFileSync("../src-tauri/icons/source.png", PNG.sync.write(png));
console.log("wrote ../src-tauri/icons/source.png");

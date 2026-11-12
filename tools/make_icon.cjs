// 生成 assets/keyguard.ico（16/32/48/64 四档，32bpp DIB）
// 形状与界面里的 logo 一致：圆角方块渐变底 + 白色盾牌 + 深色对勾
const fs = require("fs");
const path = require("path");

const SIZES = [16, 32, 48, 64];
const SS = 4; // 超采样倍数

const c1 = [34, 224, 180];  // teal
const c2 = [46, 125, 246];  // blue
const ink = [6, 18, 26];    // 对勾颜色

function sdfRoundRect(x, y, w, h, r) {
  const cx = Math.abs(x - w / 2) - (w / 2 - r);
  const cy = Math.abs(y - h / 2) - (h / 2 - r);
  const dx = Math.max(cx, 0), dy = Math.max(cy, 0);
  return Math.min(Math.max(cx, cy), 0) + Math.hypot(dx, dy) - r;
}

// 盾牌：24 单位坐标系里的多边形（上宽下尖）
const SHIELD = [
  [12, 2.6], [19.2, 5.6], [19.2, 12.2], [12, 21.2], [4.8, 12.2], [4.8, 5.6],
];

function inPoly(px, py, poly) {
  let inside = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const [xi, yi] = poly[i], [xj, yj] = poly[j];
    if ((yi > py) !== (yj > py) && px < ((xj - xi) * (py - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}

function distToSegment(px, py, ax, ay, bx, by) {
  const vx = bx - ax, vy = by - ay;
  const wx = px - ax, wy = py - ay;
  const t = Math.max(0, Math.min(1, (wx * vx + wy * vy) / (vx * vx + vy * vy)));
  return Math.hypot(px - (ax + t * vx), py - (ay + t * vy));
}

function render(size) {
  const px = new Uint8Array(size * size * 4);
  const S = size;
  for (let y = 0; y < S; y++) {
    for (let x = 0; x < S; x++) {
      let rSum = 0, gSum = 0, bSum = 0, aSum = 0;
      for (let sy = 0; sy < SS; sy++) {
        for (let sx = 0; sx < SS; sx++) {
          // 采样点（像素中心 + 子像素偏移），换算到 24 单位空间
          const fx = (x + (sx + 0.5) / SS) / S * 24;
          const fy = (y + (sy + 0.5) / SS) / S * 24;

          const d = sdfRoundRect(fx, fy, 24, 24, 6.2);
          if (d > 0) { aSum += 0; continue; }

          // 底：左上 teal → 右下 blue
          const t = Math.max(0, Math.min(1, (fx + fy) / 48));
          let r = c1[0] + (c2[0] - c1[0]) * t;
          let g = c1[1] + (c2[1] - c1[1]) * t;
          let b = c1[2] + (c2[2] - c1[2]) * t;

          // 盾牌描边（多边形内且离边 > 0.9 单位）
          const inShield = inPoly(fx, fy, SHIELD);
          let edge = 99;
          for (let i = 0; i < SHIELD.length; i++) {
            const a = SHIELD[i], c = SHIELD[(i + 1) % SHIELD.length];
            edge = Math.min(edge, distToSegment(fx, fy, a[0], a[1], c[0], c[1]));
          }
          if (inShield && edge > 0.95) { r = 255; g = 255; b = 255; }

          // 对勾：两条粗线段，画在盾牌上
          const d1 = distToSegment(fx, fy, 8.6, 11.9, 11.2, 14.6);
          const d2 = distToSegment(fx, fy, 11.2, 14.6, 15.6, 9.4);
          if (Math.min(d1, d2) < 1.05 && inShield) { r = ink[0]; g = ink[1]; b = ink[2]; }

          // 边缘羽化：SDF 覆盖
          const cov = Math.max(0, Math.min(1, 0.5 - d / (24 / S)));
          rSum += r * cov; gSum += g * cov; bSum += b * cov; aSum += cov;
        }
      }
      const n = SS * SS;
      const a = aSum / n;
      const i = (y * S + x) * 4;
      px[i] = a > 0 ? Math.round(rSum / aSum) : 0;
      px[i + 1] = a > 0 ? Math.round(gSum / aSum) : 0;
      px[i + 2] = a > 0 ? Math.round(bSum / aSum) : 0;
      px[i + 3] = Math.round(a * 255);
    }
  }
  return px;
}

function dib(size, rgba) {
  const header = Buffer.alloc(40);
  header.writeUInt32LE(40, 0);
  header.writeInt32LE(size, 4);
  header.writeInt32LE(size * 2, 8);
  header.writeUInt16LE(1, 12);
  header.writeUInt16LE(32, 14);
  header.writeUInt32LE(0, 16);
  header.writeUInt32LE(size * size * 4, 20);

  const xor = Buffer.alloc(size * size * 4);
  for (let y = 0; y < size; y++) {
    const srcRow = size - 1 - y; // DIB 自下而上
    for (let x = 0; x < size; x++) {
      const s = (srcRow * size + x) * 4;
      const d = (y * size + x) * 4;
      xor[d] = rgba[s + 2];     // B
      xor[d + 1] = rgba[s + 1]; // G
      xor[d + 2] = rgba[s];     // R
      xor[d + 3] = rgba[s + 3]; // A
    }
  }
  const maskRow = Math.ceil(size / 32) * 4;
  const mask = Buffer.alloc(maskRow * size);
  return Buffer.concat([header, xor, mask]);
}

const images = SIZES.map((size) => dib(size, render(size)));

const dir = Buffer.alloc(6);
dir.writeUInt16LE(0, 0);
dir.writeUInt16LE(1, 2);
dir.writeUInt16LE(SIZES.length, 4);

let offset = 6 + 16 * SIZES.length;
const entries = [];
for (let i = 0; i < SIZES.length; i++) {
  const e = Buffer.alloc(16);
  e.writeUInt8(SIZES[i] >= 256 ? 0 : SIZES[i], 0);
  e.writeUInt8(SIZES[i] >= 256 ? 0 : SIZES[i], 1);
  e.writeUInt8(0, 2);
  e.writeUInt8(0, 3);
  e.writeUInt16LE(1, 4);
  e.writeUInt16LE(32, 6);
  e.writeUInt32LE(images[i].length, 8);
  e.writeUInt32LE(offset, 12);
  offset += images[i].length;
  entries.push(e);
}

const out = Buffer.concat([dir, ...entries, ...images]);
const dest = path.join("assets", "keyguard.ico");
fs.mkdirSync("assets", { recursive: true });
fs.writeFileSync(dest, out);
console.log(dest, out.length, "bytes", SIZES.join("/"));

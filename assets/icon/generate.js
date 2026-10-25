#!/usr/bin/env node
// Renders the Process Corral icon set. No dependencies (Node's built-in zlib only).
//
//   node assets/icon/generate.js            writes the files listed below, next to this script
//   node assets/icon/generate.js --preview  also writes preview.png (16 px views on light/dark)
//
// The design (see process-corral.svg): a deep-blue rounded tile, a white ring made of fence rails
// with eight amber fence posts (the corral), and an amber CPU chip with pins inside it.
// It is drawn from scratch with simple shapes and shares nothing with any other product's logo.
//
// Each size is rendered from the vector description, not scaled down from one bitmap, and gets the
// amount of detail that survives at that size:
//   <= 24 px : thick plain ring + solid chip           (readable at 16x16)
//   32-47 px : + fence posts
//   >= 48 px : + chip die and pins
//
// Outputs:
//   process-corral.ico      16 24 32 48 64 128 256 px (BMP entries, 256 px PNG-compressed)
//   tray-running.ico        16 24 32 48 px   design + green status dot
//   tray-stopped.ico        16 24 32 48 px   desaturated design + grey status dot
//   tray-running.png / tray-stopped.png      32 px
//   process-corral-256.png                   the 256 px master

"use strict";
const fs = require("fs");
const path = require("path");
const zlib = require("zlib");

const OUT = __dirname;
const C = {
  tile: [14, 47, 74],
  rim: [70, 134, 186],
  ring: [234, 242, 248],
  amber: [255, 176, 32],
  die: [14, 47, 74],
  green: [46, 213, 115],
  grey: [142, 153, 166],
  outline: [14, 47, 74],
};

// ---- geometry, in a 256 x 256 design space -------------------------------------------------

function roundRectDist(px, py, cx, cy, hx, hy, r) {
  const qx = Math.abs(px - cx) - (hx - r);
  const qy = Math.abs(py - cy) - (hy - r);
  return Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - r;
}

const tier = (size) => (size <= 24 ? 0 : size < 48 ? 1 : 2);

/** Colour of the design at one point (null = transparent). `dot`: null | 'running' | 'stopped'. */
function shade(px, py, t, dot) {
  // Status dot (tray variants): bottom right, outlined so it separates from the artwork. It is tested
  // first so it is never clipped by the tile's rounded corner.
  if (dot) {
    const dd = Math.hypot(px - 206, py - 206);
    if (dd <= 40) return dot === "running" ? C.green : C.grey;
    if (dd <= 50) return C.outline;
  }
  const edge = roundRectDist(px, py, 128, 128, 120, 120, 56);
  if (edge > 0) return null;
  // A lighter rim keeps the dark tile visible on dark backgrounds.
  let col = edge > -(t === 0 ? 11 : 7) ? C.rim : C.tile;

  // Corral ring.
  const ringR = 84;
  const ringT = t === 0 ? 32 : 26;
  const d = Math.hypot(px - 128, py - 128);
  if (Math.abs(d - ringR) <= ringT / 2) col = C.ring;

  // Eight fence posts on the ring.
  if (t >= 1) {
    for (let k = 0; k < 8; k++) {
      const a = (Math.PI / 4) * k + Math.PI / 8;
      const cx = 128 + ringR * Math.cos(a);
      const cy = 128 + ringR * Math.sin(a);
      // post: a small rounded square, rotated with the ring
      const dx = px - cx, dy = py - cy;
      const u = dx * Math.cos(a) + dy * Math.sin(a);
      const v = -dx * Math.sin(a) + dy * Math.cos(a);
      if (roundRectDist(u, v, 0, 0, 15, 15, 5) <= 0) col = C.amber;
    }
  }

  // CPU chip.
  const half = t === 0 ? 44 : 40;
  if (t === 2) {
    // pins: three per side
    for (const off of [-22, 0, 22]) {
      if (roundRectDist(px, py, 128 + off, 128, 7, half + 20, 3) <= 0) col = C.amber;
      if (roundRectDist(px, py, 128, 128 + off, half + 20, 7, 3) <= 0) col = C.amber;
    }
  }
  if (roundRectDist(px, py, 128, 128, half, half, 11) <= 0) col = C.amber;
  if (t === 2 && roundRectDist(px, py, 128, 128, 17, 17, 4) <= 0) col = C.die;

  return col;
}

function desaturate(c, k) {
  const g = 0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2];
  return [0, 1, 2].map((i) => Math.round(c[i] * (1 - k) + g * k));
}

/** RGBA pixels (Buffer, top-down) at `size`, supersampled for clean edges. */
function render(size, dot = null) {
  const t = tier(size);
  const SS = size <= 32 ? 8 : 5;
  const buf = Buffer.alloc(size * size * 4);
  const dim = dot === "stopped";
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let r = 0, g = 0, b = 0, a = 0;
      for (let sy = 0; sy < SS; sy++) {
        for (let sx = 0; sx < SS; sx++) {
          const px = ((x + (sx + 0.5) / SS) / size) * 256;
          const py = ((y + (sy + 0.5) / SS) / size) * 256;
          let c = shade(px, py, t, dot);
          if (!c) continue;
          if (dim && c !== C.grey && c !== C.outline && c !== C.rim) c = desaturate(c, 0.7);
          r += c[0]; g += c[1]; b += c[2]; a += 1;
        }
      }
      const n = SS * SS;
      const i = (y * size + x) * 4;
      if (a > 0) {
        buf[i] = Math.round(r / a);
        buf[i + 1] = Math.round(g / a);
        buf[i + 2] = Math.round(b / a);
        buf[i + 3] = Math.round((255 * a) / n);
      }
    }
  }
  return buf;
}

// ---- encoders ------------------------------------------------------------------------------

function pngChunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "latin1"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(zlib.crc32(body) >>> 0);
  return Buffer.concat([len, body, crc]);
}

function encodePng(rgba, w, h) {
  const raw = Buffer.alloc((w * 4 + 1) * h);
  for (let y = 0; y < h; y++) {
    raw[y * (w * 4 + 1)] = 0; // filter: none
    rgba.copy(raw, y * (w * 4 + 1) + 1, y * w * 4, (y + 1) * w * 4);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8; ihdr[9] = 6; // 8-bit RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk("IHDR", ihdr),
    pngChunk("IDAT", zlib.deflateSync(raw, { level: 9 })),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

/** ICO entry payload as a classic 32-bit BMP (DIB) with an AND mask. */
function encodeDib(rgba, s) {
  const maskRow = Math.ceil(s / 32) * 4;
  const hdr = Buffer.alloc(40);
  hdr.writeUInt32LE(40, 0);
  hdr.writeInt32LE(s, 4);
  hdr.writeInt32LE(s * 2, 8); // height includes the mask
  hdr.writeUInt16LE(1, 12);
  hdr.writeUInt16LE(32, 14);
  hdr.writeUInt32LE(s * s * 4 + maskRow * s, 20);
  const px = Buffer.alloc(s * s * 4);
  for (let y = 0; y < s; y++) {
    for (let x = 0; x < s; x++) {
      const src = ((s - 1 - y) * s + x) * 4; // bottom-up rows
      const dst = (y * s + x) * 4;
      px[dst] = rgba[src + 2]; px[dst + 1] = rgba[src + 1]; px[dst + 2] = rgba[src]; px[dst + 3] = rgba[src + 3];
    }
  }
  return Buffer.concat([hdr, px, Buffer.alloc(maskRow * s)]);
}

function encodeIco(images) {
  // images: [{ size, rgba, png: bool }]
  const head = Buffer.alloc(6);
  head.writeUInt16LE(1, 2);
  head.writeUInt16LE(images.length, 4);
  const payloads = images.map((im) => (im.png ? encodePng(im.rgba, im.size, im.size) : encodeDib(im.rgba, im.size)));
  let offset = 6 + 16 * images.length;
  const entries = images.map((im, i) => {
    const e = Buffer.alloc(16);
    e[0] = im.size >= 256 ? 0 : im.size;
    e[1] = im.size >= 256 ? 0 : im.size;
    e.writeUInt16LE(1, 4);
    e.writeUInt16LE(32, 6);
    e.writeUInt32LE(payloads[i].length, 8);
    e.writeUInt32LE(offset, 12);
    offset += payloads[i].length;
    return e;
  });
  return Buffer.concat([head, ...entries, ...payloads]);
}

// ---- outputs -------------------------------------------------------------------------------

const write = (name, data) => {
  fs.writeFileSync(path.join(OUT, name), data);
  console.log(`${name.padEnd(26)} ${data.length} bytes`);
};

const mainSizes = [16, 24, 32, 48, 64, 128, 256];
write("process-corral.ico", encodeIco(mainSizes.map((s) => ({ size: s, rgba: render(s), png: s === 256 }))));
write("process-corral-256.png", encodePng(render(256), 256, 256));

const traySizes = [16, 24, 32, 48];
for (const [name, dot] of [["running", "running"], ["stopped", "stopped"]]) {
  write(`tray-${name}.ico`, encodeIco(traySizes.map((s) => ({ size: s, rgba: render(s, dot), png: false }))));
  write(`tray-${name}.png`, encodePng(render(32, dot), 32, 32));
}

if (process.argv.includes("--preview")) {
  // 16 px (and 32 px) views, enlarged 8x with nearest-neighbour, on light and dark backgrounds.
  const items = [[16, null], [16, "running"], [16, "stopped"], [32, null], [32, "running"], [32, "stopped"]];
  const Z = 8, cell = 32 * Z + 16, W = cell * items.length, H = cell * 2;
  const sheet = Buffer.alloc(W * H * 4);
  const bgs = [[255, 255, 255], [32, 32, 32]];
  for (let row = 0; row < 2; row++) {
    for (let i = 0; i < items.length; i++) {
      const [s, dot] = items[i];
      const img = render(s, dot);
      for (let y = 0; y < H / 2; y++) {
        for (let x = 0; x < cell; x++) {
          const o = ((row * (H / 2) + y) * W + i * cell + x) * 4;
          const ix = Math.floor((x - 8) / Z), iy = Math.floor((y - 8) / Z);
          let c = bgs[row];
          if (ix >= 0 && iy >= 0 && ix < s && iy < s) {
            const p = (iy * s + ix) * 4, a = img[p + 3] / 255;
            c = [0, 1, 2].map((k) => Math.round(img[p + k] * a + bgs[row][k] * (1 - a)));
          }
          sheet[o] = c[0]; sheet[o + 1] = c[1]; sheet[o + 2] = c[2]; sheet[o + 3] = 255;
        }
      }
    }
  }
  fs.writeFileSync(path.join(process.env.PREVIEW_DIR || OUT, "preview.png"), encodePng(sheet, W, H));
  console.log("preview.png written");
}

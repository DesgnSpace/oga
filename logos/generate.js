#!/usr/bin/env bun
// Renders every raster asset derived from the logo. Run from the repository root.

import { $ } from "bun";
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const SOURCE = "logos/export/logo.svg";
const EXPORT_SIZES = [16, 32, 48, 192, 512, 1024, 2048];
const DESKTOP_ICONS = [
  ["32x32.png", 32],
  ["64x64.png", 64],
  ["128x128.png", 128],
  ["128x128@2x.png", 256],
  ["icon.png", 512],
];
const ICO_SIZES = [16, 24, 32, 48, 64, 256];
const ICNS_SLICES = [
  ["icon_16x16.png", 16],
  ["icon_16x16@2x.png", 32],
  ["icon_32x32.png", 32],
  ["icon_32x32@2x.png", 64],
  ["icon_128x128.png", 128],
  ["icon_128x128@2x.png", 256],
  ["icon_256x256.png", 256],
  ["icon_256x256@2x.png", 512],
  ["icon_512x512.png", 512],
  ["icon_512x512@2x.png", 1024],
];

async function render(size, out) {
  await $`rsvg-convert -w ${size} -h ${size} ${SOURCE} -o ${out}`;
}

async function packIco(out) {
  const scratch = await mkdtemp(join(tmpdir(), "oga-ico-"));
  const frames = [];

  for (const size of ICO_SIZES) {
    const path = join(scratch, `${size}.png`);
    await render(size, path);
    frames.push({ size, png: await Bun.file(path).arrayBuffer() });
  }
  await rm(scratch, { recursive: true });

  const directory = new DataView(new ArrayBuffer(6 + frames.length * 16));
  directory.setUint16(2, 1, true);
  directory.setUint16(4, frames.length, true);

  let offset = directory.byteLength;
  frames.forEach(({ size, png }, index) => {
    const entry = 6 + index * 16;
    // Each dimension gets one byte, so 256 is written as 0.
    directory.setUint8(entry, size % 256);
    directory.setUint8(entry + 1, size % 256);
    directory.setUint16(entry + 4, 1, true);
    directory.setUint16(entry + 6, 32, true);
    directory.setUint32(entry + 8, png.byteLength, true);
    directory.setUint32(entry + 12, offset, true);
    offset += png.byteLength;
  });

  await Bun.write(out, new Blob([directory.buffer, ...frames.map(({ png }) => png)]));
}

async function packIcns(out) {
  const scratch = await mkdtemp(join(tmpdir(), "oga-icns-"));
  const iconset = join(scratch, "icon.iconset");
  await mkdir(iconset);

  for (const [name, size] of ICNS_SLICES) {
    await render(size, join(iconset, name));
  }
  await $`iconutil -c icns ${iconset} -o ${out}`;
  await rm(scratch, { recursive: true });
}

for (const size of EXPORT_SIZES) {
  await render(size, `logos/export/logo-${size}.png`);
}

for (const [name, size] of DESKTOP_ICONS) {
  await render(size, `rust/apps/oga-desktop/icons/${name}`);
}

await packIco("rust/apps/oga-desktop/icons/icon.ico");
await packIcns("rust/apps/oga-desktop/icons/icon.icns");
await Bun.write("landing/logo.svg", Bun.file(SOURCE));

console.log("Logo assets rebuilt.");

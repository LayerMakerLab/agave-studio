// pako's deflate and inflate as Excalidraw calls them (encode.ts), on fflate (MIT), so the bundle carries no Zlib-
// licensed code. deflate makes a zlib stream like pako's default; inflate reads zlib, gzip or raw deflate.
import { zlibSync, decompressSync, strToU8, strFromU8 } from "fflate";

export function deflate(input) {
  return zlibSync(typeof input === "string" ? strToU8(input) : input);
}

export function inflate(input, options) {
  const out = decompressSync(input instanceof Uint8Array ? input : new Uint8Array(input));
  return options && options.to === "string" ? strFromU8(out) : out;
}

export default { deflate, inflate };

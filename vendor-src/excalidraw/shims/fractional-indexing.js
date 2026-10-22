// Copyright 2026 LayerMaker LLC. Licensed under the Apache License, Version 2.0 (see LICENSE at the repository root).
//
// Order keys between two keys, as Excalidraw uses them to order elements ("a0", "a1", "a0V"...). This is Agave's own
// implementation of the same scheme and API as the fractional-indexing package (3.2.0), so the board bundle carries
// only MIT, Apache-2.0, BSD and ISC code. A key is an integer part (a head letter that gives its length, a-z for
// zero and up, A-Z below zero, then base-62 digits) and an optional fraction with no trailing zero. build.mjs checks
// this file against the original package on thousands of inputs before it writes the bundle.

export const BASE_62_DIGITS = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const DIGITS = BASE_62_DIGITS;

function integerLength(head) {
  if (head >= "a" && head <= "z") return head.charCodeAt(0) - 97 + 2;
  if (head >= "A" && head <= "Z") return 90 - head.charCodeAt(0) + 2;
  throw new Error("invalid order key head: " + head);
}

function integerPart(key) {
  const n = integerLength(key[0]);
  if (n > key.length) throw new Error("invalid order key: " + key);
  return key.slice(0, n);
}

function checkInteger(int) {
  if (int.length !== integerLength(int[0])) throw new Error("invalid integer part of order key: " + int);
}

function checkKey(key, digits) {
  if (key === "A" + digits[0].repeat(26)) throw new Error("invalid order key: " + key);
  const int = integerPart(key);
  if (key.slice(int.length).slice(-1) === digits[0]) throw new Error("invalid order key: " + key);
}

// A fraction strictly between a and b (b null: no upper bound); neither may end in the zero digit.
function between(a, b, digits) {
  const zero = digits[0];
  if (b !== null && a >= b) throw new Error(a + " >= " + b);
  if (a.slice(-1) === zero || (b && b.slice(-1) === zero)) throw new Error("trailing zero");
  if (b) {
    let n = 0;
    while ((a[n] || zero) === b[n]) n++;
    if (n > 0) return b.slice(0, n) + between(a.slice(n), b.slice(n), digits);
  }
  const da = a ? digits.indexOf(a[0]) : 0;
  const db = b !== null ? digits.indexOf(b[0]) : digits.length;
  if (db - da > 1) return digits[Math.round(0.5 * (da + db))];
  if (b && b.length > 1) return b.slice(0, 1);
  return digits[da] + between(a.slice(1), null, digits);
}

function increment(int, digits) {
  checkInteger(int);
  const head = int[0];
  const rest = int.slice(1).split("");
  let carry = true;
  for (let i = rest.length - 1; carry && i >= 0; i--) {
    const d = digits.indexOf(rest[i]) + 1;
    if (d === digits.length) rest[i] = digits[0];
    else {
      rest[i] = digits[d];
      carry = false;
    }
  }
  if (!carry) return head + rest.join("");
  if (head === "Z") return "a" + digits[0];
  if (head === "z") return null;
  const next = String.fromCharCode(head.charCodeAt(0) + 1);
  if (next > "a") rest.push(digits[0]);
  else rest.pop();
  return next + rest.join("");
}

function decrement(int, digits) {
  checkInteger(int);
  const head = int[0];
  const rest = int.slice(1).split("");
  let borrow = true;
  for (let i = rest.length - 1; borrow && i >= 0; i--) {
    const d = digits.indexOf(rest[i]) - 1;
    if (d === -1) rest[i] = digits.slice(-1);
    else {
      rest[i] = digits[d];
      borrow = false;
    }
  }
  if (!borrow) return head + rest.join("");
  if (head === "a") return "Z" + digits.slice(-1);
  if (head === "A") return null;
  const next = String.fromCharCode(head.charCodeAt(0) - 1);
  if (next < "Z") rest.push(digits.slice(-1));
  else rest.pop();
  return next + rest.join("");
}

export function generateKeyBetween(a, b, digits = DIGITS) {
  if (a !== null && a !== undefined) checkKey(a, digits);
  else a = null;
  if (b !== null && b !== undefined) checkKey(b, digits);
  else b = null;
  if (a !== null && b !== null && a >= b) throw new Error(a + " >= " + b);
  if (a === null) {
    if (b === null) return "a" + digits[0];
    const ib = integerPart(b);
    const fb = b.slice(ib.length);
    if (ib === "A" + digits[0].repeat(26)) return ib + between("", fb, digits);
    if (ib < b) return ib;
    const lower = decrement(ib, digits);
    if (lower === null) throw new Error("cannot decrement any more");
    return lower;
  }
  if (b === null) {
    const ia = integerPart(a);
    const fa = a.slice(ia.length);
    const higher = increment(ia, digits);
    return higher === null ? ia + between(fa, null, digits) : higher;
  }
  const ia = integerPart(a);
  const fa = a.slice(ia.length);
  const ib = integerPart(b);
  const fb = b.slice(ib.length);
  if (ia === ib) return ia + between(fa, fb, digits);
  const higher = increment(ia, digits);
  if (higher === null) throw new Error("cannot increment any more");
  if (higher < b) return higher;
  return ia + between(fa, null, digits);
}

export function generateNKeysBetween(a, b, n, digits = DIGITS) {
  a = a === undefined ? null : a;
  b = b === undefined ? null : b;
  if (n === 0) return [];
  if (n === 1) return [generateKeyBetween(a, b, digits)];
  if (b === null) {
    let c = generateKeyBetween(a, b, digits);
    const out = [c];
    for (let i = 0; i < n - 1; i++) {
      c = generateKeyBetween(c, b, digits);
      out.push(c);
    }
    return out;
  }
  if (a === null) {
    let c = generateKeyBetween(a, b, digits);
    const out = [c];
    for (let i = 0; i < n - 1; i++) {
      c = generateKeyBetween(a, c, digits);
      out.push(c);
    }
    out.reverse();
    return out;
  }
  const mid = Math.floor(n / 2);
  const c = generateKeyBetween(a, b, digits);
  return [...generateNKeysBetween(a, c, mid, digits), c, ...generateNKeysBetween(c, b, n - mid - 1, digits)];
}

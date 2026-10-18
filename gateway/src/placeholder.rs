//! Test-mode placeholders: a plain picture that says TEST OUTPUT and names the step it stands in for, so a
//! fake result is never mistaken for a real one. Drawn here with a 5x7 pixel font into an uncompressed PNG
//! (no image library, no font file); test mode then scales it up with ffmpeg.

/// 5x7 glyphs, one row per entry, bit 4 is the left pixel.
fn glyph(c: char) -> [u8; 7] {
    match c {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 2, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 17, 25, 21, 19, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [31, 2, 4, 2, 1, 17, 14],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 30, 1, 1, 17, 14],
        '6' => [6, 8, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 2, 12],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        ',' => [0, 0, 0, 0, 12, 4, 8],
        ':' => [0, 12, 12, 0, 12, 12, 0],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '/' => [0, 1, 2, 4, 8, 16, 0],
        '·' => [0, 0, 0, 12, 12, 0, 0],
        '@' => [14, 17, 23, 21, 23, 16, 14],
        '(' => [2, 4, 8, 8, 8, 4, 2],
        ')' => [8, 4, 2, 2, 2, 4, 8],
        '\'' => [4, 4, 8, 0, 0, 0, 0],
        '?' => [14, 17, 1, 2, 4, 0, 4],
        _ => [0; 7],
    }
}

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<[u8; 3]>,
}

impl Canvas {
    fn new(w: usize, h: usize, bg: [u8; 3]) -> Self {
        Canvas { w, h, px: vec![bg; w * h] }
    }

    fn set(&mut self, x: usize, y: usize, c: [u8; 3]) {
        if x < self.w && y < self.h {
            self.px[y * self.w + x] = c;
        }
    }

    /// One line of text centred at row `y` (top), each font pixel drawn `scale` x `scale`.
    fn text(&mut self, y: usize, s: &str, scale: usize, c: [u8; 3]) {
        let chars: Vec<char> = s.chars().map(|ch| ch.to_ascii_uppercase()).collect();
        let width = chars.len() * 6 * scale;
        let x0 = self.w.saturating_sub(width) / 2;
        for (i, ch) in chars.iter().enumerate() {
            for (row, bits) in glyph(*ch).iter().enumerate() {
                for col in 0..5 {
                    if bits & (16 >> col) != 0 {
                        for dy in 0..scale {
                            for dx in 0..scale {
                                self.set(x0 + (i * 6 + col) * scale + dx, y + row * scale + dy, c);
                            }
                        }
                    }
                }
            }
        }
    }

    /// A dashed frame `inset` pixels in from the edge.
    fn dashed_frame(&mut self, inset: usize, c: [u8; 3]) {
        let (w, h) = (self.w, self.h);
        for x in inset..w.saturating_sub(inset) {
            if (x / 6) % 2 == 0 {
                self.set(x, inset, c);
                self.set(x, h - 1 - inset, c);
            }
        }
        for y in inset..h.saturating_sub(inset) {
            if (y / 6) % 2 == 0 {
                self.set(inset, y, c);
                self.set(w - 1 - inset, y, c);
            }
        }
    }
}

/// Words cut into lines of at most `max` characters.
fn wrap(text: &str, max: usize, lines: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > max {
            out.push(std::mem::take(&mut cur));
            if out.len() == lines {
                break;
            }
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.extend(word.chars().take(max));
    }
    if out.len() < lines && !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The placeholder picture for one request: TEST OUTPUT, what it stands in for, and the start of the prompt.
/// `aspect` is "16:9", "9:16" or "1:1"; the picture is small (it is scaled up later).
pub fn picture(aspect: &str, label: &str, detail: &str, prompt: &str) -> Vec<u8> {
    let (w, h) = match aspect {
        "9:16" => (180, 320),
        "1:1" => (240, 240),
        _ => (320, 180),
    };
    let mut c = Canvas::new(w, h, [24, 24, 28]);
    c.dashed_frame(8, [255, 207, 107]);
    let cols = (w - 24) / 6;
    let big = if w >= 300 { 3 } else { 2 };
    let mut y = h / 2 - 44;
    c.text(y, "TEST OUTPUT", big, [255, 207, 107]);
    y += 7 * big + 12;
    for line in wrap(label, (w - 24) / 12, 2) {
        c.text(y, &line, 2, [242, 239, 233]);
        y += 18;
    }
    y += 4;
    c.text(y, &wrap(detail, cols, 1).concat(), 1, [185, 181, 173]);
    y += 14;
    for line in wrap(prompt, cols, 3) {
        c.text(y, &line, 1, [144, 139, 131]);
        y += 10;
    }
    encode(&c)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
}

/// An RGB PNG with stored (uncompressed) deflate blocks.
fn encode(c: &Canvas) -> Vec<u8> {
    let mut raw = Vec::with_capacity((c.w * 3 + 1) * c.h);
    for row in c.px.chunks(c.w) {
        raw.push(0);
        for p in row {
            raw.extend_from_slice(p);
        }
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = b.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + u32::from(x)) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(c.w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(c.h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_valid_pngs_of_the_right_shape() {
        let png = picture("9:16", "Step 3 · Video", "video · kling", "Slow orbit around a loaf");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(&png[16..24], &[0, 0, 0, 180, 0, 0, 1, 64]);
        assert_eq!(crc32(b"IEND"), 0xae42_6082, "the PNG chunk checksum");
        assert_eq!(wrap("one two three four", 9, 2), vec!["one two", "three"]);
    }
}

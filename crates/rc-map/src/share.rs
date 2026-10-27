//! The sharing file, `.racmap` (the launcher's RAC Maker tile reads and writes the same, rerac-launcher
//! `docs/contract.md`): the map's JSON with one more top-level field, `"thumb_png"`, its picture as base64 PNG. A map
//! holds no game assets, only references, so the file is all a second PC needs (with its own extracted game).

use crate::{MapDef, MapError, Result};

/// The `.racmap` text of a map (`map_json` as `MapDef::to_json` writes it) and its picture.
pub fn to_racmap(map_json: &str, png: Option<&[u8]>) -> String {
    let Some(png) = png else { return map_json.to_string() };
    match map_json.find('{') {
        Some(k) => format!("{}{{\n  \"thumb_png\": \"{}\",{}", &map_json[..k], base64_encode(png), &map_json[k + 1..]),
        None => map_json.to_string(),
    }
}

/// A `.racmap` (or a plain map JSON): the map and its picture, if it has a readable one.
pub fn from_racmap(text: &str) -> Result<(MapDef, Option<Vec<u8>>)> {
    let text = text.trim_start_matches('\u{feff}');
    let map = MapDef::parse(text)?;
    let v = crate::json::parse(text).map_err(MapError)?;
    let png = v.get("thumb_png").and_then(|p| p.as_str()).and_then(base64_decode).filter(|b| b.starts_with(b"\x89PNG"));
    Ok((map, png))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() { out.push(B64[(n >> (18 - 6 * k) & 63) as usize] as char); } else { out.push('='); }
        }
    }
    out
}

/// Standard base64 (padding optional, whitespace ignored); None on any other character.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in s.bytes().filter(|b| !b.is_ascii_whitespace()) {
        if b == b'=' { break; }
        let v = B64.iter().position(|&c| c == b)? as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for n in 0..20 {
            let data: Vec<u8> = (0..n).map(|k| (k * 37 + 5) as u8).collect();
            assert_eq!(base64_decode(&base64_encode(&data)).unwrap(), data);
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_decode("TWE="), Some(b"Ma".to_vec()));
    }

    #[test]
    fn racmap_round_trips() {
        let map = MapDef::new_default("Shared", 1);
        let png = b"\x89PNG\r\n\x1a\nrest".to_vec();
        let text = to_racmap(&map.to_json(), Some(&png));
        let (back, pic) = from_racmap(&text).unwrap();
        assert_eq!(back, map);
        assert_eq!(pic, Some(png));
        // A plain map JSON (with a BOM) reads too, without a picture.
        let (plain, none) = from_racmap(&format!("\u{feff}{}", map.to_json())).unwrap();
        assert_eq!((plain, none), (map, None));
    }
}

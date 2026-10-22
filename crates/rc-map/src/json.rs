//! A small JSON reader for map files (RFC 8259 values; numbers as f64). The extractor's writer/reader
//! (`rc-extract/src/export/jsonv.rs`) lives in a crate the engine does not depend on, so maps have their own.

/// A parsed JSON value. Objects keep their key order.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    /// The member `key` of an object (None for a missing key or a non-object).
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> { if let Value::Num(n) = self { Some(*n) } else { None } }
    pub fn as_f32(&self) -> Option<f32> { self.as_f64().map(|n| n as f32) }
    /// An integer-valued number.
    pub fn as_i64(&self) -> Option<i64> { self.as_f64().filter(|n| n.fract() == 0.0 && n.abs() < 9.0e15).map(|n| n as i64) }
    pub fn as_bool(&self) -> Option<bool> { if let Value::Bool(b) = self { Some(*b) } else { None } }
    pub fn as_str(&self) -> Option<&str> { if let Value::Str(s) = self { Some(s) } else { None } }
    pub fn as_arr(&self) -> Option<&[Value]> { if let Value::Arr(a) = self { Some(a) } else { None } }
    /// An array of exactly `N` numbers.
    pub fn as_f32s<const N: usize>(&self) -> Option<[f32; N]> {
        let a = self.as_arr().filter(|a| a.len() == N)?;
        let mut out = [0.0; N];
        for (o, v) in out.iter_mut().zip(a) { *o = v.as_f32()?; }
        Some(out)
    }
}

/// Parses one JSON document (surrounding whitespace allowed, nothing else after it).
pub fn parse(s: &str) -> Result<Value, String> {
    let mut p = P { b: s.as_bytes(), i: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != p.b.len() { return Err(p.err("trailing characters after the value")); }
    Ok(v)
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
}

/// Nesting limit (a map file is a few levels deep).
const MAX_DEPTH: usize = 64;

impl P<'_> {
    fn err(&self, what: &str) -> String {
        let line = 1 + self.b[..self.i.min(self.b.len())].iter().filter(|&&c| c == b'\n').count();
        format!("JSON line {line}: {what}")
    }
    fn ws(&mut self) { while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') { self.i += 1; } }
    fn peek(&self) -> Option<u8> { self.b.get(self.i).copied() }
    fn eat(&mut self, c: u8) -> Result<(), String> {
        if self.peek() == Some(c) { self.i += 1; Ok(()) } else { Err(self.err(&format!("expected '{}'", c as char))) }
    }
    fn lit(&mut self, word: &str, v: Value) -> Result<Value, String> {
        if self.b[self.i..].starts_with(word.as_bytes()) { self.i += word.len(); Ok(v) } else { Err(self.err("unknown literal")) }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH { return Err(self.err("nested too deeply")); }
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b't') => self.lit("true", Value::Bool(true)),
            Some(b'f') => self.lit("false", Value::Bool(false)),
            Some(b'n') => self.lit("null", Value::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            _ => Err(self.err("expected a value")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, String> {
        self.eat(b'{')?;
        let mut m = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') { self.i += 1; return Ok(Value::Obj(m)); }
        loop {
            self.ws();
            let k = self.string()?;
            self.ws();
            self.eat(b':')?;
            self.ws();
            m.push((k, self.value(depth + 1)?));
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => { self.i += 1; return Ok(Value::Obj(m)); }
                _ => return Err(self.err("expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, String> {
        self.eat(b'[')?;
        let mut a = Vec::new();
        self.ws();
        if self.peek() == Some(b']') { self.i += 1; return Ok(Value::Arr(a)); }
        loop {
            self.ws();
            a.push(self.value(depth + 1)?);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => { self.i += 1; return Ok(Value::Arr(a)); }
                _ => return Err(self.err("expected ',' or ']'")),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.b.get(self.i..self.i + 4).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u32::from_str_radix(h, 16).ok());
        let v = h.ok_or_else(|| self.err("bad \\u escape"))?;
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, String> {
        self.eat(b'"')?;
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.i < self.b.len() && !matches!(self.b[self.i], b'"' | b'\\') && self.b[self.i] >= 0x20 { self.i += 1; }
            out.push_str(std::str::from_utf8(&self.b[start..self.i]).map_err(|_| self.err("invalid UTF-8"))?);
            match self.peek() {
                Some(b'"') => { self.i += 1; return Ok(out); }
                Some(b'\\') => {
                    self.i += 1;
                    let c = self.peek().ok_or_else(|| self.err("unterminated escape"))?;
                    self.i += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let mut u = self.hex4()?;
                            // A surrogate pair.
                            if (0xd800..0xdc00).contains(&u) && self.b[self.i..].starts_with(b"\\u") {
                                self.i += 2;
                                let lo = self.hex4()?;
                                u = 0x10000 + ((u - 0xd800) << 10) + (lo.wrapping_sub(0xdc00) & 0x3ff);
                            }
                            out.push(char::from_u32(u).ok_or_else(|| self.err("bad \\u code point"))?);
                        }
                        _ => return Err(self.err("unknown escape")),
                    }
                }
                _ => return Err(self.err("unterminated string")),
            }
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.i;
        while self.i < self.b.len() && matches!(self.b[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') { self.i += 1; }
        let s = std::str::from_utf8(&self.b[start..self.i]).map_err(|_| self.err("bad number"))?;
        s.parse::<f64>().map(Value::Num).map_err(|_| self.err(&format!("bad number {s:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_values() {
        let v = parse(r#" {"a": [1, -2.5e1, true, null], "b": {"c": "x\"é\n"}} "#).unwrap();
        assert_eq!(v.get("a").unwrap().as_arr().unwrap()[1].as_f64(), Some(-25.0));
        assert_eq!(v.get("a").unwrap().as_arr().unwrap()[2].as_bool(), Some(true));
        assert_eq!(v.get("b").unwrap().get("c").unwrap().as_str(), Some("x\"é\n"));
        assert_eq!(parse("[1, 2, 3]").unwrap().as_f32s::<3>(), Some([1.0, 2.0, 3.0]));
    }

    #[test]
    fn rejects_bad_input() {
        for s in ["", "{", "[1,]", "{\"a\" 1}", "1 2", "\"abc", "tru"] { assert!(parse(s).is_err(), "{s:?} parsed"); }
        assert!(parse("[1, 2]").unwrap().as_f32s::<3>().is_none());
    }
}

//! Just enough JSON for the stand-in provider and the reports: a reader for
//! request bodies and a string quoter for output.

use anyhow::{bail, Result};

#[derive(Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn items(&self) -> &[Json] {
        match self {
            Json::Arr(items) => items,
            _ => &[],
        }
    }
}

pub fn parse(text: &str) -> Result<Json> {
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    let value = p.value()?;
    p.ws();
    if p.i != p.s.len() {
        bail!("trailing characters at {}", p.i);
    }
    Ok(value)
}

/// `s` as a JSON string literal.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }

    fn eat(&mut self, word: &str) -> bool {
        let hit = self.s[self.i..].starts_with(word.as_bytes());
        if hit {
            self.i += word.len();
        }
        hit
    }

    fn value(&mut self) -> Result<Json> {
        self.ws();
        Ok(match self.s.get(self.i) {
            Some(b'{') => {
                self.i += 1;
                let mut fields = Vec::new();
                self.ws();
                if !self.eat("}") {
                    loop {
                        self.ws();
                        let key = self.string()?;
                        self.ws();
                        if !self.eat(":") {
                            bail!("expected : at {}", self.i);
                        }
                        fields.push((key, self.value()?));
                        self.ws();
                        if self.eat("}") {
                            break;
                        }
                        if !self.eat(",") {
                            bail!("expected , or }} at {}", self.i);
                        }
                    }
                }
                Json::Obj(fields)
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if !self.eat("]") {
                    loop {
                        items.push(self.value()?);
                        self.ws();
                        if self.eat("]") {
                            break;
                        }
                        if !self.eat(",") {
                            bail!("expected , or ] at {}", self.i);
                        }
                    }
                }
                Json::Arr(items)
            }
            Some(b'"') => Json::Str(self.string()?),
            _ if self.eat("true") => Json::Bool(true),
            _ if self.eat("false") => Json::Bool(false),
            _ if self.eat("null") => Json::Null,
            _ => {
                let start = self.i;
                while self.s.get(self.i).is_some_and(|b| b"+-.eE0123456789".contains(b)) {
                    self.i += 1;
                }
                let n = std::str::from_utf8(&self.s[start..self.i])?;
                match n.parse() {
                    Ok(n) => Json::Num(n),
                    Err(_) => bail!("unexpected input at {start}"),
                }
            }
        })
    }

    fn string(&mut self) -> Result<String> {
        if !self.eat("\"") {
            bail!("expected a string at {}", self.i);
        }
        let mut out = Vec::new();
        loop {
            let Some(&b) = self.s.get(self.i) else { bail!("unterminated string") };
            self.i += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let Some(&e) = self.s.get(self.i) else { bail!("unterminated string") };
                    self.i += 1;
                    let c = match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'u' => {
                            let mut code = self.hex4()?;
                            if (0xd800..0xdc00).contains(&code) && self.eat("\\u") {
                                code = 0x10000 + ((code - 0xd800) << 10) + (self.hex4()? - 0xdc00);
                            }
                            char::from_u32(code).unwrap_or('\u{fffd}')
                        }
                        other => other as char,
                    };
                    out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                }
                _ => out.push(b),
            }
        }
        Ok(String::from_utf8(out)?)
    }

    fn hex4(&mut self) -> Result<u32> {
        let digits = self.s.get(self.i..self.i + 4).and_then(|d| std::str::from_utf8(d).ok());
        let Some(code) = digits.and_then(|d| u32::from_str_radix(d, 16).ok()) else { bail!("bad \\u escape") };
        self.i += 4;
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_chat_request() {
        let body = r#"{"model":"m","stream":true,"messages":[{"role":"user","content":"café 🦀 \"x\""},
            {"role":"tool","content":"ok","n":-1.5e2,"x":[null,false]}]}"#;
        let v = parse(body).unwrap();
        let msgs = v.get("messages").unwrap().items();
        assert_eq!(msgs[0].get("content").unwrap().as_str(), Some("café 🦀 \"x\""));
        assert_eq!(msgs[1].get("n"), Some(&Json::Num(-150.0)));
        assert!(parse("{\"a\":1} x").is_err());
    }

    #[test]
    fn quotes_round_trip() {
        let s = "a\"b\\c\nd\x1b[0m✓";
        assert_eq!(quote(s), r#""a\"b\\c\nd\u001b[0m✓""#);
        assert_eq!(parse(&quote(s)).unwrap().as_str(), Some(s));
    }
}

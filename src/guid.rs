//! A GUID as the plain data Windows uses for power schemes, with parse/format that work anywhere.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    /// `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}` with or without braces, any case.
    pub fn parse(s: &str) -> Option<Guid> {
        let s = s.trim();
        let s = s.strip_prefix('{').map_or(s, |r| r.strip_suffix('}').unwrap_or(r));
        let parts: Vec<&str> = s.split('-').collect();
        let lens = [8, 4, 4, 4, 12];
        if parts.len() != 5 || parts.iter().zip(lens).any(|(p, l)| p.len() != l || !p.bytes().all(|b| b.is_ascii_hexdigit())) {
            return None;
        }
        let data1 = u32::from_str_radix(parts[0], 16).ok()?;
        let data2 = u16::from_str_radix(parts[1], 16).ok()?;
        let data3 = u16::from_str_radix(parts[2], 16).ok()?;
        let tail = format!("{}{}", parts[3], parts[4]);
        let mut data4 = [0u8; 8];
        for (i, b) in data4.iter_mut().enumerate() {
            *b = u8::from_str_radix(&tail[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(Guid { data1, data2, data3, data4 })
    }
}

impl std::fmt::Display for Guid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
            self.data1, self.data2, self.data3, self.data4[0], self.data4[1], self.data4[2], self.data4[3], self.data4[4], self.data4[5], self.data4[6], self.data4[7]
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_formats_and_round_trips() {
        let g = Guid::parse("{8C5E7FDA-E8BF-4A96-9A85-A6E23A8C635C}").unwrap();
        assert_eq!(g.data1, 0x8C5E7FDA);
        assert_eq!(g.data4, [0x9A, 0x85, 0xA6, 0xE2, 0x3A, 0x8C, 0x63, 0x5C]);
        assert_eq!(g.to_string(), "{8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c}");
        assert_eq!(Guid::parse(&g.to_string()), Some(g));
        assert_eq!(Guid::parse("8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c"), Some(g), "braces optional");
    }

    #[test]
    fn rejects_malformed() {
        for bad in ["", "{}", "not-a-guid", "{8C5E7FDA-E8BF-4A96-9A85}", "{8C5E7FDA-E8BF-4A96-9A85-A6E23A8C635}", "{ZC5E7FDA-E8BF-4A96-9A85-A6E23A8C635C}", "{8C5E7FDA-E8BF-4A96-9A85-A6E23A8C635C-00}"] {
            assert_eq!(Guid::parse(bad), None, "{bad}");
        }
    }
}

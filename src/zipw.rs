//! Minimal ZIP (STORE method) writer + CRC-32, for the generated resource
//! pack. Vanilla Minecraft accepts stored entries; atlas PNG is already
//! compressed, so STORE keeps the writer dependency-free and fast.

pub fn crc32(data: &[u8]) -> u32 {
    // Slice-by-one table, computed once.
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *e = c;
        }
        t
    });
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

pub struct ZipWriter {
    out: Vec<u8>,
    entries: Vec<CentralEntry>,
}

struct CentralEntry {
    name: Vec<u8>,
    crc: u32,
    size: u32,
    offset: u32,
}

impl Default for ZipWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl ZipWriter {
    pub fn new() -> Self {
        Self {
            out: Vec::with_capacity(64 * 1024),
            entries: Vec::new(),
        }
    }

    /// Add a file entry (STORE, no compression, no extra fields).
    pub fn add(&mut self, name: &str, data: &[u8]) {
        assert!(self.entries.len() < 65_535, "zip: too many entries");
        assert!(
            !name.contains('\\') && !name.starts_with('/'),
            "zip: unsafe entry name {name}"
        );
        let name_bytes = name.as_bytes();
        let offset = self.out.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;

        // Local file header
        self.out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        self.out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        self.out.extend_from_slice(&0u16.to_le_bytes()); // flags
        self.out.extend_from_slice(&0u16.to_le_bytes()); // method = store
        self.out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        self.out.extend_from_slice(&0x21u16.to_le_bytes()); // mod date (1980-01-01)
        self.out.extend_from_slice(&crc.to_le_bytes());
        self.out.extend_from_slice(&size.to_le_bytes()); // compressed
        self.out.extend_from_slice(&size.to_le_bytes()); // uncompressed
        self.out
            .extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        self.out.extend_from_slice(name_bytes);
        self.out.extend_from_slice(data);

        self.entries.push(CentralEntry {
            name: name_bytes.to_vec(),
            crc,
            size,
            offset,
        });
    }

    pub fn finish(mut self) -> Vec<u8> {
        let cd_start = self.out.len() as u32;
        let count = self.entries.len() as u16;
        for e in &self.entries {
            let n = &e.name;
            self.out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            self.out.extend_from_slice(&20u16.to_le_bytes()); // version made by
            self.out.extend_from_slice(&20u16.to_le_bytes()); // version needed
            self.out.extend_from_slice(&0u16.to_le_bytes()); // flags
            self.out.extend_from_slice(&0u16.to_le_bytes()); // method
            self.out.extend_from_slice(&0u16.to_le_bytes()); // time
            self.out.extend_from_slice(&0x21u16.to_le_bytes()); // date
            self.out.extend_from_slice(&e.crc.to_le_bytes());
            self.out.extend_from_slice(&e.size.to_le_bytes());
            self.out.extend_from_slice(&e.size.to_le_bytes());
            self.out.extend_from_slice(&(n.len() as u16).to_le_bytes());
            self.out.extend_from_slice(&0u16.to_le_bytes()); // extra
            self.out.extend_from_slice(&0u16.to_le_bytes()); // comment
            self.out.extend_from_slice(&0u16.to_le_bytes()); // disk start
            self.out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            self.out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            self.out.extend_from_slice(&e.offset.to_le_bytes());
            self.out.extend_from_slice(n);
        }
        let cd_size = self.out.len() as u32 - cd_start;
        // EOCD
        self.out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes()); // disk
        self.out.extend_from_slice(&0u16.to_le_bytes()); // cd disk
        self.out.extend_from_slice(&count.to_le_bytes());
        self.out.extend_from_slice(&count.to_le_bytes());
        self.out.extend_from_slice(&cd_size.to_le_bytes());
        self.out.extend_from_slice(&cd_start.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes()); // comment len
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn zip_roundtrip_magic() {
        let mut z = ZipWriter::new();
        z.add("pack.mcmeta", b"{\"pack\":{\"pack_format\":97}}");
        z.add("a/b/c.png", &[1, 2, 3, 4, 5]);
        let bytes = z.finish();
        // Local header magic for the first entry.
        assert_eq!(&bytes[0..4], &[0x50, 0x4b, 0x03, 0x04]);
        // EOCD magic at the end (last 22 bytes start with it).
        let n = bytes.len();
        assert_eq!(&bytes[n - 22..n - 18], &[0x50, 0x4b, 0x05, 0x06]);
    }

    #[test]
    fn zip_parse_with_std() {
        // Parse our own output with a tiny reader to be sure the structure is
        // valid enough for Java's ZipInputStream (used by the client).
        let mut z = ZipWriter::new();
        let payload: Vec<u8> = (0..4096).map(|i| (i % 7) as u8).collect();
        z.add("dir/file.bin", &payload);
        z.add("dir2/fff.txt", b"hello clod");
        let bytes = z.finish();

        // Locate EOCD
        let n = bytes.len();
        let eocd = n - 22;
        let count = u16::from_le_bytes([bytes[eocd + 10], bytes[eocd + 11]]) as usize;
        let cd_off = u32::from_le_bytes([
            bytes[eocd + 16],
            bytes[eocd + 17],
            bytes[eocd + 18],
            bytes[eocd + 19],
        ]) as usize;
        let cd_size = u32::from_le_bytes([
            bytes[eocd + 12],
            bytes[eocd + 13],
            bytes[eocd + 14],
            bytes[eocd + 15],
        ]) as usize;
        assert_eq!(count, 2);
        // Walk central directory
        let mut p = cd_off;
        let end = cd_off + cd_size;
        let mut seen = Vec::new();
        while p < end {
            assert_eq!(&bytes[p..p + 4], &[0x50, 0x4b, 0x01, 0x02]);
            let name_len = u16::from_le_bytes([bytes[p + 28], bytes[p + 29]]) as usize;
            let local_off =
                u32::from_le_bytes([bytes[p + 42], bytes[p + 43], bytes[p + 44], bytes[p + 45]])
                    as usize;
            let name = String::from_utf8_lossy(&bytes[p + 46..p + 46 + name_len]).to_string();
            seen.push((name, local_off));
            p += 46 + name_len;
        }
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].0, "dir/file.bin");
        // Verify local header + stored content of the first entry.
        let off = seen[0].1;
        assert_eq!(&bytes[off..off + 4], &[0x50, 0x4b, 0x03, 0x04]);
        let ln = u16::from_le_bytes([bytes[off + 26], bytes[off + 27]]) as usize;
        let data_start = off + 30 + ln;
        assert_eq!(&bytes[data_start..data_start + 4096], &payload[..]);
    }
}

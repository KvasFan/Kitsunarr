//! Torrent identities: the info hash of a magnet link or a `.torrent` file (Sonarr's
//! `MagnetLink.Parse(...).InfoHashes` and `TorrentFileInfoReader.GetHashFromTorrentFile`).
//! Downloads are tracked by this hash, lower-case hexadecimal.

use sha1::{Digest, Sha1};

/// The BitTorrent v1 info hash of a magnet link (`xt=urn:btih:` in hex or base32).
pub fn magnet_info_hash(magnet: &str) -> Option<String> {
    let query = magnet.strip_prefix("magnet:?")?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        if !key.eq_ignore_ascii_case("xt") {
            return None;
        }
        let value = value.replace("%3A", ":").replace("%3a", ":");
        let hash = value
            .strip_prefix("urn:btih:")
            .or_else(|| value.strip_prefix("URN:BTIH:"))?;
        match hash.len() {
            40 if hash.chars().all(|c| c.is_ascii_hexdigit()) => Some(hash.to_ascii_lowercase()),
            32 => data_encoding::BASE32
                .decode(hash.to_ascii_uppercase().as_bytes())
                .ok()
                .filter(|b| b.len() == 20)
                .map(|b| hex(&b)),
            _ => None,
        }
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The end of the bencoded value starting at `pos`.
fn skip(data: &[u8], pos: usize) -> Result<usize, String> {
    match data.get(pos) {
        Some(b'i') => data[pos..]
            .iter()
            .position(|b| *b == b'e')
            .map(|e| pos + e + 1)
            .ok_or_else(|| "unterminated integer".into()),
        Some(b'l') | Some(b'd') => {
            let mut p = pos + 1;
            while data.get(p) != Some(&b'e') {
                if p >= data.len() {
                    return Err("unterminated list or dictionary".into());
                }
                p = skip(data, p)?;
            }
            Ok(p + 1)
        }
        Some(c) if c.is_ascii_digit() => {
            let colon = data[pos..]
                .iter()
                .position(|b| *b == b':')
                .ok_or("invalid string length")?;
            let len: usize = std::str::from_utf8(&data[pos..pos + colon])
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or("invalid string length")?;
            let end = pos + colon + 1 + len;
            if end > data.len() {
                return Err("truncated string".into());
            }
            Ok(end)
        }
        _ => Err(format!("unexpected byte at {pos}")),
    }
}

/// The SHA-1 of the bencoded `info` dictionary of a `.torrent` file.
pub fn torrent_info_hash(data: &[u8]) -> Result<String, String> {
    if data.first() != Some(&b'd') {
        return Err("not a torrent file".into());
    }
    let mut p = 1;
    while data.get(p).is_some_and(|b| *b != b'e') {
        let key_end = skip(data, p)?;
        let colon = data[p..].iter().position(|b| *b == b':').unwrap_or(0);
        let key = &data[p + colon + 1..key_end];
        let value_end = skip(data, key_end)?;
        if key == b"info" {
            return Ok(hex(&Sha1::digest(&data[key_end..value_end])));
        }
        p = value_end;
    }
    Err("the torrent file has no info dictionary".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnet_hashes() {
        let hex = "c9e15763f722f23e98a29decdfae341b98d53056";
        assert_eq!(
            magnet_info_hash(&format!(
                "magnet:?xt=urn:btih:{}&dn=x&tr=udp%3A%2F%2Ft",
                hex.to_uppercase()
            )),
            Some(hex.into())
        );
        // The same hash in base32.
        assert_eq!(
            magnet_info_hash("magnet:?dn=x&xt=urn:btih:ZHQVOY7XELZD5GFCTXWN7LRUDOMNKMCW"),
            Some(hex.into())
        );
        assert_eq!(magnet_info_hash("magnet:?xt=urn:btmh:1220abcd"), None);
        assert_eq!(magnet_info_hash("http://example.org/x.torrent"), None);
    }

    #[test]
    fn torrent_file_hash_is_the_sha1_of_the_info_dictionary() {
        let info =
            b"d6:lengthi12e4:name5:a.mkv12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaae";
        let mut file = b"d8:announce14:http://t/a/b/c4:info".to_vec();
        file.extend_from_slice(info);
        file.push(b'e');
        assert_eq!(torrent_info_hash(&file).unwrap(), hex(&Sha1::digest(info)));
        assert!(torrent_info_hash(b"<html>").is_err());
        assert!(torrent_info_hash(b"d8:announce3:abce").is_err());
    }
}

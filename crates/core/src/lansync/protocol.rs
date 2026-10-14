//! Messages between launchers: a 4-byte big-endian length, then JSON. A file follows its
//! `File` message as zlib-compressed chunks, each length-prefixed, ending with an empty one.

use std::io::{self, Read, Write};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use serde::{Deserialize, Serialize};

use super::manifest::Manifest;
use crate::{Error, Result};

/// TCP port for connections and UDP port for discovery beacons.
pub const PORT: u16 = 47625;
/// Bumped when launchers of different versions could no longer understand each other.
pub const VERSION: u32 = 1;

const MAX_MESSAGE: usize = 64 << 20;
const CHUNK: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// Server, on connect.
    Hello { version: u32, device_id: String, device_name: String, nonce: String },
    /// Client, answering `Hello`.
    Auth { device_id: String, device_name: String, nonce: String, proof: String },
    /// Server: the client proved the same account; this proves it back.
    Welcome { proof: String },
    GetManifest { sync_id: String },
    Manifest { manifest: Manifest },
    GetFile { sync_id: String, path: String },
    /// Followed by the file's chunks.
    File { size: u64 },
    /// Client: this instance changed here, pull it when you can.
    Changed { sync_id: String },
    Ok,
    Error { message: String },
}

pub fn send(w: &mut impl Write, message: &Message) -> Result<()> {
    let json = serde_json::to_vec(message)?;
    w.write_all(&(json.len() as u32).to_be_bytes())?;
    w.write_all(&json)?;
    w.flush()?;
    Ok(())
}

pub fn receive(r: &mut impl Read) -> Result<Message> {
    let len = read_len(r)?;
    if len > MAX_MESSAGE {
        return Err(Error::msg("message too large"));
    }
    let mut json = vec![0; len];
    r.read_exact(&mut json)?;
    Ok(serde_json::from_slice(&json)?)
}

/// Receives a message, turning a peer's `Error` into an error here.
pub fn expect(r: &mut impl Read) -> Result<Message> {
    match receive(r)? {
        Message::Error { message } => Err(Error::msg(message)),
        other => Ok(other),
    }
}

fn read_len(r: &mut impl Read) -> io::Result<usize> {
    let mut len = [0; 4];
    r.read_exact(&mut len)?;
    Ok(u32::from_be_bytes(len) as usize)
}

/// Streams `file` (`size` bytes) after its `File` header.
pub fn send_file(w: &mut impl Write, file: &mut impl Read, size: u64) -> Result<()> {
    send(w, &Message::File { size })?;
    let mut buf = vec![0; CHUNK];
    loop {
        let n = read_full(file, &mut buf)?;
        if n == 0 {
            break;
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&buf[..n])?;
        let packed = encoder.finish()?;
        w.write_all(&(packed.len() as u32).to_be_bytes())?;
        w.write_all(&packed)?;
    }
    w.write_all(&0u32.to_be_bytes())?;
    w.flush()?;
    Ok(())
}

/// Receives a file's chunks into `out`; fails if it isn't `size` bytes.
pub fn receive_file(r: &mut impl Read, out: &mut impl Write, size: u64) -> Result<()> {
    let mut total = 0u64;
    loop {
        let len = read_len(r)?;
        if len == 0 {
            break;
        }
        if len > CHUNK * 2 {
            return Err(Error::msg("file chunk too large"));
        }
        let mut packed = vec![0; len];
        r.read_exact(&mut packed)?;
        // Bounded so a malicious chunk can't decompress into gigabytes.
        let mut decoder = ZlibDecoder::new(&packed[..]).take(CHUNK as u64 + 1);
        let n = io::copy(&mut decoder, out)?;
        total += n;
        if n > CHUNK as u64 || total > size {
            return Err(Error::msg("file is larger than announced"));
        }
    }
    if total != size {
        return Err(Error::msg("file ended early"));
    }
    Ok(())
}

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip() {
        let mut wire = Vec::new();
        let msg = Message::GetFile { sync_id: "s".into(), path: ".minecraft/servers.dat".into() };
        send(&mut wire, &msg).unwrap();
        send(&mut wire, &Message::Error { message: "busy".into() }).unwrap();
        let mut r = &wire[..];
        assert_eq!(receive(&mut r).unwrap(), msg);
        assert_eq!(expect(&mut r).unwrap_err().to_string(), "busy");
    }

    #[test]
    fn files_round_trip() {
        let data: Vec<u8> = (0..(CHUNK * 2 + 123)).map(|i| (i % 251) as u8).collect();
        let mut wire = Vec::new();
        send_file(&mut wire, &mut &data[..], data.len() as u64).unwrap();
        let mut r = &wire[..];
        let Message::File { size } = receive(&mut r).unwrap() else { panic!() };
        let mut out = Vec::new();
        receive_file(&mut r, &mut out, size).unwrap();
        assert_eq!(out, data);
        assert!(r.is_empty());
    }

    #[test]
    fn short_file_is_an_error() {
        let mut wire = Vec::new();
        send_file(&mut wire, &mut &b"abc"[..], 3).unwrap();
        let mut r = &wire[..];
        receive(&mut r).unwrap();
        assert!(receive_file(&mut r, &mut Vec::new(), 4).is_err());
    }
}

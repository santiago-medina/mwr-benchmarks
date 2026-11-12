#![forbid(unsafe_code)]
// A safe-Rust framed request/response protocol for a future KVM backend.
// The transport is generic: a child process now, a VM-backed adapter later.
use std::io::{self, Read, Write};

pub const MAGIC: [u8; 4] = *b"MWR1";
pub const REQUEST_SIZE: usize = 16;
pub const RESPONSE_SIZE: usize = 12;
pub const MAX_BATCH: u32 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request { pub batch: u32, pub seed: u64 }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Response { pub value: u64 }

impl Request {
    pub fn encode(self) -> io::Result<[u8; REQUEST_SIZE]> {
        if self.batch == 0 || self.batch > MAX_BATCH {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid batch"));
        }
        let mut buf = [0; REQUEST_SIZE];
        buf[..4].copy_from_slice(&MAGIC);
        buf[4..8].copy_from_slice(&self.batch.to_le_bytes());
        buf[8..].copy_from_slice(&self.seed.to_le_bytes());
        Ok(buf)
    }
    pub fn decode(buf: [u8; REQUEST_SIZE]) -> io::Result<Self> {
        if buf[..4] != MAGIC { return Err(io::Error::new(io::ErrorKind::InvalidData, "bad request magic")); }
        let batch = u32::from_le_bytes(buf[4..8].try_into().unwrap());
        if batch == 0 || batch > MAX_BATCH {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid batch"));
        }
        Ok(Self { batch, seed: u64::from_le_bytes(buf[8..].try_into().unwrap()) })
    }
}
impl Response {
    pub fn encode(self) -> [u8; RESPONSE_SIZE] {
        let mut buf = [0; RESPONSE_SIZE];
        buf[..4].copy_from_slice(&MAGIC);
        buf[4..].copy_from_slice(&self.value.to_le_bytes());
        buf
    }
    pub fn decode(buf: [u8; RESPONSE_SIZE]) -> io::Result<Self> {
        if buf[..4] != MAGIC { return Err(io::Error::new(io::ErrorKind::InvalidData, "bad response magic")); }
        Ok(Self { value: u64::from_le_bytes(buf[4..].try_into().unwrap()) })
    }
}
pub fn send_request<W: Write>(writer: &mut W, req: Request) -> io::Result<()> {
    writer.write_all(&req.encode()?)?;
    writer.flush()
}
pub fn recv_request<R: Read>(reader: &mut R) -> io::Result<Request> {
    let mut buf = [0; REQUEST_SIZE];
    reader.read_exact(&mut buf)?;
    Request::decode(buf)
}
pub fn send_response<W: Write>(writer: &mut W, response: Response) -> io::Result<()> {
    writer.write_all(&response.encode())?;
    writer.flush()
}
pub fn recv_response<R: Read>(reader: &mut R) -> io::Result<Response> {
    let mut buf = [0; RESPONSE_SIZE];
    reader.read_exact(&mut buf)?;
    Response::decode(buf)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn request_roundtrip() {
        let req = Request { batch: 256, seed: u64::MAX };
        assert_eq!(Request::decode(req.encode().unwrap()).unwrap(), req);
    }
    #[test] fn response_roundtrip() {
        let res = Response { value: 42 };
        assert_eq!(Response::decode(res.encode()).unwrap(), res);
    }
    #[test] fn reject_corrupt_magic() {
        let mut b = Request { batch: 1, seed: 7 }.encode().unwrap();
        b[0] = 0;
        assert!(Request::decode(b).is_err());
    }
    #[test] fn reject_invalid_batch() {
        assert!(Request { batch: 0, seed: 1 }.encode().is_err());
        assert!(Request { batch: MAX_BATCH + 1, seed: 1 }.encode().is_err());
    }
    #[test] fn truncated_request() {
        assert_eq!(recv_request(&mut &b"MWR1"[..]).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }
}

//! Cassandra Native Protocol Frame
//!
//! Frame format (v4):
//! ```
//!   0         8        16        24        32         40
//!   +---------+---------+---------+---------+---------+
//!   | version |  flags  |      stream       | opcode  |
//!   +---------+---------+---------+---------+---------+
//!   |                length                           |
//!   +---------+---------+---------+---------+---------+
//!   |                body                ...          |
//!   +---------+---------+---------+---------+---------+
//! ```

use bytes::{Bytes, BytesMut, Buf, BufMut};
use std::io::{self, Cursor};

/// 프로토콜 버전
pub const PROTOCOL_VERSION_V4: u8 = 0x04;
pub const PROTOCOL_VERSION_V4_RESPONSE: u8 = 0x84; // 0x80 | 0x04

/// 프레임 헤더 크기
pub const FRAME_HEADER_SIZE: usize = 9;

/// Opcode 정의
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Opcode {
    Error = 0x00,
    Startup = 0x01,
    Ready = 0x02,
    Authenticate = 0x03,
    Options = 0x05,
    Supported = 0x06,
    Query = 0x07,
    Result = 0x08,
    Prepare = 0x09,
    Execute = 0x0A,
    Register = 0x0B,
    Event = 0x0C,
    Batch = 0x0D,
    AuthChallenge = 0x0E,
    AuthResponse = 0x0F,
    AuthSuccess = 0x10,
}

impl TryFrom<u8> for Opcode {
    type Error = io::Error;
    
    fn try_from(value: u8) -> Result<Self, io::Error> {
        match value {
            0x00 => Ok(Opcode::Error),
            0x01 => Ok(Opcode::Startup),
            0x02 => Ok(Opcode::Ready),
            0x03 => Ok(Opcode::Authenticate),
            0x05 => Ok(Opcode::Options),
            0x06 => Ok(Opcode::Supported),
            0x07 => Ok(Opcode::Query),
            0x08 => Ok(Opcode::Result),
            0x09 => Ok(Opcode::Prepare),
            0x0A => Ok(Opcode::Execute),
            0x0B => Ok(Opcode::Register),
            0x0C => Ok(Opcode::Event),
            0x0D => Ok(Opcode::Batch),
            0x0E => Ok(Opcode::AuthChallenge),
            0x0F => Ok(Opcode::AuthResponse),
            0x10 => Ok(Opcode::AuthSuccess),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, format!("Unknown opcode: {}", value))),
        }
    }
}

/// 프레임 플래그
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameFlags(u8);

impl FrameFlags {
    pub const COMPRESSION: u8 = 0x01;
    pub const TRACING: u8 = 0x02;
    pub const CUSTOM_PAYLOAD: u8 = 0x04;
    pub const WARNING: u8 = 0x08;
    
    pub fn new(value: u8) -> Self {
        Self(value)
    }
    
    pub fn is_compressed(&self) -> bool {
        self.0 & Self::COMPRESSION != 0
    }
    
    pub fn has_tracing(&self) -> bool {
        self.0 & Self::TRACING != 0
    }
    
    pub fn has_custom_payload(&self) -> bool {
        self.0 & Self::CUSTOM_PAYLOAD != 0
    }
    
    pub fn has_warning(&self) -> bool {
        self.0 & Self::WARNING != 0
    }
    
    pub fn value(&self) -> u8 {
        self.0
    }
}

/// 프레임 헤더
#[derive(Debug, Clone)]
pub struct FrameHeader {
    pub version: u8,
    pub flags: FrameFlags,
    pub stream: i16,
    pub opcode: Opcode,
    pub length: u32,
}

impl FrameHeader {
    pub fn new(stream: i16, opcode: Opcode, length: u32) -> Self {
        Self {
            version: PROTOCOL_VERSION_V4_RESPONSE,
            flags: FrameFlags::default(),
            stream,
            opcode,
            length,
        }
    }
    
    pub fn is_request(&self) -> bool {
        self.version & 0x80 == 0
    }
    
    pub fn is_response(&self) -> bool {
        self.version & 0x80 != 0
    }
    
    /// 헤더를 바이트로 인코딩
    pub fn encode(&self, buf: &mut BytesMut) {
        buf.put_u8(self.version);
        buf.put_u8(self.flags.value());
        buf.put_i16(self.stream);
        buf.put_u8(self.opcode as u8);
        buf.put_u32(self.length);
    }
    
    /// 바이트에서 헤더 디코딩
    pub fn decode(buf: &mut Cursor<&[u8]>) -> io::Result<Self> {
        if buf.remaining() < FRAME_HEADER_SIZE {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Not enough bytes for header"));
        }
        
        let version = buf.get_u8();
        let flags = FrameFlags::new(buf.get_u8());
        let stream = buf.get_i16();
        let opcode = Opcode::try_from(buf.get_u8())?;
        let length = buf.get_u32();
        
        Ok(Self {
            version,
            flags,
            stream,
            opcode,
            length,
        })
    }
}

/// 전체 프레임
#[derive(Debug, Clone)]
pub struct Frame {
    pub header: FrameHeader,
    pub body: Bytes,
}

impl Frame {
    pub fn new(stream: i16, opcode: Opcode, body: Bytes) -> Self {
        let header = FrameHeader::new(stream, opcode, body.len() as u32);
        Self { header, body }
    }
    
    /// 프레임을 바이트로 인코딩
    pub fn encode(&self) -> BytesMut {
        let mut buf = BytesMut::with_capacity(FRAME_HEADER_SIZE + self.body.len());
        self.header.encode(&mut buf);
        buf.extend_from_slice(&self.body);
        buf
    }
    
    /// 바이트에서 프레임 디코딩
    pub fn decode(buf: &[u8]) -> io::Result<Self> {
        let mut cursor = Cursor::new(buf);
        let header = FrameHeader::decode(&mut cursor)?;
        
        if cursor.remaining() < header.length as usize {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Not enough bytes for body"));
        }
        
        let body = Bytes::copy_from_slice(&buf[FRAME_HEADER_SIZE..FRAME_HEADER_SIZE + header.length as usize]);
        
        Ok(Self { header, body })
    }
}

/// Result 타입
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ResultKind {
    Void = 0x0001,
    Rows = 0x0002,
    SetKeyspace = 0x0003,
    Prepared = 0x0004,
    SchemaChange = 0x0005,
}

/// Error 코드
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ErrorCode {
    ServerError = 0x0000,
    ProtocolError = 0x000A,
    AuthenticationError = 0x0100,
    Unavailable = 0x1000,
    Overloaded = 0x1001,
    IsBootstrapping = 0x1002,
    TruncateError = 0x1003,
    WriteTimeout = 0x1100,
    ReadTimeout = 0x1200,
    ReadFailure = 0x1300,
    FunctionFailure = 0x1400,
    WriteFailure = 0x1500,
    SyntaxError = 0x2000,
    Unauthorized = 0x2100,
    Invalid = 0x2200,
    ConfigError = 0x2300,
    AlreadyExists = 0x2400,
    Unprepared = 0x2500,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_frame_header_encode_decode() {
        let header = FrameHeader::new(1, Opcode::Ready, 0);
        let mut buf = BytesMut::new();
        header.encode(&mut buf);
        
        let mut cursor = Cursor::new(&buf[..]);
        let decoded = FrameHeader::decode(&mut cursor).unwrap();
        
        assert_eq!(decoded.version, PROTOCOL_VERSION_V4_RESPONSE);
        assert_eq!(decoded.stream, 1);
        assert_eq!(decoded.opcode, Opcode::Ready);
        assert_eq!(decoded.length, 0);
    }
    
    #[test]
    fn test_frame_encode_decode() {
        let body = Bytes::from_static(b"test body");
        let frame = Frame::new(42, Opcode::Query, body.clone());
        
        let encoded = frame.encode();
        let decoded = Frame::decode(&encoded).unwrap();
        
        assert_eq!(decoded.header.stream, 42);
        assert_eq!(decoded.header.opcode, Opcode::Query);
        assert_eq!(decoded.body, body);
    }
}

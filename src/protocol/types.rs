//! CQL 데이터 타입 직렬화/역직렬화

use bytes::{Bytes, BytesMut, Buf, BufMut};
use std::io::{self, Cursor};
use std::collections::HashMap;
use crate::schema::CassandraValue;

/// CQL 데이터 타입 ID
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum CqlType {
    Custom = 0x0000,
    Ascii = 0x0001,
    Bigint = 0x0002,
    Blob = 0x0003,
    Boolean = 0x0004,
    Counter = 0x0005,
    Decimal = 0x0006,
    Double = 0x0007,
    Float = 0x0008,
    Int = 0x0009,
    Timestamp = 0x000B,
    Uuid = 0x000C,
    Varchar = 0x000D,
    Varint = 0x000E,
    Timeuuid = 0x000F,
    Inet = 0x0010,
    Date = 0x0011,
    Time = 0x0012,
    Smallint = 0x0013,
    Tinyint = 0x0014,
    Duration = 0x0015,
    List = 0x0020,
    Map = 0x0021,
    Set = 0x0022,
    Udt = 0x0030,
    Tuple = 0x0031,
}

impl TryFrom<u16> for CqlType {
    type Error = io::Error;
    
    fn try_from(value: u16) -> Result<Self, io::Error> {
        match value {
            0x0000 => Ok(CqlType::Custom),
            0x0001 => Ok(CqlType::Ascii),
            0x0002 => Ok(CqlType::Bigint),
            0x0003 => Ok(CqlType::Blob),
            0x0004 => Ok(CqlType::Boolean),
            0x0005 => Ok(CqlType::Counter),
            0x0006 => Ok(CqlType::Decimal),
            0x0007 => Ok(CqlType::Double),
            0x0008 => Ok(CqlType::Float),
            0x0009 => Ok(CqlType::Int),
            0x000B => Ok(CqlType::Timestamp),
            0x000C => Ok(CqlType::Uuid),
            0x000D => Ok(CqlType::Varchar),
            0x000E => Ok(CqlType::Varint),
            0x000F => Ok(CqlType::Timeuuid),
            0x0010 => Ok(CqlType::Inet),
            0x0011 => Ok(CqlType::Date),
            0x0012 => Ok(CqlType::Time),
            0x0013 => Ok(CqlType::Smallint),
            0x0014 => Ok(CqlType::Tinyint),
            0x0015 => Ok(CqlType::Duration),
            0x0020 => Ok(CqlType::List),
            0x0021 => Ok(CqlType::Map),
            0x0022 => Ok(CqlType::Set),
            0x0030 => Ok(CqlType::Udt),
            0x0031 => Ok(CqlType::Tuple),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, format!("Unknown CQL type: {}", value))),
        }
    }
}

/// [short] 읽기 (2 bytes, big-endian)
pub fn read_short(buf: &mut Cursor<&[u8]>) -> io::Result<u16> {
    if buf.remaining() < 2 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Not enough bytes"));
    }
    Ok(buf.get_u16())
}

/// [short] 쓰기
pub fn write_short(buf: &mut BytesMut, value: u16) {
    buf.put_u16(value);
}

/// [int] 읽기 (4 bytes, big-endian)
pub fn read_int(buf: &mut Cursor<&[u8]>) -> io::Result<i32> {
    if buf.remaining() < 4 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Not enough bytes"));
    }
    Ok(buf.get_i32())
}

/// [int] 쓰기
pub fn write_int(buf: &mut BytesMut, value: i32) {
    buf.put_i32(value);
}

/// [long] 읽기 (8 bytes, big-endian)
pub fn read_long(buf: &mut Cursor<&[u8]>) -> io::Result<i64> {
    if buf.remaining() < 8 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Not enough bytes"));
    }
    Ok(buf.get_i64())
}

/// [long] 쓰기
pub fn write_long(buf: &mut BytesMut, value: i64) {
    buf.put_i64(value);
}

/// [string] 읽기 (2-byte length prefix + UTF-8 bytes)
pub fn read_string(buf: &mut Cursor<&[u8]>) -> io::Result<String> {
    let len = read_short(buf)? as usize;
    if buf.remaining() < len {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Not enough bytes for string"));
    }
    
    let pos = buf.position() as usize;
    let slice = &buf.get_ref()[pos..pos + len];
    buf.advance(len);
    
    String::from_utf8(slice.to_vec())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// [string] 쓰기
pub fn write_string(buf: &mut BytesMut, s: &str) {
    write_short(buf, s.len() as u16);
    buf.extend_from_slice(s.as_bytes());
}

/// [long string] 읽기 (4-byte length prefix + UTF-8 bytes)
pub fn read_long_string(buf: &mut Cursor<&[u8]>) -> io::Result<String> {
    let len = read_int(buf)? as usize;
    if buf.remaining() < len {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Not enough bytes for long string"));
    }
    
    let pos = buf.position() as usize;
    let slice = &buf.get_ref()[pos..pos + len];
    buf.advance(len);
    
    String::from_utf8(slice.to_vec())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// [long string] 쓰기
pub fn write_long_string(buf: &mut BytesMut, s: &str) {
    write_int(buf, s.len() as i32);
    buf.extend_from_slice(s.as_bytes());
}

/// [bytes] 읽기 (4-byte length prefix + bytes, -1 = null)
pub fn read_bytes(buf: &mut Cursor<&[u8]>) -> io::Result<Option<Bytes>> {
    let len = read_int(buf)?;
    if len < 0 {
        return Ok(None);
    }
    
    let len = len as usize;
    if buf.remaining() < len {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Not enough bytes"));
    }
    
    let pos = buf.position() as usize;
    let slice = &buf.get_ref()[pos..pos + len];
    buf.advance(len);
    
    Ok(Some(Bytes::copy_from_slice(slice)))
}

/// [bytes] 쓰기
pub fn write_bytes(buf: &mut BytesMut, bytes: Option<&[u8]>) {
    match bytes {
        Some(b) => {
            write_int(buf, b.len() as i32);
            buf.extend_from_slice(b);
        },
        None => write_int(buf, -1),
    }
}

/// [string map] 읽기
pub fn read_string_map(buf: &mut Cursor<&[u8]>) -> io::Result<HashMap<String, String>> {
    let n = read_short(buf)? as usize;
    let mut map = HashMap::with_capacity(n);
    
    for _ in 0..n {
        let key = read_string(buf)?;
        let value = read_string(buf)?;
        map.insert(key, value);
    }
    
    Ok(map)
}

/// [string map] 쓰기
pub fn write_string_map(buf: &mut BytesMut, map: &HashMap<String, String>) {
    write_short(buf, map.len() as u16);
    for (key, value) in map {
        write_string(buf, key);
        write_string(buf, value);
    }
}

/// [string multimap] 읽기
pub fn read_string_multimap(buf: &mut Cursor<&[u8]>) -> io::Result<HashMap<String, Vec<String>>> {
    let n = read_short(buf)? as usize;
    let mut map = HashMap::with_capacity(n);
    
    for _ in 0..n {
        let key = read_string(buf)?;
        let m = read_short(buf)? as usize;
        let mut values = Vec::with_capacity(m);
        for _ in 0..m {
            values.push(read_string(buf)?);
        }
        map.insert(key, values);
    }
    
    Ok(map)
}

/// [string multimap] 쓰기
pub fn write_string_multimap(buf: &mut BytesMut, map: &HashMap<String, Vec<String>>) {
    write_short(buf, map.len() as u16);
    for (key, values) in map {
        write_string(buf, key);
        write_short(buf, values.len() as u16);
        for value in values {
            write_string(buf, value);
        }
    }
}

/// CassandraValue를 바이트로 인코딩
pub fn encode_value(buf: &mut BytesMut, value: &CassandraValue) {
    match value {
        CassandraValue::Null => {
            write_int(buf, -1);
        },
        CassandraValue::Int(v) => {
            write_int(buf, 4);
            buf.put_i32(*v);
        },
        CassandraValue::BigInt(v) => {
            write_int(buf, 8);
            buf.put_i64(*v);
        },
        CassandraValue::Double(v) => {
            write_int(buf, 8);
            buf.put_f64(*v);
        },
        CassandraValue::Boolean(v) => {
            write_int(buf, 1);
            buf.put_u8(if *v { 1 } else { 0 });
        },
        CassandraValue::Text(s) => {
            write_int(buf, s.len() as i32);
            buf.extend_from_slice(s.as_bytes());
        },
        CassandraValue::Blob(b) => {
            write_int(buf, b.len() as i32);
            buf.extend_from_slice(b);
        },
        CassandraValue::UUID(u) => {
            write_int(buf, 16);
            buf.extend_from_slice(u.as_bytes());
        },
        CassandraValue::Timestamp(ts) => {
            write_int(buf, 8);
            buf.put_i64(*ts);
        },
        CassandraValue::List(items) => {
            let mut temp = BytesMut::new();
            write_int(&mut temp, items.len() as i32);
            for item in items {
                encode_value(&mut temp, item);
            }
            write_int(buf, temp.len() as i32);
            buf.extend_from_slice(&temp);
        },
        CassandraValue::Set(items) => {
            let mut temp = BytesMut::new();
            write_int(&mut temp, items.len() as i32);
            for item in items {
                encode_value(&mut temp, item);
            }
            write_int(buf, temp.len() as i32);
            buf.extend_from_slice(&temp);
        },
        CassandraValue::Map(m) => {
            let mut temp = BytesMut::new();
            write_int(&mut temp, m.len() as i32);
            for (k, v) in m {
                write_string(&mut temp, k);
                encode_value(&mut temp, v);
            }
            write_int(buf, temp.len() as i32);
            buf.extend_from_slice(&temp);
        },
        CassandraValue::Counter(c) => {
            write_int(buf, 8);
            buf.put_i64(*c);
        },
        CassandraValue::UDT(fields) => {
            // UDT는 Map과 유사하게 인코딩
            let mut temp = BytesMut::new();
            write_int(&mut temp, fields.len() as i32);
            for (k, v) in fields {
                write_string(&mut temp, k);
                encode_value(&mut temp, v);
            }
            write_int(buf, temp.len() as i32);
            buf.extend_from_slice(&temp);
        },
    }
}

/// Consistency Level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u16)]
pub enum Consistency {
    Any = 0x0000,
    #[default]
    One = 0x0001,
    Two = 0x0002,
    Three = 0x0003,
    Quorum = 0x0004,
    All = 0x0005,
    LocalQuorum = 0x0006,
    EachQuorum = 0x0007,
    Serial = 0x0008,
    LocalSerial = 0x0009,
    LocalOne = 0x000A,
}

impl TryFrom<u16> for Consistency {
    type Error = io::Error;
    
    fn try_from(value: u16) -> Result<Self, io::Error> {
        match value {
            0x0000 => Ok(Consistency::Any),
            0x0001 => Ok(Consistency::One),
            0x0002 => Ok(Consistency::Two),
            0x0003 => Ok(Consistency::Three),
            0x0004 => Ok(Consistency::Quorum),
            0x0005 => Ok(Consistency::All),
            0x0006 => Ok(Consistency::LocalQuorum),
            0x0007 => Ok(Consistency::EachQuorum),
            0x0008 => Ok(Consistency::Serial),
            0x0009 => Ok(Consistency::LocalSerial),
            0x000A => Ok(Consistency::LocalOne),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, format!("Unknown consistency: {}", value))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_string_roundtrip() {
        let mut buf = BytesMut::new();
        write_string(&mut buf, "hello world");
        
        let mut cursor = Cursor::new(&buf[..]);
        let result = read_string(&mut cursor).unwrap();
        assert_eq!(result, "hello world");
    }
    
    #[test]
    fn test_int_roundtrip() {
        let mut buf = BytesMut::new();
        write_int(&mut buf, 42);
        write_int(&mut buf, -12345);
        
        let mut cursor = Cursor::new(&buf[..]);
        assert_eq!(read_int(&mut cursor).unwrap(), 42);
        assert_eq!(read_int(&mut cursor).unwrap(), -12345);
    }
    
    #[test]
    fn test_string_map_roundtrip() {
        let mut map = HashMap::new();
        map.insert("CQL_VERSION".to_string(), "3.0.0".to_string());
        map.insert("COMPRESSION".to_string(), "snappy".to_string());
        
        let mut buf = BytesMut::new();
        write_string_map(&mut buf, &map);
        
        let mut cursor = Cursor::new(&buf[..]);
        let result = read_string_map(&mut cursor).unwrap();
        assert_eq!(result, map);
    }
}

//! Cassandra Native Protocol TCP 서버

use std::sync::Arc;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use bytes::{BytesMut, Bytes};
use tracing::{info, error, debug, warn};
use crate::database::CoreDB;
use crate::protocol::frame::{Frame, FrameHeader, Opcode, FRAME_HEADER_SIZE, PROTOCOL_VERSION_V4_RESPONSE};
use crate::protocol::codec::{Request, Response};
use crate::protocol::handler::RequestHandler;

/// Native Protocol 서버 설정
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub max_connections: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 9042, // Cassandra 기본 포트
            max_connections: 1000,
        }
    }
}

/// Native Protocol TCP 서버
pub struct NativeServer {
    config: ServerConfig,
    db: Arc<CoreDB>,
}

impl NativeServer {
    pub fn new(db: Arc<CoreDB>, config: ServerConfig) -> Self {
        Self { db, config }
    }
    
    /// 서버 시작
    pub async fn start(&self) -> std::io::Result<()> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let listener = TcpListener::bind(&addr).await?;
        
        info!("🚀 CoreDB Native Protocol Server listening on {}", addr);
        info!("   Compatible with Cassandra drivers (CQL v3, Protocol v4)");
        
        loop {
            match listener.accept().await {
                Ok((socket, addr)) => {
                    let db = Arc::clone(&self.db);
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(socket, addr, db).await {
                            error!("Connection error from {}: {}", addr, e);
                        }
                    });
                },
                Err(e) => {
                    error!("Accept error: {}", e);
                }
            }
        }
    }
}

async fn handle_connection(mut socket: TcpStream, addr: SocketAddr, db: Arc<CoreDB>) -> std::io::Result<()> {
    debug!("New connection from {}", addr);
    
    let mut handler = RequestHandler::new(db);
    let mut buf = BytesMut::with_capacity(8192);
    
    loop {
        // 데이터 읽기
        let n = socket.read_buf(&mut buf).await?;
        if n == 0 {
            debug!("Connection closed by {}", addr);
            return Ok(());
        }
        
        // 프레임 파싱 및 처리
        while buf.len() >= FRAME_HEADER_SIZE {
            // 헤더에서 길이 확인
            let length = u32::from_be_bytes([buf[5], buf[6], buf[7], buf[8]]) as usize;
            let total_len = FRAME_HEADER_SIZE + length;
            
            if buf.len() < total_len {
                // 아직 전체 프레임이 도착하지 않음
                break;
            }
            
            // 프레임 추출
            let frame_bytes = buf.split_to(total_len);
            
            match Frame::decode(&frame_bytes) {
                Ok(frame) => {
                    debug!("Received {:?} from {}", frame.header.opcode, addr);
                    
                    // 요청 디코딩
                    match Request::decode(frame.header.opcode, &frame.body) {
                        Ok(request) => {
                            // 요청 처리
                            let response = handler.handle(request).await;
                            
                            // 응답 인코딩
                            let (opcode, body) = response.encode();
                            let response_frame = Frame::new(frame.header.stream, opcode, body.freeze());
                            let response_bytes = response_frame.encode();
                            
                            // 응답 전송
                            socket.write_all(&response_bytes).await?;
                            debug!("Sent {:?} to {}", opcode, addr);
                        },
                        Err(e) => {
                            warn!("Failed to decode request: {}", e);
                            // 에러 응답 전송
                            let error_response = Response::Error(
                                crate::protocol::codec::ErrorResponse::server_error(e.to_string())
                            );
                            let (opcode, body) = error_response.encode();
                            let response_frame = Frame::new(frame.header.stream, opcode, body.freeze());
                            socket.write_all(&response_frame.encode()).await?;
                        }
                    }
                },
                Err(e) => {
                    error!("Failed to decode frame: {}", e);
                    return Err(e);
                }
            }
        }
    }
}

/// 서버 시작 헬퍼 함수
pub async fn start_native_server(db: Arc<CoreDB>, host: &str, port: u16) -> std::io::Result<()> {
    let config = ServerConfig {
        host: host.to_string(),
        port,
        ..Default::default()
    };
    
    let server = NativeServer::new(db, config);
    server.start().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::frame::*;
    use bytes::BufMut;
    
    #[test]
    fn test_frame_roundtrip() {
        let body = Bytes::from_static(b"test");
        let frame = Frame::new(1, Opcode::Query, body.clone());
        let encoded = frame.encode();
        let decoded = Frame::decode(&encoded).unwrap();
        
        assert_eq!(decoded.header.stream, 1);
        assert_eq!(decoded.header.opcode, Opcode::Query);
        assert_eq!(decoded.body, body);
    }
}

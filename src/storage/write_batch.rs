use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, RwLock, Notify};
use crate::schema::Row;
use crate::wal::{CommitLogEntry, Mutation};
use crate::error::*;

/// Write Batch 설정
#[derive(Debug, Clone)]
pub struct WriteBatchConfig {
    /// 최대 배치 크기 (엔트리 수)
    pub max_batch_size: usize,
    /// 최대 대기 시간 (ms)
    pub max_wait_ms: u64,
    /// 배치 채널 버퍼 크기
    pub channel_buffer_size: usize,
}

impl Default for WriteBatchConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 100,
            max_wait_ms: 5,
            channel_buffer_size: 1000,
        }
    }
}

/// 개별 쓰기 요청
#[derive(Debug)]
pub struct WriteRequest {
    pub keyspace: String,
    pub table: String,
    pub row: Row,
    pub response: oneshot::Sender<Result<()>>,
}

/// 삭제 요청
#[derive(Debug)]
pub struct DeleteRequest {
    pub keyspace: String,
    pub table: String,
    pub partition_key: crate::schema::PartitionKey,
    pub clustering_key: Option<crate::schema::ClusteringKey>,
    pub response: oneshot::Sender<Result<()>>,
}

/// 배치 작업 유형
#[derive(Debug)]
pub enum BatchOperation {
    Write(WriteRequest),
    Delete(DeleteRequest),
    Shutdown,
}

/// Write Batch 통계
#[derive(Debug, Default)]
pub struct WriteBatchStats {
    pub batches_committed: AtomicU64,
    pub operations_batched: AtomicU64,
    pub avg_batch_size: AtomicU64,
    pub total_wait_time_us: AtomicU64,
}

impl WriteBatchStats {
    pub fn record_batch(&self, size: usize, wait_time_us: u64) {
        self.batches_committed.fetch_add(1, Ordering::Relaxed);
        self.operations_batched.fetch_add(size as u64, Ordering::Relaxed);
        self.total_wait_time_us.fetch_add(wait_time_us, Ordering::Relaxed);
        
        // 평균 배치 크기 업데이트 (간단한 이동 평균)
        let batches = self.batches_committed.load(Ordering::Relaxed);
        let ops = self.operations_batched.load(Ordering::Relaxed);
        if batches > 0 {
            self.avg_batch_size.store(ops / batches, Ordering::Relaxed);
        }
    }
    
    pub fn status(&self) -> String {
        let batches = self.batches_committed.load(Ordering::Relaxed);
        let ops = self.operations_batched.load(Ordering::Relaxed);
        let avg = self.avg_batch_size.load(Ordering::Relaxed);
        format!(
            "WriteBatch: {} batches, {} ops total, avg {} ops/batch",
            batches, ops, avg
        )
    }
}

/// Write Batcher - 여러 쓰기를 배치로 묶어서 커밋
pub struct WriteBatcher {
    sender: mpsc::Sender<BatchOperation>,
    stats: Arc<WriteBatchStats>,
    is_running: Arc<AtomicBool>,
}

impl WriteBatcher {
    /// 새 WriteBatcher 생성 및 백그라운드 태스크 시작
    pub fn new(config: WriteBatchConfig) -> Self {
        let (sender, receiver) = mpsc::channel(config.channel_buffer_size);
        let stats = Arc::new(WriteBatchStats::default());
        let is_running = Arc::new(AtomicBool::new(true));
        
        // 백그라운드 배치 처리 태스크 시작
        let stats_clone = Arc::clone(&stats);
        let is_running_clone = Arc::clone(&is_running);
        
        tokio::spawn(async move {
            Self::batch_processor(receiver, config, stats_clone, is_running_clone).await;
        });
        
        Self {
            sender,
            stats,
            is_running,
        }
    }
    
    /// 쓰기 요청 제출 (비동기, 배치 커밋 후 완료)
    pub async fn submit_write(
        &self,
        keyspace: String,
        table: String,
        row: Row,
    ) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        
        let request = WriteRequest {
            keyspace,
            table,
            row,
            response: tx,
        };
        
        self.sender
            .send(BatchOperation::Write(request))
            .await
            .map_err(|_| CoreDBError::Generic { message: "Write batcher channel closed".to_string() })?;
        
        rx.await
            .map_err(|_| CoreDBError::Generic { message: "Write response channel closed".to_string() })?
    }
    
    /// 삭제 요청 제출
    pub async fn submit_delete(
        &self,
        keyspace: String,
        table: String,
        partition_key: crate::schema::PartitionKey,
        clustering_key: Option<crate::schema::ClusteringKey>,
    ) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        
        let request = DeleteRequest {
            keyspace,
            table,
            partition_key,
            clustering_key,
            response: tx,
        };
        
        self.sender
            .send(BatchOperation::Delete(request))
            .await
            .map_err(|_| CoreDBError::Generic { message: "Write batcher channel closed".to_string() })?;
        
        rx.await
            .map_err(|_| CoreDBError::Generic { message: "Delete response channel closed".to_string() })?
    }
    
    /// 배치 처리기 (백그라운드)
    async fn batch_processor(
        mut receiver: mpsc::Receiver<BatchOperation>,
        config: WriteBatchConfig,
        stats: Arc<WriteBatchStats>,
        is_running: Arc<AtomicBool>,
    ) {
        let max_wait = Duration::from_millis(config.max_wait_ms);
        
        loop {
            let mut batch: Vec<BatchOperation> = Vec::with_capacity(config.max_batch_size);
            let batch_start = Instant::now();
            
            // 첫 번째 요청 대기
            match receiver.recv().await {
                Some(BatchOperation::Shutdown) => break,
                Some(op) => batch.push(op),
                None => break, // 채널 닫힘
            }
            
            // 추가 요청 수집 (max_batch_size 또는 max_wait까지)
            let deadline = Instant::now() + max_wait;
            
            while batch.len() < config.max_batch_size {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                
                match tokio::time::timeout(remaining, receiver.recv()).await {
                    Ok(Some(BatchOperation::Shutdown)) => {
                        // 현재 배치 처리 후 종료
                        Self::process_batch(&mut batch).await;
                        is_running.store(false, Ordering::Relaxed);
                        return;
                    }
                    Ok(Some(op)) => batch.push(op),
                    Ok(None) => break, // 채널 닫힘
                    Err(_) => break,    // 타임아웃
                }
            }
            
            // 배치 처리
            let batch_size = batch.len();
            Self::process_batch(&mut batch).await;
            
            // 통계 기록
            let wait_time = batch_start.elapsed().as_micros() as u64;
            stats.record_batch(batch_size, wait_time);
        }
        
        is_running.store(false, Ordering::Relaxed);
    }
    
    /// 배치 처리 (실제 WAL 쓰기는 여기서)
    async fn process_batch(batch: &mut Vec<BatchOperation>) {
        // TODO: 실제 WAL 쓰기 통합 시 여기서 처리
        // 현재는 각 요청에 성공 응답만 전송
        
        for op in batch.drain(..) {
            match op {
                BatchOperation::Write(req) => {
                    let _ = req.response.send(Ok(()));
                }
                BatchOperation::Delete(req) => {
                    let _ = req.response.send(Ok(()));
                }
                BatchOperation::Shutdown => {}
            }
        }
    }
    
    /// 통계 반환
    pub fn stats(&self) -> &Arc<WriteBatchStats> {
        &self.stats
    }
    
    /// 상태 문자열
    pub fn status(&self) -> String {
        self.stats.status()
    }
    
    /// 종료
    pub async fn shutdown(&self) {
        let _ = self.sender.send(BatchOperation::Shutdown).await;
    }
}

/// WriteBatch 빌더 - 수동 배치 구성용
#[derive(Debug, Default)]
pub struct WriteBatch {
    entries: Vec<CommitLogEntry>,
}

impl WriteBatch {
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }
    
    pub fn with_capacity(capacity: usize) -> Self {
        Self { entries: Vec::with_capacity(capacity) }
    }
    
    /// Insert 추가
    pub fn put(&mut self, keyspace: &str, table: &str, row: Row) {
        self.entries.push(CommitLogEntry {
            keyspace: keyspace.to_string(),
            table: table.to_string(),
            mutation: Mutation::Insert(row),
            timestamp: chrono::Utc::now().timestamp_micros(),
        });
    }
    
    /// Delete 추가
    pub fn delete(
        &mut self,
        keyspace: &str,
        table: &str,
        partition_key: crate::schema::PartitionKey,
        clustering_key: Option<crate::schema::ClusteringKey>,
    ) {
        self.entries.push(CommitLogEntry {
            keyspace: keyspace.to_string(),
            table: table.to_string(),
            mutation: Mutation::Delete { partition_key, clustering_key },
            timestamp: chrono::Utc::now().timestamp_micros(),
        });
    }
    
    /// 배치 크기
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    
    /// 비어있는지 확인
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    
    /// 엔트리들 반환
    pub fn into_entries(self) -> Vec<CommitLogEntry> {
        self.entries
    }
    
    /// 클리어
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{CassandraValue, Cell, PartitionKey, ClusteringKey};
    use std::collections::HashMap;
    
    fn create_test_row(id: i32) -> Row {
        Row {
            partition_key: PartitionKey {
                components: vec![CassandraValue::Int(id)],
            },
            clustering_key: Some(ClusteringKey {
                components: vec![CassandraValue::BigInt(id as i64)],
            }),
            cells: {
                let mut cells = HashMap::new();
                cells.insert("value".to_string(), Cell {
                    value: CassandraValue::Text(format!("test_{}", id)),
                    timestamp: 0,
                    ttl: None,
                    is_deleted: false,
                });
                cells
            },
            timestamp: chrono::Utc::now().timestamp_micros(),
        }
    }
    
    #[test]
    fn test_write_batch_builder() {
        let mut batch = WriteBatch::new();
        
        batch.put("ks", "tbl", create_test_row(1));
        batch.put("ks", "tbl", create_test_row(2));
        batch.put("ks", "tbl", create_test_row(3));
        
        assert_eq!(batch.len(), 3);
        
        let entries = batch.into_entries();
        assert_eq!(entries.len(), 3);
    }
    
    #[tokio::test]
    async fn test_write_batcher() {
        let config = WriteBatchConfig {
            max_batch_size: 10,
            max_wait_ms: 50,
            channel_buffer_size: 100,
        };
        
        let batcher = WriteBatcher::new(config);
        
        // 여러 쓰기 제출
        for i in 0..5 {
            batcher
                .submit_write("test_ks".to_string(), "test_tbl".to_string(), create_test_row(i))
                .await
                .unwrap();
        }
        
        // 통계 확인
        tokio::time::sleep(Duration::from_millis(100)).await;
        let stats = batcher.stats();
        assert!(stats.operations_batched.load(Ordering::Relaxed) >= 5);
        
        batcher.shutdown().await;
    }
}

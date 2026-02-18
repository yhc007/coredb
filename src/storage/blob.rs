use std::path::PathBuf;
use std::sync::Arc;
use std::collections::HashMap;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt, AsyncSeekExt, SeekFrom};
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};
use uuid::Uuid;
use crate::error::*;

/// Blob 파일 설정
pub const BLOB_SIZE_THRESHOLD: usize = 4 * 1024; // 4KB 이상은 blob으로
pub const BLOB_FILE_MAX_SIZE: u64 = 256 * 1024 * 1024; // 256MB per blob file

/// Blob 참조 (SSTable 에 저장)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlobRef {
    pub file_id: String,
    pub offset: u64,
    pub size: u64,
    pub checksum: u32,
}

impl BlobRef {
    /// 인라인 데이터인지 확인
    pub fn is_inline(data: &[u8]) -> bool {
        data.len() < BLOB_SIZE_THRESHOLD
    }
}

/// Blob 파일 헤더
#[derive(Debug, Serialize, Deserialize)]
struct BlobFileHeader {
    pub magic: [u8; 4],       // "BLOB"
    pub version: u32,
    pub created_at: i64,
    pub blob_count: u64,
}

impl Default for BlobFileHeader {
    fn default() -> Self {
        Self {
            magic: *b"BLOB",
            version: 1,
            created_at: chrono::Utc::now().timestamp(),
            blob_count: 0,
        }
    }
}

/// Blob 엔트리 헤더
#[derive(Debug, Serialize, Deserialize)]
struct BlobEntryHeader {
    pub size: u64,
    pub checksum: u32,
    pub compression: u8,  // 0 = none, 1 = lz4, 2 = zstd
}

/// Blob 파일 작성기
pub struct BlobWriter {
    file: File,
    file_id: String,
    file_path: PathBuf,
    current_offset: u64,
    blob_count: u64,
}

impl BlobWriter {
    /// 새 Blob 파일 생성
    pub async fn create(base_dir: &PathBuf) -> Result<Self> {
        let file_id = Uuid::new_v4().to_string();
        let file_path = base_dir.join(format!("{}-Blob.db", file_id));
        
        let mut file = File::create(&file_path).await?;
        
        // 헤더 쓰기
        let header = BlobFileHeader::default();
        let header_data = bincode::serialize(&header)?;
        file.write_all(&header_data).await?;
        
        let current_offset = header_data.len() as u64;
        
        Ok(Self {
            file,
            file_id,
            file_path,
            current_offset,
            blob_count: 0,
        })
    }
    
    /// Blob 데이터 쓰기 (LZ4 압축 적용)
    pub async fn write_blob(&mut self, data: &[u8]) -> Result<BlobRef> {
        // 원본 데이터 체크섬 (압축 전)
        let original_checksum = crc32fast::hash(data);
        
        // LZ4 압축 시도
        let (write_data, compression) = if data.len() > 256 {
            let compressed = lz4_flex::compress_prepend_size(data);
            // 압축 효과가 있는 경우에만 사용 (10% 이상 줄어야 함)
            if compressed.len() < data.len() * 9 / 10 {
                (compressed, 1u8) // 1 = LZ4
            } else {
                (data.to_vec(), 0u8) // 0 = None
            }
        } else {
            (data.to_vec(), 0u8)
        };
        
        // 엔트리 헤더 (압축된 크기 저장)
        let entry_header = BlobEntryHeader {
            size: write_data.len() as u64,
            checksum: original_checksum, // 원본 체크섬 저장
            compression,
        };
        
        let entry_header_data = bincode::serialize(&entry_header)?;
        
        // 오프셋 저장
        let blob_offset = self.current_offset;
        
        // 헤더 쓰기
        self.file.write_all(&entry_header_data).await?;
        self.current_offset += entry_header_data.len() as u64;
        
        // 데이터 쓰기 (압축된 데이터)
        self.file.write_all(&write_data).await?;
        self.current_offset += write_data.len() as u64;
        
        self.blob_count += 1;
        
        Ok(BlobRef {
            file_id: self.file_id.clone(),
            offset: blob_offset,
            size: data.len() as u64, // 원본 크기 반환
            checksum: original_checksum,
        })
    }
    
    /// 파일 크기가 최대치에 도달했는지 확인
    pub fn is_full(&self) -> bool {
        self.current_offset >= BLOB_FILE_MAX_SIZE
    }
    
    /// 파일 ID
    pub fn file_id(&self) -> &str {
        &self.file_id
    }
    
    /// 파일 닫기 및 헤더 업데이트
    pub async fn finish(mut self) -> Result<PathBuf> {
        // 헤더 업데이트
        self.file.seek(SeekFrom::Start(0)).await?;
        
        let header = BlobFileHeader {
            magic: *b"BLOB",
            version: 1,
            created_at: chrono::Utc::now().timestamp(),
            blob_count: self.blob_count,
        };
        
        let header_data = bincode::serialize(&header)?;
        self.file.write_all(&header_data).await?;
        
        self.file.sync_all().await?;
        
        Ok(self.file_path)
    }
}

/// Blob 파일 읽기
pub struct BlobReader {
    base_dir: PathBuf,
    /// 파일 핸들 캐시
    file_cache: Arc<RwLock<HashMap<String, Arc<RwLock<File>>>>>,
    cache_max_size: usize,
}

impl BlobReader {
    pub fn new(base_dir: PathBuf, cache_max_files: usize) -> Self {
        Self {
            base_dir,
            file_cache: Arc::new(RwLock::new(HashMap::new())),
            cache_max_size: cache_max_files,
        }
    }
    
    /// Blob 데이터 읽기 (압축 해제 지원)
    pub async fn read_blob(&self, blob_ref: &BlobRef) -> Result<Vec<u8>> {
        let file = self.get_file(&blob_ref.file_id).await?;
        
        let mut file_guard = file.write().await;
        
        // 오프셋으로 이동
        file_guard.seek(SeekFrom::Start(blob_ref.offset)).await?;
        
        // 엔트리 헤더 읽기
        let mut header_buf = vec![0u8; 13]; // BlobEntryHeader 크기 추정
        file_guard.read_exact(&mut header_buf).await?;
        
        let entry_header: BlobEntryHeader = bincode::deserialize(&header_buf)?;
        
        // 압축된 데이터 읽기
        let mut compressed_data = vec![0u8; entry_header.size as usize];
        file_guard.read_exact(&mut compressed_data).await?;
        
        // 압축 해제
        let data = match entry_header.compression {
            1 => {
                // LZ4 압축 해제
                lz4_flex::decompress_size_prepended(&compressed_data)
                    .map_err(|e| CoreDBError::DataCorruption(
                        format!("LZ4 decompression failed: {}", e)
                    ))?
            },
            _ => compressed_data, // 압축 없음
        };
        
        // 체크섬 검증 (원본 데이터 기준)
        let checksum = crc32fast::hash(&data);
        if checksum != blob_ref.checksum {
            return Err(CoreDBError::DataCorruption(
                format!("Blob checksum mismatch: expected {}, got {}", 
                    blob_ref.checksum, checksum)
            ));
        }
        
        Ok(data)
    }
    
    /// 파일 핸들 가져오기 (캐시 사용)
    async fn get_file(&self, file_id: &str) -> Result<Arc<RwLock<File>>> {
        // 캐시 확인
        {
            let cache = self.file_cache.read().await;
            if let Some(file) = cache.get(file_id) {
                return Ok(file.clone());
            }
        }
        
        // 파일 열기
        let file_path = self.base_dir.join(format!("{}-Blob.db", file_id));
        let file = OpenOptions::new()
            .read(true)
            .open(&file_path)
            .await?;
        
        let file_arc = Arc::new(RwLock::new(file));
        
        // 캐시에 추가
        {
            let mut cache = self.file_cache.write().await;
            
            // 캐시 크기 제한
            if cache.len() >= self.cache_max_size {
                // 가장 오래된 항목 제거 (간단한 FIFO)
                if let Some(oldest) = cache.keys().next().cloned() {
                    cache.remove(&oldest);
                }
            }
            
            cache.insert(file_id.to_string(), file_arc.clone());
        }
        
        Ok(file_arc)
    }
    
    /// 캐시 클리어
    pub async fn clear_cache(&self) {
        let mut cache = self.file_cache.write().await;
        cache.clear();
    }
}

/// Blob 관리자
pub struct BlobStore {
    base_dir: PathBuf,
    current_writer: Arc<RwLock<Option<BlobWriter>>>,
    reader: BlobReader,
}

impl BlobStore {
    pub fn new(base_dir: PathBuf) -> Self {
        let reader = BlobReader::new(base_dir.clone(), 16);
        
        Self {
            base_dir,
            current_writer: Arc::new(RwLock::new(None)),
            reader,
        }
    }
    
    /// 데이터 저장 - 인라인 또는 Blob
    pub async fn store(&self, data: &[u8]) -> Result<StoredValue> {
        if BlobRef::is_inline(data) {
            Ok(StoredValue::Inline(data.to_vec()))
        } else {
            let blob_ref = self.write_blob(data).await?;
            Ok(StoredValue::Blob(blob_ref))
        }
    }
    
    /// Blob으로 강제 저장
    async fn write_blob(&self, data: &[u8]) -> Result<BlobRef> {
        let mut writer_guard = self.current_writer.write().await;
        
        // 현재 writer가 없거나 꽉 찼으면 새로 생성
        if writer_guard.is_none() || writer_guard.as_ref().unwrap().is_full() {
            // 기존 writer 마무리
            if let Some(old_writer) = writer_guard.take() {
                old_writer.finish().await?;
            }
            
            // 새 writer 생성
            *writer_guard = Some(BlobWriter::create(&self.base_dir).await?);
        }
        
        let writer = writer_guard.as_mut().unwrap();
        writer.write_blob(data).await
    }
    
    /// 데이터 읽기
    pub async fn read(&self, value: &StoredValue) -> Result<Vec<u8>> {
        match value {
            StoredValue::Inline(data) => Ok(data.clone()),
            StoredValue::Blob(blob_ref) => self.reader.read_blob(blob_ref).await,
        }
    }
    
    /// 현재 writer 마무리
    pub async fn flush(&self) -> Result<()> {
        let mut writer_guard = self.current_writer.write().await;
        
        if let Some(writer) = writer_guard.take() {
            writer.finish().await?;
        }
        
        Ok(())
    }
    
    /// Blob 파일 삭제 (GC용)
    pub async fn delete_blob_file(&self, file_id: &str) -> Result<()> {
        let file_path = self.base_dir.join(format!("{}-Blob.db", file_id));
        
        // 캐시에서 제거
        self.reader.clear_cache().await;
        
        // 파일 삭제
        if file_path.exists() {
            tokio::fs::remove_file(&file_path).await?;
        }
        
        Ok(())
    }
    
    /// Blob 파일 목록
    pub async fn list_blob_files(&self) -> Result<Vec<String>> {
        let mut files = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.base_dir).await?;
        
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with("-Blob.db") {
                let file_id = name.trim_end_matches("-Blob.db").to_string();
                files.push(file_id);
            }
        }
        
        Ok(files)
    }
    
    /// 통계
    pub async fn get_stats(&self) -> Result<BlobStats> {
        let files = self.list_blob_files().await?;
        let mut total_size = 0u64;
        
        for file_id in &files {
            let file_path = self.base_dir.join(format!("{}-Blob.db", file_id));
            if let Ok(meta) = tokio::fs::metadata(&file_path).await {
                total_size += meta.len();
            }
        }
        
        Ok(BlobStats {
            file_count: files.len(),
            total_size_bytes: total_size,
        })
    }
}

/// 저장된 값 (인라인 또는 Blob 참조)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StoredValue {
    Inline(Vec<u8>),
    Blob(BlobRef),
}

impl StoredValue {
    pub fn size_hint(&self) -> usize {
        match self {
            StoredValue::Inline(data) => data.len(),
            StoredValue::Blob(blob_ref) => blob_ref.size as usize,
        }
    }
}

/// Blob 통계
#[derive(Debug, Clone)]
pub struct BlobStats {
    pub file_count: usize,
    pub total_size_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    #[tokio::test]
    async fn test_blob_store_inline() {
        let temp_dir = TempDir::new().unwrap();
        let store = BlobStore::new(temp_dir.path().to_path_buf());
        
        // 작은 데이터는 인라인
        let small_data = b"Hello, World!";
        let stored = store.store(small_data).await.unwrap();
        
        assert!(matches!(stored, StoredValue::Inline(_)));
        
        let read_back = store.read(&stored).await.unwrap();
        assert_eq!(read_back, small_data);
    }
    
    #[tokio::test]
    async fn test_blob_store_large() {
        let temp_dir = TempDir::new().unwrap();
        let store = BlobStore::new(temp_dir.path().to_path_buf());
        
        // 큰 데이터는 Blob
        let large_data: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();
        let stored = store.store(&large_data).await.unwrap();
        
        assert!(matches!(stored, StoredValue::Blob(_)));
        
        // flush 후 읽기
        store.flush().await.unwrap();
        
        let read_back = store.read(&stored).await.unwrap();
        assert_eq!(read_back, large_data);
    }
    
    #[test]
    fn test_blob_ref_threshold() {
        let small = vec![0u8; 1000];
        assert!(BlobRef::is_inline(&small));
        
        let large = vec![0u8; 10_000];
        assert!(!BlobRef::is_inline(&large));
    }
    
    #[tokio::test]
    async fn test_blob_checksum() {
        let temp_dir = TempDir::new().unwrap();
        let mut writer = BlobWriter::create(&temp_dir.path().to_path_buf()).await.unwrap();
        
        let data = b"Test data for checksum";
        let blob_ref = writer.write_blob(data).await.unwrap();
        
        // checksum이 올바른지 확인
        let expected_checksum = crc32fast::hash(data);
        assert_eq!(blob_ref.checksum, expected_checksum);
    }
}

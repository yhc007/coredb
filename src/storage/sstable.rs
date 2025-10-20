use std::path::PathBuf;
use std::collections::BTreeMap;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt, SeekFrom, AsyncSeekExt};
use uuid::Uuid;
use crate::schema::{PartitionKey, Row};
use crate::storage::{Memtable, BloomFilter};
use crate::storage::memtable::Partition;
use crate::error::*;

/// 압축 타입
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionType {
    None,
    LZ4,
    Snappy,
    ZSTD,
}

/// SSTable 구조
#[derive(Debug, Clone)]
pub struct SSTable {
    pub id: String,
    pub file_path: PathBuf,
    pub bloom_filter: BloomFilter,
    pub partition_index: BTreeMap<PartitionKey, u64>, // 파티션 -> 파일 오프셋
    pub summary_index: BTreeMap<PartitionKey, u64>,   // 파티션 인덱스의 샘플
    pub min_timestamp: i64,
    pub max_timestamp: i64,
    pub compression: CompressionType,
    pub size_bytes: u64,
}

/// SSTable 헤더
#[derive(Debug, Serialize, Deserialize)]
struct SSTableHeader {
    pub version: u32,
    pub compression: CompressionType,
    pub min_timestamp: i64,
    pub max_timestamp: i64,
    pub partition_count: u64,
    pub bloom_filter_offset: u64,
    pub partition_index_offset: u64,
    pub summary_index_offset: u64,
}

impl SSTable {
    /// Memtable에서 SSTable 생성
    pub async fn create_from_memtable(
        memtable: &Memtable,
        base_dir: &PathBuf,
        compression: CompressionType
    ) -> Result<Self, Error> {
        let sstable_id = Uuid::new_v4().to_string();
        let data_file_path = base_dir.join(format!("{}-Data.db", sstable_id));
        
        let mut data_file = File::create(&data_file_path).await?;
        
        let mut bloom_filter = BloomFilter::new(
            memtable.partition_count() as u64, 
            0.01
        );
        
        let mut partition_index = BTreeMap::new();
        let mut current_offset = 0u64;
        let mut min_timestamp = i64::MAX;
        let mut max_timestamp = i64::MIN;
        let mut total_size = 0u64;
        
        // 헤더 공간 예약 (나중에 업데이트)
        let header_size = bincode::serialized_size(&SSTableHeader {
            version: 1,
            compression: CompressionType::None,
            min_timestamp: 0,
            max_timestamp: 0,
            partition_count: 0,
            bloom_filter_offset: 0,
            partition_index_offset: 0,
            summary_index_offset: 0,
        })? as u64;
        
        current_offset += header_size;
        
        // 파티션별로 정렬하여 SSTable에 쓰기
        let mut partitions = memtable.get_all_partitions();
        partitions.sort_by(|a, b| a.0.cmp(&b.0));
        
        for (partition_key, partition) in partitions {
            // 블룸 필터에 파티션 키 추가
            bloom_filter.add(&partition_key);
            
            // 파티션 인덱스 업데이트
            partition_index.insert(partition_key.clone(), current_offset);
            
            // 파티션 데이터 직렬화 및 압축
            let partition_data = Self::serialize_partition(&partition, &compression).await?;
            
            // 데이터 파일에 쓰기
            data_file.write_u32(partition_data.len() as u32).await?;
            data_file.write_all(&partition_data).await?;
            
            let partition_size = 4 + partition_data.len() as u64;
            current_offset += partition_size;
            total_size += partition_size;
            
            // 타임스탬프 범위 업데이트
            for row_entry in partition.rows.iter() {
                let row = row_entry.value();
                min_timestamp = min_timestamp.min(row.timestamp);
                max_timestamp = max_timestamp.max(row.timestamp);
            }
        }
        
        let bloom_filter_offset = current_offset;
        let bloom_filter_data = bincode::serialize(&bloom_filter)?;
        data_file.write_all(&bloom_filter_data).await?;
        current_offset += bloom_filter_data.len() as u64;
        
        let partition_index_offset = current_offset;
        let partition_index_data = bincode::serialize(&partition_index)?;
        data_file.write_all(&partition_index_data).await?;
        current_offset += partition_index_data.len() as u64;
        
        let summary_index_offset = current_offset;
        let summary_index = Self::build_summary_index(&partition_index);
        let summary_index_data = bincode::serialize(&summary_index)?;
        data_file.write_all(&summary_index_data).await?;
        
        // 헤더 업데이트
        let header = SSTableHeader {
            version: 1,
            compression,
            min_timestamp,
            max_timestamp,
            partition_count: partition_index.len() as u64,
            bloom_filter_offset,
            partition_index_offset,
            summary_index_offset,
        };
        
        let header_data = bincode::serialize(&header)?;
        data_file.seek(SeekFrom::Start(0)).await?;
        data_file.write_all(&header_data).await?;
        data_file.sync_all().await?;
        
        Ok(SSTable {
            id: sstable_id,
            file_path: data_file_path,
            bloom_filter,
            partition_index,
            summary_index,
            min_timestamp,
            max_timestamp,
            compression,
            size_bytes: total_size,
        })
    }
    
    /// 파티션 읽기
    pub async fn read_partition(&self, partition_key: &PartitionKey) -> Result<Option<Partition>, Error> {
        // 1. 블룸 필터 체크
        if !self.bloom_filter.might_contain(partition_key) {
            return Ok(None);
        }
        
        // 2. 파티션 인덱스에서 오프셋 찾기
        let offset = match self.partition_index.get(partition_key) {
            Some(offset) => *offset,
            None => return Ok(None),
        };
        
        // 3. 디스크에서 파티션 데이터 읽기
        let mut file = File::open(&self.file_path).await?;
        file.seek(SeekFrom::Start(offset)).await?;
        
        // 파티션 크기 읽기
        let mut size_buf = [0u8; 4];
        file.read_exact(&mut size_buf).await?;
        let partition_size = u32::from_le_bytes(size_buf) as usize;
        
        // 파티션 데이터 읽기
        let mut partition_data = vec![0u8; partition_size];
        file.read_exact(&mut partition_data).await?;
        
        // 압축 해제 및 역직렬화
        let partition = Self::deserialize_partition(&partition_data, &self.compression).await?;
        
        Ok(Some(partition))
    }
    
    /// 파티션 직렬화 및 압축
    async fn serialize_partition(partition: &Partition, compression: &CompressionType) -> Result<Vec<u8>, Error> {
        let mut data = Vec::new();
        
        // Static 컬럼들 직렬화
        let static_data = bincode::serialize(&partition.static_columns)?;
        data.write_u32(static_data.len() as u32).await?;
        data.write_all(&static_data).await?;
        
        // 행들 직렬화
        let mut rows: Vec<Row> = partition.rows.iter().map(|entry| entry.value().clone()).collect();
        rows.sort_by(|a, b| {
            match (&a.clustering_key, &b.clustering_key) {
                (Some(ak), Some(bk)) => ak.cmp(bk),
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        
        data.write_u32(rows.len() as u32).await?;
        for row in &rows {
            let row_data = bincode::serialize(row)?;
            data.write_u32(row_data.len() as u32).await?;
            data.write_all(&row_data).await?;
        }
        
        // 압축 적용
        match compression {
            CompressionType::None => Ok(data),
            CompressionType::LZ4 => {
                Ok(lz4_flex::compress_prepend_size(&data))
            },
            CompressionType::Snappy => {
                let mut encoder = snap::raw::Encoder::new();
                Ok(encoder.compress_vec(&data)?)
            },
            CompressionType::ZSTD => {
                Ok(zstd::bulk::compress(&data, 3)?)
            },
        }
    }
    
    /// 파티션 역직렬화 및 압축 해제
    async fn deserialize_partition(data: &[u8], compression: &CompressionType) -> Result<Partition, Error> {
        // 압축 해제
        let decompressed_data = match compression {
            CompressionType::None => data.to_vec(),
            CompressionType::LZ4 => {
                lz4_flex::decompress_size_prepended(data)?
            },
            CompressionType::Snappy => {
                let mut decoder = snap::raw::Decoder::new();
                decoder.decompress_vec(data)?
            },
            CompressionType::ZSTD => {
                zstd::bulk::decompress(data, 1024 * 1024)? // 1MB max
            },
        };
        
        let mut cursor = std::io::Cursor::new(&decompressed_data);
        
        // Static 컬럼들 역직렬화
        let mut size_buf = [0u8; 4];
        cursor.read_exact(&mut size_buf).await?;
        let static_size = u32::from_le_bytes(size_buf) as usize;
        
        let mut static_data = vec![0u8; static_size];
        cursor.read_exact(&mut static_data).await?;
        let static_columns: std::collections::HashMap<String, crate::schema::Cell> = 
            bincode::deserialize(&static_data)?;
        
        // 행들 역직렬화
        cursor.read_exact(&mut size_buf).await?;
        let row_count = u32::from_le_bytes(size_buf) as usize;
        
        let mut rows = crossbeam_skiplist::SkipMap::new();
        
        for _ in 0..row_count {
            cursor.read_exact(&mut size_buf).await?;
            let row_size = u32::from_le_bytes(size_buf) as usize;
            
            let mut row_data = vec![0u8; row_size];
            cursor.read_exact(&mut row_data).await?;
            
            let row: Row = bincode::deserialize(&row_data)?;
            rows.insert(row.clustering_key.clone(), row);
        }
        
        Ok(Partition {
            rows,
            static_columns,
        })
    }
    
    /// 요약 인덱스 생성 (메모리 효율성을 위해)
    fn build_summary_index(full_index: &BTreeMap<PartitionKey, u64>) -> BTreeMap<PartitionKey, u64> {
        let sample_rate = 128; // 128개 파티션마다 하나씩 샘플링
        
        full_index.iter()
            .enumerate()
            .filter(|(i, _)| i % sample_rate == 0)
            .map(|(_, (k, v))| (k.clone(), *v))
            .collect()
    }
    
    /// SSTable 삭제
    pub async fn delete(&self) -> Result<(), Error> {
        tokio::fs::remove_file(&self.file_path).await?;
        Ok(())
    }
    
    /// 파일 크기 가져오기
    pub async fn file_size(&self) -> Result<u64, Error> {
        let metadata = tokio::fs::metadata(&self.file_path).await?;
        Ok(metadata.len())
    }
}

use serde::{Serialize, Deserialize};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{CassandraValue, ColumnDefinition, CassandraDataType, Cell};
    use std::collections::HashMap;
    
    fn create_test_schema() -> std::sync::Arc<crate::schema::TableSchema> {
        std::sync::Arc::new(crate::schema::TableSchema::new(
            "test_table".to_string(),
            "test_keyspace".to_string(),
            vec![ColumnDefinition {
                name: "id".to_string(),
                data_type: CassandraDataType::Int,
                is_static: false,
            }],
            vec![ColumnDefinition {
                name: "timestamp".to_string(),
                data_type: CassandraDataType::BigInt,
                is_static: false,
            }],
            vec![ColumnDefinition {
                name: "value".to_string(),
                data_type: CassandraDataType::Text,
                is_static: false,
            }],
            vec![],
        ))
    }
    
    fn create_test_row(id: i32, timestamp: i64, value: &str) -> Row {
        Row {
            partition_key: PartitionKey {
                components: vec![CassandraValue::Int(id)],
            },
            clustering_key: Some(ClusteringKey {
                components: vec![CassandraValue::BigInt(timestamp)],
            }),
            cells: {
                let mut cells = HashMap::new();
                cells.insert("value".to_string(), Cell {
                    value: CassandraValue::Text(value.to_string()),
                    timestamp: chrono::Utc::now().timestamp_micros(),
                    ttl: None,
                    is_deleted: false,
                });
                cells
            },
            timestamp: chrono::Utc::now().timestamp_micros(),
        }
    }
    
    #[tokio::test]
    async fn test_sstable_creation_and_read() {
        let temp_dir = std::env::temp_dir().join("coredb_test");
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        
        let schema = create_test_schema();
        let memtable = crate::storage::Memtable::new(schema);
        
        // 테스트 데이터 추가
        for i in 1..=5 {
            let row = create_test_row(i, i * 1000, &format!("value_{}", i));
            memtable.put(row).unwrap();
        }
        
        // SSTable 생성
        let sstable = SSTable::create_from_memtable(
            &memtable,
            &temp_dir,
            CompressionType::None
        ).await.unwrap();
        
        // 데이터 읽기 테스트
        let partition_key = PartitionKey {
            components: vec![CassandraValue::Int(3)],
        };
        
        let partition = sstable.read_partition(&partition_key).await.unwrap();
        assert!(partition.is_some());
        
        let partition = partition.unwrap();
        assert_eq!(partition.rows.len(), 1);
        
        // 정리
        sstable.delete().await.unwrap();
    }
}

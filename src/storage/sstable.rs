use std::path::PathBuf;
use std::collections::BTreeMap;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt, SeekFrom, AsyncSeekExt};
use uuid::Uuid;
use serde::{Serialize, Deserialize};
use crate::schema::{PartitionKey, Row};
use crate::storage::{Memtable, BloomFilter};
use crate::storage::memtable::Partition;
use crate::error::*;

/// 압축 타입
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionType {
    None,
    LZ4,
    Snappy,
    ZSTD,
}

/// SSTable 구조
#[derive(Debug, Clone, PartialEq)]
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
    /// Total row count across every partition in this SSTable.
    /// Populated at creation time (from memtable or compaction) and
    /// persisted to a `{id}-Stats.json` side file. Loaded back via
    /// [`SSTable::open`] when present; `None` for legacy SSTables on
    /// disk that predate the stats sidecar. A `None` value tells the
    /// COUNT(*) fast path to fall back to the row-materialization
    /// slow path for *that* SSTable; new memtable flushes / compactions
    /// always write the sidecar so the fleet self-heals over time.
    pub row_count: Option<u64>,
    /// Smallest partition key in this SSTable (by `PartitionKey: Ord`).
    /// Populated alongside `row_count` and persisted via the same
    /// `{id}-Stats.json` sidecar (version 2). `None` for legacy
    /// SSTables that haven't been re-opened since the v2 upgrade and
    /// for empty SSTables (no partitions at all).
    ///
    /// Used by [`SSTable::read_partition`] as an O(1) range veto
    /// before the O(log N) `partition_index` lookup, and exposed for
    /// callers (e.g. range scans, future compaction scheduling) that
    /// want to know an SSTable's key extent without touching disk.
    pub min_partition_key: Option<PartitionKey>,
    /// Largest partition key in this SSTable. Companion to
    /// [`Self::min_partition_key`]; same lifetime, same backfill, same
    /// veto role.
    pub max_partition_key: Option<PartitionKey>,
}

/// Stats sidecar — `{id}-Stats.json` next to `{id}-Data.db`.
/// Versioned so a future addition (e.g. tombstone count, per-side
/// counts) doesn't break older readers.
///
/// Version log:
/// - v1: `row_count` only.
/// - v2: adds `min_partition_key` + `max_partition_key`. Files
///   written by older binaries still parse via `#[serde(default)]`
///   on the new fields, and v2 readers back-fill the bounds from
///   the partition_index on the first re-open.
#[derive(Serialize, Deserialize, Debug)]
struct SSTableStats {
    version: u32,
    row_count: u64,
    #[serde(default)]
    min_partition_key: Option<PartitionKey>,
    #[serde(default)]
    max_partition_key: Option<PartitionKey>,
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
    ) -> Result<Self> {
        let sstable_id = Uuid::new_v4().to_string();
        let data_file_path = base_dir.join(format!("{}-Data.db", sstable_id));
        
        let mut data_file = File::create(&data_file_path).await?;

        // The external `bloomfilter` crate's `new` panics on
        // items_count = 0. Caller paths that rotate an empty
        // memtable (e.g. a freshly-rotated immutable in the
        // size-threshold watcher's loop) would crash the runtime;
        // floor to 1 so the SSTable can still be written (empty
        // partition_index → readers return None for every key
        // anyway, which is the correct behavior).
        let mut bloom_filter = BloomFilter::new(
            (memtable.partition_count() as u64).max(1),
            0.01,
        );
        
        let mut partition_index = BTreeMap::new();
        let mut current_offset: u64;
        let mut min_timestamp = i64::MAX;
        let mut max_timestamp = i64::MIN;
        let mut total_size = 0u64;
        let mut row_count: u64 = 0;
        
        // 헤더 공간 예약 (나중에 업데이트)
        let _header_size = bincode::serialized_size(&SSTableHeader {
            version: 1,
            compression: CompressionType::None,
            min_timestamp: 0,
            max_timestamp: 0,
            partition_count: 0,
            bloom_filter_offset: 0,
            partition_index_offset: 0,
            summary_index_offset: 0,
        })? as u64;
        
        // Write dummy header to reserve space and advance file pointer
        let dummy_header = SSTableHeader {
            version: 1,
            compression: CompressionType::None,
            min_timestamp: 0,
            max_timestamp: 0,
            partition_count: 0,
            bloom_filter_offset: 0,
            partition_index_offset: 0,
            summary_index_offset: 0,
        };
        let dummy_header_data = bincode::serialize(&dummy_header)?;
        data_file.write_all(&dummy_header_data).await?;
        
        current_offset = dummy_header_data.len() as u64;
        
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
            data_file.write_u32_le(partition_data.len() as u32).await?;
            data_file.write_all(&partition_data).await?;
            
            let partition_size = 4 + partition_data.len() as u64;
            current_offset += partition_size;
            total_size += partition_size;
            
            // 타임스탬프 범위 업데이트 + 행 카운트
            for row_entry in partition.rows.iter() {
                let row = row_entry.value();
                min_timestamp = min_timestamp.min(row.timestamp);
                max_timestamp = max_timestamp.max(row.timestamp);
                row_count += 1;
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

        // partition_index를 별도 JSON 파일로 저장 (안정적인 복구용)
        let index_file_path = base_dir.join(format!("{}-Index.json", sstable_id));
        let index_vec: Vec<(PartitionKey, u64)> = partition_index.iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        let index_json = serde_json::to_string(&index_vec)?;
        tokio::fs::write(&index_file_path, index_json).await?;

        // Bloom Filter를 별도 파일로 저장
        let bloom_file_path = base_dir.join(format!("{}-Bloom.db", sstable_id));
        let bloom_data = bincode::serialize(&bloom_filter)?;
        tokio::fs::write(&bloom_file_path, bloom_data).await?;

        // Stats sidecar — small JSON file next to the data file. Lets
        // a future COUNT(*) reuse the row count without rescanning
        // partitions. Failure to write the sidecar is non-fatal: the
        // SSTable itself is durable, and a missing sidecar just means
        // the fast path will skip this file on next load.
        let stats_file_path = base_dir.join(format!("{}-Stats.json", sstable_id));
        // partition_index is a BTreeMap → first/last keys give us the
        // min/max bounds for free (sorted by PartitionKey: Ord).
        let min_pk = partition_index.keys().next().cloned();
        let max_pk = partition_index.keys().next_back().cloned();
        let stats = SSTableStats {
            version: 2,
            row_count,
            min_partition_key: min_pk.clone(),
            max_partition_key: max_pk.clone(),
        };
        if let Ok(stats_json) = serde_json::to_string(&stats) {
            tokio::fs::write(&stats_file_path, stats_json).await.ok();
        }

        Ok(SSTable {
            id: sstable_id,
            file_path: data_file_path,
            bloom_filter,
            partition_index,
            summary_index,
            min_timestamp,
            max_timestamp,
            compression: compression.clone(),
            size_bytes: total_size,
            row_count: Some(row_count),
            min_partition_key: min_pk,
            max_partition_key: max_pk,
        })
    }

    /// 파티션 데이터로부터 SSTable 생성 (컴팩션용)
    pub async fn create_from_partitions(
        partitions: &std::collections::BTreeMap<PartitionKey, crate::storage::memtable::Partition>,
        base_dir: &PathBuf,
        compression: CompressionType,
    ) -> Result<Self> {
        if partitions.is_empty() {
            return Err(CoreDBError::Generic { message: "Empty partitions".to_string() });
        }
        
        let sstable_id = Uuid::new_v4().to_string();
        let data_file_path = base_dir.join(format!("{}-Data.db", sstable_id));
        
        let mut data_file = File::create(&data_file_path).await?;
        
        let mut bloom_filter = BloomFilter::new(partitions.len() as u64, 0.01);
        let mut partition_index = BTreeMap::new();
        let mut min_timestamp = i64::MAX;
        let mut max_timestamp = i64::MIN;
        let mut total_size = 0u64;
        let mut row_count: u64 = 0;
        
        // 헤더 공간 예약
        let dummy_header = SSTableHeader {
            version: 1,
            compression: CompressionType::None,
            min_timestamp: 0,
            max_timestamp: 0,
            partition_count: 0,
            bloom_filter_offset: 0,
            partition_index_offset: 0,
            summary_index_offset: 0,
        };
        let dummy_header_data = bincode::serialize(&dummy_header)?;
        data_file.write_all(&dummy_header_data).await?;
        
        let mut current_offset = dummy_header_data.len() as u64;
        
        // 파티션별로 SSTable에 쓰기
        for (partition_key, partition) in partitions {
            // 블룸 필터에 파티션 키 추가
            bloom_filter.add(partition_key);
            
            // 파티션 인덱스 업데이트
            partition_index.insert(partition_key.clone(), current_offset);
            
            // 파티션 데이터 직렬화 및 압축
            let partition_data = Self::serialize_partition(partition, &compression).await?;
            
            // 데이터 파일에 쓰기
            data_file.write_u32_le(partition_data.len() as u32).await?;
            data_file.write_all(&partition_data).await?;
            
            let partition_size = 4 + partition_data.len() as u64;
            current_offset += partition_size;
            total_size += partition_size;
            
            // 타임스탬프 범위 업데이트 + 행 카운트
            for row_entry in partition.rows.iter() {
                let row = row_entry.value();
                min_timestamp = min_timestamp.min(row.timestamp);
                max_timestamp = max_timestamp.max(row.timestamp);
                row_count += 1;
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

        // 인덱스 파일 저장
        let index_file_path = base_dir.join(format!("{}-Index.json", sstable_id));
        let index_vec: Vec<(PartitionKey, u64)> = partition_index.iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        let index_json = serde_json::to_string(&index_vec)?;
        tokio::fs::write(&index_file_path, index_json).await?;

        // Bloom Filter 별도 파일 저장
        let bloom_file_path = base_dir.join(format!("{}-Bloom.db", sstable_id));
        tokio::fs::write(&bloom_file_path, &bloom_filter_data).await?;

        // Stats sidecar (see create_from_memtable for rationale).
        let stats_file_path = base_dir.join(format!("{}-Stats.json", sstable_id));
        let min_pk = partition_index.keys().next().cloned();
        let max_pk = partition_index.keys().next_back().cloned();
        let stats = SSTableStats {
            version: 2,
            row_count,
            min_partition_key: min_pk.clone(),
            max_partition_key: max_pk.clone(),
        };
        if let Ok(stats_json) = serde_json::to_string(&stats) {
            tokio::fs::write(&stats_file_path, stats_json).await.ok();
        }

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
            row_count: Some(row_count),
            min_partition_key: min_pk,
            max_partition_key: max_pk,
        })
    }

    /// 기존 SSTable 파일 열기
    pub async fn open(file_path: &std::path::Path) -> Result<Self> {
        let mut file = File::open(file_path).await?;
        
        // 헤더 읽기
        let mut header_buf = vec![0u8; 128]; // 충분한 크기
        file.read_exact(&mut header_buf).await.ok();
        file.seek(SeekFrom::Start(0)).await?;
        
        // 헤더 크기 추정 (고정 크기 사용)
        let header_size = std::mem::size_of::<SSTableHeader>() + 16; // 여유 공간
        let mut header_data = vec![0u8; header_size];
        file.read_exact(&mut header_data).await.ok();
        
        let header: SSTableHeader = bincode::deserialize(&header_data)
            .map_err(|e| CoreDBError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?;
        
        // Bloom filter 읽기 (별도 파일 우선)
        let bloom_file_path = file_path.with_file_name(
            file_path.file_stem().unwrap().to_string_lossy().replace("-Data", "-Bloom") + ".db"
        );
        
        let bloom_filter = if bloom_file_path.exists() {
            // 별도 파일에서 로드
            let bloom_data = tokio::fs::read(&bloom_file_path).await?;
            bincode::deserialize(&bloom_data).unwrap_or_else(|_| BloomFilter::new(1000, 0.01))
        } else {
            // 레거시: Data.db에서 로드 시도
            file.seek(SeekFrom::Start(header.bloom_filter_offset)).await?;
            let bloom_size = if header.partition_index_offset > header.bloom_filter_offset {
                (header.partition_index_offset - header.bloom_filter_offset) as usize
            } else {
                4096
            };
            let mut bloom_buf = vec![0u8; bloom_size];
            file.read_exact(&mut bloom_buf).await.ok();
            bincode::deserialize(&bloom_buf).unwrap_or_else(|_| BloomFilter::new(1000, 0.01))
        };
        
        // Partition index 읽기 (JSON 파일 우선)
        let index_file_path = file_path.with_file_name(
            file_path.file_stem().unwrap().to_string_lossy().replace("-Data", "-Index") + ".json"
        );
        
        let partition_index: BTreeMap<PartitionKey, u64> = if index_file_path.exists() {
            // JSON 파일에서 로드 (안정적)
            let index_json = tokio::fs::read_to_string(&index_file_path).await?;
            let index_vec: Vec<(PartitionKey, u64)> = serde_json::from_str(&index_json).unwrap_or_default();
            
            index_vec.into_iter().collect()
        } else {
            // 레거시: bincode에서 로드 시도
            file.seek(SeekFrom::Start(header.partition_index_offset)).await?;
            let mut index_buf = Vec::new();
            let summary_size = if header.summary_index_offset > header.partition_index_offset {
                (header.summary_index_offset - header.partition_index_offset) as usize
            } else {
                1024 * 1024
            };
            index_buf.resize(summary_size, 0);
            file.read_exact(&mut index_buf).await.ok();
            bincode::deserialize(&index_buf).unwrap_or_default()
        };
        
        // Summary index 읽기
        file.seek(SeekFrom::Start(header.summary_index_offset)).await?;
        let mut summary_buf = vec![0u8; 4096];
        file.read_exact(&mut summary_buf).await.ok();
        let summary_index: BTreeMap<PartitionKey, u64> = bincode::deserialize(&summary_buf)
            .unwrap_or_default();
        
        // 파일 크기
        let metadata = tokio::fs::metadata(file_path).await?;
        
        // ID 추출 (파일 이름에서)
        let id = file_path.file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.replace("-Data", ""))
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        
        // Stats sidecar — optional. Legacy SSTables on disk won't
        // have one. When absent we backfill it inline: scan every
        // partition once to count rows, persist the sidecar, then
        // hand back a fully-stats'd SSTable. One-time cost per
        // legacy file at startup; subsequent CoreDB starts are
        // fast.
        let stats_file_path = file_path.with_file_name(
            file_path.file_stem().unwrap().to_string_lossy().replace("-Data", "-Stats") + ".json",
        );
        let loaded_stats: Option<SSTableStats> = if stats_file_path.exists() {
            tokio::fs::read_to_string(&stats_file_path)
                .await
                .ok()
                .and_then(|s| serde_json::from_str::<SSTableStats>(&s).ok())
        } else {
            None
        };
        let row_count_from_sidecar = loaded_stats.as_ref().map(|s| s.row_count);
        let min_pk_from_sidecar = loaded_stats
            .as_ref()
            .and_then(|s| s.min_partition_key.clone());
        let max_pk_from_sidecar = loaded_stats
            .as_ref()
            .and_then(|s| s.max_partition_key.clone());

        let mut sstable = SSTable {
            id,
            file_path: file_path.to_path_buf(),
            bloom_filter,
            partition_index,
            summary_index,
            min_timestamp: header.min_timestamp,
            max_timestamp: header.max_timestamp,
            compression: header.compression,
            size_bytes: metadata.len(),
            row_count: row_count_from_sidecar,
            min_partition_key: min_pk_from_sidecar,
            max_partition_key: max_pk_from_sidecar,
        };

        // Treat the row_count and min/max bounds as a single "stats
        // backfill" event: an SSTable that's missing either signal
        // came from a pre-stats build (or a pre-v2 build that knew
        // row_count but not bounds), so we regenerate the sidecar
        // once and self-heal.
        let needs_row_count_backfill = sstable.row_count.is_none();
        let needs_bounds_backfill = sstable.min_partition_key.is_none()
            && !sstable.partition_index.is_empty();
        if needs_row_count_backfill || needs_bounds_backfill {
            // Empty SSTable shortcuts: 0 rows, no bounds — no
            // partition reads needed.
            if sstable.partition_index.is_empty() {
                if sstable.row_count.is_none() {
                    sstable.row_count = Some(0);
                }
            } else {
                // Bounds are free from the in-memory partition_index
                // regardless of whether row_count needs a scan.
                if sstable.min_partition_key.is_none() {
                    sstable.min_partition_key =
                        sstable.partition_index.keys().next().cloned();
                    sstable.max_partition_key =
                        sstable.partition_index.keys().next_back().cloned();
                }
                if needs_row_count_backfill {
                    let mut total: u64 = 0;
                    let keys: Vec<PartitionKey> =
                        sstable.partition_index.keys().cloned().collect();
                    for pk in &keys {
                        if let Ok(Some(partition)) = sstable.read_partition(pk).await {
                            total += partition.rows.len() as u64;
                        }
                    }
                    sstable.row_count = Some(total);
                }
                let stats = SSTableStats {
                    version: 2,
                    row_count: sstable.row_count.unwrap_or(0),
                    min_partition_key: sstable.min_partition_key.clone(),
                    max_partition_key: sstable.max_partition_key.clone(),
                };
                if let Ok(stats_json) = serde_json::to_string(&stats) {
                    if let Err(e) = tokio::fs::write(&stats_file_path, stats_json).await {
                        // Non-fatal: the sidecar will be regenerated
                        // on the next open. Just log and continue.
                        tracing::warn!(
                            "sstable {}: stats backfill write failed ({e}); fast paths will retry next open",
                            sstable.id,
                        );
                    } else {
                        tracing::info!(
                            "sstable {}: backfilled stats sidecar (row_count={}, bounds_known={})",
                            sstable.id,
                            sstable.row_count.unwrap_or(0),
                            sstable.min_partition_key.is_some(),
                        );
                    }
                }
            }
        }

        Ok(sstable)
    }

    /// Cheap O(1) range check against this SSTable's persisted
    /// `[min_partition_key, max_partition_key]` bounds. Returns
    /// `true` only when the key is *provably* outside the range —
    /// safe to skip this SSTable without entering [`Self::read_partition`].
    /// Returns `false` when the bounds are unknown (legacy v1 stats
    /// sidecar, empty SSTable) so the caller falls through to the
    /// standard read path.
    ///
    /// Intended for the engine's point-lookup paths so they can
    /// `continue` over irrelevant SSTables without dispatching an
    /// async fn. `read_partition` enforces the same veto internally
    /// as a safety net — skipping this check still produces correct
    /// results; it just costs the async dispatch + BTreeMap lookup
    /// this would have avoided.
    pub fn excludes_partition_key(&self, key: &PartitionKey) -> bool {
        match (&self.min_partition_key, &self.max_partition_key) {
            (Some(min), Some(max)) => key < min || key > max,
            _ => false,
        }
    }

    /// 파티션 읽기
    pub async fn read_partition(&self, partition_key: &PartitionKey) -> Result<Option<Partition>> {
        // Range veto: O(1) bounds check before the O(log N)
        // partition_index lookup. When the sidecar stored min/max
        // bounds (v2 stats), any key outside `[min, max]` is
        // guaranteed-absent and we don't even need to consult the
        // BTreeMap. For legacy SSTables with no bounds known, we
        // fall through to the index lookup unchanged.
        if let (Some(min), Some(max)) = (&self.min_partition_key, &self.max_partition_key) {
            if partition_key < min || partition_key > max {
                return Ok(None);
            }
        }
        // Authoritative check: if the in-memory partition_index has the
        // key, the partition is in this SSTable. The bloom filter used
        // to be queried first as a fast-negative hint, but the disk
        // serialization round-trip leaves the loaded filter
        // false-negativing every key that's actually present, so a
        // bloom veto here silently dropped every restored partition on
        // startup. partition_index lookups are O(log n) against a
        // BTreeMap that's already in memory — the bloom optimization
        // wasn't pulling its weight anyway.
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
    async fn serialize_partition(partition: &Partition, compression: &CompressionType) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        
        // Static 컬럼들 직렬화
        let static_data = bincode::serialize(&partition.static_columns)?;
        data.write_u32_le(static_data.len() as u32).await?;
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
        
        data.write_u32_le(rows.len() as u32).await?;
        for row in &rows {
            let row_data = bincode::serialize(row)?;
            data.write_u32_le(row_data.len() as u32).await?;
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
    async fn deserialize_partition(data: &[u8], compression: &CompressionType) -> Result<Partition> {
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
        
        let rows = crossbeam_skiplist::SkipMap::new();
        
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
    pub async fn delete(&self) -> Result<()> {
        tokio::fs::remove_file(&self.file_path).await?;
        Ok(())
    }
    
    /// 파일 크기 가져오기
    pub async fn file_size(&self) -> Result<u64> {
        let metadata = tokio::fs::metadata(&self.file_path).await?;
        Ok(metadata.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{CassandraValue, ColumnDefinition, CassandraDataType, Cell, ClusteringKey};
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
    
    /// On create, the SSTable carries min/max partition-key bounds
    /// derived from the sorted partition_index. The bounds survive a
    /// close/reopen round-trip via the v2 Stats sidecar, so a daemon
    /// restart doesn't lose the prune-fast-path.
    #[tokio::test]
    async fn stats_sidecar_persists_min_max_partition_key() {
        let temp_dir = std::env::temp_dir().join("coredb_test_minmax");
        if temp_dir.exists() {
            tokio::fs::remove_dir_all(&temp_dir).await.unwrap();
        }
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let schema = create_test_schema();
        let memtable = crate::storage::Memtable::new(schema);
        for i in [4, 1, 3, 7, 2] {
            memtable.put(create_test_row(i, (i * 1000) as i64, "v")).unwrap();
        }
        let sstable = SSTable::create_from_memtable(&memtable, &temp_dir, CompressionType::None)
            .await
            .unwrap();

        let min_expected = PartitionKey { components: vec![CassandraValue::Int(1)] };
        let max_expected = PartitionKey { components: vec![CassandraValue::Int(7)] };
        assert_eq!(sstable.min_partition_key.as_ref(), Some(&min_expected));
        assert_eq!(sstable.max_partition_key.as_ref(), Some(&max_expected));

        // Reopen via SSTable::open and confirm the sidecar round-tripped.
        let reopened = SSTable::open(&sstable.file_path).await.unwrap();
        assert_eq!(reopened.min_partition_key.as_ref(), Some(&min_expected));
        assert_eq!(reopened.max_partition_key.as_ref(), Some(&max_expected));

        sstable.delete().await.unwrap();
    }

    /// `excludes_partition_key` is the cheap O(1) bounds check the
    /// engine point-lookup path uses to skip irrelevant SSTables
    /// without dispatching the async `read_partition`. Bounds-known
    /// SSTables veto out-of-range keys and pass in-range keys
    /// through; bounds-unknown SSTables (legacy v1 sidecars, empty
    /// SSTables) never veto — they fall through to the standard
    /// read path which is still correct.
    #[tokio::test]
    async fn excludes_partition_key_only_vetoes_when_bounds_known() {
        let temp_dir = std::env::temp_dir().join("coredb_test_excl_pk");
        if temp_dir.exists() {
            tokio::fs::remove_dir_all(&temp_dir).await.unwrap();
        }
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let schema = create_test_schema();
        let memtable = crate::storage::Memtable::new(schema);
        for i in [10, 20, 30] {
            memtable.put(create_test_row(i, (i * 1000) as i64, "v")).unwrap();
        }
        let mut sstable = SSTable::create_from_memtable(&memtable, &temp_dir, CompressionType::None)
            .await
            .unwrap();

        // Bounds-known: in-range / out-of-range answers match
        // partition-key ordering.
        let in_range = PartitionKey { components: vec![CassandraValue::Int(20)] };
        let above_range = PartitionKey { components: vec![CassandraValue::Int(99)] };
        let below_range = PartitionKey { components: vec![CassandraValue::Int(0)] };
        assert!(!sstable.excludes_partition_key(&in_range));
        assert!(sstable.excludes_partition_key(&above_range));
        assert!(sstable.excludes_partition_key(&below_range));
        // Boundary inclusivity: equal to min/max is NOT excluded.
        assert!(!sstable.excludes_partition_key(
            sstable.min_partition_key.as_ref().unwrap()
        ));
        assert!(!sstable.excludes_partition_key(
            sstable.max_partition_key.as_ref().unwrap()
        ));

        // Bounds-unknown: simulate a legacy SSTable that didn't
        // record bounds (e.g. v1 sidecar that hadn't been
        // backfilled yet). `excludes_partition_key` must NEVER veto
        // in this state — the caller has to consult read_partition
        // and the partition_index instead. Letting it veto here
        // would silently hide every key on a legacy SSTable until
        // its first reopen.
        sstable.min_partition_key = None;
        sstable.max_partition_key = None;
        assert!(!sstable.excludes_partition_key(&in_range));
        assert!(!sstable.excludes_partition_key(&above_range));
        assert!(!sstable.excludes_partition_key(&below_range));

        sstable.delete().await.ok();
    }

    /// `read_partition` short-circuits with `Ok(None)` when the
    /// requested key is outside `[min, max]`, regardless of what
    /// partition_index would say. Confirms the O(1) range veto is
    /// wired in — operationally important once N SSTables stack up.
    #[tokio::test]
    async fn read_partition_returns_none_for_out_of_range_key() {
        let temp_dir = std::env::temp_dir().join("coredb_test_range_veto");
        if temp_dir.exists() {
            tokio::fs::remove_dir_all(&temp_dir).await.unwrap();
        }
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let schema = create_test_schema();
        let memtable = crate::storage::Memtable::new(schema);
        for i in [10, 20, 30] {
            memtable.put(create_test_row(i, (i * 1000) as i64, "v")).unwrap();
        }
        let sstable = SSTable::create_from_memtable(&memtable, &temp_dir, CompressionType::None)
            .await
            .unwrap();
        // Sanity: bounds came back from create.
        assert!(sstable.min_partition_key.is_some());
        assert!(sstable.max_partition_key.is_some());

        // Below the min: no partition.
        let below = PartitionKey { components: vec![CassandraValue::Int(1)] };
        assert!(sstable.read_partition(&below).await.unwrap().is_none());
        // Above the max: no partition.
        let above = PartitionKey { components: vec![CassandraValue::Int(99)] };
        assert!(sstable.read_partition(&above).await.unwrap().is_none());
        // Inside the range but not present (gap between 10 and 20):
        // the partition_index lookup is still authoritative — this
        // returns None too, just on the second branch instead of the
        // bounds veto.
        let gap = PartitionKey { components: vec![CassandraValue::Int(15)] };
        assert!(sstable.read_partition(&gap).await.unwrap().is_none());
        // Inside the range and present: returns Some.
        let present = PartitionKey { components: vec![CassandraValue::Int(20)] };
        assert!(sstable.read_partition(&present).await.unwrap().is_some());

        sstable.delete().await.unwrap();
    }

    /// v1 Stats sidecar (row_count only, no bounds) → on reopen the
    /// SSTable backfills both bounds from the in-memory partition_index
    /// and rewrites the sidecar at version 2. Next open is fully
    /// stats'd without rescanning partitions.
    #[tokio::test]
    async fn open_backfills_v1_sidecar_to_v2_with_bounds() {
        let temp_dir = std::env::temp_dir().join("coredb_test_v1_backfill");
        if temp_dir.exists() {
            tokio::fs::remove_dir_all(&temp_dir).await.unwrap();
        }
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let schema = create_test_schema();
        let memtable = crate::storage::Memtable::new(schema);
        for i in [5, 6, 8] {
            memtable.put(create_test_row(i, (i * 1000) as i64, "v")).unwrap();
        }
        let sstable = SSTable::create_from_memtable(&memtable, &temp_dir, CompressionType::None)
            .await
            .unwrap();

        // Simulate a pre-v2 sidecar on disk: rewrite the Stats.json
        // with version 1 + row_count, no bounds. Older binaries that
        // ran before this commit would have written exactly this
        // shape.
        let stats_path = sstable.file_path.with_file_name(
            sstable
                .file_path
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .replace("-Data", "-Stats")
                + ".json",
        );
        let v1_json = serde_json::json!({
            "version": 1,
            "row_count": 3,
        })
        .to_string();
        tokio::fs::write(&stats_path, v1_json).await.unwrap();

        let reopened = SSTable::open(&sstable.file_path).await.unwrap();
        // Bounds got backfilled from partition_index, not lost.
        assert_eq!(
            reopened.min_partition_key,
            Some(PartitionKey { components: vec![CassandraValue::Int(5)] }),
        );
        assert_eq!(
            reopened.max_partition_key,
            Some(PartitionKey { components: vec![CassandraValue::Int(8)] }),
        );

        // And the sidecar on disk was rewritten as v2 with bounds.
        let rewritten: SSTableStats = serde_json::from_str(
            &tokio::fs::read_to_string(&stats_path).await.unwrap(),
        )
        .unwrap();
        assert_eq!(rewritten.version, 2);
        assert!(rewritten.min_partition_key.is_some());
        assert!(rewritten.max_partition_key.is_some());

        sstable.delete().await.unwrap();
    }

    #[tokio::test]
    async fn test_sstable_creation_and_read() {
        let temp_dir = std::env::temp_dir().join("coredb_test");
        if temp_dir.exists() {
            tokio::fs::remove_dir_all(&temp_dir).await.unwrap();
        }
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        
        let schema = create_test_schema();
        let memtable = crate::storage::Memtable::new(schema);
        
        // 테스트 데이터 추가
        for i in 1..=5 {
            let row = create_test_row(i, (i * 1000) as i64, &format!("value_{}", i));
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

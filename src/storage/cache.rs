use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::sync::RwLock;
use crate::schema::PartitionKey;
use crate::storage::memtable::Partition;

/// 캐시 엔트리
#[derive(Debug)]
struct CacheEntry {
    partition: Arc<Partition>,
    size_bytes: usize,
    access_count: AtomicU64,
    last_access: AtomicU64,
}

/// LRU Block Cache 설정
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// 최대 캐시 크기 (바이트)
    pub max_size_bytes: usize,
    /// 최대 엔트리 수
    pub max_entries: usize,
    /// 캐시 샤드 수 (동시성 향상)
    pub num_shards: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_size_bytes: 128 * 1024 * 1024, // 128MB
            max_entries: 10_000,
            num_shards: 16,
        }
    }
}

/// 캐시 통계
#[derive(Debug, Default)]
pub struct CacheStats {
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub evictions: AtomicU64,
    pub current_size: AtomicUsize,
    pub current_entries: AtomicUsize,
}

impl CacheStats {
    pub fn hit_rate(&self) -> f64 {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }
}

/// 캐시 키 (테이블 + 파티션)
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CacheKey {
    pub keyspace: String,
    pub table: String,
    pub partition_key: PartitionKey,
}

/// 샤드 (동시성을 위한 분할)
struct CacheShard {
    entries: RwLock<HashMap<CacheKey, CacheEntry>>,
    size_bytes: AtomicUsize,
}

impl CacheShard {
    fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            size_bytes: AtomicUsize::new(0),
        }
    }
}

/// LRU Block Cache
pub struct BlockCache {
    shards: Vec<CacheShard>,
    config: CacheConfig,
    stats: CacheStats,
    clock: AtomicU64,
}

impl BlockCache {
    pub fn new(config: CacheConfig) -> Self {
        let shards = (0..config.num_shards)
            .map(|_| CacheShard::new())
            .collect();
        
        Self {
            shards,
            config,
            stats: CacheStats::default(),
            clock: AtomicU64::new(0),
        }
    }
    
    /// 캐시에서 샤드 인덱스 계산
    fn shard_index(&self, key: &CacheKey) -> usize {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        (hasher.finish() as usize) % self.shards.len()
    }
    
    /// 현재 시간 틱 (LRU용)
    fn tick(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::Relaxed)
    }
    
    /// 캐시에서 파티션 조회
    pub async fn get(&self, key: &CacheKey) -> Option<Arc<Partition>> {
        let shard_idx = self.shard_index(key);
        let shard = &self.shards[shard_idx];
        
        let entries = shard.entries.read().await;
        if let Some(entry) = entries.get(key) {
            // 접근 횟수 및 시간 업데이트
            entry.access_count.fetch_add(1, Ordering::Relaxed);
            entry.last_access.store(self.tick(), Ordering::Relaxed);
            
            self.stats.hits.fetch_add(1, Ordering::Relaxed);
            Some(Arc::clone(&entry.partition))
        } else {
            self.stats.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
    
    /// 캐시에 파티션 저장
    pub async fn put(&self, key: CacheKey, partition: Partition) {
        let size_bytes = Self::estimate_partition_size(&partition);
        
        // 캐시가 가득 찼으면 eviction 수행
        self.maybe_evict(size_bytes).await;
        
        let shard_idx = self.shard_index(&key);
        let shard = &self.shards[shard_idx];
        
        let entry = CacheEntry {
            partition: Arc::new(partition),
            size_bytes,
            access_count: AtomicU64::new(1),
            last_access: AtomicU64::new(self.tick()),
        };
        
        let mut entries = shard.entries.write().await;
        
        // 기존 엔트리가 있으면 크기 조정
        if let Some(old) = entries.remove(&key) {
            shard.size_bytes.fetch_sub(old.size_bytes, Ordering::Relaxed);
            self.stats.current_size.fetch_sub(old.size_bytes, Ordering::Relaxed);
            self.stats.current_entries.fetch_sub(1, Ordering::Relaxed);
        }
        
        entries.insert(key, entry);
        shard.size_bytes.fetch_add(size_bytes, Ordering::Relaxed);
        self.stats.current_size.fetch_add(size_bytes, Ordering::Relaxed);
        self.stats.current_entries.fetch_add(1, Ordering::Relaxed);
    }
    
    /// 캐시에서 파티션 제거
    pub async fn invalidate(&self, key: &CacheKey) {
        let shard_idx = self.shard_index(key);
        let shard = &self.shards[shard_idx];
        
        let mut entries = shard.entries.write().await;
        if let Some(old) = entries.remove(key) {
            shard.size_bytes.fetch_sub(old.size_bytes, Ordering::Relaxed);
            self.stats.current_size.fetch_sub(old.size_bytes, Ordering::Relaxed);
            self.stats.current_entries.fetch_sub(1, Ordering::Relaxed);
        }
    }
    
    /// 테이블의 모든 캐시 무효화
    pub async fn invalidate_table(&self, keyspace: &str, table: &str) {
        for shard in &self.shards {
            let mut entries = shard.entries.write().await;
            let keys_to_remove: Vec<CacheKey> = entries.keys()
                .filter(|k| k.keyspace == keyspace && k.table == table)
                .cloned()
                .collect();
            
            for key in keys_to_remove {
                if let Some(old) = entries.remove(&key) {
                    shard.size_bytes.fetch_sub(old.size_bytes, Ordering::Relaxed);
                    self.stats.current_size.fetch_sub(old.size_bytes, Ordering::Relaxed);
                    self.stats.current_entries.fetch_sub(1, Ordering::Relaxed);
                }
            }
        }
    }
    
    /// 전체 캐시 클리어
    pub async fn clear(&self) {
        for shard in &self.shards {
            let mut entries = shard.entries.write().await;
            entries.clear();
            shard.size_bytes.store(0, Ordering::Relaxed);
        }
        self.stats.current_size.store(0, Ordering::Relaxed);
        self.stats.current_entries.store(0, Ordering::Relaxed);
    }
    
    /// LRU eviction 수행
    async fn maybe_evict(&self, needed_bytes: usize) {
        let current_size = self.stats.current_size.load(Ordering::Relaxed);
        let current_entries = self.stats.current_entries.load(Ordering::Relaxed);
        
        // 공간 또는 엔트리 수가 한계에 도달했는지 확인
        let need_eviction = current_size + needed_bytes > self.config.max_size_bytes
            || current_entries >= self.config.max_entries;
        
        if !need_eviction {
            return;
        }
        
        // 10% 공간 확보 목표
        let target_free = self.config.max_size_bytes / 10;
        let mut freed = 0usize;
        
        // 모든 샤드에서 LRU 엔트리 수집
        let mut candidates: Vec<(CacheKey, u64, usize)> = Vec::new();
        
        for shard in &self.shards {
            let entries = shard.entries.read().await;
            for (key, entry) in entries.iter() {
                candidates.push((
                    key.clone(),
                    entry.last_access.load(Ordering::Relaxed),
                    entry.size_bytes,
                ));
            }
        }
        
        // 접근 시간 기준 정렬 (오래된 것부터)
        candidates.sort_by_key(|(_, last_access, _)| *last_access);
        
        // Eviction 수행
        for (key, _, size) in candidates {
            if freed >= target_free {
                break;
            }
            
            self.invalidate(&key).await;
            freed += size;
            self.stats.evictions.fetch_add(1, Ordering::Relaxed);
        }
    }
    
    /// 파티션 크기 추정
    fn estimate_partition_size(partition: &Partition) -> usize {
        let mut size = 0usize;
        
        // Static 컬럼 크기
        for (k, v) in &partition.static_columns {
            size += k.len() + std::mem::size_of_val(v);
        }
        
        // 행 크기 (대략적 추정)
        size += partition.rows.len() * 256; // 평균 256 바이트/행
        
        size.max(64) // 최소 64 바이트
    }
    
    /// 통계 반환
    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }
    
    /// 캐시 상태 출력
    pub fn status(&self) -> String {
        format!(
            "BlockCache: {} entries, {} bytes, {:.1}% hit rate",
            self.stats.current_entries.load(Ordering::Relaxed),
            self.stats.current_size.load(Ordering::Relaxed),
            self.stats.hit_rate() * 100.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{CassandraValue, Cell};
    use crossbeam_skiplist::SkipMap;
    
    fn create_test_partition(num_rows: usize) -> Partition {
        let rows = SkipMap::new();
        for i in 0..num_rows {
            let row = crate::schema::Row {
                partition_key: crate::schema::PartitionKey {
                    components: vec![CassandraValue::Int(i as i32)],
                },
                clustering_key: Some(crate::schema::ClusteringKey {
                    components: vec![CassandraValue::BigInt(i as i64)],
                }),
                cells: {
                    let mut cells = std::collections::HashMap::new();
                    cells.insert("value".to_string(), Cell {
                        value: CassandraValue::Text(format!("test_value_{}", i)),
                        timestamp: 0,
                        ttl: None,
                        is_deleted: false,
                    });
                    cells
                },
                timestamp: 0,
            };
            rows.insert(row.clustering_key.clone(), row);
        }
        
        Partition {
            rows,
            static_columns: std::collections::HashMap::new(),
        }
    }
    
    #[tokio::test]
    async fn test_cache_put_get() {
        let config = CacheConfig {
            max_size_bytes: 1024 * 1024,
            max_entries: 100,
            num_shards: 4,
        };
        let cache = BlockCache::new(config);
        
        let key = CacheKey {
            keyspace: "test_ks".to_string(),
            table: "test_table".to_string(),
            partition_key: crate::schema::PartitionKey {
                components: vec![CassandraValue::Int(1)],
            },
        };
        
        let partition = create_test_partition(10);
        cache.put(key.clone(), partition).await;
        
        // Get should hit
        let result = cache.get(&key).await;
        assert!(result.is_some());
        
        // Stats check
        assert_eq!(cache.stats.hits.load(Ordering::Relaxed), 1);
        assert_eq!(cache.stats.misses.load(Ordering::Relaxed), 0);
    }
    
    #[tokio::test]
    async fn test_cache_miss() {
        let cache = BlockCache::new(CacheConfig::default());
        
        let key = CacheKey {
            keyspace: "test_ks".to_string(),
            table: "test_table".to_string(),
            partition_key: crate::schema::PartitionKey {
                components: vec![CassandraValue::Int(999)],
            },
        };
        
        let result = cache.get(&key).await;
        assert!(result.is_none());
        assert_eq!(cache.stats.misses.load(Ordering::Relaxed), 1);
    }
    
    #[tokio::test]
    async fn test_cache_eviction() {
        let config = CacheConfig {
            max_size_bytes: 1024, // Very small
            max_entries: 2,
            num_shards: 1,
        };
        let cache = BlockCache::new(config);
        
        // Insert 3 entries (should trigger eviction)
        for i in 0..3 {
            let key = CacheKey {
                keyspace: "test_ks".to_string(),
                table: "test_table".to_string(),
                partition_key: crate::schema::PartitionKey {
                    components: vec![CassandraValue::Int(i)],
                },
            };
            cache.put(key, create_test_partition(5)).await;
        }
        
        // Should have evicted at least one
        assert!(cache.stats.evictions.load(Ordering::Relaxed) > 0);
    }
}

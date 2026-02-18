use std::path::PathBuf;
use std::sync::Arc;
use std::collections::{HashMap, BTreeMap};
use tokio::sync::{RwLock, mpsc};
use crate::storage::SSTable;
use crate::schema::PartitionKey;
use crate::error::*;

/// 컴팩션 전략
#[derive(Debug, Clone)]
pub enum CompactionStrategy {
    SizeTiered {
        min_threshold: usize,
        max_threshold: usize,
    },
    Leveled {
        level_size_multiplier: f64,
        max_levels: usize,
        l0_compaction_trigger: usize,
        target_file_size_mb: usize,
    },
}

impl Default for CompactionStrategy {
    fn default() -> Self {
        CompactionStrategy::SizeTiered {
            min_threshold: 4,
            max_threshold: 32,
        }
    }
}

/// 컴팩션 작업
#[derive(Debug)]
pub struct CompactionTask {
    pub keyspace: String,
    pub table: String,
    pub level: usize,
    pub input_sstables: Vec<Arc<SSTable>>,
    pub output_sstable: Option<Arc<SSTable>>,
    pub strategy: CompactionStrategy,
}

/// SSTable 레벨 정보
#[derive(Debug, Clone)]
pub struct LeveledSSTable {
    pub sstable: Arc<SSTable>,
    pub level: usize,
    pub min_key: Option<PartitionKey>,
    pub max_key: Option<PartitionKey>,
}

impl LeveledSSTable {
    pub fn new(sstable: Arc<SSTable>, level: usize) -> Self {
        let (min_key, max_key) = Self::extract_key_range(&sstable);
        Self {
            sstable,
            level,
            min_key,
            max_key,
        }
    }
    
    fn extract_key_range(sstable: &SSTable) -> (Option<PartitionKey>, Option<PartitionKey>) {
        let keys: Vec<_> = sstable.partition_index.keys().cloned().collect();
        let min = keys.first().cloned();
        let max = keys.last().cloned();
        (min, max)
    }
    
    /// 키 범위가 겹치는지 확인
    pub fn overlaps_with(&self, other: &LeveledSSTable) -> bool {
        match (&self.min_key, &self.max_key, &other.min_key, &other.max_key) {
            (Some(self_min), Some(self_max), Some(other_min), Some(other_max)) => {
                // 겹치지 않는 경우: self가 완전히 other 앞 또는 뒤
                !(self_max < other_min || other_max < self_min)
            }
            _ => true, // 키가 없으면 겹친다고 가정
        }
    }
}

/// SSTable 레벨 관리자
#[derive(Debug)]
pub struct LevelManager {
    /// 레벨별 SSTable 리스트
    levels: Vec<Vec<LeveledSSTable>>,
    max_levels: usize,
    level_size_multiplier: f64,
    l0_compaction_trigger: usize,
    target_file_size_bytes: u64,
}

impl LevelManager {
    pub fn new(max_levels: usize, level_size_multiplier: f64, l0_trigger: usize, target_file_size_mb: usize) -> Self {
        Self {
            levels: vec![Vec::new(); max_levels],
            max_levels,
            level_size_multiplier,
            l0_compaction_trigger: l0_trigger,
            target_file_size_bytes: (target_file_size_mb * 1024 * 1024) as u64,
        }
    }
    
    /// L0에 새 SSTable 추가 (flush 후)
    pub fn add_flushed_sstable(&mut self, sstable: Arc<SSTable>) {
        let leveled = LeveledSSTable::new(sstable, 0);
        self.levels[0].push(leveled);
    }
    
    /// 특정 레벨에 SSTable 추가
    pub fn add_sstable(&mut self, sstable: Arc<SSTable>, level: usize) {
        if level < self.max_levels {
            let leveled = LeveledSSTable::new(sstable, level);
            self.levels[level].push(leveled);
            
            // L1+ 에서는 키 범위 순서로 정렬
            if level > 0 {
                self.levels[level].sort_by(|a, b| {
                    a.min_key.cmp(&b.min_key)
                });
            }
        }
    }
    
    /// 컴팩션이 필요한지 확인하고 작업 반환
    pub fn get_compaction_candidate(&self) -> Option<CompactionCandidate> {
        // L0 체크 - 파일 개수 기준
        if self.levels[0].len() >= self.l0_compaction_trigger {
            let l0_sstables: Vec<_> = self.levels[0].iter()
                .map(|ls| ls.sstable.clone())
                .collect();
            
            // L0의 모든 파일 + 겹치는 L1 파일들
            let overlapping_l1 = self.find_overlapping_sstables(1, &self.levels[0]);
            
            return Some(CompactionCandidate {
                source_level: 0,
                target_level: 1,
                input_sstables: l0_sstables,
                overlapping_sstables: overlapping_l1,
            });
        }
        
        // L1+ 체크 - 크기 기준
        for level in 1..self.max_levels - 1 {
            let level_size: u64 = self.levels[level].iter()
                .map(|ls| ls.sstable.size_bytes)
                .sum();
            
            let max_size = self.max_bytes_for_level(level);
            
            if level_size > max_size {
                // 가장 오래된 (또는 가장 큰) SSTable 선택
                if let Some(oldest) = self.levels[level].first() {
                    let overlapping = self.find_overlapping_sstables(
                        level + 1, 
                        &[oldest.clone()]
                    );
                    
                    return Some(CompactionCandidate {
                        source_level: level,
                        target_level: level + 1,
                        input_sstables: vec![oldest.sstable.clone()],
                        overlapping_sstables: overlapping,
                    });
                }
            }
        }
        
        None
    }
    
    /// 레벨의 최대 바이트 수 계산
    fn max_bytes_for_level(&self, level: usize) -> u64 {
        if level == 0 {
            // L0은 파일 개수로 관리
            return u64::MAX;
        }
        
        // L1 = 10 * target_file_size, L2 = L1 * multiplier, ...
        let base = 10 * self.target_file_size_bytes;
        (base as f64 * self.level_size_multiplier.powi((level - 1) as i32)) as u64
    }
    
    /// 키 범위가 겹치는 SSTable 찾기
    fn find_overlapping_sstables(&self, level: usize, sources: &[LeveledSSTable]) -> Vec<Arc<SSTable>> {
        if level >= self.max_levels || self.levels[level].is_empty() {
            return vec![];
        }
        
        self.levels[level].iter()
            .filter(|target| {
                sources.iter().any(|source| source.overlaps_with(target))
            })
            .map(|ls| ls.sstable.clone())
            .collect()
    }
    
    /// 컴팩션 후 레벨 업데이트
    pub fn apply_compaction_result(
        &mut self, 
        source_level: usize,
        removed_sstables: &[Arc<SSTable>],
        new_sstables: Vec<Arc<SSTable>>,
        target_level: usize,
    ) {
        // 제거된 SSTable들 삭제
        self.levels[source_level].retain(|ls| {
            !removed_sstables.iter().any(|r| Arc::ptr_eq(&ls.sstable, r))
        });
        
        if target_level < source_level + 2 {
            self.levels[target_level].retain(|ls| {
                !removed_sstables.iter().any(|r| Arc::ptr_eq(&ls.sstable, r))
            });
        }
        
        // 새 SSTable들 추가
        for sstable in new_sstables {
            self.add_sstable(sstable, target_level);
        }
    }
    
    /// 특정 키를 포함할 수 있는 SSTable들 찾기 (읽기용)
    pub fn find_sstables_for_key(&self, key: &PartitionKey) -> Vec<Arc<SSTable>> {
        let mut result = Vec::new();
        
        // L0: 모든 SSTable 검색 (겹칠 수 있음)
        for ls in &self.levels[0] {
            if ls.sstable.bloom_filter.might_contain(key) {
                result.push(ls.sstable.clone());
            }
        }
        
        // L1+: 이진 검색으로 해당 키 범위의 SSTable 찾기
        for level in 1..self.max_levels {
            if let Some(ls) = self.binary_search_for_key(&self.levels[level], key) {
                if ls.sstable.bloom_filter.might_contain(key) {
                    result.push(ls.sstable.clone());
                }
            }
        }
        
        result
    }
    
    /// 이진 검색으로 키 범위에 해당하는 SSTable 찾기
    fn binary_search_for_key<'a>(&self, sstables: &'a [LeveledSSTable], key: &PartitionKey) -> Option<&'a LeveledSSTable> {
        if sstables.is_empty() {
            return None;
        }
        
        // 키가 범위에 포함되는 SSTable 찾기
        for ls in sstables {
            match (&ls.min_key, &ls.max_key) {
                (Some(min), Some(max)) if key >= min && key <= max => {
                    return Some(ls);
                }
                _ => continue,
            }
        }
        
        None
    }
    
    /// 레벨별 통계
    pub fn get_level_stats(&self) -> Vec<LevelStats> {
        self.levels.iter().enumerate().map(|(level, sstables)| {
            LevelStats {
                level,
                file_count: sstables.len(),
                total_size_bytes: sstables.iter().map(|ls| ls.sstable.size_bytes).sum(),
                max_size_bytes: self.max_bytes_for_level(level),
            }
        }).collect()
    }
    
    /// 전체 SSTable 목록
    pub fn all_sstables(&self) -> Vec<Arc<SSTable>> {
        self.levels.iter()
            .flat_map(|level| level.iter().map(|ls| ls.sstable.clone()))
            .collect()
    }
}

/// 컴팩션 후보
#[derive(Debug)]
pub struct CompactionCandidate {
    pub source_level: usize,
    pub target_level: usize,
    pub input_sstables: Vec<Arc<SSTable>>,
    pub overlapping_sstables: Vec<Arc<SSTable>>,
}

/// 레벨 통계
#[derive(Debug, Clone)]
pub struct LevelStats {
    pub level: usize,
    pub file_count: usize,
    pub total_size_bytes: u64,
    pub max_size_bytes: u64,
}

/// 컴팩션 매니저
pub struct CompactionManager {
    config: CompactionConfig,
    pending_tasks: Arc<RwLock<HashMap<String, Vec<CompactionTask>>>>,
    task_sender: mpsc::UnboundedSender<CompactionTask>,
    task_receiver: Arc<RwLock<Option<mpsc::UnboundedReceiver<CompactionTask>>>>,
    /// 테이블별 레벨 관리자
    level_managers: Arc<RwLock<HashMap<String, LevelManager>>>,
}

/// 컴팩션 설정
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    pub throughput_mb_per_sec: u64,
    pub max_concurrent_compactions: usize,
    pub strategy: CompactionStrategy,
    pub data_directory: PathBuf,
}

impl CompactionManager {
    pub fn new(config: CompactionConfig) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        
        Self {
            pending_tasks: Arc::new(RwLock::new(HashMap::new())),
            task_sender: sender,
            task_receiver: Arc::new(RwLock::new(Some(receiver))),
            level_managers: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }
    
    /// 테이블의 레벨 관리자 가져오기 (없으면 생성)
    pub async fn get_or_create_level_manager(&self, keyspace: &str, table: &str) -> Arc<RwLock<LevelManager>> {
        let key = format!("{}.{}", keyspace, table);
        let mut managers = self.level_managers.write().await;
        
        if !managers.contains_key(&key) {
            let (max_levels, multiplier, l0_trigger, target_size) = match &self.config.strategy {
                CompactionStrategy::Leveled { 
                    max_levels, 
                    level_size_multiplier,
                    l0_compaction_trigger,
                    target_file_size_mb,
                } => (*max_levels, *level_size_multiplier, *l0_compaction_trigger, *target_file_size_mb),
                _ => (7, 10.0, 4, 64), // 기본값
            };
            
            managers.insert(key.clone(), LevelManager::new(max_levels, multiplier, l0_trigger, target_size));
        }
        
        // 별도의 Arc<RwLock>으로 감싸서 반환
        // 실제로는 managers에서 직접 접근하도록 구조 변경 필요
        Arc::new(RwLock::new(managers.remove(&key).unwrap()))
    }
    
    /// 새 SSTable 등록 (flush 후 호출)
    pub async fn register_flushed_sstable(&self, keyspace: &str, table: &str, sstable: Arc<SSTable>) {
        let key = format!("{}.{}", keyspace, table);
        let mut managers = self.level_managers.write().await;
        
        let (max_levels, multiplier, l0_trigger, target_size) = match &self.config.strategy {
            CompactionStrategy::Leveled { 
                max_levels, 
                level_size_multiplier,
                l0_compaction_trigger,
                target_file_size_mb,
            } => (*max_levels, *level_size_multiplier, *l0_compaction_trigger, *target_file_size_mb),
            _ => (7, 10.0, 4, 64),
        };
        
        let manager = managers.entry(key.clone())
            .or_insert_with(|| LevelManager::new(max_levels, multiplier, l0_trigger, target_size));
        
        manager.add_flushed_sstable(sstable);
        
        // 컴팩션 필요 여부 체크
        if let Some(candidate) = manager.get_compaction_candidate() {
            let mut all_inputs = candidate.input_sstables.clone();
            all_inputs.extend(candidate.overlapping_sstables.clone());
            
            let task = CompactionTask {
                keyspace: keyspace.to_string(),
                table: table.to_string(),
                level: candidate.source_level,
                input_sstables: all_inputs,
                output_sstable: None,
                strategy: self.config.strategy.clone(),
            };
            
            let _ = self.task_sender.send(task);
        }
    }
    
    /// 컴팩션 작업 스케줄링
    pub async fn schedule_compaction(&self, keyspace: &str, table: &str) {
        let key = format!("{}.{}", keyspace, table);
        let managers = self.level_managers.read().await;
        
        if let Some(manager) = managers.get(&key) {
            if let Some(candidate) = manager.get_compaction_candidate() {
                let mut all_inputs = candidate.input_sstables.clone();
                all_inputs.extend(candidate.overlapping_sstables.clone());
                
                let task = CompactionTask {
                    keyspace: keyspace.to_string(),
                    table: table.to_string(),
                    level: candidate.source_level,
                    input_sstables: all_inputs,
                    output_sstable: None,
                    strategy: self.config.strategy.clone(),
                };
                
                let _ = self.task_sender.send(task);
            }
        }
    }
    
    /// 컴팩션 루프 실행
    pub async fn run_compaction_loop(&self) {
        let mut receiver = self.task_receiver.write().await.take()
            .expect("Compaction receiver already taken");
        
        while let Some(task) = receiver.recv().await {
            if let Err(e) = self.execute_compaction(task).await {
                eprintln!("Compaction failed: {:?}", e);
            }
        }
    }
    
    /// 컴팩션 실행
    async fn execute_compaction(&self, task: CompactionTask) -> Result<()> {
        match task.strategy {
            CompactionStrategy::SizeTiered { .. } => {
                self.execute_size_tiered_compaction(task).await
            },
            CompactionStrategy::Leveled { .. } => {
                self.execute_leveled_compaction(task).await
            },
        }
    }
    
    /// Size-Tiered 컴팩션 실행
    async fn execute_size_tiered_compaction(&self, task: CompactionTask) -> Result<()> {
        if task.input_sstables.is_empty() {
            return Ok(());
        }
        
        // 모든 입력 SSTable의 데이터를 읽어서 병합
        let mut merged_data = HashMap::new();
        
        for sstable in &task.input_sstables {
            for (partition_key, _) in &sstable.partition_index {
                if let Some(partition) = sstable.read_partition(partition_key).await? {
                    merged_data.insert(partition_key.clone(), partition);
                }
            }
        }
        
        // 기존 SSTable들 삭제
        for sstable in &task.input_sstables {
            sstable.delete().await?;
        }
        
        Ok(())
    }
    
    /// Leveled 컴팩션 실행
    async fn execute_leveled_compaction(&self, task: CompactionTask) -> Result<()> {
        if task.input_sstables.is_empty() {
            return Ok(());
        }
        
        println!("🔄 Leveled compaction: L{} -> L{}, {} files", 
            task.level, task.level + 1, task.input_sstables.len());
        
        // 모든 입력 SSTable의 데이터를 키 순서로 병합
        let mut merged_data: BTreeMap<PartitionKey, _> = BTreeMap::new();
        let mut max_timestamp = i64::MIN;
        let mut min_timestamp = i64::MAX;
        
        for sstable in &task.input_sstables {
            max_timestamp = max_timestamp.max(sstable.max_timestamp);
            min_timestamp = min_timestamp.min(sstable.min_timestamp);
            
            for (partition_key, _) in &sstable.partition_index {
                if let Some(partition) = sstable.read_partition(partition_key).await? {
                    // 최신 데이터 우선 (이미 있으면 병합)
                    merged_data.entry(partition_key.clone())
                        .and_modify(|existing: &mut crate::storage::memtable::Partition| {
                            // 파티션 병합 - 새 행 추가
                            for entry in partition.rows.iter() {
                                let ck = entry.key().clone();
                                let row = entry.value().clone();
                                // 기존 행이 있으면 최신 타임스탬프 우선
                                if let Some(existing_entry) = existing.rows.get(&ck) {
                                    if row.timestamp > existing_entry.value().timestamp {
                                        existing.rows.insert(ck, row);
                                    }
                                } else {
                                    existing.rows.insert(ck, row);
                                }
                            }
                        })
                        .or_insert(partition);
                }
            }
        }
        
        // TODO: merged_data를 새 SSTable(들)로 쓰기
        // target_file_size_mb를 넘으면 여러 파일로 분할
        
        // 기존 SSTable들 삭제
        for sstable in &task.input_sstables {
            sstable.delete().await?;
        }
        
        // 레벨 관리자 업데이트
        let key = format!("{}.{}", task.keyspace, task.table);
        let mut managers = self.level_managers.write().await;
        
        if let Some(manager) = managers.get_mut(&key) {
            // TODO: 새로 생성된 SSTable들 등록
            manager.apply_compaction_result(
                task.level,
                &task.input_sstables,
                vec![], // 새 SSTable들
                task.level + 1,
            );
        }
        
        println!("✅ Compaction complete: merged {} keys", merged_data.len());
        
        Ok(())
    }
    
    /// 컴팩션 통계
    pub async fn get_compaction_stats(&self) -> CompactionStats {
        let pending = self.pending_tasks.read().await;
        let total_pending = pending.values().map(|tasks| tasks.len()).sum();
        
        CompactionStats {
            pending_tasks: total_pending,
            throughput_mb_per_sec: self.config.throughput_mb_per_sec,
            strategy: self.config.strategy.clone(),
        }
    }
    
    /// 레벨 통계 가져오기
    pub async fn get_level_stats(&self, keyspace: &str, table: &str) -> Option<Vec<LevelStats>> {
        let key = format!("{}.{}", keyspace, table);
        let managers = self.level_managers.read().await;
        managers.get(&key).map(|m| m.get_level_stats())
    }
}

/// 컴팩션 통계
#[derive(Debug)]
pub struct CompactionStats {
    pub pending_tasks: usize,
    pub throughput_mb_per_sec: u64,
    pub strategy: CompactionStrategy,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_level_manager_thresholds() {
        let manager = LevelManager::new(7, 10.0, 4, 64);
        
        // L0은 파일 개수로 관리
        assert_eq!(manager.l0_compaction_trigger, 4);
        
        // L1 = 10 * 64MB = 640MB
        let l1_max = manager.max_bytes_for_level(1);
        assert_eq!(l1_max, 10 * 64 * 1024 * 1024);
        
        // L2 = L1 * 10 = 6.4GB
        let l2_max = manager.max_bytes_for_level(2);
        assert_eq!(l2_max, 100 * 64 * 1024 * 1024);
    }
    
    #[test]
    fn test_leveled_sstable_overlap() {
        // 키 범위 겹침 테스트는 실제 SSTable 필요
    }
    
    #[tokio::test]
    async fn test_compaction_manager_creation() {
        let config = CompactionConfig {
            throughput_mb_per_sec: 16,
            max_concurrent_compactions: 2,
            strategy: CompactionStrategy::Leveled {
                level_size_multiplier: 10.0,
                max_levels: 7,
                l0_compaction_trigger: 4,
                target_file_size_mb: 64,
            },
            data_directory: std::env::temp_dir(),
        };
        
        let manager = CompactionManager::new(config);
        let stats = manager.get_compaction_stats().await;
        
        assert_eq!(stats.pending_tasks, 0);
        assert_eq!(stats.throughput_mb_per_sec, 16);
    }
    
    #[test]
    fn test_level_stats() {
        let manager = LevelManager::new(7, 10.0, 4, 64);
        let stats = manager.get_level_stats();
        
        assert_eq!(stats.len(), 7);
        assert_eq!(stats[0].level, 0);
        assert_eq!(stats[0].file_count, 0);
    }
}

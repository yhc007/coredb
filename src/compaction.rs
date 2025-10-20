use std::path::PathBuf;
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::{RwLock, mpsc};
use crate::storage::SSTable;
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
    pub input_sstables: Vec<Arc<SSTable>>,
    pub output_sstable: Option<Arc<SSTable>>,
    pub strategy: CompactionStrategy,
}

/// 컴팩션 매니저
pub struct CompactionManager {
    config: CompactionConfig,
    pending_tasks: Arc<RwLock<HashMap<String, Vec<CompactionTask>>>>,
    task_sender: mpsc::UnboundedSender<CompactionTask>,
    task_receiver: Arc<RwLock<Option<mpsc::UnboundedReceiver<CompactionTask>>>>,
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
            config,
        }
    }
    
    /// 컴팩션 작업 스케줄링
    pub async fn schedule_compaction(&self, keyspace: &str, table: &str) {
        let table_key = format!("{}.{}", keyspace, table);
        
        // TODO: 실제로는 SSTable 리스트를 받아서 컴팩션 전략에 따라 작업 생성
        let task = CompactionTask {
            keyspace: keyspace.to_string(),
            table: table.to_string(),
            input_sstables: vec![], // 실제 구현에서는 SSTable 리스트를 전달받아야 함
            output_sstable: None,
            strategy: self.config.strategy.clone(),
        };
        
        let _ = self.task_sender.send(task);
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
    async fn execute_compaction(&self, task: CompactionTask) -> Result<(), Error> {
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
    async fn execute_size_tiered_compaction(&self, task: CompactionTask) -> Result<(), Error> {
        if task.input_sstables.is_empty() {
            return Ok(());
        }
        
        // 모든 입력 SSTable의 데이터를 읽어서 병합
        let mut merged_data = HashMap::new();
        
        for sstable in &task.input_sstables {
            // SSTable의 모든 파티션을 읽어서 병합
            // 실제 구현에서는 더 효율적인 방법을 사용해야 함
            for (partition_key, _) in &sstable.partition_index {
                if let Some(partition) = sstable.read_partition(partition_key).await? {
                    // 파티션 병합 로직
                    // 최신 타임스탬프의 데이터를 우선시
                    merged_data.insert(partition_key.clone(), partition);
                }
            }
        }
        
        // 병합된 데이터로 새 SSTable 생성
        // TODO: 실제 구현에서는 Memtable을 거쳐서 SSTable 생성
        
        // 기존 SSTable들 삭제
        for sstable in &task.input_sstables {
            sstable.delete().await?;
        }
        
        Ok(())
    }
    
    /// Leveled 컴팩션 실행
    async fn execute_leveled_compaction(&self, task: CompactionTask) -> Result<(), Error> {
        // Leveled 컴팩션은 레벨별로 SSTable을 관리
        // 각 레벨의 SSTable 크기가 일정 비율로 증가
        // L0 -> L1 -> L2 ... 순서로 컴팩션
        
        // TODO: 실제 구현에서는 레벨별 SSTable 관리 구조가 필요
        self.execute_size_tiered_compaction(task).await
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
}

/// 컴팩션 통계
#[derive(Debug)]
pub struct CompactionStats {
    pub pending_tasks: usize,
    pub throughput_mb_per_sec: u64,
    pub strategy: CompactionStrategy,
}

/// SSTable 레벨 관리
pub struct LevelManager {
    levels: Vec<Vec<Arc<SSTable>>>,
    max_levels: usize,
    level_size_multiplier: f64,
}

impl LevelManager {
    pub fn new(max_levels: usize, level_size_multiplier: f64) -> Self {
        Self {
            levels: vec![Vec::new(); max_levels],
            max_levels,
            level_size_multiplier,
        }
    }
    
    /// SSTable 추가
    pub fn add_sstable(&mut self, sstable: Arc<SSTable>, level: usize) {
        if level < self.max_levels {
            self.levels[level].push(sstable);
        }
    }
    
    /// 컴팩션이 필요한지 확인
    pub fn needs_compaction(&self) -> Option<(usize, Vec<Arc<SSTable>>)> {
        for (level, sstables) in self.levels.iter().enumerate() {
            if sstables.len() >= self.get_threshold_for_level(level) {
                return Some((level, sstables.clone()));
            }
        }
        None
    }
    
    /// 레벨별 임계값 계산
    fn get_threshold_for_level(&self, level: usize) -> usize {
        if level == 0 {
            4 // L0은 4개
        } else {
            (10.0 * self.level_size_multiplier.powi(level as i32)) as usize
        }
    }
    
    /// 컴팩션 후 레벨 업데이트
    pub fn update_after_compaction(&mut self, level: usize, input_sstables: &[Arc<SSTable>], output_sstable: Arc<SSTable>) {
        // 입력 SSTable들 제거
        self.levels[level].retain(|sstable| !input_sstables.contains(sstable));
        
        // 출력 SSTable을 다음 레벨에 추가
        if level + 1 < self.max_levels {
            self.levels[level + 1].push(output_sstable);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_level_manager_thresholds() {
        let mut manager = LevelManager::new(5, 10.0);
        
        assert_eq!(manager.get_threshold_for_level(0), 4);
        assert_eq!(manager.get_threshold_for_level(1), 100);
        assert_eq!(manager.get_threshold_for_level(2), 1000);
    }
    
    #[test]
    fn test_level_manager_compaction_trigger() {
        let mut manager = LevelManager::new(3, 10.0);
        
        // 아직 컴팩션이 필요하지 않음
        assert!(manager.needs_compaction().is_none());
        
        // L0에 4개 SSTable 추가하면 컴팩션 필요
        // TODO: 실제 SSTable 객체를 생성해서 테스트
    }
    
    #[tokio::test]
    async fn test_compaction_manager_creation() {
        let config = CompactionConfig {
            throughput_mb_per_sec: 16,
            max_concurrent_compactions: 2,
            strategy: CompactionStrategy::SizeTiered {
                min_threshold: 4,
                max_threshold: 32,
            },
            data_directory: std::env::temp_dir(),
        };
        
        let manager = CompactionManager::new(config);
        let stats = manager.get_compaction_stats().await;
        
        assert_eq!(stats.pending_tasks, 0);
        assert_eq!(stats.throughput_mb_per_sec, 16);
    }
}

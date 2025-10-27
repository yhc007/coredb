use std::collections::{HashMap, BTreeMap};
use std::fs::{File, OpenOptions, create_dir_all};
use std::io::{Write, Read, BufReader, BufWriter};
use std::path::Path;
use serde::{Serialize, Deserialize};
use chrono::Utc;

// 영속성 지원 데이터 타입
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PersistentValue {
    Text(String),
    Int(i64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentRow {
    pub key: PersistentValue,
    pub value: PersistentValue,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentTable {
    pub name: String,
    #[serde(skip)]
    data: BTreeMap<PersistentValue, PersistentRow>,
    #[serde(default)]
    data_serialized: Vec<PersistentRow>, // 직렬화용
}

impl PersistentTable {
    pub fn new(name: String) -> Self {
        PersistentTable {
            name,
            data: BTreeMap::new(),
            data_serialized: Vec::new(),
        }
    }

    pub fn insert(&mut self, key: PersistentValue, value: PersistentValue) {
        let timestamp = Utc::now().timestamp_micros();
        let row = PersistentRow { key: key.clone(), value, timestamp };
        self.data.insert(key, row);
    }

    pub fn get(&self, key: &PersistentValue) -> Option<PersistentRow> {
        self.data.get(key).cloned()
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&PersistentValue, &PersistentRow)> {
        self.data.iter()
    }

    // 직렬화 준비
    fn prepare_for_serialization(&mut self) {
        self.data_serialized = self.data.values().cloned().collect();
    }

    // 역직렬화 후 복원
    fn restore_after_deserialization(&mut self) {
        self.data = self.data_serialized
            .iter()
            .map(|row| (row.key.clone(), row.clone()))
            .collect();
        self.data_serialized.clear();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentKeyspace {
    pub name: String,
    #[serde(skip)]
    tables: HashMap<String, PersistentTable>,
    #[serde(default)]
    tables_serialized: Vec<PersistentTable>, // 직렬화용
}

impl PersistentKeyspace {
    pub fn new(name: String) -> Self {
        PersistentKeyspace {
            name,
            tables: HashMap::new(),
            tables_serialized: Vec::new(),
        }
    }

    pub fn create_table(&mut self, table_name: String) {
        let table = PersistentTable::new(table_name.clone());
        self.tables.insert(table_name, table);
    }

    pub fn get_table(&mut self, table_name: &str) -> Option<&mut PersistentTable> {
        self.tables.get_mut(table_name)
    }

    pub fn tables_count(&self) -> usize {
        self.tables.len()
    }

    // 직렬화 준비
    fn prepare_for_serialization(&mut self) {
        for table in self.tables.values_mut() {
            table.prepare_for_serialization();
        }
        self.tables_serialized = self.tables.values().cloned().collect();
    }

    // 역직렬화 후 복원
    fn restore_after_deserialization(&mut self) {
        for table in &mut self.tables_serialized {
            table.restore_after_deserialization();
        }
        self.tables = self.tables_serialized
            .drain(..)
            .map(|table| (table.name.clone(), table))
            .collect();
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PersistentCoreDB {
    #[serde(skip)]
    keyspaces: HashMap<String, PersistentKeyspace>,
    #[serde(default)]
    keyspaces_serialized: Vec<PersistentKeyspace>, // 직렬화용
    #[serde(skip)]
    data_directory: String,
}

impl PersistentCoreDB {
    pub fn new(data_directory: String) -> Self {
        // 데이터 디렉토리 생성
        create_dir_all(&data_directory).expect("Failed to create data directory");
        
        PersistentCoreDB {
            keyspaces: HashMap::new(),
            keyspaces_serialized: Vec::new(),
            data_directory,
        }
    }

    pub fn create_keyspace(&mut self, keyspace_name: String) {
        let ks = PersistentKeyspace::new(keyspace_name.clone());
        self.keyspaces.insert(keyspace_name, ks);
    }

    pub fn get_keyspace(&mut self, keyspace_name: &str) -> Option<&mut PersistentKeyspace> {
        self.keyspaces.get_mut(keyspace_name)
    }

    pub fn keyspace_count(&self) -> usize {
        self.keyspaces.len()
    }

    pub fn total_tables(&self) -> usize {
        self.keyspaces.values().map(|ks| ks.tables_count()).sum()
    }

    pub fn total_keys(&self) -> usize {
        self.keyspaces.values()
            .flat_map(|ks| ks.tables.values())
            .map(|tbl| tbl.len())
            .sum()
    }

    // 직렬화 준비
    fn prepare_for_serialization(&mut self) {
        for ks in self.keyspaces.values_mut() {
            ks.prepare_for_serialization();
        }
        self.keyspaces_serialized = self.keyspaces.values().cloned().collect();
    }

    // 역직렬화 후 복원
    fn restore_after_deserialization(&mut self) {
        for ks in &mut self.keyspaces_serialized {
            ks.restore_after_deserialization();
        }
        self.keyspaces = self.keyspaces_serialized
            .drain(..)
            .map(|ks| (ks.name.clone(), ks))
            .collect();
    }

    /// 데이터베이스를 디스크에 저장 (JSON 형식)
    pub fn save_to_disk(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let file_path = format!("{}/coredb_snapshot.json", self.data_directory);
        println!("💾 Saving database to: {}", file_path);
        
        // 직렬화 준비
        self.prepare_for_serialization();
        
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&file_path)?;
        
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, &self)?;
        
        println!("✅ Database saved successfully!");
        Ok(())
    }

    /// 데이터베이스를 바이너리 형식으로 저장 (더 빠르고 컴팩트)
    pub fn save_to_disk_binary(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let file_path = format!("{}/coredb_snapshot.bin", self.data_directory);
        println!("💾 Saving database (binary) to: {}", file_path);
        
        // 직렬화 준비
        self.prepare_for_serialization();
        
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&file_path)?;
        
        let encoded = bincode::serialize(&self)?;
        file.write_all(&encoded)?;
        
        println!("✅ Database saved successfully (binary)!");
        Ok(())
    }

    /// 디스크에서 데이터베이스 복구 (JSON 형식)
    pub fn load_from_disk(data_directory: String) -> Result<Self, Box<dyn std::error::Error>> {
        let file_path = format!("{}/coredb_snapshot.json", data_directory);
        
        if !Path::new(&file_path).exists() {
            println!("⚠️  No snapshot found, creating new database");
            return Ok(Self::new(data_directory));
        }
        
        println!("📂 Loading database from: {}", file_path);
        
        let file = File::open(&file_path)?;
        let reader = BufReader::new(file);
        let mut db: PersistentCoreDB = serde_json::from_reader(reader)?;
        
        // 역직렬화 후 복원
        db.data_directory = data_directory;
        db.restore_after_deserialization();
        
        println!("✅ Database loaded successfully!");
        println!("   Keyspaces: {}", db.keyspace_count());
        println!("   Tables: {}", db.total_tables());
        println!("   Total keys: {}", db.total_keys());
        
        Ok(db)
    }

    /// 디스크에서 데이터베이스 복구 (바이너리 형식)
    pub fn load_from_disk_binary(data_directory: String) -> Result<Self, Box<dyn std::error::Error>> {
        let file_path = format!("{}/coredb_snapshot.bin", data_directory);
        
        if !Path::new(&file_path).exists() {
            println!("⚠️  No snapshot found, creating new database");
            return Ok(Self::new(data_directory));
        }
        
        println!("📂 Loading database (binary) from: {}", file_path);
        
        let mut file = File::open(&file_path)?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;
        
        let mut db: PersistentCoreDB = bincode::deserialize(&buffer)?;
        
        // 역직렬화 후 복원
        db.data_directory = data_directory;
        db.restore_after_deserialization();
        
        println!("✅ Database loaded successfully (binary)!");
        println!("   Keyspaces: {}", db.keyspace_count());
        println!("   Tables: {}", db.total_tables());
        println!("   Total keys: {}", db.total_keys());
        
        Ok(db)
    }

    /// Write-Ahead Log (WAL) 쓰기
    pub fn write_wal(&self, operation: &str) -> Result<(), Box<dyn std::error::Error>> {
        let wal_path = format!("{}/wal.log", self.data_directory);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(wal_path)?;
        
        let timestamp = Utc::now().timestamp_micros();
        writeln!(file, "{}\t{}", timestamp, operation)?;
        
        Ok(())
    }
}

fn main() {
    println!("🚀 CoreDB - Persistent Database Demo");
    println!("====================================\n");

    let data_dir = "./data".to_string();

    // 1. 기존 데이터 로드 또는 새 DB 생성
    println!("1️⃣  LOADING DATABASE");
    let mut db = PersistentCoreDB::load_from_disk(data_dir.clone())
        .unwrap_or_else(|e| {
            println!("⚠️  Load failed: {}, creating new database", e);
            PersistentCoreDB::new(data_dir.clone())
        });

    // 2. 키스페이스 생성
    println!("\n2️⃣  CREATING KEYSPACES");
    if db.keyspace_count() == 0 {
        db.create_keyspace("demo".to_string());
        println!("✓ Created keyspace: demo");
        db.create_keyspace("system".to_string());
        println!("✓ Created keyspace: system");
    } else {
        println!("✓ Using existing keyspaces: {}", db.keyspace_count());
    }

    // 3. 테이블 생성
    println!("\n3️⃣  CREATING TABLES");
    if let Some(demo_ks) = db.get_keyspace("demo") {
        if demo_ks.tables_count() == 0 {
            demo_ks.create_table("users".to_string());
            println!("✓ Created table: demo.users");
            demo_ks.create_table("products".to_string());
            println!("✓ Created table: demo.products");
        }
    }
    
    if let Some(system_ks) = db.get_keyspace("system") {
        if system_ks.tables_count() == 0 {
            system_ks.create_table("metadata".to_string());
            println!("✓ Created table: system.metadata");
        }
    }

    // 4. 데이터 삽입
    println!("\n4️⃣  INSERTING DATA");
    let new_user_count = 3;
    
    // WAL 작업 목록 수집
    let mut wal_operations = Vec::new();
    
    if let Some(demo_ks) = db.get_keyspace("demo") {
        if let Some(users_table) = demo_ks.get_table("users") {
            let current_count = users_table.len();
            
            for i in 1..=new_user_count {
                let user_id = current_count + i;
                users_table.insert(
                    PersistentValue::Int(user_id as i64),
                    PersistentValue::Text(format!("User #{}", user_id))
                );
                println!("✓ Inserted: demo.users.{} = User #{}", user_id, user_id);
                
                // WAL 작업 저장
                wal_operations.push(format!("INSERT demo.users {} User#{}", user_id, user_id));
            }
        }

        if let Some(products_table) = demo_ks.get_table("products") {
            if products_table.is_empty() {
                products_table.insert(
                    PersistentValue::Text("p1".to_string()),
                    PersistentValue::Text("Laptop".to_string())
                );
                println!("✓ Inserted: demo.products.p1 = Laptop");
                
                products_table.insert(
                    PersistentValue::Text("p2".to_string()),
                    PersistentValue::Text("Mouse".to_string())
                );
                println!("✓ Inserted: demo.products.p2 = Mouse");
            }
        }
    }
    
    // WAL 기록
    for operation in wal_operations {
        db.write_wal(&operation).ok();
    }

    // 5. 데이터 조회
    println!("\n5️⃣  RETRIEVING DATA");
    if let Some(demo_ks) = db.get_keyspace("demo") {
        if let Some(users_table) = demo_ks.get_table("users") {
            println!("Users (total: {}):", users_table.len());
            for (key, row) in users_table.iter().take(5) {
                println!("  {:?}: {:?}", key, row.value);
            }
            if users_table.len() > 5 {
                println!("  ... and {} more", users_table.len() - 5);
            }
        }

        if let Some(products_table) = demo_ks.get_table("products") {
            println!("\nProducts:");
            for (key, row) in products_table.iter() {
                println!("  {:?}: {:?}", key, row.value);
            }
        }
    }

    // 6. 데이터베이스 상태
    println!("\n6️⃣  DATABASE STATISTICS");
    println!("  Keyspaces: {}", db.keyspace_count());
    println!("  Tables: {}", db.total_tables());
    println!("  Total keys: {}", db.total_keys());

    // 7. 디스크에 저장
    println!("\n7️⃣  SAVING TO DISK");
    
    // JSON 형식으로 저장
    match db.save_to_disk() {
        Ok(_) => println!("✅ JSON snapshot saved"),
        Err(e) => println!("❌ JSON save failed: {}", e),
    }

    // 바이너리 형식으로 저장
    match db.save_to_disk_binary() {
        Ok(_) => println!("✅ Binary snapshot saved"),
        Err(e) => println!("❌ Binary save failed: {}", e),
    }

    println!("\n📁 Data files:");
    println!("  - {}/coredb_snapshot.json", data_dir);
    println!("  - {}/coredb_snapshot.bin", data_dir);
    println!("  - {}/wal.log", data_dir);

    println!("\n✅ CoreDB persistent demo completed!");
    println!("💡 Tip: Run this program again to see data persistence in action!");
}

